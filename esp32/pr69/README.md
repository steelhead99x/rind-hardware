<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# ESP32-S3 re-run for rind PR 69

Build two sets of benchmark images and time them on the Seeed XIAO ESP32-S3. One set is `steelhead99x/rind` main `8dfb7942d231219eed477fb58f5e15fe8a821bd2`. The other is `ef58672f28c16f49b727d2e360b474ada1913b8a` on `perf/esp32s3-rolled-sha512` ([rind#69](https://github.com/steelhead99x/rind/pull/69)). Compare those two fresh runs with each other. This directory does not flash the board.

The rows live in three firmware images, because one image per bench file is what fits Xtensa's `call8` range: `sha2`, `hmac`, and `hkdf`.

## Toolchain

The same one the PR names: espup 0.19.0, Rust `esp` 1.97.0-nightly (`8ea53bcd7`, 1.97.0.0, LLVM 21.1.3), GCC `esp-14.2.0_20241119`, ldproxy 0.3.5. ESP-IDF v5.5.1 is fetched by `esp-idf-sys` on the first build (`ESP_IDF_VERSION` in `esp32/.cargo/config.toml`).

```sh
cargo install espup --version 0.19.0 --locked
cargo install ldproxy --version 0.3.5 --locked
mkdir -p .tools
espup install --toolchain-version 1.97.0.0 --targets esp32s3 \
  --crosstool-toolchain-version 14.2.0_20241119 \
  --export-file .tools/export-esp.sh
ln -sfn "$(command -v ldproxy)" .tools/ldproxy
. .tools/export-esp.sh
rustc +esp --version
```

`rustc +esp --version` should print `1.97.0-nightly (8ea53bcd7 2026-07-08) (1.97.0.0)`.

## Build

From the repository root. This resets the `rind` submodule checkout while it runs and puts it back afterwards. It does not flash, and it does not push to `steelhead99x/rind`.

```sh
./esp32/pr69/build-ab.sh
```

What it does, in order:

1. Fetches `steelhead99x/rind` `main` and `perf/esp32s3-rolled-sha512`.
2. Vendors ring 0.17.14 and applies `patches/ring-0.17.14-xtensa-poly1305-align.patch`, the same patch `cargo xtask vendor-ring` applies for the ESP32-S3.
3. Builds `esp-idf-sys` 0.38.1 (`std`, `binstart`, `native`) into `esp32/target` and `esp32/.embuild` before any bench image, so ring's C files are compiled after that toolchain install.
4. For each rind commit, checks it out and commits `patches/0001` through `patches/0005` with the same unsigned `git am` as `cargo xtask apply-patches` (committer `steelhead99x`, `--no-gpg-sign`, `--committer-date-is-author-date`). `cargo xtask apply-patches` itself only starts from the submodule pin, so this script checks out the requested commit first and then uses that `git am`.
5. Builds `sha2`, `hmac`, and `hkdf` in release (`--features <bench>`, `RIND_BENCH_PAD=0`) and copies the ELFs out.

`0003` (no_std) does not apply as a plain diff on either commit. Upstream inserted `MissingFeature::name` in `rind/src/low/cpu.rs` between the `Display` impl and the `Error` impl, so that hunk's context does not match. The two commits have the same `cpu.rs`. The script retries that patch with `git am -3`. The 3-way merge is clean: the new method stays, and the no_std change lands around it. `0001`, `0002`, `0004`, and `0005` (including the cache-without-CAS patch) apply with a plain `git am` on both commits. The script writes this to `esp32/images-pr69/patch-status.txt` and stops if a patch does not merge.

Images, gitignored:

```text
esp32/images-pr69/8dfb7942/sha2.elf
esp32/images-pr69/8dfb7942/hmac.elf
esp32/images-pr69/8dfb7942/hkdf.elf
esp32/images-pr69/ef58672f/sha2.elf
esp32/images-pr69/ef58672f/hmac.elf
esp32/images-pr69/ef58672f/hkdf.elf
```

Pass bench names to build a different set: `./esp32/pr69/build-ab.sh sha2`.

## On the board

Seeed XIAO ESP32-S3, USB-Serial/JTAG. The PR's port is `/dev/ttyACM1`. Confirm that is the XIAO before flashing. 240 MHz CPU (the image checks this and stops if it is not), DIO flash at 80 MHz. `espflash` writes the flash clock; without `--flash-freq 80mhz` the bootloader runs the flash at 40 MHz.

Flash one ELF, then capture three fresh boots, then the next ELF. Six ELFs, three boots each. Reset the board between boots (reset button or replug) so each capture starts from a new boot. A second `espflash monitor` on a chip that has already printed `{"type":"done"}` is not a new boot.

`8dfb7942` is main. `ef58672f` is the PR head. Do main first.

```sh
cd esp32
espflash flash --port /dev/ttyACM1 --flash-freq 80mhz images-pr69/8dfb7942/sha2.elf
espflash monitor --port /dev/ttyACM1
```

Repeat the flash with `hmac.elf` and `hkdf.elf`, then the three ELFs under `images-pr69/ef58672f/`. The bootloader line `Boot SPI Speed` should say 80 MHz. The `RB:` start record should have `"cpu_hz":240000000`.

Each boot prints `RB:` lines. Keep the whole log. Known-answer checks print `{"type":"checks-done","failed":N}`. `failed` must be 0 on every boot. If it is not, do not use that boot's timings.

For a result line, `ns` is the median of that boot's samples (the same number the harness reports as the wall time at 240 MHz). `cycles.median` is the same sample median in cycles. For each id below, take the three boots' `ns` values, sort them, and report the middle one. That is the median of the three run medians. Compare those middles between `8dfb7942` and `ef58672f`.

`sha256-updates` hashes 4096 bytes, fed as 1-byte or 16-byte `update` calls.

| Image | Id |
|---|---|
| sha2 | `sha256/rind/32B` |
| sha2 | `sha256/rind/64B` |
| sha2 | `sha256/rind/1200B` |
| sha2 | `sha256/rind/1350B` |
| sha2 | `sha256/rind/8KB` |
| sha2 | `sha256/rind/16KB` |
| sha2 | `sha256-updates/rind/1B` |
| sha2 | `sha256-updates/rind/16B` |
| sha2 | `sha512/rind/32B` |
| sha2 | `sha512/rind/1200B` |
| sha2 | `sha512/rind/16KB` |
| hmac | `hmac-sha256/rind/32B` |
| hmac | `hmac-sha256/rind/64B` |
| hmac | `hmac-sha256/rind/reused-key/32B` |
| hmac | `hmac-sha256/rind/reused-key/1200B` |
| hmac | `hmac-sha256/rind/16KB` |
| hkdf | `hkdf-sha256/extract-expand/rind` |

The hmac image also prints the other hash sizes and reused-key sizes. The hkdf image also prints `hkdf-sha256/expand/rind` and `hkdf-sha256/expand-aes-256-gcm-key/rind`. The table is the comparison set from the PR: SHA-256, the 1-byte and 16-byte updates, HMAC-SHA256, HKDF extract-expand, and SHA-512 at 32 B, 1200 B, and 16 KB.

PR 69's hold on this run: SHA-256, `sha256-updates`, HMAC-SHA256, and HKDF extract-expand should match the fresh main image within the run-to-run spread. SHA-512 should still show the rolled-loop win. SHA-256 at 32 B and 64 B is not expected to beat main.
