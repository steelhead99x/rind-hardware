// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
//! rind-hardware's tooling: `cargo xtask <command>` from the repository root.
//!
//! The firmware crates build with target toolchains (espup's `esp` for the
//! ESP32-S3, a pinned stable with thumbv7em for the nRF52840 and thumbv6m
//! for the RP2040) whose environment `esp32/env.sh`, `nrf52840/env.sh` and
//! `rp2040/env.sh` set up; those stay shell, sourced by these commands
//! before each cargo call, since that environment is the toolchains' own
//! export scripts.

mod pbm;
mod uf2;

use clap::{Parser, Subcommand, ValueEnum};
use sha2::{Digest, Sha256};
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

/// Local branch in the rind submodule. `apply-patches` commits the series
/// here. The parent repository's gitlink stays at the upstream pin.
const PATCH_BRANCH: &str = "hardware/applied";

/// Committer for those commits. It is fixed so `fork_commit` does not depend
/// on who runs the tool. The author stays the one named in the patch.
const PATCH_COMMITTER_NAME: &str = "steelhead99x";
const PATCH_COMMITTER_EMAIL: &str = "steelhead99x@users.noreply.github.com";

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
    /// Commit patches/[0-9]*.patch onto the rind submodule's local branch
    /// `hardware/applied` (`git am`; an applied patch is skipped), then with
    /// --set the optional set patches/<set>/[0-9]*.patch. Refuses a dirty tree.
    ApplyPatches {
        /// an optional patch set, a directory under patches/ such as poly1305-32
        #[arg(long)]
        set: Option<String>,
        /// take the set back out (newest patch first), leaving the base patches
        /// committed. Resets to the ancestor whose tree matches, when there is one.
        #[arg(long, requires = "set")]
        reverse: bool,
    },
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
    /// Each image's sha256 and source. Refuses a dirty rind tree. Prints JSON
    /// with `commit` (upstream pin), `fork_commit` (patched HEAD) and
    /// `patches` as `{name, sha256}`, including the board's ring vendor patch.
    Provenance {
        board: Board,
        #[arg(long, default_value = "images")]
        images: PathBuf,
        /// this repo's commit and tree state the images were built from
        #[arg(long)]
        rind_bench: String,
        #[arg(long)]
        note: String,
        /// the optional patch set (under patches/) the images were built with
        #[arg(long)]
        set: Option<String>,
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

/// Runs `sh -c script` in `dir` (after sourcing `env`, if given), echoing it.
fn sh(dir: &Path, env: Option<&str>, script: &str, vars: &[(&str, String)]) -> Result<(), String> {
    let full = match env {
        Some(e) => format!(". ./{e} && {script}"),
        None => script.to_string(),
    };
    let mut c = Command::new("sh");
    c.arg("-c").arg(&full).current_dir(dir);
    for (k, v) in vars {
        c.env(k, v);
    }
    let st = c.status().map_err(|e| format!("sh: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("failed ({st}): {script} in {}", dir.display()))
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
    sh(
        &r,
        None,
        &format!(
            "rustfmt +stable --edition 2024 --config-path .rustfmt.toml --check {}",
            files.join(" ")
        ),
        &[],
    )?;
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
    sh(
        &r.join("xtask"),
        None,
        "cargo +stable clippy -q --all-targets -- -D warnings",
        &tdir("xtask"),
    )?;
    println!("== clippy xtask: ok");
    sh(
        &r.join("demo/oled-ui"),
        None,
        "cargo +stable clippy --all-targets -- -D warnings",
        &tdir("demo/oled-ui"),
    )?;
    // The firmware images compile the rind submodule. A dirty or unpatched
    // tree would make the clippy result unreproducible.
    require_rind_tree(None)?;
    let esp = r.join("esp32");
    let v = tdir("esp32");
    for b in BENCHES {
        println!("== clippy {b}");
        sh(
            &esp,
            Some("env.sh"),
            &format!("cargo clippy --release --features {b} -- -D warnings"),
            &v,
        )?;
    }
    // The RNG diagnostics (src/rng.rs) take getrandom's custom backend, set
    // through a merged --config so .cargo/config.toml's rustflags stay.
    let rng =
        r#"'target.xtensa-esp32s3-espidf.rustflags=["--cfg","getrandom_backend=\"custom\""]'"#;
    for f in ["rng-diag", "rng-diag seeded-rng"] {
        println!("== clippy {f}");
        sh(
            &esp,
            Some("env.sh"),
            &format!("cargo clippy --release --features '{f}' --config {rng} -- -D warnings"),
            &v,
        )?;
    }
    println!("== clippy aes_gcm oled + examples");
    sh(
        &esp,
        Some("env.sh"),
        "cargo clippy --release --features 'aes_gcm oled' --examples -- -D warnings",
        &v,
    )?;
    let nrf = r.join("nrf52840");
    let v = tdir("nrf52840");
    for b in BENCHES {
        println!("== clippy nrf52840 {b}");
        sh(
            &nrf,
            Some("env.sh"),
            &format!("cargo clippy --release --features {b} -- -D warnings"),
            &v,
        )?;
    }
    println!("== clippy nrf52840 wdt-probe");
    sh(
        &nrf,
        Some("env.sh"),
        "cargo clippy --release --features wdt-probe -- -D warnings",
        &v,
    )?;
    let rp = r.join("rp2040");
    let v = tdir("rp2040");
    for b in BENCHES {
        println!("== clippy rp2040 {b}");
        sh(
            &rp,
            Some("env.sh"),
            &format!("cargo clippy --release --features {b} -- -D warnings"),
            &v,
        )?;
    }
    println!("clippy: ok");
    Ok(())
}

/// The numbered patches in `dir`, in the order they apply.
fn numbered_patches(dir: &Path) -> Result<Vec<String>, String> {
    let mut patches: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".patch") && n.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    patches.sort();
    Ok(patches)
}

/// A patch set's directory under patches/: one plain name, not a path.
fn patch_set_dir(set: &str) -> Result<PathBuf, String> {
    if set.is_empty()
        || !set
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "patch set {set:?}: expected a directory name under patches/"
        ));
    }
    let dir = root().join("patches").join(set);
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(format!("no patch set patches/{set}/"))
    }
}

