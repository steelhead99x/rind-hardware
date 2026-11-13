// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! Known-answer checks, run before any timing: each library computes
//! published test vectors, and the AEADs also seal the benches' own 1350 B
//! input, which every library must agree on. A library that fails an
//! operation's check is excluded from that operation's benchmarks.
//!
//! Shared by the ESP32-S3, nRF52840 and RP2040 firmware; the hardware
//! baseline (`hw`) exists on the ESP32-S3 only. ring 0.17 is left out on
//! bare-metal targets without CAS (ARMv6-M): its Arm assembly uses NEON.

extern crate alloc;
#[allow(unused_imports)]
use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

#[cfg(target_os = "espidf")]
use crate::hw;

/// With `RIND_BENCH_TRACE` set at build time, prints each step of the checks
/// and the heap's integrity, to find a check that corrupts memory.
fn trace(step: &str) {
    if option_env!("RIND_BENCH_TRACE").is_some() {
        // SAFETY: walks ESP-IDF's heaps; no other task allocates meanwhile.
        #[cfg(target_os = "espidf")]
        let ok = unsafe { esp_idf_svc::sys::heap_caps_check_integrity_all(true) };
        #[cfg(not(target_os = "espidf"))]
        let ok = "null";
        criterion::emit(&format!(
            "{{\"type\":\"trace\",\"step\":\"{step}\",\"heap_ok\":{ok}}}"
        ));
    }
}

