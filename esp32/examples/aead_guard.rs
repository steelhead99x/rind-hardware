// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! Diagnoses out-of-bounds writes in each library's ChaCha20-Poly1305 seal:
//! seals into a buffer followed by guard bytes, at many lengths, and checks
//! the guards and the heap after each call.

use esp_idf_svc::sys;

fn heap_ok() -> bool {
    // SAFETY: walks ESP-IDF's heaps; no other task allocates meanwhile.
    unsafe { sys::heap_caps_check_integrity_all(true) }
}

fn main() {
    sys::link_patches();
    // Same thread as the benches: core 1, priority 20, 160 KB stack.
    esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration {
        name: Some(c"probe"),
        stack_size: 160 * 1024,
        priority: 20,
        pin_to_core: Some(esp_idf_svc::hal::cpu::Core::Core1),
        ..Default::default()
    }
    .set()
    .unwrap();
    std::thread::Builder::new()
        .stack_size(160 * 1024)
        .spawn(probe)
        .unwrap()
        .join()
        .unwrap();
}

fn probe() {
    let key = [0x42u8; 32];
    let nonce = [7u8; 12];
    let aad = [1u8; 12];
    println!(
        "RB:{{\"type\":\"start\",\"probe\":\"aead_guard\",\"heap_ok\":{}}}",
        heap_ok()
    );
    for len in [
        0usize, 1, 15, 16, 17, 31, 32, 33, 63, 64, 65, 114, 127, 128, 129, 255, 256, 257, 1350,
    ] {
        for lib in ["rind", "ring", "rustcrypto", "libcrux"] {
            let mut buf = vec![0xAAu8; len + 64];
            for (i, b) in buf[..len].iter_mut().enumerate() {
                *b = i as u8;
            }
            let (data, guard) = buf.split_at_mut(len);
            let tag: Vec<u8> = match lib {
                "rind" => {
                    let k = rind::aead::Key::new(&rind::aead::CHACHA20_POLY1305, &key).unwrap();
                    let t = k
                        .seal_in_place_separate_tag(
                            rind::aead::Nonce::assume_unique_for_key(nonce),
                            &aad,
                            data,
                        )
                        .unwrap();
                    t.as_ref().to_vec()
                }
                "ring" => {
                    let k = ring::aead::LessSafeKey::new(
                        ring::aead::UnboundKey::new(&ring::aead::CHACHA20_POLY1305, &key).unwrap(),
                    );
                    let t = k
                        .seal_in_place_separate_tag(
                            ring::aead::Nonce::assume_unique_for_key(nonce),
                            ring::aead::Aad::from(&aad),
                            data,
                        )
                        .unwrap();
                    t.as_ref().to_vec()
                }
                "rustcrypto" => {
                    use chacha20poly1305::{AeadInOut, KeyInit};
                    let k = chacha20poly1305::ChaCha20Poly1305::new_from_slice(&key).unwrap();
                    let t = k
                        .encrypt_inout_detached((&nonce).into(), &aad, data.into())
                        .unwrap();
                    t.to_vec()
                }
                _ => {
                    let pt = data.to_vec();
                    let mut t = [0u8; 16];
                    libcrux_chacha20poly1305::encrypt_detached(
                        &key, &pt, data, &mut t, &aad, &nonce,
                    )
                    .unwrap();
                    t.to_vec()
                }
            };
            let guard_ok = guard.iter().all(|&b| b == 0xAA);
            let h = heap_ok();
            println!(
                "RB:{{\"type\":\"guard\",\"lib\":\"{lib}\",\"len\":{len},\"guard_ok\":{guard_ok},\"heap_ok\":{h},\"tag\":\"{:02x}{:02x}{:02x}{:02x}\"}}",
                tag[0], tag[1], tag[2], tag[3]
            );
            if !h {
                println!("RB:{{\"type\":\"done\",\"stopped\":\"heap\"}}");
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        }
    }
    println!("RB:{{\"type\":\"done\"}}");
}