fn rind_dir() -> PathBuf {
    root().join("rind")
}

fn patches_dir() -> PathBuf {
    root().join("patches")
}

/// `git -C dir` with the fixed patch committer, and no author/committer
/// environment variables that would change the commit id.
fn git_identity(dir: &Path, args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .arg("-c")
        .arg(format!("user.name={PATCH_COMMITTER_NAME}"))
        .arg("-c")
        .arg(format!("user.email={PATCH_COMMITTER_EMAIL}"))
        .args(args)
        .stdin(Stdio::null())
        .env("GIT_EDITOR", "true")
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_AUTHOR_DATE")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .env_remove("GIT_COMMITTER_DATE");
    let st = cmd.status().map_err(|e| format!("git: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("git {} failed ({st})", args.join(" ")))
    }
}

fn is_ancestor(dir: &Path, ancestor: &str, descendant: &str) -> Result<bool, String> {
    if ancestor == descendant {
        return Ok(true);
    }
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    match o.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "git merge-base --is-ancestor {ancestor} {descendant}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
    }
}

fn rind_status() -> Result<String, String> {
    let o = Command::new("git")
        .arg("-C")
        .arg(rind_dir())
        .args(["status", "--porcelain"])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !o.status.success() {
        return Err(format!(
            "git status: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&o.stdout)
        .trim_end_matches(['\n', '\r'])
        .to_string())
}

fn require_clean_rind() -> Result<(), String> {
    let status = rind_status()?;
    if status.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "rind submodule has uncommitted changes. apply-patches commits them on {PATCH_BRANCH}; build-images and provenance refuse a dirty tree:\n{status}"
        ))
    }
}

