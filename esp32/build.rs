// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

fn main() {
    embuild::espidf::sysenv::output();
    write_pad(&std::path::PathBuf::from(
        std::env::var_os("OUT_DIR").unwrap(),
    ));
}
/// Writes `pad.rs` for `src/pad.rs` from `RIND_BENCH_PAD` (bytes, default 0).
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
            "/// The padding in this build, in bytes.\npub const BYTES: usize = {n};\n#[unsafe(link_section = \".text.rind_bench_pad\")]\nstatic PAD: [u8; {n}] = [0; {n}];\n/// Keeps the padding linked: `--gc-sections` would drop an unreferenced section.\npub fn keep() {{\n    core::hint::black_box(&PAD);\n}}\n"
        )
    };
    std::fs::write(out.join("pad.rs"), code).unwrap();
    if n > 0 {
        // ESP-IDF's `.flash.text` takes every `.text.*` in one rule, and ld
        // put the padding's section after the libraries there; this places
        // it first, so all library code moves by the padding.
        let order = out.join("rind-bench-pad-order.ld");
        std::fs::write(&order, ".flash.text : { *(.text.rind_bench_pad) }\n").unwrap();
        println!(
            "cargo::rustc-link-arg=-Wl,--section-ordering-file={}",
            order.display()
        );
    }
}
