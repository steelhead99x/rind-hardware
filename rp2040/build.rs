// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! Puts `memory.x` on the linker's path, and writes each of rind-bench's bench
//! files to `OUT_DIR` for a `no_std` crate: `std::` paths become `core::`
//! (the files use only `std::hint` and `std::num` from it, both in core), and
//! the std prelude's `Vec`, `String`, `ToString`, `ToOwned`, `Box`, `vec!` and
//! `format!` are imported from `alloc`; and `mod criterion;`, the shared
//! `benches/criterion/mod.rs`, gets its path in `OUT_DIR`. Ring rows and
//! helpers are compiled out: ring 0.17's Arm assembly uses NEON and does not
//! build for ARMv6-M. Nothing else in the files changes.

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
    format!("{PRELUDE}{}", gate_ring(&text.replace("std::", "core::")))
}

/// Compile out ring rows and helpers. ring 0.17 does not build for
/// `thumbv6m-none-eabi`: its Arm assembly uses NEON (`vpush`).
fn gate_ring(src: &str) -> String {
    const CFG: &str = "#[cfg(any())]\n";
    let mut out = String::new();
    let mut rest = src;
    while !rest.is_empty() {
        let candidates = [
            find_use_ring(rest),
            find_fn_test_ring(rest),
            find_impl_ring(rest),
            find_ring_bench(rest),
            find_ring_arg(rest),
        ];
        let next = candidates.into_iter().flatten().min_by_key(|&(at, _)| at);
        let Some((at, kind)) = next else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..at]);
        let (item, tail) = match kind {
            Kind::Use => split_stmt(&rest[at..]),
            Kind::Fn => split_item(&rest[at..]),
            Kind::Bench => split_stmt(&rest[at..]),
            Kind::Arg => split_arg(&rest[at..]),
        };
        out.push_str(CFG);
        out.push_str(item);
        rest = tail;
    }
    gate_leftover(&out)
}

/// `let ring_key = ring::...` setup that sits beside a cfg'd-out bench row.
fn gate_leftover(src: &str) -> String {
    const CFG: &str = "#[cfg(any())]\n";
    let mut out = String::new();
    let mut rest = src;
    while let Some(rel) = rest.find("let ") {
        let at = line_start_idx(rest, rel);
        let line = line_start(rest, rel).trim_start();
        if line.starts_with("let ") && rest[rel..].contains("ring::") {
            // Only a `let` whose statement mentions ring, and is not already cfg'd.
            let already = rest[..at].trim_end().ends_with("#[cfg(any())]");
            if !already && statement_contains_ring(&rest[at..]) {
                out.push_str(&rest[..at]);
                let (item, tail) = split_stmt(&rest[at..]);
                out.push_str(CFG);
                out.push_str(item);
                rest = tail;
                continue;
            }
        }
        out.push_str(&rest[..rel + 4]);
        rest = &rest[rel + 4..];
    }
    out.push_str(rest);
    out
}

fn statement_contains_ring(s: &str) -> bool {
    let (item, _) = split_stmt(s);
    item.contains("ring::")
}

#[derive(Clone, Copy)]
enum Kind {
    Use,
    Fn,
    Bench,
    Arg,
}

fn find_use_ring(s: &str) -> Option<(usize, Kind)> {
    let mut i = 0;
    while let Some(rel) = s[i..].find("use ring") {
        let at = i + rel;
        if line_start(s, at).trim_start().starts_with("use ring") {
            return Some((at, Kind::Use));
        }
        i = at + 1;
    }
    None
}

fn find_impl_ring(s: &str) -> Option<(usize, Kind)> {
    let mut i = 0;
    while let Some(rel) = s[i..].find("impl ring::") {
        let at = i + rel;
        if line_start(s, at).trim_start().starts_with("impl ring::") {
            return Some((line_start_idx(s, at), Kind::Fn));
        }
        i = at + 1;
    }
    None
}

fn find_fn_test_ring(s: &str) -> Option<(usize, Kind)> {
    if let Some(rel) = s.find("fn test_ring_") {
        // include a leading `pub ` if present, from the line start
        return Some((line_start_idx(s, rel), Kind::Fn));
    }
    None
}

fn find_ring_bench(s: &str) -> Option<(usize, Kind)> {
    let pats = [
        "BenchmarkId::new(\"ring",
        "bench_function(\"ring",
        "bench_function(format!(\"{name}/ring",
        // "extract-expand/ring" and other names that end in /ring
        "/ring\"",
    ];
    let mut best: Option<usize> = None;
    for p in pats {
        if let Some(at) = s.find(p) {
            best = Some(best.map_or(at, |b| b.min(at)));
        }
    }
    let at = best?;
    // Walk back to the start of the statement: a `group.bench_*` call may
    // begin on the previous line, with the id on this one.
    Some((bench_stmt_start(s, at), Kind::Bench))
}

/// A `ring:` function parameter or a `&ring::...` argument on its own line.
fn find_ring_arg(s: &str) -> Option<(usize, Kind)> {
    let mut i = 0;
    while let Some(rel) = s[i..].find("ring") {
        let at = i + rel;
        let line = line_start(s, at);
        let t = line.trim_start();
        // `ring: &'static ring::...` in a signature, or `&ring::...` as an arg.
        if t.starts_with("ring:") || t.starts_with("&ring::") || t.starts_with("ring::") {
            return Some((line_start_idx(s, at), Kind::Arg));
        }
        i = at + 4;
    }
    None
}

fn bench_stmt_start(s: &str, at: usize) -> usize {
    let line = line_start_idx(s, at);
    let prev = s[..line].trim_end();
    if prev.ends_with('(') {
        line_start_idx(s, prev.len().saturating_sub(1))
    } else {
        line
    }
}

fn line_start_idx(s: &str, at: usize) -> usize {
    s[..at].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

fn line_start(s: &str, at: usize) -> &str {
    let i = line_start_idx(s, at);
    match s[i..].find('\n') {
        Some(n) => &s[i..i + n],
        None => &s[i..],
    }
}

fn split_stmt(s: &str) -> (&str, &str) {
    let end = end_of_stmt(s);
    (&s[..end], &s[end..])
}

fn split_item(s: &str) -> (&str, &str) {
    let brace = s.find('{').expect("fn test_ring_ has a body");
    let end = matching_brace(&s[brace..]) + brace + 1;
    // include a trailing newline if present
    let end = if s[end..].starts_with('\n') {
        end + 1
    } else {
        end
    };
    (&s[..end], &s[end..])
}

fn split_arg(s: &str) -> (&str, &str) {
    // The rest of this line, including the newline, is one argument or field.
    match s.find('\n') {
        Some(n) => (&s[..n + 1], &s[n + 1..]),
        None => (s, ""),
    }
}

fn end_of_stmt(s: &str) -> usize {
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            b';' if depth == 0 => return i + 1,
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    s.len()
}

fn matching_brace(s: &str) -> usize {
    // s starts with '{'
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("unbalanced brace in fn test_ring_");
}

/// Writes `pad.rs` for `src/pad.rs` from `RIND_BENCH_PAD` (bytes, default 0).
/// The padding goes in `.Reset`, which cortex-m-rt's `link.x` places first in
/// `.text` after the vector table (the `.boot2` section sits in BOOT2, ahead
/// of FLASH), so library code in `.text.*` moves by the padding.
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
