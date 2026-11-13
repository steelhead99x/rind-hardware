// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! rind-bench's benches, compiled unchanged from `rind/rind-bench/benches`,
//! run on one ESP32-S3 core and printed as `RB:` JSON lines.

// Each bench file is its own crate upstream; here they share one binary, and
// a few of their helpers go unused by the rows that build for Xtensa.
#![allow(dead_code, unused_imports, unused_variables)]

#[cfg(feature = "aes_gcm")]
#[path = "../../rind/rind-bench/benches/aes_gcm.rs"]
mod aes_gcm;
#[cfg(feature = "chacha20poly1305")]
#[path = "../../rind/rind-bench/benches/chacha20poly1305.rs"]
mod chacha20poly1305;
#[cfg(feature = "cipher")]
#[path = "../../rind/rind-bench/benches/cipher.rs"]
mod cipher;
#[cfg(feature = "ed25519")]
#[path = "../../rind/rind-bench/benches/ed25519.rs"]
mod ed25519;
#[cfg(feature = "hkdf")]
#[path = "../../rind/rind-bench/benches/hkdf.rs"]
mod hkdf;
#[cfg(feature = "hmac")]
#[path = "../../rind/rind-bench/benches/hmac.rs"]
mod hmac;
#[cfg(feature = "mlkem768")]
#[path = "../../rind/rind-bench/benches/mlkem768.rs"]
mod mlkem768;
#[cfg(feature = "p256")]
#[path = "../../rind/rind-bench/benches/p256.rs"]
mod p256;
#[cfg(feature = "sha2")]
#[path = "../../rind/rind-bench/benches/sha2.rs"]
mod sha2;
#[cfg(feature = "sha3")]
#[path = "../../rind/rind-bench/benches/sha3.rs"]
mod sha3;
#[cfg(feature = "x25519")]
#[path = "../../rind/rind-bench/benches/x25519.rs"]
mod x25519;
#[cfg(feature = "xchacha20poly1305")]
#[path = "../../rind/rind-bench/benches/xchacha20poly1305.rs"]
mod xchacha20poly1305;

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

mod hw;
mod pad;
#[cfg(any(feature = "rng-diag", feature = "seeded-rng"))]
mod rng;
mod vectors;

// The demo display, kept out of the port: see demo/README.md.
#[cfg(feature = "oled")]
#[path = "../../demo/firmware/oled.rs"]
mod oled;

use esp_idf_svc::sys;

/// The core's 32-bit cycle counter, CCOUNT.
fn ccount() -> u32 {
    // SAFETY: reads a special register; no memory is touched.
    unsafe { sys::xthal_get_ccount() }
}

/// ESP-IDF's microsecond timer (the SYSTIMER, clocked from the 40 MHz
/// crystal, not the CPU PLL): an independent check on CCOUNT's rate.
fn wall_ns() -> u64 {
    // SAFETY: a read of a free-running hardware timer.
    unsafe { sys::esp_timer_get_time() as u64 * 1000 }
}

/// Measures CCOUNT against the wall clock over a busy second, and times the
/// harness on empty routines, so that its own cost is in every run's record.
fn harness_checks() {
    let (w0, c0) = (wall_ns(), ccount());
    let mut spins = 0u64;
    while wall_ns() - w0 < 1_000_000_000 {
        spins += 1;
    }
    let (w1, c1) = (wall_ns(), ccount());
    criterion::emit(&format!(
        "{{\"type\":\"clock\",\"wall_ns\":{},\"cycles\":{},\"spins\":{spins}}}",
        w1 - w0,
        c1.wrapping_sub(c0)
    ));
    let mut c = criterion::Criterion::default();
    c.bench_function("harness/empty/iter", |b| b.iter(|| ()));
    c.bench_function("harness/empty/iter_batched", |b| {
        b.iter_batched(|| (), |()| (), criterion::BatchSize::SmallInput)
    });
}

/// The bench thread's stack, from internal SRAM (PSRAM is off): 160 KB, or
/// `RIND_BENCH_STACK_KB` at build time, for diagnosing stack-dependent faults.
const STACK: usize = match option_env!("RIND_BENCH_STACK_KB") {
    Some(kb) => parse_kb(kb) * 1024,
    None => 160 * 1024,
};

const fn parse_kb(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    let mut n = 0;
    while i < b.len() {
        assert!(
            b[i].is_ascii_digit(),
            "RIND_BENCH_STACK_KB must be a number"
        );
        n = n * 10 + (b[i] - b'0') as usize;
        i += 1;
    }
    n
}

fn main() {
    sys::link_patches();
    pad::keep();

    // The benchmarks run on core 1, which runs no ESP-IDF service task, in a
    // thread big enough for ML-KEM's stack use: on Xtensa, rind's
    // `DecapKey::generate` alone takes a 27 KB frame, and the combined bench
    // nests it under two ~31 KB frames, past a 96 KB stack.
    esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration {
        name: Some(c"bench"),
        stack_size: STACK,
        priority: 20,
        pin_to_core: Some(esp_idf_svc::hal::cpu::Core::Core1),
        ..Default::default()
    }
    .set()
    .unwrap();
    let thread = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(run)
        .unwrap();
    thread.join().unwrap();
}

fn run() {
    criterion::set_clock(ccount);
    criterion::set_wall_clock(wall_ns);
    // SAFETY: plain reads of the clock tree and of this task's core.
    let (cpu_hz, core) = unsafe {
        (
            sys::esp_rom_get_cpu_ticks_per_us() * 1_000_000,
            sys::xTaskGetCoreID(sys::xTaskGetCurrentTaskHandle()),
        )
    };
    assert_eq!(cpu_hz, 240_000_000, "the benchmarks assume 240 MHz");
    criterion::set_config(criterion::Config::ESP32S3_240MHZ);
    if let Some(filter) = option_env!("RIND_BENCH_FILTER") {
        criterion::set_filter(filter.split(',').map(str::to_string).collect());
    }

    criterion::emit(&format!(
        concat!(
            "{{\"type\":\"start\",\"chip\":\"esp32s3\",\"cpu_hz\":{},\"core\":{},",
            "\"idf\":\"{}\",\"rind_backend\":\"portable\",\"profile\":\"{}\",",
            "\"free_heap\":{},\"pad\":{}}}"
        ),
        cpu_hz,
        core,
        // SAFETY: ESP-IDF's version string is static and NUL-terminated.
        unsafe { core::ffi::CStr::from_ptr(sys::esp_get_idf_version()) }.to_string_lossy(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        // SAFETY: a read of the heap's statistics.
        unsafe { sys::esp_get_free_heap_size() },
        pad::BYTES,
    ));

    #[cfg(feature = "oled")]
    oled::init();

    // Known-answer checks first: a library whose output is wrong is not raced.
    let failed = vectors::check_all();
    #[cfg(feature = "rng-diag")]
    rng::run();
    harness_checks();

    for (name, benches) in BENCHES {
        criterion::emit(&format!("{{\"type\":\"bench\",\"file\":\"{name}\"}}"));
        #[cfg(feature = "oled")]
        oled::begin(name, *benches);
        benches();
    }
    // The hardware baseline races only AES-GCM and SHA-256.
    #[cfg(any(feature = "aes_gcm", feature = "sha2"))]
    hw::benches(&failed);
    let _ = &failed;
    criterion::emit("{\"type\":\"done\"}");
    #[cfg(feature = "oled")]
    oled::finish();
}
