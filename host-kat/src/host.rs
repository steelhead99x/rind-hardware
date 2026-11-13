// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! The libraries only a host races (aws-lc-rs, graviola, OpenSSL), checked
//! against the same published vectors as `vectors.rs` and against rind's
//! output for the benches' 1350 B and 16 KB inputs.

fn hex(s: &str) -> Vec<u8> {
    let s: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    s.chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

fn report(failed: &mut Vec<(String, String)>, check: &str, lib: &str, ok: bool) {
    criterion::emit(&format!(
        "{{\"type\":\"check\",\"check\":\"{check}\",\"library\":\"{lib}\",\"pass\":{ok}}}"
    ));
    if !ok {
        let prefix = check.split('/').next().unwrap_or(check);
        criterion::exclude(prefix, lib);
        failed.push((prefix.to_string(), lib.to_string()));
    }
}

#[derive(Clone, Copy)]
enum Aead {
    Aes128Gcm,
    ChaCha20Poly1305,
}

/// ciphertext || tag from rind and each host-only library.
fn seal_all(
    alg: Aead,
    key: &[u8],
    nonce: &[u8; 12],
    aad: &[u8],
    pt: &[u8],
) -> Vec<(&'static str, Vec<u8>)> {
    let mut out = Vec::new();

    let ours = match alg {
        Aead::Aes128Gcm => &rind::aead::AES_128_GCM,
        Aead::ChaCha20Poly1305 => &rind::aead::CHACHA20_POLY1305,
    };
    let k = rind::aead::Key::new(ours, key).unwrap();
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

    let aws = match alg {
        Aead::Aes128Gcm => &aws_lc_rs::aead::AES_128_GCM,
        Aead::ChaCha20Poly1305 => &aws_lc_rs::aead::CHACHA20_POLY1305,
    };
    let k = aws_lc_rs::aead::LessSafeKey::new(aws_lc_rs::aead::UnboundKey::new(aws, key).unwrap());
    let mut buf = pt.to_vec();
    let tag = k
        .seal_in_place_separate_tag(
            aws_lc_rs::aead::Nonce::assume_unique_for_key(*nonce),
            aws_lc_rs::aead::Aad::from(aad),
            &mut buf,
        )
        .unwrap();
    buf.extend_from_slice(tag.as_ref());
    out.push(("aws-lc-rs", buf));

    let mut buf = pt.to_vec();
    let mut tag = [0u8; 16];
    match alg {
        Aead::Aes128Gcm => graviola::aead::AesGcm::new(key).encrypt(nonce, aad, &mut buf, &mut tag),
        Aead::ChaCha20Poly1305 => graviola::aead::ChaCha20Poly1305::new(key.try_into().unwrap())
            .encrypt(nonce, aad, &mut buf, &mut tag),
    }
    buf.extend_from_slice(&tag);
    out.push(("graviola", buf));

    let cipher = match alg {
        Aead::Aes128Gcm => openssl::symm::Cipher::aes_128_gcm(),
        Aead::ChaCha20Poly1305 => openssl::symm::Cipher::chacha20_poly1305(),
    };
    let mut tag = [0u8; 16];
    let mut ct = openssl::symm::encrypt_aead(cipher, key, Some(nonce), aad, pt, &mut tag).unwrap();
    ct.extend_from_slice(&tag);
    out.push(("openssl", ct));
    out
}

fn sha_all(bits: usize, data: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    let (ours, aws) = if bits == 256 {
        (&rind::digest::SHA256, &aws_lc_rs::digest::SHA256)
    } else {
        (&rind::digest::SHA512, &aws_lc_rs::digest::SHA512)
    };
    let (grav, ossl) = if bits == 256 {
        let mut c = graviola::hashing::sha2::Sha256Context::new();
        c.update(data);
        (c.finish().to_vec(), openssl::sha::sha256(data).to_vec())
    } else {
        let mut c = graviola::hashing::sha2::Sha512Context::new();
        c.update(data);
        (c.finish().to_vec(), openssl::sha::sha512(data).to_vec())
    };
    vec![
        ("rind", rind::digest::digest(ours, data).as_ref().to_vec()),
        (
            "aws-lc-rs",
            aws_lc_rs::digest::digest(aws, data).as_ref().to_vec(),
        ),
        ("graviola", grav),
        ("openssl", ossl),
    ]
}

fn hmac_sha256_all(key: &[u8], data: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    let mut g = graviola::hashing::hmac::Hmac::<graviola::hashing::Sha256>::new(key);
    g.update(data);
    let pkey = openssl::pkey::PKey::hmac(key).unwrap();
    let mut signer =
        openssl::sign::Signer::new(openssl::hash::MessageDigest::sha256(), &pkey).unwrap();
    signer.update(data).unwrap();
    let aws = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, key);
    vec![
        (
            "rind",
            rind::hmac::sign(&rind::hmac::Key::new(&rind::hmac::HMAC_SHA256, key), data)
                .as_ref()
                .to_vec(),
        ),
        (
            "aws-lc-rs",
            aws_lc_rs::hmac::sign(&aws, data).as_ref().to_vec(),
        ),
        ("graviola", g.finish().as_ref().to_vec()),
        ("openssl", signer.sign_to_vec().unwrap()),
    ]
}