fn ensure_rind_repo() -> Result<(), String> {
    if rind_dir().join(".git").exists() {
        return Ok(());
    }
    git(&root(), &["submodule", "update", "--init", "--", "rind"])?;
    if rind_dir().join(".git").exists() {
        Ok(())
    } else {
        Err("rind submodule is not checked out".into())
    }
}

fn upstream_pin() -> Result<String, String> {
    git(&root(), &["rev-parse", "HEAD:rind"])
}

fn rind_head() -> Result<String, String> {
    git(&rind_dir(), &["rev-parse", "HEAD"])
}

fn patch_path_name(path: &Path) -> Result<String, String> {
    path.strip_prefix(patches_dir())
        .map(|p| p.display().to_string())
        .map_err(|_| format!("{} is not under patches/", path.display()))
}

fn base_patch_paths() -> Result<Vec<PathBuf>, String> {
    let dir = patches_dir();
    Ok(numbered_patches(&dir)?
        .into_iter()
        .map(|n| dir.join(n))
        .collect())
}

fn set_patch_paths(set: &str) -> Result<Vec<PathBuf>, String> {
    let dir = patch_set_dir(set)?;
    Ok(numbered_patches(&dir)?
        .into_iter()
        .map(|n| dir.join(n))
        .collect())
}