fn hex(s: &str) -> Vec<u8> {
    let s: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    s.chunks(2)
        .map(|p| u8::from_str_radix(core::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

/// One check's outcome: printed, and the library excluded on failure.
fn report(failed: &mut Vec<(String, String)>, check: &str, prefixes: &[&str], lib: &str, ok: bool) {
    criterion::emit(&format!(
        "{{\"type\":\"check\",\"check\":\"{check}\",\"library\":\"{lib}\",\"pass\":{ok}}}"
    ));
    if !ok {
        for prefix in prefixes {
            criterion::exclude(prefix, lib);
            failed.push(((*prefix).to_string(), lib.to_string()));
        }
    }
}

/// Seals `pt` under each library's AES-128-GCM, returning ciphertext || tag.
fn aes128_gcm_all(
    key: &[u8; 16],
    nonce: &[u8; 12],
    aad: &[u8],
    pt: &[u8],
) -> Vec<(&'static str, Vec<u8>)> {
    let mut out = Vec::new();

    let k = rind::aead::Key::new(&rind::aead::AES_128_GCM, key).unwrap();
    let mut buf = pt.to_vec();
    let tag = k
        .seal_in_place_separate_tag(
            rind::aead::Nonce::assume_unique_for_key(*nonce),
            aad,
            &mut buf,
        )
        .unwrap();
    trace("chacha/rind/sealed");
    buf.extend_from_slice(tag.as_ref());
    out.push(("rind", buf));
    #[cfg(not(all(target_os = "none", not(target_has_atomic = "ptr"))))]
    {
        trace("chacha/ring");

        let k = ring::aead::LessSafeKey::new(
            ring::aead::UnboundKey::new(&ring::aead::AES_128_GCM, key).unwrap(),
        );
        let mut buf = pt.to_vec();
        let tag = k
            .seal_in_place_separate_tag(
                ring::aead::Nonce::assume_unique_for_key(*nonce),
                ring::aead::Aad::from(aad),
                &mut buf,
            )
            .unwrap();
        buf.extend_from_slice(tag.as_ref());
        out.push(("ring", buf));
    }

    {
        use aes_gcm::{AeadInOut, KeyInit};
        let k = aes_gcm::Aes128Gcm::new_from_slice(key).unwrap();
        let mut buf = pt.to_vec();
        let tag = k
            .encrypt_inout_detached(nonce.into(), aad, buf.as_mut_slice().into())
            .unwrap();
        buf.extend_from_slice(tag.as_slice());
        out.push(("rustcrypto", buf));
    }
    trace("chacha/libcrux");

    {
        let k = libcrux_aes::AesGcm128Key::from(*key);
        let n = libcrux_aes::AesGcm128Nonce::from(*nonce);
        let mut tag = libcrux_aes::AesGcm128Tag::from([0u8; 16]);
        let mut ct = vec![0u8; pt.len()];
        k.encrypt(&mut ct, &mut tag, &n, aad, pt).unwrap();
        let tag: &[u8; 16] = tag.as_ref();
        ct.extend_from_slice(tag);
        out.push(("libcrux", ct));
    }

    #[cfg(target_os = "espidf")]
    out.push((hw::LIB, hw::aes128_gcm_seal(key, nonce, aad, pt)));
    out
}

fn chacha_all(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    pt: &[u8],
) -> Vec<(&'static str, Vec<u8>)> {
    let mut out = Vec::new();

    trace("chacha/rind/key");
    let k = rind::aead::Key::new(&rind::aead::CHACHA20_POLY1305, key).unwrap();
    trace("chacha/rind/seal");
    let mut buf = pt.to_vec();
    let tag = k
        .seal_in_place_separate_tag(
            rind::aead::Nonce::assume_unique_for_key(*nonce),
            aad,
            &mut buf,
        )
        .unwrap();
    buf.extend_from_slice(tag.as_ref());
    out.push(("rind", buf));

    #[cfg(not(all(target_os = "none", not(target_has_atomic = "ptr"))))]
    {
        let k = ring::aead::LessSafeKey::new(
            ring::aead::UnboundKey::new(&ring::aead::CHACHA20_POLY1305, key).unwrap(),
        );
        let mut buf = pt.to_vec();
        let tag = k
            .seal_in_place_separate_tag(
                ring::aead::Nonce::assume_unique_for_key(*nonce),
                ring::aead::Aad::from(aad),
                &mut buf,
            )
            .unwrap();
        buf.extend_from_slice(tag.as_ref());
        out.push(("ring", buf));
    }

    {
        use chacha20poly1305::{AeadInOut, KeyInit};
        let k = chacha20poly1305::ChaCha20Poly1305::new_from_slice(key).unwrap();
        let mut buf = pt.to_vec();
        let tag = k
            .encrypt_inout_detached(nonce.into(), aad, buf.as_mut_slice().into())
            .unwrap();
        buf.extend_from_slice(tag.as_slice());
        out.push(("rustcrypto", buf));
    }

    let mut ct = vec![0u8; pt.len()];
    let mut tag = [0u8; 16];
    libcrux_chacha20poly1305::encrypt_detached(key, pt, &mut ct, &mut tag, aad, nonce).unwrap();
    ct.extend_from_slice(&tag);
    out.push(("libcrux", ct));
    out
}

fn sha256_all(data: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    use sha2::Digest;
    vec![
        (
            "rind",
            rind::digest::digest(&rind::digest::SHA256, data)
                .as_ref()
                .to_vec(),
        ),
        #[cfg(not(all(target_os = "none", not(target_has_atomic = "ptr"))))]
        (
            "ring",
            ring::digest::digest(&ring::digest::SHA256, data)
                .as_ref()
                .to_vec(),
        ),
        ("rustcrypto", sha2::Sha256::digest(data).to_vec()),
        ("libcrux", libcrux_sha2::sha256(data).to_vec()),
        #[cfg(target_os = "espidf")]
        (hw::LIB, hw::sha256(data).to_vec()),
    ]
}

/// SHA3-256, SHA3-384 or SHA3-512 (by output bytes) from every library.
#[cfg(feature = "sha3")]
fn sha3_all(bits: usize, data: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    use sha3::Digest;
    let (ours, rc, crux): (_, Vec<u8>, Vec<u8>) = match bits {
        256 => (
            &rind::digest::SHA3_256,
            sha3::Sha3_256::digest(data).to_vec(),
            libcrux_sha3::sha256(data).to_vec(),
        ),
        384 => (
            &rind::digest::SHA3_384,
            sha3::Sha3_384::digest(data).to_vec(),
            libcrux_sha3::sha384(data).to_vec(),
        ),
        _ => (
            &rind::digest::SHA3_512,
            sha3::Sha3_512::digest(data).to_vec(),
            libcrux_sha3::sha512(data).to_vec(),
        ),
    };
    vec![
        ("rind", rind::digest::digest(ours, data).as_ref().to_vec()),
        ("rustcrypto", rc),
        ("libcrux", crux),
    ]
}

fn sha512_all(data: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    use sha2::Digest;
    vec![
        (
            "rind",
            rind::digest::digest(&rind::digest::SHA512, data)
                .as_ref()
                .to_vec(),
        ),
        #[cfg(not(all(target_os = "none", not(target_has_atomic = "ptr"))))]
        (
            "ring",
            ring::digest::digest(&ring::digest::SHA512, data)
                .as_ref()
                .to_vec(),
        ),
        ("rustcrypto", sha2::Sha512::digest(data).to_vec()),
        ("libcrux", libcrux_sha2::sha512(data).to_vec()),
    ]
}

fn hmac_sha256_all(key: &[u8], data: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    use hmac::{KeyInit, Mac};
    let mut rc = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).unwrap();
    rc.update(data);
    let mut crux = [0u8; 32];
    libcrux_hmac::hmac_sha2_256(&mut crux, key, data);
    vec![
        (
            "rind",
            rind::hmac::sign(&rind::hmac::Key::new(&rind::hmac::HMAC_SHA256, key), data)
                .as_ref()
                .to_vec(),
        ),
        #[cfg(not(all(target_os = "none", not(target_has_atomic = "ptr"))))]
        (
            "ring",
            ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key), data)
                .as_ref()
                .to_vec(),
        ),
        ("rustcrypto", rc.finalize().into_bytes().to_vec()),
        ("libcrux", crux.to_vec()),
    ]
}

