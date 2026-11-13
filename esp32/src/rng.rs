// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! RNG diagnostics for the randomized-nonce benches (XChaCha20-Poly1305):
//! every library draws its nonce through getrandom, so with
//! `--cfg getrandom_backend="custom"` this module is that backend.
//!
//! - `rng-diag`: ESP-IDF's `esp_fill_random`, counting bytes and cycles, and
//!   [`run`] prints `rng` records splitting each library's seal into RNG
//!   time and the rest.
//! - `seeded-rng`: SplitMix64 from a fixed seed. Not random at all, and only
//!   for timing what the libraries do apart from drawing randomness.

#[cfg(not(getrandom_backend = "custom"))]
compile_error!("rng-diag and seeded-rng need --cfg getrandom_backend=\"custom\"");

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

pub static RNG_BYTES: AtomicU32 = AtomicU32::new(0);
pub static RNG_CYCLES: AtomicU32 = AtomicU32::new(0);
pub static RNG_CALLS: AtomicU32 = AtomicU32::new(0);

/// getrandom 0.4's custom backend (it keeps the 0.3 symbol name).
///
/// # Safety
///
/// getrandom passes `len` writable bytes at `dest`.
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    let start = crate::ccount();
    // SAFETY: the caller's contract.
    fill(unsafe { core::slice::from_raw_parts_mut(dest, len) });
    RNG_CALLS.fetch_add(1, Relaxed);
    RNG_BYTES.fetch_add(len as u32, Relaxed);
    RNG_CYCLES.fetch_add(crate::ccount().wrapping_sub(start), Relaxed);
    Ok(())
}

#[cfg(not(feature = "seeded-rng"))]
fn fill(out: &mut [u8]) {
    // SAFETY: `out` is writable for its length.
    unsafe { esp_idf_svc::sys::esp_fill_random(out.as_mut_ptr().cast(), out.len()) }
}

#[cfg(feature = "seeded-rng")]
fn fill(out: &mut [u8]) {
    use std::sync::Mutex;
    static STATE: Mutex<u64> = Mutex::new(0x5eed_0000_0000_0001);
    let mut s = STATE.lock().unwrap();
    for chunk in out.chunks_mut(8) {
        *s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = *s;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
    }
}

#[cfg(feature = "rng-diag")]
fn measure(id: &str, mut f: impl FnMut()) {
    f();
    for _ in 0..5 {
        let (c0, b0, r0) = (
            RNG_CALLS.load(Relaxed),
            RNG_BYTES.load(Relaxed),
            RNG_CYCLES.load(Relaxed),
        );
        let t0 = crate::ccount();
        f();
        let total = crate::ccount().wrapping_sub(t0);
        criterion::emit(&format!(
            "{{\"type\":\"rng\",\"id\":\"{id}\",\"calls\":{},\"bytes\":{},\"rng_cycles\":{},\"total_cycles\":{total}}}",
            RNG_CALLS.load(Relaxed) - c0,
            RNG_BYTES.load(Relaxed) - b0,
            RNG_CYCLES.load(Relaxed).wrapping_sub(r0),
        ));
    }
}

/// For `RIND_BENCH_SPIN`: a higher-priority task on this core that wakes
/// every tick, preempting the spinning task, and prints the PC and return
/// address saved in its context (FreeRTOS's `pxTopOfStack` frame: exit, pc,
/// ps, a0, ...), to see where the time goes without a debugger.
#[cfg(feature = "rng-diag")]
fn start_sampler() {
    use esp_idf_svc::sys;
    // SAFETY: the handle of the calling task, which outlives the sampler.
    let spinner = unsafe { sys::xTaskGetCurrentTaskHandle() } as usize;
    esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration {
        name: Some(c"sampler"),
        stack_size: 8192,
        priority: 24,
        pin_to_core: Some(esp_idf_svc::hal::cpu::Core::Core1),
        ..Default::default()
    }
    .set()
    .unwrap();
    std::thread::Builder::new()
        .stack_size(8192)
        .spawn(move || {
            for _ in 0..400 {
                // SAFETY: a one-tick sleep.
                unsafe { sys::vTaskDelay(1) };
                // SAFETY: the first TCB field is the saved stack top, and the
                // spinner, preempted, has its context frame there.
                let (pc, a0) = unsafe {
                    let top = *(spinner as *const *const u32);
                    (*top.add(1), *top.add(3))
                };
                criterion::emit(&format!(
                    "{{\"type\":\"sample\",\"pc\":\"{pc:#x}\",\"a0\":\"{a0:#x}\"}}"
                ));
            }
        })
        .unwrap();
}

