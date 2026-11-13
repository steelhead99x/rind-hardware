// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! rind-bench's benches, compiled from `rind/rind-bench/benches` (see
//! `build.rs` for the one mechanical change), run bare metal on an EFR32MG24
//! (a Seeed XIAO MG24) and printed as `RB:` JSON lines over SEGGER RTT
//! through the board's CMSIS-DAP probe.
//!
//! The benchmarks run in thread mode with no interrupts enabled, timed with
//! the Cortex-M33's DWT cycle counter at 78 MHz. Each line is drained by the
//! host before the next benchmark starts. A panic or fault prints one line
//! and halts; the probe flashes the next image.

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
mod rtt;
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
/// The heap, in .bss, as on the nRF52840; flip-link puts the stack below it.
const HEAP_SIZE: usize = 96 * 1024;

#[cortex_m_rt::entry]
fn main() -> ! {
    pad::keep();
    {
        use core::mem::MaybeUninit;
        static mut MEMORY: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        // SAFETY: runs once, before anything allocates; MEMORY is used only here.
        unsafe { HEAP.init(&raw mut MEMORY as usize, HEAP_SIZE) }
    }
    rtt::init();
    let clocks = board::init();
    criterion::set_output(rtt::write_and_flush);
    criterion::set_clock(board::cycles);
    let check = board::clock_check();
    criterion::emit(&format!(
        concat!(
            "{{\"type\":\"start\",\"chip\":\"efr32mg24\",\"board\":\"seeed-xiao-mg24\",",
            "\"cpu_hz\":{},\"dpll_locked\":{},\"rind_backend\":\"portable\",\"profile\":\"{}\",",
            "\"icache\":{},\"heap\":{},\"rng\":\"seeded-splitmix64\",",
            "\"sysclkctrl_reset\":\"{:#x}\",\"sysclkctrl\":\"{:#x}\",\"msc_readctrl\":\"{:#x}\",",
            "\"hfxo_ready\":{},\"dpll_if\":\"{:#x}\",\"hfrco_cal\":\"{:#x}\",",
            "\"hclk_hz_vs_hfxo\":{},\"hfrcodpll_hz_vs_hfxo\":{},\"window_hclk\":{},\"window_dwt\":{},",
            "\"hfxo_per_fsrco_milli\":{},\"pad\":{}}}"
        ),
        board::CPU_HZ,
        board::locked(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        board::icache_enabled(),
        HEAP_SIZE,
        clocks.sysclkctrl_at_reset,
        clocks.sysclkctrl,
        clocks.readctrl,
        clocks.hfxo_ready,
        clocks.dpll_if,
        clocks.hfrco_cal,
        check.hclk_hz,
        check.hfrcodpll_hz,
        check.window_hclk,
        check.window_dwt,
        check.hfxo_per_fsrco_milli,
        pad::BYTES,
    ));
    // Not at the clock the numbers would be reported at: time nothing.
    if !board::locked() {
        criterion::emit("{\"type\":\"done\",\"error\":\"dpll-not-locked\"}");
        board::halt();
    }

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

    // Known-answer checks first: a library whose output is wrong is not raced.
    let _failed = vectors::check_all();
    for (name, benches) in BENCHES {
        criterion::emit(&format!("{{\"type\":\"bench\",\"file\":\"{name}\"}}"));
        benches();
    }
    criterion::emit(&format!(
        "{{\"type\":\"done\",\"heap_used_max\":{}}}",
        HEAP_PEAK.load(core::sync::atomic::Ordering::Relaxed)
    ));
    board::halt()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    use core::fmt::Write;
    let mut msg = heapless_line::Line::new();
    let _ = write!(msg, "{info}");
    rtt::emergency_write(msg.as_str());
    board::halt()
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