fn optional_set_names() -> Result<Vec<String>, String> {
    let dir = patches_dir();
    let mut names = vec![];
    for e in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        if !e.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if !numbered_patches(&e.path())?.is_empty() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Whether `patch` is already in the rind tree (committed or not).
fn patch_applied(patch: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(rind_dir())
        .args(["apply", "--reverse", "--check"])
        .arg(patch)
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn for_board(board: Board, name: &str) -> bool {
    (board == Board::Esp || !name.contains("xtensa"))
        && (board == Board::Nrf || !name.contains("thumbv7em"))
}

/// Check out `hardware/applied`. A new branch is created only at the pin, so
/// a newer upstream checkout is not patched by accident. An existing branch
/// that is not based on the pin is left alone.
fn ensure_patch_branch(pin: &str) -> Result<(), String> {
    let head = rind_head()?;
    if !is_ancestor(&rind_dir(), pin, &head)? {
        return Err(format!(
            "rind HEAD {head} is not the pinned upstream {pin} or a commit on top of it"
        ));
    }
    let cur = git(&rind_dir(), &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if cur == PATCH_BRANCH {
        return Ok(());
    }
    let branch_ref = format!("refs/heads/{PATCH_BRANCH}");
    match git(
        &rind_dir(),
        &["rev-parse", "--verify", "--quiet", &branch_ref],
    ) {
        Ok(sha) => {
            let sha = sha.trim();
            if !is_ancestor(&rind_dir(), pin, sha)? {
                return Err(format!(
                    "local branch {PATCH_BRANCH} ({sha}) is not based on pinned upstream {pin}; refusing to overwrite it. Delete it with `git -C rind branch -D {PATCH_BRANCH}` and rerun"
                ));
            }
            if head != pin && head != sha {
                return Err(format!(
                    "rind HEAD {head} is neither the pinned upstream {pin} nor {PATCH_BRANCH} ({sha})"
                ));
            }
            git(&rind_dir(), &["switch", "--quiet", PATCH_BRANCH])?;
        }
        Err(_) => {
            if head != pin {
                return Err(format!(
                    "rind HEAD {head} is not the pinned upstream {pin}. Check out the pin before apply-patches (`git submodule update --init rind`)"
                ));
            }
            git(&rind_dir(), &["switch", "-c", PATCH_BRANCH])?;
        }
    }
    require_clean_rind()
}

fn git_am(patches: &[PathBuf]) -> Result<(), String> {
    let mut args = vec![
        "am".to_string(),
        "--committer-date-is-author-date".to_string(),
    ];
    for p in patches {
        args.push(p.display().to_string());
    }
    if let Err(e) = git_identity(&rind_dir(), &args) {
        let abort = Command::new("git")
            .arg("-C")
            .arg(rind_dir())
            .args(["am", "--abort"])
            .stdin(Stdio::null())
            .status();
        let extra = match abort {
            Ok(s) if s.success() => "the rind tree was reset to before this series",
            _ => "git am --abort also failed; inspect rind/",
        };
        return Err(format!("{e}; {extra}"));
    }
    Ok(())
}

fn git_apply(patch: &Path, reverse: bool) -> Result<(), String> {
    let mut args = vec!["apply".to_string()];
    if reverse {
        args.push("--reverse".to_string());
    }
    args.push(patch.display().to_string());
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    git(&rind_dir(), &borrowed).map(|_| ())
}

fn worktree_matches(rev: &str) -> Result<bool, String> {
    let o = Command::new("git")
        .arg("-C")
        .arg(rind_dir())
        .args(["diff", "--quiet", rev])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    match o.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "git diff --quiet {rev}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
    }
}

/// The base series is committed, no optional set is only partly applied, and
/// `expect_set` is committed when it is named. Returns `(upstream pin, HEAD)`.
fn require_rind_tree(expect_set: Option<&str>) -> Result<(String, String), String> {
    ensure_rind_repo()?;
    require_clean_rind()?;
    let commit = upstream_pin()?;
    let fork_commit = rind_head()?;
    if !is_ancestor(&rind_dir(), &commit, &fork_commit)? {
        return Err(format!(
            "rind HEAD {fork_commit} is not the pinned upstream {commit} or a commit on top of it"
        ));
    }
    for path in base_patch_paths()? {
        if !patch_applied(&path) {
            return Err(format!(
                "patches/{} is not committed in the rind tree; run cargo xtask apply-patches",
                patch_path_name(&path)?
            ));
        }
    }
    for set in optional_set_names()? {
        let paths = set_patch_paths(&set)?;
        let n = paths.iter().filter(|p| patch_applied(p)).count();
        if n != 0 && n != paths.len() {
            return Err(format!(
                "optional set {set} is only partly in the rind tree; run apply-patches --set {set} or --reverse"
            ));
        }
    }
    if let Some(set) = expect_set {
        let paths = set_patch_paths(set)?;
        if paths.is_empty() {
            return Err(format!("patch set {set} has no numbered patches"));
        }
        if !paths.iter().all(|p| patch_applied(p)) {
            return Err(format!(
                "optional set {set} is not committed in the rind tree; run cargo xtask apply-patches --set {set}"
            ));
        }
    }
    Ok((commit, fork_commit))
}

/// Base patches, the board's ring vendor patches, and every fully applied
/// optional set, each with its sha256.
fn patches_for_board(board: Board) -> Result<Vec<(String, String)>, String> {
    let mut paths = base_patch_paths()?;
    let dir = patches_dir();
    let mut ring = vec![];
    for e in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        if !e.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".patch")
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && for_board(board, &name)
        {
            ring.push(e.path());
        }
    }
    ring.sort();
    paths.extend(ring);
    for set in optional_set_names()? {
        let set_paths = set_patch_paths(&set)?;
        if set_paths.iter().all(|p| patch_applied(p)) {
            paths.extend(set_paths);
        }
    }
    let mut out = vec![];
    for path in paths {
        let name = patch_path_name(&path)?;
        if for_board(board, &name) {
            out.push((name, sha256_file(&path)?));
        }
    }
    Ok(out)
}

fn apply_patches(set: Option<&str>, reverse: bool) -> Result<(), String> {
    ensure_rind_repo()?;
    require_clean_rind()?;
    let pin = upstream_pin()?;
    ensure_patch_branch(&pin)?;
    require_clean_rind()?;
    if reverse {
        let set = set.ok_or_else(|| "--reverse requires --set".to_string())?;
        return reverse_set(set, &pin);
    }
    let mut pending = base_patch_paths()?;
    if let Some(set) = set {
        pending.extend(set_patch_paths(set)?);
    }
    let pending: Vec<PathBuf> = pending.into_iter().filter(|p| !patch_applied(p)).collect();
    if pending.is_empty() {
        println!("already applied at {} (upstream {pin})", rind_head()?);
        return Ok(());
    }
    git_am(&pending)?;
    require_clean_rind()?;
    println!(
        "committed on {PATCH_BRANCH} at {} (upstream {pin})",
        rind_head()?
    );
    Ok(())
}

