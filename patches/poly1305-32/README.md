<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# poly1305-32: a 32-bit-limb Poly1305 for rind's portable backend

An optional patch set, not in the published images. `0006-...patch` is written as an upstream
rind PR (`feat(portable): Poly1305 in 26-bit limbs on 32-bit targets`); it applies on top of
patches 0001-0005 at rind `98568332`. (It was written as 0005, before `0005-feat-reuse-cache-...`
landed, and renumbered; the content is unchanged.)

## Why

On the ESP32-S3, rind's ChaCha20-Poly1305 sealed 16 KiB 1.27x slower than ring, although
rind's ChaCha20 is faster than ring's. A sampling profile put 58% of the time in HACL*'s
Poly1305 (libcrux-poly1305), whose 26-bit limbs are stored in `u64`s, so a 32-bit core runs
64-bit multiplies and adds in software.

## Design

- `rind/src/low/generic/poly1305_32.rs`: five `u32` limbs in radix 2^26. Each limb product is
  one 32x32->64-bit multiply (`mull`/`muluh` on Xtensa, `umull` on Cortex-M). The radix and
  carry chain follow poly1305-donna-32. Each block gets a partial reduction, and the final
  subtraction of p is masked. It has no secret-dependent branch or index, and its time is
  constant where the 32x32->64 multiply is (ESP32-S3, Cortex-M4; not Cortex-M3).
- Selection (`low/portable/poly1305.rs`): the portable backend uses it when
  `target_pointer_width = "32"`, or when `--cfg rind_limb="32"` is forced so CI can test it on
  x86_64. 64-bit targets keep Graviola's 64-bit limbs. armv7 keeps NEON, and HACL*'s without it.
