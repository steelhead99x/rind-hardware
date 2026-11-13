<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# rind-hardware

[rind](https://github.com/moq-dev/rind)'s own benchmark suite, `rind-bench/benches/*.rs`, compiled
unchanged but for the small patches in `patches/`, as bare-metal and ESP-IDF firmware for
microcontrollers. Each benchmark prints one JSON line in the IDs and shape rind's
`admin/bench-report` reads.

| Board | Core | Target | Status |
|---|---|---|---|
| ESP32-S3 (Seeed XIAO ESP32S3) | Xtensa LX7, 240 MHz | `xtensa-esp32s3-espidf` | runs; this README |
| nRF52840 (Seeed T1000-E) | Cortex-M4F, 64 MHz | `thumbv7em-none-eabihf` | runs; [nrf52840/README.md](nrf52840/README.md) |
| RP2040 (Raspberry Pi Pico) | Cortex-M0+, 125 MHz | `thumbv6m-none-eabi` | builds; known-answer checks passed on a Pico (2026-11-12); published timing results are not yet in rind-results; [rp2040/README.md](rp2040/README.md) |

This repository was called `rind-bench`. It is not rind's `rind-bench` crate; it compiles that
crate's bench files from the `rind` submodule.

The results, the host-side capture and report tools, and the toolchain setup script live in the
companion repo, **[steelhead99x/rind-results](https://github.com/steelhead99x/rind-results)**. This
repo holds only the firmware and the upstream-ready port.

## Layout

| Path | What it is | Upstream? |
|---|---|---|
| `rind/` | Submodule: upstream `moq-dev/rind`, pinned at `98568332` (main, 2026-11-12) | upstream |
| `patches/` | Numbered patches against rind this port needs, as `git format-patch` output, including `0003` (no_std) and `0005` (`ReuseCache` without CAS); optional patch sets in subdirectories (`patches/poly1305-32/`), applied only with `cargo xtask apply-patches --set <name>` | **the upstream PR** |
| `nrf52840/` | The nRF52840 (T1000-E) firmware, `thumbv7em-none-eabihf` | [nrf52840/README.md](nrf52840/README.md) |
| `rp2040/` | The RP2040 (Pico) firmware, `thumbv6m-none-eabi`; builds; known-answer checks passed on a Pico | [rp2040/README.md](rp2040/README.md) |
| `xtask/` | The tooling, `cargo xtask <command>` from the root: `apply-patches` (idempotent; `--set` adds an optional patch set, `--reverse` takes it out), `vendor-ring`, `build-images`, `provenance`, `check`, `uf2`, `pbm2png`, `pbm2text`, `oled-media`, `repro-overalign` | no |
| `criterion-esp/` | A criterion API shim, renamed to `criterion`: the bench files' `criterion::*` calls time with the core's cycle counter and print JSON | maybe, as a dev tool |
| `esp32/` | The ESP-IDF firmware: compiles the bench files with `#[path]`, runs known-answer checks, the hardware AES/SHA baseline | maybe, under `rind-bench/esp32` |
| `host-kat/` | The boards' known-answer checks (`esp32/src/vectors.rs`, by path, unchanged) on a Linux host, plus aws-lc-rs, graviola and OpenSSL against the same vectors and rind's outputs; run before timing a host (the Jetson Orin Nano in rind-results) | no |
| `demo/` | The OLED demo screens (`oled-ui`) and their firmware glue, behind the `oled` feature | no |

The bench files are not copied: `esp32/src/main.rs` includes
`rind/rind-bench/benches/<file>.rs` by path, and their `mod criterion;` resolves to rind's own
`benches/criterion/mod.rs`, whose `pub use criterion::*` picks up the shim.

## What runs

One firmware image per bench file (upstream builds one binary per bench file too; all of them in
one image overflow Xtensa's 512 KB `call8` range): `aes_gcm`, `chacha20poly1305`,
`xchacha20poly1305`, `cipher`, `x25519`, `p256`, `ed25519`, `sha2`, `sha3`, `hmac`, `hkdf`,
`mlkem768`.

All twelve images run to completion, three times each (`sha3` since 2026-11-12). The ChaCha
images used to crash inside the known-answer checks; the cause was ring's 64-byte-aligned
`poly1305_state_st`, whose stack realignment the LLVM Xtensa backend gets wrong
(espressif/llvm-project#140, reproduced in `repro/xtensa-overalign/`).
`patches/ring-0.17.14-xtensa-poly1305-align.patch` keeps the state at 16 bytes on Xtensa, and
the `chacha20poly1305` and `xchacha20poly1305` images now pass their checks and time.

| Library | On Xtensa | Why |
|---|---|---|
| rind 0.3.2 | runs, portable backend | `build.rs` picks `rind_backend = "portable"` on every arch but x86_64, aarch64, armv7+ |
| ring 0.17.14 | runs (portable C) | needs `less-safe-getrandom-espidf` for `SystemRandom` |
| RustCrypto (aes-gcm 0.11.1, chacha20poly1305 0.11.0, sha2 0.11.1, hmac/hkdf 0.13, p256 0.14, ml-kem 0.3.4), dalek (curve25519 5.0, ed25519/x25519 3.0) | runs | |
| libcrux (0.0.9-0.0.11 crates) | runs | `libcrux-hacl-rs` at opt-level 2 and `libcrux-p256` at opt-level `s`, see below |
| aws-lc-rs | **cannot build** | `aws-lc/include/openssl/target.h:91:2: error: #error "Unknown target CPU"`; aws-lc has no Xtensa target |
| graviola 0.4.5 | **not raced** | rind-bench gates it on x86_64/aarch64; off those arches graviola builds (checked) but exports only SHA-2 and HMAC, no AEADs, curves or ML-KEM |
| OpenSSL (openssl-src 300.6.1+3.6.3) | **cannot build** | `openssl-src`: "don't know how to configure OpenSSL for xtensa-esp32s3-espidf"; also gated by `__bench_openssl` + `target_family = "unix"` |
| `esp32s3-hw` | runs, **hardware baseline** | ESP-IDF mbedTLS `esp_aes_gcm_*` (AES peripheral, GHASH in software) and `mbedtls_sha256` (SHA peripheral); not a Rust library, never counted as a competitor |


## host-kat

`host-kat/` compiles the boards' known-answer checks (`esp32/src/vectors.rs`, by
path, unchanged) on a Linux host through a small criterion shim, and adds the
host-only libraries (aws-lc-rs, graviola, OpenSSL) against the same vectors and
rind's outputs. Run it before timing a host. 91 checks passed on x86_64 and on
the Jetson Orin Nano (see [host-kat/README.md](host-kat/README.md)):

```sh
cd host-kat && cargo run --release
```

It needs the `rind` submodule initialised. A non-zero exit means a library
failed a check; do not time that host until it is clean.

## The upstream PR

What an upstream PR would contain, smallest first. None has been opened.

1. `patches/0001-fix-portable-build-MD5-on-Xtensa-...patch`, `fix(portable): build MD5 on
   Xtensa, whose inline assembly is unstable`. `low/generic/md5.rs`'s reassociation barrier is an
   empty `asm!`, which needs `asm_experimental_arch` on Xtensa; take `black_box` there, as on wasm
   and Miri. This is the one change to the library, and the only thing stopping rind itself from
   building for `xtensa-esp32s3-espidf`.
2. `patches/0002-feat-bench-leave-aws-lc-rs-rows-out-on-Xtensa.patch`, which gates every aws-lc-rs
   row in the bench files on `not(target_arch = "xtensa")`, as graviola's rows are gated on the
   arches it supports. An `aws!` macro gives the helpers `()` for aws-lc-rs's algorithm type there,
   so every other row is unchanged.
3. Not yet a patch: move `aws-lc-rs`/`aws-lc-sys` in `rind-bench/Cargo.toml` to
   `[target.'cfg(not(target_arch = "xtensa"))'.dev-dependencies]`, as graviola's already are. The
   firmware does not build the `rind-bench` crate, so it does not need this; `cargo bench` for the
   target would.
4. Optionally, `criterion-esp/` and `esp32/` as `rind-bench/esp32/`, with a `just bench-esp32`
   recipe, and an `esp32s3` host for `admin/bench-report` (rind-results' `cargo xtask report`
   already writes its schema-1 JSON).
5. A separate PR, now that the ESP32-S3 and nRF52840 have confirmed it is faster at every
   size: `patches/poly1305-32/0001-feat-portable-Poly1305-in-26-bit-limbs-on-32-bit-tar.patch`,
   a portable Poly1305 in five 26-bit `u32` limbs (poly1305-donna-32's radix) for 32-bit
   targets, in place of HACL*'s, whose 26-bit limbs live in `u64`s. Optional sets are
   numbered from `0001` inside their own directory, so this is not a sixth base patch;
   see [patches/README.md](patches/README.md). It is not in the published images.
   Hardware A/B (2026-11-12, 21/21 known-answer checks on both boards):
   ChaCha20-Poly1305 seal of 16 KB is 0.735× the previous cycles on the ESP32-S3 and
   0.698× on the T1000-E; XChaCha20-Poly1305 16 KB is 0.719× and 0.763×. The gain is
   against rind's previous Poly1305, not against ring. See
   [patches/poly1305-32/README.md](patches/poly1305-32/README.md) and the results in
   [rind-results](https://github.com/steelhead99x/rind-results/tree/main/results/esp32s3/diag/poly1305-32)
   and [the T1000-E set](https://github.com/steelhead99x/rind-results/tree/main/results/nrf52840/diag/poly1305-32).
   No upstream PR or issue has been opened.

## Build and run

The toolchain installer and the capture tools are in rind-results (`tools/setup-toolchain.sh`,
`cargo xtask run-all`). By hand:

```sh
git clone --recursive https://github.com/steelhead99x/rind-hardware && cd rind-hardware
cargo xtask apply-patches && cargo xtask vendor-ring
# Toolchain: rustup, then espup 0.19.0 (Rust "esp" 1.97.0.0, GCC esp-14.2.0_20241119), ldproxy 0.3.5, espflash 4.7.0
cargo xtask build-images esp           # esp32/images/<bench>.elf, ESP-IDF v5.5.1 is fetched on first build
cargo xtask build-images nrf           # nrf52840/images/<bench>.{elf,bin,uf2}
cargo xtask build-images rp2040        # rp2040/images/<bench>.{elf,bin,uf2}; no board needed to build
cargo xtask build-images esp --out images-pad1031 --pad 1031 aes_gcm   # a relinked (layout-shifted) image
cd esp32 && espflash flash --port <PORT> --flash-freq 80mhz images/aes_gcm.elf && espflash monitor   # RB: lines
```

`esp32/env.sh` puts the ESP GCC and `.tools/export-esp.sh` on `PATH`. ring's C needs
`CC_xtensa_esp32s3_espidf`/`AR_..._espidf` set to the chip's own `xtensa-esp32s3-elf-*` tools
(`.cargo/config.toml` does this): the generic `xtensa-esp-elf-gcc` emits big-endian objects.

## Board and method

- **Board**: ESP32-S3 (QFN56) rev v0.2, 8 MB flash, console on the USB-Serial/JTAG port; a Seeed
  XIAO ESP32S3 on its expansion board (SSD1306 OLED and PCF8563 RTC on I2C).
- **Clock**: 240 MHz, checked at boot (`esp_rom_get_cpu_ticks_per_us`); a start record gives it.
- **Core**: one FreeRTOS thread pinned to core 1 at priority 20 with a 160 KB stack (ML-KEM nests
  rind's 27 KB `DecapKey::generate` frame under two ~31 KB bench frames; 96 KB overflowed into the
  heap). The published `mlkem768` and ChaCha images were built at 160 KB; the other images' runs
  were built at 96 KB, before the change, and never came near it. The stack size does not enter a
  timed region. Core 0 keeps the main
  task (blocked joining it) and ESP-IDF's system tasks. No WiFi/BT is started; task and interrupt watchdogs are off; tick 100 Hz.
- **Memory**: PSRAM off (this board's PSRAM fails quad init), so every buffer is internal SRAM.
- **Timing**: CCOUNT, the core's cycle counter. Per benchmark a 0.2 s warm-up, then 11-51 samples of
  >= 2 ms, each `iters` calls; reports median, p5/p95, min/max, mean, stddev in cycles and ns, and
  cycles per byte.
- **Correctness first**: published vectors (NIST GCM, RFC 8439, RFC 4231, RFC 7748, RFC 8032, RFC
  5903, FIPS 180) and cross-checks between libraries run before any timing; a library failing an
  operation's check is not raced on it.
- **Profile**: release, opt-level 3, codegen-units 16, `lto = false`, `panic = abort`.
- **Cache and flash**: ESP-IDF's defaults, a 16 KB instruction cache and DIO flash at 80 MHz. espflash
  writes the flash clock into the image header when it flashes and does not read `sdkconfig`: flash
  with `--flash-freq 80mhz` (rind-results' `cargo xtask run-all` does), or the bootloader runs the flash
  at 40 MHz, the app keeps that clock, and every row that misses the instruction cache takes up to
  1.9x the cycles. The first published ESP32-S3 set (2026-11-11/12) ran at 40 MHz this way; the
  bootloader's `Boot SPI Speed` line in each run log records it, and the report prints it.
  `sdkconfig.icache32` (32 KB cache, 32 B lines) is a diagnostic build only
  (`ESP_IDF_SDKCONFIG_DEFAULTS="sdkconfig.defaults;sdkconfig.icache32"`); its results are kept
  apart in rind-results' `results/diag/`.
- **Bootloader**: `espflash flash` writes espflash's bundled second-stage bootloader (it reports
  ESP-IDF v6.1), not the one ESP-IDF v5.5.1 built. It runs before the app and changes no timing,
  but it is not the build's.

### Xtensa LLVM workarounds (disclosed)

- LLVM's Xtensa backend aborts ("Unable to properly handle scavenged register for indirect jump,
  function code size is significantly larger than estimated") on a `libcrux-hacl-rs` function at
  opt-level 3 and on the main crate with thin LTO. So `lto = false` and `libcrux-hacl-rs` builds at
  opt-level 2. rind's portable backend uses `libcrux-hacl-rs` too, so this touches rind and libcrux.
- `libcrux-p256`'s field inversion needs more than 64 KB of literals at opt-level 3, past `l32r`'s
  range, and `--mtext-section-literals` crashes the backend on it; it builds at opt-level `s`. Only
  libcrux's own P-256 rows use it.

## What the numbers don't prove

- One board, one chip revision, one toolchain. The LLVM Xtensa backend is a fork and young: a
  library's speed here says as much about how its code meets that backend as about its design.
- rind runs its portable Rust backend, not the assembly its README benchmarks on x86_64/aarch64.
  rind's headline numbers do not carry over, either way.
- Code layout moves small-input rows. Cycle counts repeat to 0.0% between runs of one image, but
  relinking moves them: adding the display feature moved 87 of the `aes_gcm` image's 171 rows by
  more than 1% (ring's 32-64 B AES-GCM up to 1.6x slower, libcrux's and RustCrypto's down to 0.6x),
  and adding the harness checks moved `sha256/rustcrypto/32B` to 0.12x and `sha256/rind/32B` to
  0.48x (rind-results `results/diag/layout-relink-2026-11-12`). No gap verdict flipped, but read
  short-input ratios as +-50% at 32-64 B and +-25% at 128-256 B, for rind as well as the others;
  1200-1350 B rows moved by up to 31% and 8-16 KB rows by under 6%. The instruction cache is 16 KB
  and code streams from flash, so where a function lands matters.
- With a 32 KB instruction cache rind's SHA-256, HMAC and HKDF gaps close (rind fastest or within
  6%); the published numbers are at ESP-IDF's default 16 KB.
- No constant-time claims: CCOUNT medians say nothing about timing leaks.
- The hardware row is mbedTLS on the accelerators, not a Rust library and not the fastest possible
  use of them (GHASH is software; DMA mode not used).
- Code size per library is attributed by symbol and shares crates (curve25519-dalek, fiat-crypto,
  libcrux-hacl-rs, libcrux-sha2) between rind's portable backend and its competitors.

## Demo display

`--features oled` adds a progress screen and result pages on the expansion board's SSD1306 128x64
(I2C0, SDA GPIO5, SCL GPIO6, address 0x3C). It draws only between benchmarks, and published
results are measured without it. See [demo/README.md](demo/README.md).

## Contributing criteria

This repo follows rind's `CONTRIBUTING.md`: conventional-commit subjects with AI attribution in a
`Co-Authored-By:` trailer; no binaries or build artifacts (`.gitignore`); rind's `.rustfmt.toml`
(`style_edition = "2024"`); `cargo clippy -- --deny warnings` for every image and both helper
crates; SPDX headers; and a gap decision, in rind-results' `gaps.toml`, for every operation another
Rust library beats rind on by more than 10%. `cargo xtask check` runs the rustfmt and clippy
checks (every bench image, the display build with the examples, the nRF52840 and RP2040 images, the display UI
on the host and the xtask itself); `--clean` (or `CHECK_CLEAN=1`) starts from empty target directories.

## Media

Recordings of the checks, builds, flashes and runs, the serial logs, charts, and every OLED page
rendered at 128x64 and 4x are in rind-results'
[media/](https://github.com/steelhead99x/rind-results/tree/main/media).

## License

Like rind, everything here is licensed under any of **Apache-2.0, ISC, or MIT-0**, at your option;
see [LICENSE](LICENSE). The `rind` submodule and the patches against it are rind's code, under
rind's copyright and its LICENSE. `criterion-esp` reimplements the names of
[criterion](https://github.com/bheisler/criterion.rs)'s API (Apache-2.0 OR MIT), not its code. The
benchmarked libraries are dependencies under their own licenses.