fn reverse_set(set: &str, pin: &str) -> Result<(), String> {
    let patches = set_patch_paths(set)?;
    if patches.is_empty() {
        return Err(format!("patch set {set} has no numbered patches"));
    }
    let applied = patches.iter().filter(|p| patch_applied(p)).count();
    if applied == 0 {
        println!("not applied: patches/{set}");
        return Ok(());
    }
    if applied != patches.len() {
        return Err(format!(
            "optional set {set} is only partly in the rind tree; refusing to reverse it"
        ));
    }
    for patch in patches.iter().rev() {
        if let Err(e) = git_apply(patch, true) {
            let _ = git(&rind_dir(), &["reset", "--hard", "HEAD"]);
            return Err(e);
        }
    }
    let log = git(&rind_dir(), &["rev-list", &format!("{pin}..HEAD")])?;
    let mut candidates: Vec<String> = log.lines().map(str::to_string).collect();
    candidates.push(pin.to_string());
    for rev in &candidates {
        if worktree_matches(rev)? {
            git(&rind_dir(), &["reset", "--hard", rev])?;
            println!("restored {PATCH_BRANCH} to {rev} (upstream {pin})");
            return require_clean_rind();
        }
    }
    git_identity(&rind_dir(), &["add".into(), "-u".into()])?;
    git_identity(
        &rind_dir(),
        &[
            "commit".into(),
            "-m".into(),
            format!("revert(patches): drop optional set {set}"),
        ],
    )?;
    require_clean_rind()?;
    println!(
        "committed revert of {set} at {} (upstream {pin})",
        rind_head()?
    );
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
    sh(
        &r.join("nrf52840"),
        Some("env.sh"),
        "cargo fetch -q --target thumbv7em-none-eabihf",
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
        sh(
            &r,
            None,
            &format!("patch -s -d .tools/{dir} -p1 < patches/{patch}"),
            &[],
        )?;
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
    require_rind_tree(None)?;
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
        sh(
            &dir,
            Some("env.sh"),
            &format!("cargo build --release --features {b}"),
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
                sh(
                    &dir,
                    Some("env.sh"),
                    &format!(
                        "arm-none-eabi-objcopy -O binary -j .vector_table -j .text -j .rodata -j .data '{}' '{}'",
                        elf.display(),
                        bin.display()
                    ),
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
                sh(
                    &dir,
                    Some("env.sh"),
                    &format!(
                        "arm-none-eabi-objcopy -O binary '{}' '{}'",
                        elf.display(),
                        bin.display()
                    ),
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

fn sha256_bytes(d: &[u8]) -> String {
    Sha256::digest(d)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn sha256_file(p: &Path) -> Result<String, String> {
    let d = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(sha256_bytes(&d))
}

struct ProvenanceImage {
    name: String,
    sha256: String,
    built: String,
}

/// The document `cargo xtask provenance` prints. `commit` is the upstream
/// pin recorded by this repo. `fork_commit` is the rind commit that was
/// built. Each image's `rind` is that same fork commit.
fn provenance_json(
    note: &str,
    commit: &str,
    fork_commit: &str,
    rind_bench: &str,
    patches: &[(String, String)],
    ext: &str,
    images: &[ProvenanceImage],
) -> String {
    let mut o = format!(
        "{{\n \"note\": {},\n \"commit\": {},\n \"fork_commit\": {},\n \"patches\": [\n",
        json_str(note),
        json_str(commit),
        json_str(fork_commit),
    );
    if !patches.is_empty() {
        o += &patches
            .iter()
            .map(|(name, sha)| {
                format!(
                    "  {{\"name\": {}, \"sha256\": {}}}",
                    json_str(name),
                    json_str(sha)
                )
            })
            .collect::<Vec<_>>()
            .join(",\n");
        o += "\n";
    }
    o += &format!(" ],\n \"file_ext\": \".{ext}\",\n \"images\": {{\n");
    let entries: Vec<String> = images
        .iter()
        .map(|image| {
            format!(
                "  {}: {{\n   \"sha256\": {},\n   \"commit\": {},\n   \"fork_commit\": {},\n   \"rind\": {},\n   \"rind_bench\": {},\n   \"built\": {}\n  }}",
                json_str(&image.name),
                json_str(&image.sha256),
                json_str(commit),
                json_str(fork_commit),
                json_str(fork_commit),
                json_str(rind_bench),
                json_str(&image.built),
            )
        })
        .collect();
    o += &entries.join(",\n");
    o += "\n }\n}";
    o
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

fn provenance(
    board: Board,
    images: &Path,
    rind_bench: &str,
    note: &str,
    set: Option<&str>,
) -> Result<(), String> {
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
    let (commit, fork_commit) = require_rind_tree(set)?;
    let patches = patches_for_board(board)?;
    let mut names: Vec<String> = std::fs::read_dir(&images)
        .map_err(|e| format!("{}: {e}", images.display()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter_map(|n| n.strip_suffix(&format!(".{ext}")).map(String::from))
        .collect();
    names.sort();
    let mut records = vec![];
    for n in &names {
        let p = images.join(format!("{n}.{ext}"));
        records.push(ProvenanceImage {
            name: n.clone(),
            sha256: sha256_file(&p)?,
            built: local_time(&p),
        });
    }
    println!(
        "{}",
        provenance_json(
            note,
            &commit,
            &fork_commit,
            rind_bench,
            &patches,
            ext,
            &records,
        )
    );
    Ok(())
}

fn oled_media(tsv: &Path, out: &Path) -> Result<(), String> {
    let tsv = std::path::absolute(tsv).map_err(|e| e.to_string())?;
    let out = std::path::absolute(out).map_err(|e| e.to_string())?;
    let tmp = std::env::temp_dir().join(format!("oled-media-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    sh(
        &root().join("demo/oled-ui"),
        None,
        &format!(
            "cargo +stable run -q --example render -- '{}' '{}/'",
            tsv.display(),
            tmp.display()
        ),
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
        Cmd::ApplyPatches { set, reverse } => apply_patches(set.as_deref(), reverse),
        Cmd::VendorRing => vendor_ring(),
        Cmd::BuildImages { board, benches, out, pad } => build_images(board, &benches, &out, pad),
        Cmd::Provenance { board, images, rind_bench, note, set } => {
            provenance(board, &images, &rind_bench, &note, set.as_deref())
        }
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

    #[test]
    fn sha256_of_abc() {
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn json_str_escapes_quotes_and_backslashes() {
        assert_eq!(json_str("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn provenance_json_records_upstream_fork_and_patch_hash() {
        let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let fork = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let body = provenance_json(
            "a note",
            commit,
            fork,
            "hw",
            &[("0001-a.patch".into(), "cc".into())],
            "elf",
            &[ProvenanceImage {
                name: "aes_gcm".into(),
                sha256: "dd".into(),
                built: "2026-11-13 00:00 UTC".into(),
            }],
        );
        assert!(body.contains(&format!("\"commit\": \"{commit}\"")));
        assert!(body.contains(&format!("\"fork_commit\": \"{fork}\"")));
        assert!(body.contains("\"name\": \"0001-a.patch\""));
        assert!(body.contains("\"sha256\": \"cc\""));
        assert!(body.contains(&format!("\"rind\": \"{fork}\"")));
        assert!(body.contains(&format!(
            "\"commit\": \"{commit}\",\n   \"fork_commit\": \"{fork}\""
        )));
    }
}
