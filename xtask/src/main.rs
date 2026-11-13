// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
//! rind-hardware's tooling: `cargo xtask <command>` from the repository root.
//!
//! The firmware crates build with target toolchains (espup's `esp` for the
//! ESP32-S3, a pinned stable with thumbv7em for the nRF52840 and thumbv6m
//! for the RP2040) whose environment `esp32/env.sh`, `nrf52840/env.sh` and
//! `rp2040/env.sh` set up. Those stay shell, sourced before each cargo call,
//! since that environment is the toolchains' own export scripts. Their
//! exports are applied with `Command::env`; the commands are argument lists.

mod pbm;
mod uf2;

use clap::{Parser, Subcommand, ValueEnum};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

/// Every bench file this port runs, in the order they build and run.
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

#[derive(Parser)]
#[command(about = "rind-hardware tooling", disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum Board {
    Esp,
    Nrf,
    Rp2040,
}

#[derive(Subcommand)]
enum Cmd {
    /// rind's contribution checks: rustfmt (style_edition 2024) and clippy with
    /// warnings denied, for every image, the display build and examples, the
    /// nRF52840 and RP2040 images, the display UI on the host and this tool
    Check {
        /// check from empty target directories (target-check/, deleted
        /// first), so no cached clippy result can hide a warning
        #[arg(long, env = "CHECK_CLEAN")]
        clean: bool,
    },
    /// Apply patches/[0-9]*.patch to the rind submodule's working tree (an applied patch is skipped)
    ApplyPatches,
    /// Copy ring 0.17.14 from cargo's registry into .tools/ring-{nrf,esp} and patch it
    VendorRing,
    /// Build one image per bench file into <board>/<out>/<bench>.elf (and .bin, .uf2 for the nRF52840 and RP2040)
    BuildImages {
        board: Board,
        /// benches to build; default every bench this port runs
        benches: Vec<String>,
        #[arg(long, default_value = "images")]
        out: PathBuf,
        /// bytes of layout-shift padding in the firmware's own .text (src/pad.rs)
        #[arg(long, env = "RIND_BENCH_PAD")]
        pad: Option<usize>,
    },
    /// Each image's sha256 and source, as rind-results' images.json takes it
    Provenance {
        board: Board,
        #[arg(long, default_value = "images")]
        images: PathBuf,
        /// this repo's commit and tree state the images were built from
        #[arg(long)]
        rind_bench: String,
        #[arg(long)]
        note: String,
    },
    /// Write a raw image as a UF2 (nRF52840 app region or RP2040 XIP), or inspect one
    Uf2 {
        /// IMAGE.bin OUT.uf2, or with --info / --region one FILE.uf2
        files: Vec<PathBuf>,
        #[arg(long, default_value = "0x27000", value_parser = hex)]
        base: u32,
        /// UF2 family ID: 0xada52840 (nRF, default) or 0xe48bff56 (RP2040)
        #[arg(long, default_value = "0xADA52840", value_parser = hex)]
        family: u32,
        /// print the file's address range, family and blocks
        #[arg(long)]
        info: bool,
        /// print the sha256 of START..END as the file holds it, e.g. 0x1000 0x27000
        #[arg(long, num_args = 2, value_parser = hex)]
        region: Option<Vec<u32>>,
    },
    /// The render example's PBM screens as PNGs: name.png at 4x, or name@Nx.png with --scale
    Pbm2png {
        #[arg(long)]
        scale: Option<usize>,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// The render example's PBM screens as half-block text
    Pbm2text {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Render the OLED demo's pages from a demo.tsv into DIR as NN-name.png (4x) and NN-name@1x.png
    OledMedia { tsv: PathBuf, out: PathBuf },
    /// repro/xtensa-overalign: does `takes_stack_arg` move a1 after `entry` without movsp?
    ReproOveralign,
}

fn hex(s: &str) -> Result<u32, String> {
    let t = s.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(t, if t.len() < s.len() { 16 } else { 10 }).map_err(|e| e.to_string())
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Source `$1` and write the environment it produces to `$2`. The file name
/// is an argument; nothing is interpolated into this script.
const SOURCE_ENV_SCRIPT: &str = ". \"$1\" && exec /usr/bin/env -0 > \"$2\"";

struct RemoveFile(PathBuf);

impl Drop for RemoveFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn parse_env0(bytes: &[u8]) -> Vec<(OsString, OsString)> {
    let mut vars = Vec::new();
    for entry in bytes.split(|b| *b == 0) {
        if entry.is_empty() {
            continue;
        }
        let Some(eq) = entry.iter().position(|b| *b == b'=') else {
            continue;
        };
        let key = &entry[..eq];
        if key.is_empty() {
            continue;
        }
        let val = &entry[eq + 1..];
        vars.push((
            OsString::from_vec(key.to_vec()),
            OsString::from_vec(val.to_vec()),
        ));
    }
    vars
}

/// Environment after sourcing `env_file` in `dir`.
///
/// `vars` are in the shell's environment before the file is sourced, so an
/// export of the same name in the script overrides them. The result is the
/// full environment a command run from that shell would inherit.
fn sourced_env(
    dir: &Path,
    env_file: &str,
    vars: &[(&str, String)],
) -> Result<Vec<(OsString, OsString)>, String> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out_path =
        std::env::temp_dir().join(format!("rind-hardware-env-{}-{n}", std::process::id()));
    let _remove = RemoveFile(out_path.clone());
    let source = if env_file.contains('/') {
        env_file.to_string()
    } else {
        format!("./{env_file}")
    };
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(SOURCE_ENV_SCRIPT)
        .arg("sh")
        .arg(&source)
        .arg(&out_path)
        .current_dir(dir);
    for (k, v) in vars {
        cmd.env(k, v);
    }
    let st = cmd.status().map_err(|e| format!("sh: {e}"))?;
    if !st.success() {
        return Err(format!(
            "failed ({st}): source {env_file} in {}",
            dir.display()
        ));
    }
    let bytes = std::fs::read(&out_path).map_err(|e| format!("env: {e}"))?;
    let parsed = parse_env0(&bytes);
    if parsed.is_empty() {
        return Err(format!(
            "source {env_file} in {} produced no environment",
            dir.display()
        ));
    }
    Ok(parsed)
}

fn command_line(program: &str, args: &[impl AsRef<std::ffi::OsStr>]) -> String {
    let mut parts = Vec::with_capacity(1 + args.len());
    parts.push(program.to_string());
    for arg in args {
        parts.push(arg.as_ref().to_string_lossy().into_owned());
    }
    parts.join(" ")
}

/// `program` and `args`, in `dir`, ready to spawn.
///
/// When `env_file` is set, that toolchain script is sourced and the resulting
/// environment is applied with [`Command::env`] after clearing the inherited
/// one, so the child sees the shell's environment. Otherwise the process
/// environment is kept and `vars` are applied with [`Command::env`].
fn prepare<S: AsRef<std::ffi::OsStr>>(
    dir: &Path,
    env_file: Option<&str>,
    program: &str,
    args: &[S],
    vars: &[(&str, String)],
) -> Result<Command, String> {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(dir);
    if let Some(env_file) = env_file {
        cmd.env_clear();
        for (k, v) in sourced_env(dir, env_file, vars)? {
            cmd.env(k, v);
        }
    } else {
        for (k, v) in vars {
            cmd.env(k, v);
        }
    }
    Ok(cmd)
}

/// Runs `program` with `args` in `dir`. See [`prepare`].
fn run<S: AsRef<std::ffi::OsStr>>(
    dir: &Path,
    env_file: Option<&str>,
    program: &str,
    args: &[S],
    vars: &[(&str, String)],
) -> Result<(), String> {
    let line = command_line(program, args);
    let mut cmd = prepare(dir, env_file, program, args, vars)?;
    let st = cmd.status().map_err(|e| format!("{program}: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("failed ({st}): {line} in {}", dir.display()))
    }
}

/// `patch -s -d strip_dir -p1`, with `patch` opened and passed on stdin.
fn patch_file(dir: &Path, strip_dir: &str, patch: &Path) -> Result<(), String> {
    let input = std::fs::File::open(patch).map_err(|e| format!("{}: {e}", patch.display()))?;
    let st = Command::new("patch")
        .arg("-s")
        .arg("-d")
        .arg(strip_dir)
        .arg("-p1")
        .stdin(Stdio::from(input))
        .current_dir(dir)
        .status()
        .map_err(|e| format!("patch: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!(
            "failed ({st}): patch -p1 {} in {}",
            patch.display(),
            dir.display()
        ))
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !o.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn crate_dir(b: Board) -> PathBuf {
    root().join(match b {
        Board::Esp => "esp32",
        Board::Nrf => "nrf52840",
        Board::Rp2040 => "rp2040",
    })
}

fn rust_files(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .map(|p| p.strip_prefix(root()).unwrap_or(&p).display().to_string())
        .collect();
    v.sort();
    v
}

fn check(clean: bool) -> Result<(), String> {
    let r = root();
    let mut files = vec![];
    for d in [
        "esp32/src",
        "esp32/examples",
        "criterion-esp/src",
        "demo/oled-ui/src",
        "demo/oled-ui/examples",
        "nrf52840/src",
        "rp2040/src",
        "xtask/src",
    ] {
        files.extend(rust_files(&r.join(d)));
    }
    files.extend(
        [
            "demo/firmware/oled.rs",
            "nrf52840/build.rs",
            "rp2040/build.rs",
            "esp32/build.rs",
            "repro/xtensa-overalign/src/lib.rs",
        ]
        .map(String::from),
    );
    let mut fmt_args = vec![
        "+stable".to_string(),
        "--edition".into(),
        "2024".into(),
        "--config-path".into(),
        ".rustfmt.toml".into(),
        "--check".into(),
    ];
    fmt_args.extend(files);
    run(&r, None, "rustfmt", &fmt_args, &[])?;
    println!("rustfmt: ok");
    let tdir = |d: &str| -> Vec<(&'static str, String)> {
        if clean {
            let t = r.join(d).join("target-check");
            let _ = std::fs::remove_dir_all(&t);
            vec![("CARGO_TARGET_DIR", t.display().to_string())]
        } else {
            vec![]
        }
    };
    run(
        &r.join("xtask"),
        None,
        "cargo",
        &[
            "+stable",
            "clippy",
            "-q",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        &tdir("xtask"),
    )?;
    println!("== clippy xtask: ok");
    run(
        &r.join("demo/oled-ui"),
        None,
        "cargo",
        &["+stable", "clippy", "--all-targets", "--", "-D", "warnings"],
        &tdir("demo/oled-ui"),
    )?;
    let esp = r.join("esp32");
    let v = tdir("esp32");
    for b in BENCHES {
        println!("== clippy {b}");
        run(
            &esp,
            Some("env.sh"),
            "cargo",
            &[
                "clippy",
                "--release",
                "--features",
                b,
                "--",
                "-D",
                "warnings",
            ],
            &v,
        )?;
    }
    // The RNG diagnostics (src/rng.rs) take getrandom's custom backend, set
    // through a merged --config so .cargo/config.toml's rustflags stay.
    // Escaped quotes stay in the value; cargo's TOML parser reads them.
    let rng = r#"target.xtensa-esp32s3-espidf.rustflags=["--cfg","getrandom_backend=\"custom\""]"#;
    for f in ["rng-diag", "rng-diag seeded-rng"] {
        println!("== clippy {f}");
        run(
            &esp,
            Some("env.sh"),
            "cargo",
            &[
                "clippy",
                "--release",
                "--features",
                f,
                "--config",
                rng,
                "--",
                "-D",
                "warnings",
            ],
            &v,
        )?;
    }
    println!("== clippy aes_gcm oled + examples");
    run(
        &esp,
        Some("env.sh"),
        "cargo",
        &[
            "clippy",
            "--release",
            "--features",
            "aes_gcm oled",
            "--examples",
            "--",
            "-D",
            "warnings",
        ],
        &v,
    )?;
    let nrf = r.join("nrf52840");
    let v = tdir("nrf52840");
    for b in BENCHES {
        println!("== clippy nrf52840 {b}");
        run(
            &nrf,
            Some("env.sh"),
            "cargo",
            &[
                "clippy",
                "--release",
                "--features",
                b,
                "--",
                "-D",
                "warnings",
            ],
            &v,
        )?;
    }
    println!("== clippy nrf52840 wdt-probe");
    run(
        &nrf,
        Some("env.sh"),
        "cargo",
        &[
            "clippy",
            "--release",
            "--features",
            "wdt-probe",
            "--",
            "-D",
            "warnings",
        ],
        &v,
    )?;
    let rp = r.join("rp2040");
    let v = tdir("rp2040");
    for b in BENCHES {
        println!("== clippy rp2040 {b}");
        run(
            &rp,
            Some("env.sh"),
            "cargo",
            &[
                "clippy",
                "--release",
                "--features",
                b,
                "--",
                "-D",
                "warnings",
            ],
            &v,
        )?;
    }
    println!("clippy: ok");
    Ok(())
}

fn apply_patches() -> Result<(), String> {
    let r = root();
    run(
        &r,
        None,
        "git",
        &["submodule", "update", "--init", "rind"],
        &[],
    )?;
    let mut patches: Vec<String> = std::fs::read_dir(r.join("patches"))
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".patch") && n.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    patches.sort();
    for p in patches {
        let rel = format!("../patches/{p}");
        let applied = Command::new("git")
            .args(["-C", "rind", "apply", "--reverse", "--check", &rel])
            .current_dir(&r)
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if applied {
            println!("already applied: patches/{p}");
        } else {
            git(&r.join("rind"), &["apply", &rel])?;
            println!("applied: patches/{p}");
        }
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let t = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &t)?;
        } else {
            std::fs::copy(e.path(), &t)?;
        }
    }
    Ok(())
}

fn vendor_ring() -> Result<(), String> {
    let r = root();
    run(
        &r.join("nrf52840"),
        Some("env.sh"),
        "cargo",
        &["fetch", "-q", "--target", "thumbv7em-none-eabihf"],
        &[],
    )?;
    let home = std::env::var("CARGO_HOME").unwrap_or(format!(
        "{}/.cargo",
        std::env::var("HOME").unwrap_or_default()
    ));
    let mut srcs: Vec<PathBuf> = std::fs::read_dir(Path::new(&home).join("registry/src"))
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path().join("ring-0.17.14"))
        .filter(|p| p.is_dir())
        .collect();
    srcs.sort();
    let src = srcs
        .first()
        .ok_or("ring 0.17.14 is not in cargo's registry")?;
    for (dir, patch) in [
        ("ring-nrf", "ring-0.17.14-thumbv7em.patch"),
        ("ring-esp", "ring-0.17.14-xtensa-poly1305-align.patch"),
    ] {
        let to = r.join(".tools").join(dir);
        let _ = std::fs::remove_dir_all(&to);
        copy_dir(src, &to).map_err(|e| e.to_string())?;
        patch_file(&r, &format!(".tools/{dir}"), &r.join("patches").join(patch))?;
        println!("ring 0.17.14, patched with {patch}, in .tools/{dir}");
    }
    Ok(())
}

fn build_images(
    board: Board,
    benches: &[String],
    out: &Path,
    pad: Option<usize>,
) -> Result<(), String> {
    let dir = crate_dir(board);
    let out = if out.is_absolute() {
        out.to_path_buf()
    } else {
        dir.join(out)
    };
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let list: Vec<String> = if benches.is_empty() {
        BENCHES.iter().map(|s| s.to_string()).collect()
    } else {
        benches.to_vec()
    };
    let vars = vec![("RIND_BENCH_PAD", pad.unwrap_or(0).to_string())];
    for b in &list {
        println!("== {b}");
        run(
            &dir,
            Some("env.sh"),
            "cargo",
            &["build", "--release", "--features", b.as_str()],
            &vars,
        )?;
        match board {
            Board::Esp => {
                let elf = dir.join("target/xtensa-esp32s3-espidf/release/rind-bench-esp32");
                std::fs::copy(elf, out.join(format!("{b}.elf"))).map_err(|e| e.to_string())?;
            }
            Board::Nrf => {
                let elf = dir.join("target/thumbv7em-none-eabihf/release/rind-bench-nrf52840");
                std::fs::copy(&elf, out.join(format!("{b}.elf"))).map_err(|e| e.to_string())?;
                // Only the loadable sections go into the UF2.
                let bin = out.join(format!("{b}.bin"));
                let mut objcopy_args: Vec<OsString> = [
                    "-O",
                    "binary",
                    "-j",
                    ".vector_table",
                    "-j",
                    ".text",
                    "-j",
                    ".rodata",
                    "-j",
                    ".data",
                ]
                .into_iter()
                .map(OsString::from)
                .collect();
                objcopy_args.push(elf.as_os_str().to_os_string());
                objcopy_args.push(bin.as_os_str().to_os_string());
                run(
                    &dir,
                    Some("env.sh"),
                    "arm-none-eabi-objcopy",
                    &objcopy_args,
                    &[],
                )?;
                println!(
                    "{}",
                    uf2::write_family(
                        &bin,
                        &out.join(format!("{b}.uf2")),
                        uf2::NRF.start,
                        uf2::NRF
                    )?
                );
            }
            Board::Rp2040 => {
                let elf = dir.join("target/thumbv6m-none-eabi/release/rind-bench-rp2040");
                std::fs::copy(&elf, out.join(format!("{b}.elf"))).map_err(|e| e.to_string())?;
                // All loadable sections, so .boot2 at 0x10000000 is first.
                let bin = out.join(format!("{b}.bin"));
                let objcopy_args = [
                    OsString::from("-O"),
                    OsString::from("binary"),
                    elf.as_os_str().to_os_string(),
                    bin.as_os_str().to_os_string(),
                ];
                run(
                    &dir,
                    Some("env.sh"),
                    "arm-none-eabi-objcopy",
                    &objcopy_args,
                    &[],
                )?;
                println!(
                    "{}",
                    uf2::write_family(
                        &bin,
                        &out.join(format!("{b}.uf2")),
                        uf2::RP2040.start,
                        uf2::RP2040,
                    )?
                );
            }
        }
    }
    Ok(())
}

fn sha256_file(p: &Path) -> Result<String, String> {
    let d = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(Sha256::digest(&d)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o += "\\\"",
            '\\' => o += "\\\\",
            '\n' => o += "\\n",
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) {
                    o += &format!("\\u{u:04x}");
                }
            }
            c => o.push(c),
        }
    }
    o + "\""
}