/// Every check; returns the (operation prefix, library) pairs that failed.
pub fn check_all() -> Vec<(String, String)> {
    let mut failed = Vec::new();
    let f = &mut failed;
    trace("checks");

    #[cfg(any(feature = "aes_gcm", feature = "cipher"))]
    {
        // AES-128-GCM: NIST's GCM spec, test case 2, then the benches' 1350 B
        // input under a 32 B AAD, where every library must match rind.
        let aes = ["aes128-gcm", "quic-header-protection/aes128"];
        let want = hex("0388dace60b6a392f328c2b971b2fe78 ab6e47d42cec13bdf53a67b21257bddf");
        for (lib, got) in aes128_gcm_all(&[0; 16], &[0; 12], &[], &[0; 16]) {
            report(f, "aes128-gcm/nist-gcm-tc2", &aes, lib, got == want);
        }
        let all = aes128_gcm_all(&[0; 16], &[0; 12], &[0; 32], &[0; 1350]);
        let reference = all[0].1.clone();
        for (lib, got) in all.into_iter().skip(1) {
            report(
                f,
                "aes128-gcm/1350B-agrees-with-rind",
                &aes,
                lib,
                got == reference,
            );
        }
    }

    #[cfg(any(
        feature = "chacha20poly1305",
        feature = "xchacha20poly1305",
        feature = "cipher"
    ))]
    {
        // ChaCha20-Poly1305: RFC 8439 section 2.8.2.
        let chacha = ["chacha20poly1305", "quic-header-protection/chacha20"];
        let key: [u8; 32] = hex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f")
            .try_into()
            .unwrap();
        let nonce: [u8; 12] = hex("070000004041424344454647").try_into().unwrap();
        let aad = hex("50515253c0c1c2c3c4c5c6c7");
        let pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let want = hex(concat!(
            "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b",
            "1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc",
            "3ff4def08e4b7a9de576d26586cec64b6116",
            "1ae10b594f09e26a7e902ecbd0600691"
        ));
        for (lib, got) in chacha_all(&key, &nonce, &aad, pt) {
            report(
                f,
                "chacha20poly1305/rfc8439-2.8.2",
                &chacha,
                lib,
                got == want,
            );
        }
        let all = chacha_all(&[0; 32], &[0; 12], &[0; 32], &[0; 1350]);
        let reference = all[0].1.clone();
        for (lib, got) in all.into_iter().skip(1) {
            report(
                f,
                "chacha20poly1305/1350B-agrees-with-rind",
                &chacha,
                lib,
                got == reference,
            );
        }
    }

    #[cfg(feature = "sha2")]
    {
        // SHA-256 and SHA-512 of "abc", FIPS 180-2's examples, and of 16 KB,
        // where each must match rind.
        let want = hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        for (lib, got) in sha256_all(b"abc") {
            report(f, "sha256/fips180-abc", &["sha256/"], lib, got == want);
        }
        let big = vec![0x5au8; 16384];
        let all = sha256_all(&big);
        let reference = all[0].1.clone();
        for (lib, got) in all.into_iter().skip(1) {
            report(
                f,
                "sha256/16KB-agrees-with-rind",
                &["sha256/"],
                lib,
                got == reference,
            );
        }
        let want = hex(concat!(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a",
            "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        ));
        for (lib, got) in sha512_all(b"abc") {
            report(f, "sha512/fips180-abc", &["sha512/"], lib, got == want);
        }
    }

    #[cfg(feature = "sha3")]
    {
        // SHA3 of "abc", FIPS 202's examples (NIST's example values), and of
        // 16 KB, where each must match rind.
        for (bits, name, want) in [
            (
                256,
                "sha3-256",
                "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532",
            ),
            (
                384,
                "sha3-384",
                concat!(
                    "ec01498288516fc926459f58e2c6ad8df9b473cb0fc08c25",
                    "96da7cf0e49be4b298d88cea927ac7f539f1edf228376d25"
                ),
            ),
            (
                512,
                "sha3-512",
                concat!(
                    "b751850b1a57168a5693cd924b6b096e08f621827444f70d884f5d0240d2712e",
                    "10e116e9192af3c91a7ec57647e3934057340b4cf408d5a56592f8274eec53f0"
                ),
            ),
        ] {
            let prefix = format!("{name}/");
            let want = hex(want);
            for (lib, got) in sha3_all(bits, b"abc") {
                report(
                    f,
                    &format!("{name}/fips202-abc"),
                    &[&prefix],
                    lib,
                    got == want,
                );
            }
            let all = sha3_all(bits, &vec![0x5au8; 16384]);
            let reference = all[0].1.clone();
            for (lib, got) in all.into_iter().skip(1) {
                report(
                    f,
                    &format!("{name}/16KB-agrees-with-rind"),
                    &[&prefix],
                    lib,
                    got == reference,
                );
            }
        }
    }

    #[cfg(any(feature = "hmac", feature = "hkdf"))]
    {
        // HMAC-SHA256: RFC 4231 test case 2.
        let want = hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        for (lib, got) in hmac_sha256_all(b"Jefe", b"what do ya want for nothing?") {
            report(
                f,
                "hmac-sha256/rfc4231-tc2",
                &["hmac-sha256/", "hkdf-sha256/"],
                lib,
                got == want,
            );
        }
    }

    #[cfg(feature = "x25519")]
    {
        // X25519: RFC 7748 section 6.1, Alice's private key and Bob's public key.
        let alice: [u8; 32] =
            hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
                .try_into()
                .unwrap();
        let bob: [u8; 32] = hex("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f")
            .try_into()
            .unwrap();
        let want = hex("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
        let x = ["x25519-ecdh/"];
        let got = rind::agreement::PrivateKey::from_scalar(&rind::agreement::X25519, &alice)
            .unwrap()
            .agree(&bob)
            .unwrap();
        report(
            f,
            "x25519/rfc7748-6.1",
            &x,
            "rind",
            got.as_ref() == want.as_slice(),
        );
        let got = x25519_dalek::x25519(alice, bob);
        report(
            f,
            "x25519/rfc7748-6.1",
            &x,
            "dalek",
            got.as_slice() == want.as_slice(),
        );
        let got = libcrux_ecdh::derive(libcrux_ecdh::Algorithm::X25519, bob, alice).unwrap();
        report(f, "x25519/rfc7748-6.1", &x, "libcrux", got == want);
        // ring takes only ephemeral X25519 keys, so it agrees with rind instead:
        // ring's key and Alice's public key, against rind's Alice and ring's public key.
        #[cfg(not(all(target_os = "none", not(target_has_atomic = "ptr"))))]
        {
            use ring::{agreement, rand};
            let rng = rand::SystemRandom::new();
            let ours = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng).unwrap();
            let ours_pub = ours.compute_public_key().unwrap();
            let alice_key =
                rind::agreement::PrivateKey::from_scalar(&rind::agreement::X25519, &alice).unwrap();
            let expect = alice_key.agree(ours_pub.as_ref()).unwrap();
            let alice_pub = alice_key.public_key();
            let peer = agreement::UnparsedPublicKey::new(&agreement::X25519, alice_pub.as_ref());
            let got = agreement::agree_ephemeral(ours, &peer, |k| k.to_vec()).unwrap();
            report(
                f,
                "x25519/agrees-with-rind",
                &x,
                "ring",
                got == expect.as_ref(),
            );
        }
    }

    #[cfg(feature = "ed25519")]
    {
        // Ed25519: RFC 8032 section 7.1, TEST 1 (an empty message).
        let seed: [u8; 32] =
            hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
                .try_into()
                .unwrap();
        let public = hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let want = hex(concat!(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155",
            "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        ));
        let ed = ["ed25519-"];
        let got = rind::signature::Ed25519KeyPair::from_seed(&seed)
            .unwrap()
            .sign(b"");
        report(
            f,
            "ed25519/rfc8032-test1",
            &ed,
            "rind",
            got.as_ref() == want.as_slice(),
        );
        #[cfg(not(all(target_os = "none", not(target_has_atomic = "ptr"))))]
        {
            let got = ring::signature::Ed25519KeyPair::from_seed_and_public_key(&seed, &public)
                .unwrap()
                .sign(b"");
            report(
                f,
                "ed25519/rfc8032-test1",
                &ed,
                "ring",
                got.as_ref() == want.as_slice(),
            );
        }
        {
            use ed25519_dalek::Signer;
            let got = ed25519_dalek::SigningKey::from_bytes(&seed)
                .sign(b"")
                .to_bytes();
            report(
                f,
                "ed25519/rfc8032-test1",
                &ed,
                "dalek",
                got.as_slice() == want.as_slice(),
            );
        }
        let got = libcrux_ed25519::sign(b"", &seed).unwrap();
        report(
            f,
            "ed25519/rfc8032-test1",
            &ed,
            "libcrux",
            got.as_slice() == want.as_slice(),
        );
    }

    #[cfg(feature = "p256")]
    {
        // P-256 ECDH: RFC 5903 section 8.1, the initiator's scalar and the
        // responder's public value.
        let i = hex("c88f01f510d9ac3f70a292daa2316de544e9aab8afe84049c62a9c57862d1433");
        let gr = hex(concat!(
            "04",
            "d12dfb5289c8d4f81208b70270398c342296970a0bccb74c736fc7554494bf63",
            "56fbf3ca366cc23e8157854c13c58d6aac23f046ada30f8353e74f33039872ab"
        ));
        let want = hex("d6840f6b42f6edafd13116e0e12565202fef8e9ece7dce03812464d04b9442de");
        let p = ["p256-ecdh"];
        let got = rind::agreement::PrivateKey::from_scalar(&rind::agreement::ECDH_P256, &i)
            .unwrap()
            .agree(&gr)
            .unwrap();
        report(
            f,
            "p256-ecdh/rfc5903-8.1",
            &p,
            "rind",
            got.as_ref() == want.as_slice(),
        );
        {
            let sk = p256::SecretKey::from_slice(&i).unwrap();
            let pk = p256::PublicKey::from_sec1_bytes(&gr).unwrap();
            let got = p256::ecdh::diffie_hellman(sk.to_nonzero_scalar(), pk.as_affine());
            let got: &[u8] = got.raw_secret_bytes().as_ref();
            report(
                f,
                "p256-ecdh/rfc5903-8.1",
                &p,
                "rustcrypto",
                got == want.as_slice(),
            );
        }
        let got = libcrux_ecdh::derive(libcrux_ecdh::Algorithm::P256, &gr[1..], &i).unwrap();
        report(
            f,
            "p256-ecdh/rfc5903-8.1",
            &p,
            "libcrux",
            got.get(..32) == Some(want.as_slice()),
        );
        // ring takes only ephemeral keys: it must agree with rind's RFC key.
        #[cfg(not(all(target_os = "none", not(target_has_atomic = "ptr"))))]
        {
            use ring::{agreement, rand};
            let rng = rand::SystemRandom::new();
            let ours =
                agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
            let ours_pub = ours.compute_public_key().unwrap();
            let rfc =
                rind::agreement::PrivateKey::from_scalar(&rind::agreement::ECDH_P256, &i).unwrap();
            let expect = rfc.agree(ours_pub.as_ref()).unwrap();
            let rfc_pub = rfc.public_key();
            let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, rfc_pub.as_ref());
            let got = agreement::agree_ephemeral(ours, &peer, |k| k.to_vec()).unwrap();
            report(
                f,
                "p256-ecdh/agrees-with-rind",
                &p,
                "ring",
                got == expect.as_ref(),
            );
        }
    }

    #[cfg(feature = "mlkem768")]
    {
        // ML-KEM-768: each library's own round trip; no cross-library check.
        let m = ["mlkem768-"];
        {
            use rind::kem::*;
            let dk = DecapsulationKey::generate(&ML_KEM_768).unwrap();
            let (ct, ss) = dk.public_key().encapsulate().unwrap();
            let ok = dk.decapsulate(ct.as_ref()).unwrap().as_ref() == ss.as_ref();
            report(f, "mlkem768/round-trip", &m, "rind", ok);
        }
        {
            use libcrux_ml_kem::mlkem768;
            let mut r = [0u8; 64];
            getrandom::fill(&mut r).unwrap();
            let (dk, ek) = mlkem768::generate_key_pair(r).into_parts();
            let mut r = [0u8; 32];
            getrandom::fill(&mut r).unwrap();
            let (ct, ss) = mlkem768::encapsulate(&ek, r);
            let ok = mlkem768::decapsulate(&dk, &ct) == ss;
            report(f, "mlkem768/round-trip", &m, "libcrux-ml-kem", ok);
        }
        {
            use ml_kem::{Decapsulate, Encapsulate, Kem, MlKem768};
            let (dk, ek) = MlKem768::generate_keypair();
            let (ct, ss) = ek.encapsulate();
            let ok = dk.decapsulate_slice(ct.as_slice()).unwrap() == ss;
            report(f, "mlkem768/round-trip", &m, "rustcrypto", ok);
        }
    }

    criterion::emit(&format!(
        "{{\"type\":\"checks-done\",\"failed\":{}}}",
        failed.len()
    ));
    failed
}
