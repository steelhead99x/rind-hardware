// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! A hardware baseline, not one of the report's libraries: ESP-IDF's mbedTLS
//! port driving the ESP32-S3's AES and SHA accelerators. `esp_aes_gcm_*` runs
//! AES on the AES peripheral and GHASH in software; `mbedtls_sha256` runs on
//! the SHA peripheral. Its rows are named `esp32s3-hw` and time the same
//! inputs as the benches' rustcrypto and libcrux rows, sealing out of place
//! as libcrux's do.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};
use esp_idf_svc::sys;

pub const LIB: &str = "esp32s3-hw";

/// An initialised hardware GCM context with a 128-bit key.
struct Gcm(Box<sys::esp_gcm_context>);

impl Gcm {
    fn new(key: &[u8; 16]) -> Self {
        // SAFETY: an all-zero context is what `esp_aes_gcm_init` expects to
        // initialise, and the key pointer and bit length describe `key`.
        unsafe {
            let mut ctx: Box<sys::esp_gcm_context> = Box::new(core::mem::zeroed());
            sys::esp_aes_gcm_init(&mut *ctx);
            let rc = sys::esp_aes_gcm_setkey(
                &mut *ctx,
                sys::mbedtls_cipher_id_t_MBEDTLS_CIPHER_ID_AES,
                key.as_ptr(),
                128,
            );
            assert_eq!(rc, 0, "esp_aes_gcm_setkey");
            Self(ctx)
        }
    }

    fn seal(&mut self, nonce: &[u8; 12], aad: &[u8], pt: &[u8], ct: &mut [u8], tag: &mut [u8; 16]) {
        assert_eq!(pt.len(), ct.len());
        // SAFETY: every pointer and length describes a live slice, and
        // `ct` does not overlap `pt`.
        let rc = unsafe {
            sys::esp_aes_gcm_crypt_and_tag(
                &mut *self.0,
                sys::MBEDTLS_GCM_ENCRYPT as i32,
                pt.len(),
                nonce.as_ptr(),
                nonce.len(),
                aad.as_ptr(),
                aad.len(),
                pt.as_ptr(),
                ct.as_mut_ptr(),
                tag.len(),
                tag.as_mut_ptr(),
            )
        };
        assert_eq!(rc, 0, "esp_aes_gcm_crypt_and_tag");
    }
}

impl Drop for Gcm {
    fn drop(&mut self) {
        // SAFETY: the context was initialised by `esp_aes_gcm_init`.
        unsafe { sys::esp_aes_gcm_free(&mut *self.0) }
    }
}

/// Ciphertext || tag, for the known-answer checks.
pub fn aes128_gcm_seal(key: &[u8; 16], nonce: &[u8; 12], aad: &[u8], pt: &[u8]) -> Vec<u8> {
    let mut ct = vec![0u8; pt.len()];
    let mut tag = [0u8; 16];
    Gcm::new(key).seal(nonce, aad, pt, &mut ct, &mut tag);
    ct.extend_from_slice(&tag);
    ct
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    // SAFETY: the pointers and length describe `data` and `out`.
    let rc = unsafe { sys::mbedtls_sha256(data.as_ptr(), data.len(), out.as_mut_ptr(), 0) };
    assert_eq!(rc, 0, "mbedtls_sha256");
    out
}

/// The hardware rows, at the sizes the benches' AES-128-GCM and SHA-256
/// groups time, with the IDs the report would give them.
#[cfg(any(feature = "aes_gcm", feature = "sha2"))]
pub fn benches(failed: &[(String, String)]) {
    let skip = |op: &str| {
        failed
            .iter()
            .any(|(p, lib)| lib == LIB && op.starts_with(p.as_str()))
    };
    let mut c = Criterion::default();

    #[cfg(feature = "aes_gcm")]
    if !skip("aes128-gcm") {
        let mut group = c.benchmark_group("aes128-gcm");
        for (size, size_name, aad) in [
            (32, "32B"),
            (1200, "1200B"),
            (1350, "1350B"),
            (16384, "16KB"),
        ]
        .map(|(s, n)| (s, n, &[0u8; 32][..]))
        {
            let buf = vec![0u8; size];
            let mut ct = vec![0u8; size];
            let mut tag = [0u8; 16];
            let mut key = Gcm::new(&[0u8; 16]);
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_function(BenchmarkId::new(LIB, size_name), |b| {
                b.iter(|| key.seal(&[0u8; 12], aad, &buf, &mut ct, &mut tag));
            });
        }
        group.finish();
    }

    #[cfg(feature = "sha2")]
    if !skip("sha256/") {
        let mut group = c.benchmark_group("sha256");
        for (size, size_name) in [
            (32, "32B"),
            (64, "64B"),
            (1200, "1200B"),
            (1350, "1350B"),
            (8192, "8KB"),
            (16384, "16KB"),
        ] {
            let input = vec![0u8; size];
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_with_input(BenchmarkId::new(LIB, size_name), &input, |b, input| {
                b.iter(|| black_box(sha256(input)));
            });
        }
        group.finish();
    }
}