fn local_time(p: &Path) -> String {
    let o = Command::new("date")
        .arg("-r")
        .arg(p)
        .arg("+%Y-%m-%d %H:%M %Z")
        .output();
    o.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn provenance(board: Board, images: &Path, rind_bench: &str, note: &str) -> Result<(), String> {
    let dir = crate_dir(board);
    let images = if images.is_absolute() {
        images.to_path_buf()
    } else {
        dir.join(images)
    };
    let ext = match board {
        Board::Esp => "elf",
        Board::Nrf | Board::Rp2040 => "uf2",
    };
    let rind = git(&root().join("rind"), &["rev-parse", "HEAD"])?;
    let mut patches: Vec<String> = std::fs::read_dir(root().join("patches"))
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".patch") && (board == Board::Esp || !n.contains("xtensa")))
        .filter(|n| board == Board::Nrf || !n.contains("thumbv7em"))
        .collect();
    patches.sort();
    let mut names: Vec<String> = std::fs::read_dir(&images)
        .map_err(|e| format!("{}: {e}", images.display()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter_map(|n| n.strip_suffix(&format!(".{ext}")).map(String::from))
        .collect();
    names.sort();
    let mut o = format!("{{\n \"note\": {},\n \"patches\": [\n", json_str(note));
    o += &patches
        .iter()
        .map(|p| format!("  {}", json_str(p)))
        .collect::<Vec<_>>()
        .join(",\n");
    o += &format!("\n ],\n \"file_ext\": \".{ext}\",\n \"images\": {{\n");
    let mut entries = vec![];
    for n in &names {
        let p = images.join(format!("{n}.{ext}"));
        entries.push(format!(
            "  {}: {{\n   \"sha256\": {},\n   \"rind\": {},\n   \"rind_bench\": {},\n   \"built\": {}\n  }}",
            json_str(n),
            json_str(&sha256_file(&p)?),
            json_str(&rind),
            json_str(rind_bench),
            json_str(&local_time(&p))
        ));
    }
    o += &entries.join(",\n");
    o += "\n }\n}";
    println!("{o}");
    Ok(())
}

fn oled_media(tsv: &Path, out: &Path) -> Result<(), String> {
    let tsv = std::path::absolute(tsv).map_err(|e| e.to_string())?;
    let out = std::path::absolute(out).map_err(|e| e.to_string())?;
    let tmp = std::env::temp_dir().join(format!("oled-media-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut tmp_arg = tmp.as_os_str().to_os_string();
    tmp_arg.push("/");
    let render_args = [
        OsString::from("+stable"),
        OsString::from("run"),
        OsString::from("-q"),
        OsString::from("--example"),
        OsString::from("render"),
        OsString::from("--"),
        tsv.as_os_str().to_os_string(),
        tmp_arg,
    ];
    run(
        &root().join("demo/oled-ui"),
        None,
        "cargo",
        &render_args,
        &[],
    )?;
    let mut pbms: Vec<PathBuf> = std::fs::read_dir(&tmp)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "pbm"))
        .collect();
    pbms.sort();
    // Copy, not rename: the temp dir may be on another filesystem.
    for p in &pbms {
        let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
        let big = pbm::to_png(p, None)?;
        std::fs::copy(&big, out.join(format!("{stem}@4x.png"))).map_err(|e| e.to_string())?;
        let one = pbm::to_png(p, Some(1))?;
        std::fs::copy(&one, out.join(format!("{stem}@1x.png"))).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir_all(&tmp);
    println!("{} pages in {}", pbms.len(), out.display());
    Ok(())
}

fn repro_overalign() -> Result<(), String> {
    let dir = root().join("repro/xtensa-overalign");
    let target =
        std::env::var("CARGO_TARGET_DIR").unwrap_or(dir.join("target").display().to_string());
    let st = Command::new("cargo")
        .args([
            "rustc",
            "-q",
            "--release",
            "-Zbuild-std=core",
            "--target",
            "xtensa-esp32s3-none-elf",
        ])
        .args(["--", "--emit", "asm"])
        .env("RUSTUP_TOOLCHAIN", "esp")
        .env("CARGO_TARGET_DIR", &target)
        .current_dir(&dir)
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("build failed".into());
    }
    let deps = Path::new(&target).join("xtensa-esp32s3-none-elf/release/deps");
    let mut s: Vec<PathBuf> = std::fs::read_dir(&deps)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy();
            n.starts_with("xtensa_overalign-") && n.ends_with(".s")
        })
        .collect();
    s.sort();
    let asm =
        std::fs::read_to_string(s.first().ok_or("no .s output")?).map_err(|e| e.to_string())?;
    let mut body = vec![];
    let mut inside = false;
    for line in asm.lines() {
        if line.starts_with("takes_stack_arg:") {
            inside = true;
        }
        if inside {
            let t = line.trim_start();
            if !t.starts_with('.') && !t.starts_with('#') {
                body.push(line);
            }
            if line.contains("retw") {
                break;
            }
        }
    }
    println!("{}", body.join("\n"));
    let _ = Command::new("rustc").args(["+esp", "--version"]).status();
    let bad = body.iter().any(|l| {
        let mut w = l.split_whitespace();
        let op = w.next().unwrap_or("");
        ["add", "add.n", "addi", "sub", "mov", "mov.n"].contains(&op)
            && w.next().is_some_and(|a| a.starts_with("a1,"))
    });
    if bad {
        println!("FAIL: a1 is written after entry without movsp");
        return Err("FAIL".into());
    }
    println!("PASS");
    Ok(())
}

