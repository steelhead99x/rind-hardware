// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! rind-bench's benches, compiled from `rind/rind-bench/benches` (see
//! `build.rs` for the mechanical changes), run bare metal on an RP2040
//! (a Raspberry Pi Pico or compatible board) and printed as `RB:` JSON
//! lines over USB CDC-ACM.
//!
//! The benchmarks run in thread mode, timed with SysTick at the 125 MHz
//! processor clock (1 count = 1 cycle). Cortex-M0+ has no DWT cycle
//! counter; see `board.rs` for the 24-bit hardware counter, the software
//! extension, and the 1 MHz TIMER cross-check. USB runs at interrupt
//! priority on its own executor, and each line is flushed to the host
//! before the next benchmark starts, so no USB transfer overlaps a timed
//! region. A 1200-baud "touch" of the port, a panic, or a HardFault
//! enters the UF2 bootloader, so the board can always be reflashed
//! without its button.

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
use core::cell::Cell;
use critical_section::Mutex;

static HEAP: embedded_alloc::LlffHeap = embedded_alloc::LlffHeap::empty();

/// The heap, plus its high-water mark for the done record. Updating the mark
/// adds a few cycles to every allocation, the same for every library.
struct PeakTracking;
/// Cortex-M0+ has no atomic RMW, so the mark is a cell under a critical section.
static HEAP_PEAK: Mutex<Cell<usize>> = Mutex::new(Cell::new(0));

// SAFETY: forwards to `HEAP`, which upholds `GlobalAlloc`'s contract.
unsafe impl core::alloc::GlobalAlloc for PeakTracking {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's layout, passed on.
        let p = unsafe { HEAP.alloc(layout) };
        let used = HEAP.used();
        critical_section::with(|cs| {
            let peak = HEAP_PEAK.borrow(cs);
            if used > peak.get() {
                peak.set(used);
            }
        });
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
    pad::keep();
    {
        use core::mem::MaybeUninit;
        static mut MEMORY: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        // SAFETY: runs once, before anything allocates; MEMORY is used only here.
        unsafe { HEAP.init(&raw mut MEMORY as usize, HEAP_SIZE) }
    }
    let p = board::init();
    usb::start(p);
    usb::wait_for_host();

    criterion::set_output(usb::write_and_flush);
    criterion::set_clock(board::cycles);
    let cpu_hz = board::cpu_hz();
    criterion::set_config(criterion::Config {
        cpu_hz,
        warm_up_cycles: cpu_hz / 5,
        sample_cycles: cpu_hz / 500,
        samples: 51,
        min_samples: 11,
        max_measure_cycles: cpu_hz * 2,
    });
    if let Some(filter) = option_env!("RIND_BENCH_FILTER") {
        criterion::set_filter(filter.split(',').map(String::from).collect());
    }
    let (check_cycles, check_us) = board::cross_check();
    criterion::emit(&format!(
        concat!(
            "{{\"type\":\"start\",\"chip\":\"rp2040\",\"board\":\"pico\",",
            "\"cpu_hz\":{},\"rind_backend\":\"portable\",\"profile\":\"{}\",",
            "\"xip_cache\":{},\"heap\":{},\"timer\":\"systick\",",
            "\"timer_resolution_cycles\":1,\"systick_bits\":24,",
            "\"check_cycles\":{},\"check_us\":{},\"rng\":\"splitmix64\",",
            "\"pad\":{}}}"
        ),
        cpu_hz,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        u32::from(board::xip_cache_enabled()),
        HEAP_SIZE,
        check_cycles,
        check_us,
        pad::BYTES,
    ));

    run_benches();
}

fn run_benches() -> ! {
    let _failed = vectors::check_all();

    for (name, benches) in BENCHES {
        criterion::emit(&format!("{{\"type\":\"bench\",\"file\":\"{name}\"}}"));
        benches();
    }
    let peak = critical_section::with(|cs| HEAP_PEAK.borrow(cs).get());
    criterion::emit(&format!("{{\"type\":\"done\",\"heap_used_max\":{peak}}}"));
    loop {
        cortex_m::asm::wfi();
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
