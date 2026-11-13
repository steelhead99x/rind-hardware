// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! rind-bench's benches, compiled from `rind/rind-bench/benches` (see
//! `build.rs` for the one mechanical change), run bare metal on an nRF52840
//! (an Arduino Nano 33 BLE) and printed as `RB:` JSON lines over USB CDC-ACM.
//!
//! The benchmarks run in thread mode, timed with the Cortex-M4's DWT cycle
//! counter at 64 MHz. USB runs at interrupt priority on its own executor, and
//! each line is flushed to the host before the next benchmark starts, so no
//! USB transfer overlaps a timed region. A 1200-baud "touch" of the port, a
//! panic, a fault, or a lockup reset enters the Arduino BOSSA bootloader, so the board
//! can always be reflashed without its button (a double-tap of reset also works).

#![no_std]
#![no_main]
// Each bench file is its own crate upstream; here they share one binary, and
// a few of their helpers go unused by the rows that build here.
#![allow(dead_code, unused_imports, unused_variables)]

#[macro_use]
extern crate alloc;

macro_rules! bench_file {
    ($feature:literal, $name:ident) => {
        #[cfg(feature = $feature)]
        mod $name {
            include!(concat!(env!("OUT_DIR"), "/", $feature, ".rs"));
        }
    };
}
bench_file!("aes_gcm", aes_gcm);
bench_file!("chacha20poly1305", chacha20poly1305);
bench_file!("xchacha20poly1305", xchacha20poly1305);
bench_file!("cipher", cipher);
bench_file!("x25519", x25519);
bench_file!("p256", p256);
bench_file!("ed25519", ed25519);
bench_file!("sha2", sha2);
bench_file!("sha3", sha3);
bench_file!("hmac", hmac);
bench_file!("hkdf", hkdf);
bench_file!("mlkem768", mlkem768);

/// Every ported bench file, by name, with its criterion groups.
const BENCHES: &[(&str, fn())] = &[
    #[cfg(feature = "aes_gcm")]
    ("aes_gcm", aes_gcm::benches),
    #[cfg(feature = "chacha20poly1305")]
    ("chacha20poly1305", chacha20poly1305::benches),
    #[cfg(feature = "xchacha20poly1305")]
    ("xchacha20poly1305", xchacha20poly1305::benches),
    #[cfg(feature = "cipher")]
    ("cipher", cipher::benches),
    #[cfg(feature = "x25519")]
    ("x25519", x25519::benches),
    #[cfg(feature = "p256")]
    ("p256", p256::benches),
    #[cfg(feature = "ed25519")]
    ("ed25519", ed25519::benches),
    #[cfg(feature = "sha2")]
    ("sha2", sha2::benches),
    #[cfg(feature = "sha3")]
    ("sha3", sha3::benches),
    #[cfg(feature = "hmac")]
    ("hmac", hmac::benches),
    #[cfg(feature = "hkdf")]
    ("hkdf", hkdf::benches),
    #[cfg(feature = "mlkem768")]
    ("mlkem768", mlkem768::benches),
];

mod board;
#[path = "../../esp32/src/pad.rs"]
mod pad;
mod usb;
#[path = "../../esp32/src/vectors.rs"]
mod vectors;

use alloc::string::String;

static HEAP: embedded_alloc::LlffHeap = embedded_alloc::LlffHeap::empty();

/// The heap, plus its high-water mark for the done record. Updating the mark
/// adds a few cycles to every allocation, the same for every library.
struct PeakTracking;
static HEAP_PEAK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