fn main() -> ExitCode {
    let r = match Cli::parse().cmd {
        Cmd::Check { clean } => check(clean),
        Cmd::ApplyPatches => apply_patches(),
        Cmd::VendorRing => vendor_ring(),
        Cmd::BuildImages { board, benches, out, pad } => build_images(board, &benches, &out, pad),
        Cmd::Provenance { board, images, rind_bench, note } => provenance(board, &images, &rind_bench, &note),
        Cmd::Uf2 { files, base, family, info, region } => match (info, region, files.as_slice()) {
            (true, None, [f]) => uf2::info(f).map(|s| println!("{s}")),
            (false, Some(r), [f]) => uf2::region(f, r[0], r[1]).map(|s| println!("{s}")),
            (false, None, [bin, out]) => {
                let fam = if family == uf2::RP2040.id {
                    uf2::RP2040
                } else if family == uf2::NRF.id {
                    uf2::NRF
                } else {
                    uf2::Family { id: family, start: base, end: u32::MAX }
                };
                uf2::write_family(bin, out, base, fam).map(|s| println!("{s}"))
            }
            _ => Err("usage: uf2 IMAGE.bin OUT.uf2 [--base A] [--family ID] | uf2 --info FILE | uf2 --region START END FILE".into()),
        },
        Cmd::Pbm2png { scale, files } => files.iter().try_for_each(|f| pbm::to_png(f, scale).map(|_| ())),
        Cmd::Pbm2text { files } => files.iter().try_for_each(|f| pbm::to_text(f).map(|s| print!("{s}"))),
        Cmd::OledMedia { tsv, out } => oled_media(&tsv, &out),
        Cmd::ReproOveralign => repro_overalign(),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("rind-hardware-xtask-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn arguments_are_not_passed_through_a_shell() {
        let weird = "$(echo injected) `echo x` ; rm -rf / # ' \" \\";
        let dir = std::env::temp_dir();
        let out = prepare(&dir, None, "printf", &["%s", weird], &[])
            .unwrap()
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout, weird.as_bytes());
    }

    #[test]
    fn command_env_sets_variables_without_replacing_the_process_environment() {
        let home = std::env::var("HOME").expect("HOME");
        let dir = std::env::temp_dir();
        let out = prepare(
            &dir,
            None,
            "sh",
            &["-c", "printf %s%s \"$HOME\" \"$RIND_FROM_VAR\""],
            &[("RIND_FROM_VAR", "from-var".into())],
        )
        .unwrap()
        .output()
        .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout, format!("{home}from-var").as_bytes());
    }

    #[test]
    fn toolchain_env_file_is_sourced_and_its_name_is_not_shell_code() {
        let scratch = Scratch::new();
        let bin = scratch.0.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let tool = bin.join("rind-fake-tool");
        std::fs::write(
            &tool,
            "#!/bin/sh\nprintf %s \"${RIND_FROM_FILE}-${RIND_FROM_VAR}\"\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&tool).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&tool, perms).unwrap();
        std::fs::write(
            scratch.0.join("env name.sh"),
            format!(
                "export RIND_FROM_FILE='from file'\nexport PATH=\"{}:$PATH\"\n",
                bin.display()
            ),
        )
        .unwrap();

        let out = prepare(
            &scratch.0,
            Some("env name.sh"),
            "rind-fake-tool",
            &[] as &[&str],
            &[
                ("RIND_FROM_VAR", "from-var".into()),
                ("RIND_FROM_FILE", "nope".into()),
            ],
        )
        .unwrap()
        .output()
        .unwrap();
        assert!(
            out.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout, b"from file-from-var");

        let marker = scratch.0.join("injected");
        let evil = format!("env.sh; touch {}", marker.display());
        std::fs::write(scratch.0.join("env.sh"), "export OK=1\n").unwrap();
        let failed = prepare(&scratch.0, Some(&evil), "printf", &["%s", "ran"], &[]);
        assert!(failed.is_err());
        assert!(
            !marker.exists(),
            "env file name was executed as a shell script"
        );
    }

    #[test]
    fn patch_file_reads_the_path_instead_of_a_shell_redirect() {
        let scratch = Scratch::new();
        std::fs::write(scratch.0.join("f.txt"), "a\n").unwrap();
        let patch = scratch.0.join("name; echo INJECTED.patch");
        std::fs::write(
            &patch,
            "--- a/f.txt\n+++ b/f.txt\n@@ -1,1 +1,1 @@\n-a\n+b\n",
        )
        .unwrap();
        patch_file(&scratch.0, ".", &patch).unwrap();
        assert_eq!(
            std::fs::read_to_string(scratch.0.join("f.txt")).unwrap(),
            "b\n"
        );
        assert!(!scratch.0.join("INJECTED").exists());
    }
}
