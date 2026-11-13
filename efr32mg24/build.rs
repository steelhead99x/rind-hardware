// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! Puts `memory.x` on the linker's path, and writes each of rind-bench's bench
//! files to `OUT_DIR` for a `no_std` crate: `std::` paths become `core::`
//! (the files use only `std::hint` and `std::num` from it, both in core), and
//! the std prelude's `Vec`, `String`, `ToString`, `ToOwned`, `Box`, `vec!` and
//! `format!` are imported from `alloc`; and `mod criterion;`, the shared
//! `benches/criterion/mod.rs`, gets its path in `OUT_DIR`. Nothing else in
//! the files changes.

use std::{env, fs, path::PathBuf};

const BENCHES: &[&str] = &[
    "aes_gcm",
    "chacha20poly1305",
    "xchacha20poly1305",
    "cipher",
    "x25519",
    "p256",
    "ed25519",
    "sha2",
    "sha3",
    "hmac",
    "hkdf",
    "mlkem768",
];

const PRELUDE: &str = "#[allow(unused_imports)]\nuse alloc::{borrow::ToOwned, boxed::Box, format, string::{String, ToString}, vec, vec::Vec};\n";

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    write_pad(&out);
    println!("cargo::rustc-link-search={}", out.display());
    println!("cargo::rerun-if-changed=memory.x");
    let shared = format!("{}/rind-bench/benches/criterion/mod.rs", rind_dir());
    println!("cargo::rerun-if-changed={shared}");
    let shared_out = out.join("criterion.rs");
    fs::write(&shared_out, port(&fs::read_to_string(&shared).unwrap())).unwrap();
    let declare = format!(
        "#[path = {:?}]\nmod criterion;",
        shared_out.display().to_string()
    );
    for b in BENCHES {
        let src = format!("{}/rind-bench/benches/{b}.rs", rind_dir());
        println!("cargo::rerun-if-changed={src}");
        let text = fs::read_to_string(&src).unwrap();
        let text = port(&text).replacen("mod criterion;", &declare, 1);
        fs::write(out.join(format!("{b}.rs")), text).unwrap();
    }
}

/// The rind checkout: the submodule, or `RIND_DIR` while developing a patch.
fn rind_dir() -> String {
    println!("cargo::rerun-if-env-changed=RIND_DIR");
    env::var("RIND_DIR").unwrap_or_else(|_| "../rind".into())
}

fn port(text: &str) -> String {
    format!("{PRELUDE}{}", text.replace("std::", "core::"))
}

/// Writes `pad.rs` for `src/pad.rs` from `RIND_BENCH_PAD` (bytes, default 0).
/// The padding goes in `.Reset`, which cortex-m-rt's `link.x` places first in
/// `.text`, ahead of every `.text.*` section, so all library code moves by
/// the padding (`.text.*` would put it wherever its object lands).
fn write_pad(out: &std::path::Path) {
    println!("cargo::rerun-if-env-changed=RIND_BENCH_PAD");
    let n: usize = std::env::var("RIND_BENCH_PAD")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().expect("RIND_BENCH_PAD: a byte count"))
        .unwrap_or(0);
    let code = if n == 0 {
        "/// No padding in this build.\npub const BYTES: usize = 0;\n/// Keeps the padding linked.\npub fn keep() {}\n".to_string()
    } else {
        format!(
            "/// The padding in this build, in bytes.\npub const BYTES: usize = {n};\n#[unsafe(link_section = \".Reset\")]\nstatic PAD: [u8; {n}] = [0; {n}];\n/// Keeps the padding linked: `--gc-sections` would drop an unreferenced section.\npub fn keep() {{\n    core::hint::black_box(&PAD);\n}}\n"
        )
    };
    std::fs::write(out.join("pad.rs"), code).unwrap();
}
