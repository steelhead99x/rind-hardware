<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# rind-hardware: RP2040 (Raspberry Pi Pico)

The same twelve rind-bench bench files as [the ESP32-S3 port](../README.md) and
[the nRF52840 port](../nrf52840/README.md), built for `thumbv6m-none-eabi` and
run bare metal on a Raspberry Pi RP2040 (Cortex-M0+, 125 MHz). No board is
required to build the images; flashing and a run need a Pico or compatible
board, as [Hardware](#hardware) lists.

## What runs

- **No OS, `no_std` + `alloc`.** `patches/0003` makes rind build without std on
  `target_os = "none"` and gives M-profile Arm the portable backend.
  `patches/0005` makes `ReuseCache` work on targets without pointer-sized
  atomics: ARMv6-M has no CAS, so `AtomicU8::compare_exchange` and `fetch_max`
  do not exist. The fallback keeps the same state machine in a `Cell`, with no
  new crate, matching rind's existing "one execution context on bare metal"
  assumption.
- **Two ARMv6-M speedups**, both on Arm targets without CAS atomics only.
  `patches/0006` gives SHA-512 a rolled loop: HACL*'s unrolled one is about
  27 KB of Thumb code, larger than the 16 KB XIP cache, and ran at about
  2,200 cycles per byte (0.095x the time at 16 KB). `patches/0007` takes
  GHASH's 32x32->64 carry-less products with `muls` alone instead of
  `__aeabi_lmul` calls (AES-GCM 0.48x to 0.53x the time).
- `build.rs` here rewrites each bench file's `std::` paths to
  `core::`/`alloc::` and compiles ring rows out (see below); the bench code is
  otherwise upstream's.
- **The criterion shim** (`../criterion-esp`) builds without its `std` feature:
  it reads the cycle counter it is given (SysTick here), and writes its `RB:`
  JSON lines through a callback, here USB CDC-ACM.
- **Known-answer checks first** (`../esp32/src/vectors.rs`, shared with the
  other ports), then the benchmarks; a library failing a check is not raced.
  ring's checks are compiled out on this target.
- **Libraries**: rind, RustCrypto, dalek, libcrux. **ring 0.17.14 is left
  out**: its Arm assembly uses NEON (`vpush {q4,q5,q6,q7}` in
  `x25519-asm-arm.S`) and does not assemble for ARMv6-M. aws-lc-rs needs an OS
  (`patches/0002` leaves its rows out). The chip has no crypto accelerator.

## Timing

- **SysTick at the processor clock**, 125 MHz with the official Pico 12 MHz
  crystal and PLL (`embassy_rp::init` default). Cortex-M0+ has no DWT cycle
  counter (`DWT_CYCCNT`). SysTick is a 24-bit down-counter (wraps every
  134.2 ms at 125 MHz); a software extension gives a 32-bit count that wraps
  every 34.4 s, the same width as DWT on the nRF52840. **Resolution is 1 CPU
  cycle.**
- The RP2040's **1 MHz TIMER** (64-bit, 1 µs) is an independent check of that
  count at boot, over about 300 ms (at least two SysTick wraps), reported in
  the start record as `check_cycles` and `check_us`. It is not the timed clock: 1 µs is too coarse for the short
  rows.
- The 16 KB XIP cache is left on, as boot2 enables it. There is no data
  cache. Core 1 is left parked, so the `critical-section` impl (a hardware
  spinlock) is only used from core 0 and its interrupts.
- 0.2 s warm-up, 2 ms samples, 11 to 51 samples per benchmark, at most 2 s of
  measurement, the other ports' settings scaled to the clock.
- USB runs on an interrupt executor (embassy-usb), but each `RB:` line is
  flushed before the next benchmark starts, so no transfer overlaps a timed
  region.
- **Fat LTO and one codegen unit**, as on the nRF52840 images, so every
  library on this board is built alike.
- 96 KB heap (`embedded-alloc`), the rest of the 256 KB main SRAM is stack,
  placed below the data by `flip-link` so an overflow faults instead of
  corrupting memory. USB has its own 4 KB DPRAM.
- **RNG**: the RP2040 has no TRNG. `getrandom` is a seeded SplitMix64, the
  same generator the nRF52840 `seeded-rng` image uses, so libraries that draw
  randomness do the same work. It is not a source of entropy. The start
  record's `rng` field is `splitmix64`.

## Hardware

To flash and run, plug in:

- A **Raspberry Pi Pico**, Pico W, or another RP2040 board with a **12 MHz
  crystal** (the official Pico default; a different crystal needs a rebuild
  with the matching `ClockConfig`) and **W25Q080 or compatible QSPI flash**
  (embassy-rp's `boot2-w25q080`, the Pico default; other chips need a
  different `boot2-*` feature).
- A USB cable to the board's USB device port. Hold **BOOTSEL**, plug in, and
  the board appears as a UF2 mass-storage drive. Copy
  `rp2040/images/<bench>.uf2`, open the CDC-ACM port, and send
  `rind-bench:go\n`. Opening the port at 1200 baud and closing it re-enters
  the bootloader; at 300 baud it restarts the firmware for the next run.

There is no official QEMU machine for the RP2040 with enough RAM to run these
images (only `qemu-system-xtensa` is vendored here), so this port ships as a
build and a UF2; known-answer checks run on the board after a flash.

First hardware run, 2026-11-12: a Raspberry Pi Pico (12 MHz crystal,
W25Q080-compatible flash). Every known-answer check passed on all twelve
images. Published timing results are not yet in rind-results (there is no
`results/rp2040/` tree on that repo's main); do not claim they are there
until they land.

## Build

```sh
cargo xtask apply-patches   # commits onto rind's hardware/applied branch
cargo xtask build-images rp2040    # rp2040/images/<bench>.{elf,bin,uf2}; sources rp2040/env.sh
cargo xtask provenance rp2040 --rind-bench <commit> --note "rp2040 build"   # refuses a dirty rind tree
```

`cargo xtask uf2 IMAGE.bin OUT.uf2 --base 0x10000000 --family 0xe48bff56` writes
a BOOTSEL UF2 of a raw image. The 2 MiB XIP region is `0x10000000`-`0x10200000`;
the first 256 bytes are boot2.