// SAFETY: forwards to `HEAP`, which upholds `GlobalAlloc`'s contract.
unsafe impl core::alloc::GlobalAlloc for PeakTracking {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's layout, passed on.
        let p = unsafe { HEAP.alloc(layout) };
        HEAP_PEAK.fetch_max(HEAP.used(), core::sync::atomic::Ordering::Relaxed);
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        // SAFETY: `ptr` came from `alloc` above with this layout.
        unsafe { HEAP.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: PeakTracking = PeakTracking;
/// The heap, in .bss; flip-link puts the stack below it, in what is left.
const HEAP_SIZE: usize = 96 * 1024;

#[cortex_m_rt::entry]
fn main() -> ! {
    board::check_reset_reason();
    pad::keep();
    {
        use core::mem::MaybeUninit;
        static mut MEMORY: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        // SAFETY: runs once, before anything allocates; MEMORY is used only here.
        unsafe { HEAP.init(&raw mut MEMORY as usize, HEAP_SIZE) }
    }
    let p = board::init();
    board::enable_fault_reports();
    usb::start(p);
    usb::wait_for_host();

    criterion::set_output(usb::write_and_flush);
    criterion::set_clock(board::cycles);
    criterion::set_config(criterion::Config {
        cpu_hz: board::CPU_HZ,
        warm_up_cycles: board::CPU_HZ / 5,
        sample_cycles: board::CPU_HZ / 500,
        samples: 51,
        min_samples: 11,
        max_measure_cycles: board::CPU_HZ * 2,
    });
    if let Some(filter) = option_env!("RIND_BENCH_FILTER") {
        criterion::set_filter(filter.split(',').map(String::from).collect());
    }
    criterion::emit(&format!(
        concat!(
            "{{\"type\":\"start\",\"chip\":\"nrf52840\",\"board\":\"arduino-nano-33-ble\",",
            "\"cpu_hz\":{},\"rind_backend\":\"portable\",\"profile\":\"{}\",",
            "\"icache\":{},\"heap\":{},\"reset_reason\":\"{:#x}\",",
            "\"wdt_runstatus\":{},\"wdt_crv\":{},\"wdt_rren\":\"{:#x}\",",
            "\"wdt_config\":\"{:#x}\",\"pad\":{}}}"
        ),
        board::CPU_HZ,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        board::icache_enabled(),
        HEAP_SIZE,
        board::reset_reason(),
        u32::from(board::wdt_runstatus()),
        board::wdt_crv(),
        board::wdt_rren(),
        board::wdt_config(),
        pad::BYTES,
    ));

    // A watchdog reset is reported in the start record above. Enter the
    // bootloader so the host can flash the next image; the timeout also
    // stops the nRF52 watchdog, which a software reset does not.
    if board::dog_reset() {
        board::enter_bootloader();
    }

    #[cfg(feature = "wdt-probe")]
    {
        wdt_probe::run();
        board::enter_bootloader();
    }

    // Known-answer checks first: a library whose output is wrong is not raced.
    #[cfg(not(feature = "wdt-probe"))]
    run_benches();
}

/// The known-answer checks and the criterion groups. Split out so a
/// `wdt-probe` build, which never returns from `enter_bootloader` above,
/// does not leave this as unreachable code under `-D warnings`.
#[cfg(not(feature = "wdt-probe"))]
fn run_benches() -> ! {
    let _failed = vectors::check_all();
    #[cfg(feature = "rng-diag")]
    rng_diag::run();

    for (name, benches) in BENCHES {
        criterion::emit(&format!("{{\"type\":\"bench\",\"file\":\"{name}\"}}"));
        benches();
    }
    criterion::emit(&format!(
        "{{\"type\":\"done\",\"heap_used_max\":{}}}",
        HEAP_PEAK.load(core::sync::atomic::Ordering::Relaxed)
    ));
    loop {
        board::feed_wdt();
        if board::wdt_runstatus() {
            let start = board::cycles();
            while board::cycles().wrapping_sub(start) < board::CPU_HZ as u32 / 10 {}
        } else {
            cortex_m::asm::wfi();
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    use core::fmt::Write;
    let mut msg = heapless_line::Line::new();
    let _ = write!(msg, "{info}");
    usb::emergency_write(msg.as_str());
    board::enter_bootloader()
}

/// A fixed buffer for the panic message: the heap may be what failed.
mod heapless_line {
    pub struct Line {
        buf: [u8; 256],
        len: usize,
    }

    impl Line {
        pub const fn new() -> Self {
            Self {
                buf: [0; 256],
                len: 0,
            }
        }

        pub fn as_str(&self) -> &str {
            core::str::from_utf8(&self.buf[..self.len]).unwrap_or("panic")
        }
    }

    impl core::fmt::Write for Line {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for &b in s.as_bytes() {
                if self.len < self.buf.len() && b != b'"' && b != b'\\' && b >= 0x20 {
                    self.buf[self.len] = b;
                    self.len += 1;
                }
            }
            Ok(())
        }
    }
}

/// `wdt-probe`: first image after Meshtastic. Prints the watchdog registers,
/// feeds for a few seconds to show that a reload keeps the board alive, then
/// stops feeding and waits for the timeout, which should reset the chip and
/// come back with `RESETREAS.DOG`. Not used in published runs.
#[cfg(feature = "wdt-probe")]
mod wdt_probe {
    use crate::board::{self, CPU_HZ, cycles, feed_wdt};

    fn wait_secs(secs: u32, feeding: bool) {
        for _ in 0..secs {
            let start = cycles();
            while cycles().wrapping_sub(start) < CPU_HZ as u32 {
                if feeding {
                    feed_wdt();
                }
            }
            criterion::emit(&format!(
                "{{\"type\":\"wdt\",\"event\":\"{}\"}}",
                if feeding { "feed" } else { "wait" }
            ));
        }
    }

    pub fn run() {
        criterion::emit(&format!(
            concat!(
                "{{\"type\":\"wdt\",\"event\":\"probe\",",
                "\"runstatus\":{},\"crv\":{},\"rren\":{},\"config\":{}}}"
            ),
            u32::from(board::wdt_runstatus()),
            board::wdt_crv(),
            board::wdt_rren(),
            board::wdt_config(),
        ));
        if board::wdt_runstatus() {
            // Eight feeds, one per second: the board would already have reset
            // if a reload did not take. Then stop and wait for the timeout.
            wait_secs(8, true);
            criterion::emit("{\"type\":\"wdt\",\"event\":\"stop-feed\"}");
            // write_and_flush feeds; turn feeding off only after that line
            // has been sent, then wait silently so a log line cannot reload
            // the counter.
            board::set_wdt_feed(false);
            // Wait longer than any plausible CRV (CRV/32768 seconds, plus a
            // margin). The chip should reset first.
            for _ in 0..180 {
                let start = cycles();
                while cycles().wrapping_sub(start) < CPU_HZ as u32 {}
            }
            criterion::emit("{\"type\":\"wdt\",\"event\":\"no-timeout\"}");
        }
    }
}

/// `rng-diag`: what one ML-KEM keygen or encaps draws from getrandom, per
/// library, and what that costs, as `rng` records. Uses the bench file's calls.
#[cfg(feature = "rng-diag")]
mod rng_diag {
    use crate::board::{RNG_BYTES, RNG_CYCLES, cycles};
    use core::hint::black_box;
    use core::sync::atomic::Ordering::Relaxed;

    fn measure(id: &str, f: impl FnOnce()) {
        let (b0, r0) = (RNG_BYTES.load(Relaxed), RNG_CYCLES.load(Relaxed));
        let t0 = cycles();
        f();
        let total = cycles().wrapping_sub(t0);
        let bytes = RNG_BYTES.load(Relaxed) - b0;
        let rng = RNG_CYCLES.load(Relaxed) - r0;
        criterion::emit(&format!(
            "{{\"type\":\"rng\",\"id\":\"{id}\",\"bytes\":{bytes},\"rng_cycles\":{rng},\"total_cycles\":{total}}}"
        ));
    }

    pub fn run() {
        use libcrux_ml_kem::mlkem768;
        use ml_kem::{Encapsulate, Kem, MlKem768};
        use rind::kem::{DecapsulationKey, EncapsulationKey, ML_KEM_768};
        for _ in 0..3 {
            measure("mlkem768-keygen/rind", || {
                black_box(DecapsulationKey::generate(&ML_KEM_768).unwrap());
            });
            measure("mlkem768-keygen/libcrux-ml-kem", || {
                let mut rand = [0u8; 64];
                getrandom::fill(&mut rand).unwrap();
                black_box(mlkem768::generate_key_pair(rand).into_parts());
            });
            measure("mlkem768-keygen/rustcrypto", || {
                black_box(MlKem768::generate_keypair());
            });

            let ours = DecapsulationKey::generate(&ML_KEM_768)
                .unwrap()
                .public_key();
            measure("mlkem768-encaps/rind", || {
                let encaps = EncapsulationKey::new(&ML_KEM_768, ours.as_ref()).unwrap();
                black_box(encaps.encapsulate().unwrap());
            });
            let mut rand = [0u8; 64];
            getrandom::fill(&mut rand).unwrap();
            let (_, theirs) = mlkem768::generate_key_pair(rand).into_parts();
            measure("mlkem768-encaps/libcrux-ml-kem", || {
                let mut rand = [0u8; 32];
                getrandom::fill(&mut rand).unwrap();
                black_box(mlkem768::encapsulate(&theirs, rand));
            });
            let (_decaps, rc) = MlKem768::generate_keypair();
            measure("mlkem768-encaps/rustcrypto", || {
                black_box(rc.encapsulate());
            });
        }
    }
}