/// Every host check; returns the (operation prefix, library) pairs that failed.
pub fn check_all() -> Vec<(String, String)> {
    let mut failed = Vec::new();
    let f = &mut failed;

    // AES-128-GCM: NIST's GCM spec, test case 2, then the benches' 1350 B input.
    let want = hex("0388dace60b6a392f328c2b971b2fe78 ab6e47d42cec13bdf53a67b21257bddf");
    for (lib, got) in seal_all(Aead::Aes128Gcm, &[0; 16], &[0; 12], &[], &[0; 16]) {
        report(f, "aes128-gcm/nist-gcm-tc2", lib, got == want);
    }
    let all = seal_all(Aead::Aes128Gcm, &[0; 16], &[0; 12], &[0; 32], &[0; 1350]);
    for (lib, got) in &all[1..] {
        report(
            f,
            "aes128-gcm/1350B-agrees-with-rind",
            lib,
            *got == all[0].1,
        );
    }

    // ChaCha20-Poly1305: RFC 8439 section 2.8.2, then 1350 B.
    let key = hex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
    let nonce: [u8; 12] = hex("070000004041424344454647").try_into().unwrap();
    let aad = hex("50515253c0c1c2c3c4c5c6c7");
    let pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    let want = hex(concat!(
        "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b",
        "1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc",
        "3ff4def08e4b7a9de576d26586cec64b6116",
        "1ae10b594f09e26a7e902ecbd0600691"
    ));
    for (lib, got) in seal_all(Aead::ChaCha20Poly1305, &key, &nonce, &aad, pt) {
        report(f, "chacha20poly1305/rfc8439-2.8.2", lib, got == want);
    }
    let all = seal_all(
        Aead::ChaCha20Poly1305,
        &[0; 32],
        &[0; 12],
        &[0; 32],
        &[0; 1350],
    );
    for (lib, got) in &all[1..] {
        report(
            f,
            "chacha20poly1305/1350B-agrees-with-rind",
            lib,
            *got == all[0].1,
        );
    }

    // SHA-256 and SHA-512 of "abc" (FIPS 180-2), and of 16 KB against rind.
    let want = hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    for (lib, got) in sha_all(256, b"abc") {
        report(f, "sha256/fips180-abc", lib, got == want);
    }
    let all = sha_all(256, &[0x5a; 16384]);
    for (lib, got) in &all[1..] {
        report(f, "sha256/16KB-agrees-with-rind", lib, *got == all[0].1);
    }
    let want = hex(concat!(
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a",
        "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    ));
    for (lib, got) in sha_all(512, b"abc") {
        report(f, "sha512/fips180-abc", lib, got == want);
    }

    // HMAC-SHA256: RFC 4231 test case 2.
    let want = hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    for (lib, got) in hmac_sha256_all(b"Jefe", b"what do ya want for nothing?") {
        report(f, "hmac-sha256/rfc4231-tc2", lib, got == want);
    }

    // X25519: RFC 7748 section 6.1 through each library's static-key API.
    let alice: [u8; 32] = hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
        .try_into()
        .unwrap();
    let bob: [u8; 32] = hex("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f")
        .try_into()
        .unwrap();
    let want = hex("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
    let g = graviola::key_agreement::x25519::StaticPrivateKey::from_array(&alice)
        .diffie_hellman(&graviola::key_agreement::x25519::PublicKey::from_array(
            &bob,
        ))
        .unwrap();
    report(
        f,
        "x25519/rfc7748-6.1",
        "graviola",
        g.0.as_slice() == want.as_slice(),
    );
    let a =
        openssl::pkey::PKey::private_key_from_raw_bytes(&alice, openssl::pkey::Id::X25519).unwrap();
    let b =
        openssl::pkey::PKey::public_key_from_raw_bytes(&bob, openssl::pkey::Id::X25519).unwrap();
    let mut d = openssl::derive::Deriver::new(&a).unwrap();
    d.set_peer(&b).unwrap();
    report(
        f,
        "x25519/rfc7748-6.1",
        "openssl",
        d.derive_to_vec().unwrap() == want,
    );
    let a =
        aws_lc_rs::agreement::PrivateKey::from_private_key(&aws_lc_rs::agreement::X25519, &alice)
            .unwrap();
    let got = aws_lc_rs::agreement::agree(
        &a,
        aws_lc_rs::agreement::UnparsedPublicKey::new(&aws_lc_rs::agreement::X25519, bob),
        aws_lc_rs::error::Unspecified,
        |k| Ok(k.to_vec()),
    )
    .unwrap();
    report(f, "x25519/rfc7748-6.1", "aws-lc-rs", got == want);

    failed
}