- No input counter, so no 2^32-byte limit (HACL*'s streaming state has one).
- The correctness argument is the bounds in the comments (limb sums < 2^27 + 2^6, product sums
  < 2^58, carries < 2^32). Debug assertions check them.
- `ALGORITHMS.md` and `TESTING.md` are updated to match.

## Tests (run 2026-11-12, x86_64 host)

| Run | Result |
|---|---|
| native `cargo test -p rind --lib poly1305` (new `poly1305_32` tests: RFC 8439 2.5.2 and A.3 #5-#11, aws-lc vectors, split updates, differential vs Graviola's 64-bit Poly1305) | 10 passed |
| forced `--cfg rind_portable --cfg rind_limb="32"`, release with debug assertions and overflow checks: `--lib` | 418 passed; the only 4 failures are CCTV ML-KEM files not checked out (`thirdparty/cctv`), unrelated |
| same, `--test wycheproof --test aws_lc --test crosschecks` (ChaCha20-Poly1305 and XChaCha20-Poly1305 run through the new code) | 21 + 9 + 72 passed |
| forced `--cfg rind_portable` (64-bit limbs), poly1305 tests incl. differential vs HACL* | passed |
| clippy `-D warnings`: native all-targets, forced portable 64-bit and 32-limb all-targets, thumbv7em-none-eabihf, riscv32imac-unknown-none-elf, armv7-unknown-linux-gnueabihf | clean |
| xtensa-esp32s3-espidf (esp toolchain) | builds. Clippy reports the same 3 errors with and without the patch (`mid/mldsa.rs`, `low/cpu.rs`, `low/entry.rs`, from patch 0003 and the esp toolchain's older clippy), none in this patch |
| riscv32imc-unknown-none-elf, thumbv6m-none-eabi | clippy clean with base patch 0005 (`ReuseCache` without CAS) applied; without it they fail in `reuse_cache.rs` with or without this patch |

Wycheproof caught a bound that an earlier draft's debug assertion set too tight (h1 can reach
2^27 + 2^6). The arithmetic was right; the assertion and its comment were fixed.

The differential test (`matches_hacl_and_generic`) compares against Graviola's Poly1305 and,
where the portable backend builds, HACL*'s. It covers every length up to 300 bytes, random keys
and lengths up to 16 KiB fed in random pieces (512 cases in release, 96 in debug), and
all-ones and all-zeros keys and messages. It marks the key and message secret for
`admin/ctgrind`.

## QEMU (correctness only)

QEMU timing means nothing, so this run checks correctness only. The two images were rebuilt with
`esp32/sdkconfig.qemu` layered on (console on UART0, since QEMU doesn't emulate the USB
Serial/JTAG port) and run in Espressif's `qemu-system-xtensa -machine esp32s3` (9.0.0).
ESP-IDF's `adc_hw_calibration` spins forever in QEMU, so gdb skips it: break after its
`entry`, then set `pc` to its `retw.n`.

- `chacha20poly1305` (sha256 `a0f9e6d9…`): every known-answer check passed for rind, ring,
  RustCrypto and libcrux (`checks-done failed:0`), and all 58 results finished with no fault.
- `xchacha20poly1305` (sha256 `8bdcc8d1…`): `checks-done failed:0`, and all 23 results
  finished with no fault.

```sh
cd esp32 && . ./env.sh
ESP_IDF_SDKCONFIG_DEFAULTS="sdkconfig.defaults;sdkconfig.qemu" cargo build --release --features chacha20poly1305
espflash save-image --chip esp32s3 --merge --flash-size 8mb --flash-freq 80mhz \
  target/xtensa-esp32s3-espidf/release/rind-bench-esp32 /tmp/q.bin
LD_LIBRARY_PATH=../.tools/qemu/lib ../.tools/qemu/qemu/bin/qemu-system-xtensa -nographic \
  -machine esp32s3 -drive file=/tmp/q.bin,if=mtd,format=raw -no-reboot -S -gdb tcp::1240 &
E=target/xtensa-esp32s3-espidf/release/rind-bench-esp32
RET=$(xtensa-esp32s3-elf-gdb -q -batch -ex 'disassemble adc_hw_calibration' $E | awk '/retw/ {print $1; exit}')
xtensa-esp32s3-elf-gdb -q -batch -ex 'target remote :1240' -ex 'hbreak *(adc_hw_calibration+3)' \
  -ex continue -ex "set \$pc = $RET" -ex detach $E
```

`xtensa-esp32s3-elf-gdb` is in `esp32/.embuild/espressif/tools/xtensa-esp-elf-gdb/`.

## Images (built, not flashed)

Built 2026-11-12 16:27-16:52 PST from rind `98568332` with patches 0001-0004, using the same tree
and toolchain for both. Base patch 0005 landed afterwards. It changes only code under
`cfg(not(target_has_atomic = "ptr"))`, and both chips have pointer-width atomics, so rind's code in
these images matches a 0001-0005 build. `-base` leaves out `poly1305-32` and is the A/B baseline from the
same build. Each folder has `SHA256SUMS` and `images.json` (provenance).

| Image | sha256 |
|---|---|
| `esp32/images-poly1305-32/chacha20poly1305.elf` | `4c6ce5d5dc3e40068b6694cda269492963229d8a8d68a5cc0a7f56f3986ad58a` |
| `esp32/images-poly1305-32/xchacha20poly1305.elf` | `65d8b8884d938c390d1e08b147a8110e86087ab5b237a7071c849d666a4d37a9` |
| `esp32/images-poly1305-32-base/chacha20poly1305.elf` | `0ff96fdc84eb56ce927964ff7ecbe6b27d7e08c8c4e7dd4b4af634ba17c93f50` |
| `esp32/images-poly1305-32-base/xchacha20poly1305.elf` | `b3e48a9f164914f52c00e240f5d70eb0393e9f07ca47d58c1c6f410b7b8d1484` |
| `nrf52840/images-poly1305-32/chacha20poly1305.uf2` | `98ee3da1ef2055e1d8cb10bfb3bf9cdada626f97f9008fb5a2331aa39e9963de` |
| `nrf52840/images-poly1305-32/xchacha20poly1305.uf2` | `de180373221f762cdcff4299a4ac9fd17f887be05af6f7926a5ab3bde584919e` |
| `nrf52840/images-poly1305-32-base/chacha20poly1305.uf2` | `e48da3d89d91a949c383457f103898512edd3175fcd2265c5331c2013fcbf52f` |
| `nrf52840/images-poly1305-32-base/xchacha20poly1305.uf2` | `1e05845983051f5495c78ce8824a283714f40ff615a66e1501363c3c6327dc35` |

(nRF ELFs: `321d76f5…` / `6ee6365f…` patched, `dc0b1e11…` / `4eba6d12…` base.)

To rebuild:

```sh
cargo xtask apply-patches --set poly1305-32
cargo xtask build-images esp chacha20poly1305 xchacha20poly1305 --out images-poly1305-32
cargo xtask build-images nrf chacha20poly1305 xchacha20poly1305 --out images-poly1305-32
cargo xtask apply-patches --set poly1305-32 --reverse   # back to the published patch set
```

The images are a relink, so short inputs also move with code layout (see rind-results'
layout notes). Read the 16 KiB rows first, and compare against `-base`, not against the
published set.

## Hardware run

ESP32-S3, from `~/rind-results` (about 10 minutes). Set `ESPFLASH_PORT` if more than one board is plugged in:

```sh
cd ~/rind-hardware/esp32/images-poly1305-32 && sha256sum -c SHA256SUMS && cd -
cd ~/rind-hardware/esp32/images-poly1305-32-base && sha256sum -c SHA256SUMS && cd -
cd ~/rind-results
D=results/esp32s3/diag/poly1305-32
OUT=$D/raw-base cargo xtask run-all ~/rind-hardware/esp32/images-poly1305-32-base chacha20poly1305 xchacha20poly1305
OUT=$D/raw      cargo xtask run-all ~/rind-hardware/esp32/images-poly1305-32      chacha20poly1305 xchacha20poly1305
cargo xtask compare $D/raw-base $D/raw chacha20poly1305 xchacha20poly1305 \
  --label-a hacl --label-b poly1305-32 --out $D/compare.md
```

`run-all` flashes at 80 MHz and captures 3 runs each. Every known-answer check must pass on
both sides. The rows to read are rind's ChaCha20-Poly1305 and XChaCha20-Poly1305
seal and open, against ring's rows from the same runs.

nRF52840 (T1000-E): this needs a Meshtastic bridge window. Follow
[nrf52840/README.md](../../nrf52840/README.md)'s backup and restore steps: stop
`mesh-watcher.timer` and `meshtastic-bridge`, export the config, run, restore the Meshtastic UF2,
and restart both. Between those steps:

```sh
cd ~/rind-results
D=results/nrf52840/diag/poly1305-32
OUT=$D/raw-base cargo xtask run-all-nrf ~/rind-hardware/nrf52840/images-poly1305-32-base chacha20poly1305 xchacha20poly1305
OUT=$D/raw      cargo xtask run-all-nrf ~/rind-hardware/nrf52840/images-poly1305-32      chacha20poly1305 xchacha20poly1305
cargo xtask compare $D/raw-base $D/raw chacha20poly1305 xchacha20poly1305 \
  --label-a hacl --label-b poly1305-32 --out $D/compare.md
```

The upstream PR goes ahead only if seal and open are faster at every size on both boards
(AGENTS.md: a verified source gives way only to a faster one).