/// One 32 B XChaCha20-Poly1305 seal per library, as the benches do it.
#[cfg(feature = "rng-diag")]
pub fn run() {
    use core::hint::black_box;
    if option_env!("RIND_BENCH_SPIN").is_some() {
        start_sampler();
    }
    // `RIND_BENCH_SPIN=chacha-16k-{rind,ring}`: the 16 KB ChaCha20-Poly1305
    // seal, to split its time between ChaCha20 and Poly1305.
    let mut big = vec![0u8; 16384];
    let aad32 = [0u8; 32];
    if option_env!("RIND_BENCH_SPIN") == Some("chacha-16k-rind") {
        let k = rind::aead::Key::new(&rind::aead::CHACHA20_POLY1305, &[0u8; 32]).unwrap();
        loop {
            let n = rind::aead::Nonce::assume_unique_for_key([0u8; 12]);
            black_box(k.seal_in_place_separate_tag(n, &aad32, &mut big).unwrap());
        }
    }
    if option_env!("RIND_BENCH_SPIN") == Some("chacha-16k-ring") {
        use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
        let k = LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, &[0u8; 32]).unwrap());
        loop {
            let n = Nonce::assume_unique_for_key([0u8; 12]);
            let tag = k
                .seal_in_place_separate_tag(n, Aad::from(&aad32), &mut big)
                .unwrap();
            black_box(tag.as_ref());
        }
    }
    // `RIND_BENCH_SPIN=hmac-64-rind`, `hkdf-expand-rind`: the HMAC-SHA256
    // 64 B and HKDF-SHA256 expand rows' rind calls, as the benches make them.
    if option_env!("RIND_BENCH_SPIN") == Some("hmac-64-rind") {
        let input = [0u8; 64];
        loop {
            black_box(rind::hmac::sign(
                &rind::hmac::Key::new(&rind::hmac::HMAC_SHA256, &[0u8; 32]),
                black_box(&input),
            ));
        }
    }
    if option_env!("RIND_BENCH_SPIN") == Some("hkdf-expand-rind") {
        let prk = rind::hkdf::Salt::new(&rind::hkdf::HKDF_SHA256, b"salt").extract(&[0u8; 32]);
        let info: [&[u8]; 1] = [b"label"];
        loop {
            let mut out = [0u8; 32];
            prk.expand(&info, out.len()).fill(&mut out).unwrap();
            black_box(out);
        }
    }
    let key = [0u8; 32];
    let aad = [0u8; 32];
    let mut buf = vec![0u8; 32];
    {
        let k = rind::aead::Key::new(&rind::aead::CHACHA20_POLY1305, &key).unwrap();
        measure("chacha20poly1305/rind/32B/before-rng", || {
            let n = rind::aead::Nonce::assume_unique_for_key([0u8; 12]);
            black_box(k.seal_in_place_separate_tag(n, &aad, &mut buf).unwrap());
        });
    }
    {
        use chacha20poly1305::{AeadInOut, KeyInit};
        let k = chacha20poly1305::XChaCha20Poly1305::new_from_slice(&key).unwrap();
        measure("xchacha20poly1305/rustcrypto/32B", || {
            let mut nonce = [0u8; 24];
            rind::rand::fill(&mut nonce).unwrap();
            let tag = k
                .encrypt_inout_detached((&nonce).into(), &aad, buf.as_mut_slice().into())
                .unwrap();
            black_box(tag);
        });
    }
    {
        let k = rind::aead::RandomizedNonceKey::new(&key);
        let mut v = Vec::with_capacity(48);
        // `RIND_BENCH_SPIN=rind` at build time: seal forever, for sampling
        // the PC over JTAG.
        while option_env!("RIND_BENCH_SPIN") == Some("rind") {
            v.clear();
            v.resize(32, 0);
            black_box(k.seal_in_place_append_tag(&aad, &mut v).unwrap());
        }
        measure("xchacha20poly1305/rind/32B", || {
            v.clear();
            v.resize(32, 0);
            black_box(k.seal_in_place_append_tag(&aad, &mut v).unwrap());
        });
    }
    {
        let mut ct = vec![0u8; 48];
        while option_env!("RIND_BENCH_SPIN") == Some("libcrux") {
            let nonce = [0u8; 24];
            libcrux_chacha20poly1305::xchacha20_poly1305::encrypt(
                &key, &buf, &mut ct, &aad, &nonce,
            )
            .unwrap();
            black_box(&ct);
        }
        measure("xchacha20poly1305/libcrux/32B", || {
            let mut nonce = [0u8; 24];
            rind::rand::fill(&mut nonce).unwrap();
            libcrux_chacha20poly1305::xchacha20_poly1305::encrypt(
                &key, &buf, &mut ct, &aad, &nonce,
            )
            .unwrap();
            black_box(&ct);
        });
    }
    {
        let k = rind::aead::Key::new(&rind::aead::CHACHA20_POLY1305, &key).unwrap();
        measure("chacha20poly1305/rind/32B/after-rng", || {
            let n = rind::aead::Nonce::assume_unique_for_key([0u8; 12]);
            black_box(k.seal_in_place_separate_tag(n, &aad, &mut buf).unwrap());
        });
    }
    measure("rand-fill-24B", || {
        let mut nonce = [0u8; 24];
        rind::rand::fill(&mut nonce).unwrap();
        black_box(nonce);
    });
}
