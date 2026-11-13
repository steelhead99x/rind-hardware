<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# rind-hardware: nRF52840 (Arduino Nano 33 BLE)

The same rind-bench bench files as [the ESP32-S3 port](../README.md), built for
`thumbv7em-none-eabihf` and run bare metal on the Nordic nRF52840 (Cortex-M4F, 64 MHz) of an
Arduino Nano 33 BLE, through its stock BOSSA bootloader. This branch is the T1000-E port
adapted for that board's flash map. Same-chip numbers from the Seeed T1000-E live in
[rind-results/results/nrf52840](https://github.com/steelhead99x/rind-results/tree/main/results/nrf52840);
Nano 33 BLE results go to `results/nano33ble` on a `results/nano33ble-<date>` branch.

## What runs

- **No OS, `no_std` + `alloc`.** rind has no bare-metal support upstream; `patches/0003` makes it
  build without std on `target_os = "none"` and gives M-profile Arm the portable backend (rind's
  32-bit Arm assembly needs ARM state and NEON). `build.rs` here rewrites each bench file's `std::`
  paths to `core::`/`alloc::` on the way in, as it does the `criterion` shim's module path; the
  bench code is otherwise upstream's.
- **The criterion shim** (`../criterion-esp`) builds without its `std` feature: it reads the
  cycle counter it is given (`DWT_CYCCNT` here), and writes its `RB:` JSON lines through a
  callback, here USB CDC-ACM.
- **Known-answer checks first** (`../esp32/src/vectors.rs`, shared with the ESP32 port), then the
  benchmarks; a library failing a check is not raced.
- **Libraries**: rind, ring 0.17.14, RustCrypto, dalek, libcrux, as on the ESP32. aws-lc-rs
  needs an OS (`patches/0002` leaves its rows out), graviola has no 32-bit Arm code, and there is
  no hardware baseline: the chip's CryptoCell CC310 needs Nordic's closed `nrf_cc310` library.
- **ring** needs `patches/ring-0.17.14-thumbv7em.patch` (applied by `cargo xtask vendor-ring` into
  `.tools/ring-nrf`, used through `[patch.crates-io]`): its build script assembles its Arm
  perlasm for `target_os = "none"` too, and skips the two NEON-in-ARM-state files a Thumb-only
  core cannot assemble. Its `SystemRandom` is enabled with the
  `less-safe-getrandom-custom-or-rdrand` feature on getrandom 0.2's custom backend, which
  `src/board.rs` serves from the chip's RNG peripheral (bias correction on), as it does getrandom
  0.4's.

## Timing

- `DWT_CYCCNT` at 64 MHz, the flash instruction cache (`NVMC.ICACHECNF`) on, as Meshtastic and
  Nordic's SDK run it. Flash runs with its default wait states; there is no data cache.
- 0.2 s warm-up, 2 ms samples, 11 to 51 samples per benchmark, at most 2 s of measurement, the
  ESP32 port's settings scaled to the clock. Each record carries the median, spread and
  percentiles in cycles.
- USB runs on an interrupt executor (embassy-usb), but each `RB:` line is flushed before the next
  benchmark starts, so no transfer overlaps a timed region; there is no radio, no SoftDevice
  running, and no other interrupt source.
- **Fat LTO and one codegen unit**, unlike the ESP32 images: the `p256` image overflows the
  792 KB application region by 111 KB without it. Every nRF52840 image uses it, so the libraries
  on this board are built alike. As on the ESP32, `libcrux-p256` alone is built at
  `opt-level = "s"`.
- 96 KB heap (`embedded-alloc`), the rest of RAM (about 157 KB) is stack, placed below the data by
  `flip-link` so an overflow faults instead of corrupting memory.

## Flash layout and what is never touched

This matches ArduinoCore-mbed's official Nano 33 BLE linker script
(`variants/ARDUINO_NANO33BLE/linker_script.ld`): the application starts at `0x10000`, and
`bossac` is always invoked with `--offset 0x10000`. There is no SoftDevice and no trailing
UF2 bootloader. RAM starts at `0x20000200` (the first 512 bytes are left alone, as in that
script's `RAM_NVIC` and `RAM_CRASH_DATA` regions).

| Range | Contents | Touched? |
|---|---|---|
| `0x00000`-`0x10000` | Arduino BOSSA bootloader | never |
| `0x10000`-`0x100000` | application | the image (`memory.x`); flash only with `bossac --offset 0x10000` |
| UICR | reset pin, NFC pins, APPROTECT | never: `embassy_nrf::init` is not called, since it writes UICR |

The firmware never writes flash itself, and every way out of it leads back to the bootloader: a
panic, a usage, bus or memory fault (reported over USB first), a HardFault, or a lockup reset
all set `GPREGRET` to `0xB0` (`DFU_MAGIC_SERIAL_ONLY_RESET` in ArduinoCore-mbed) and reset into
the BOSSA bootloader; opening the port at 1200 baud and closing it does the same, and at 300 baud
restarts the firmware for the next run. A double-tap of the reset button also enters the
bootloader if the 1200-baud touch cannot.

## Build, flash, run

```sh
cargo xtask apply-patches && cargo xtask vendor-ring   # once
# Build .elf/.bin only (no UF2): source nrf52840/env.sh (Arm GNU Toolchain 14.2.rel1 in .tools)
# and cargo build --release --features <bench>, then
# arm-none-eabi-objcopy -O binary -j .vector_table -j .text -j .rodata -j .data <elf> <bin>
# Confirm the ELF has no LOAD segment below 0x10000, then:
bossac --port <bootloader-cu> -U -e -w -v -R --offset=0x10000 <bench>.bin
# Enter the bootloader first with a 1200-baud touch of the firmware (or Arduino) CDC port,
# or by double-tapping reset. Identify the board by its USB port; do not open other devices.
# Then, from rind-results, at least five runs per image:
cargo xtask capture-nrf --port <firmware-cu> --runs 5 --out results/raw-nano33ble/<bench>
```

`cargo xtask capture-nrf` waits 15 s after the port appears before it sends `rind-bench:go`:
ModemManager opens new serial ports to probe them, and while it does, it reads some of the
output (an early run lost its first lines and then dropped off USB that way). The firmware starts only on
that line, and a run whose first record is not the start record is marked failed.

## On the T1000-E (not this branch)

The Nano 33 BLE has no Meshtastic firmware to restore. The notes below describe the T1000-E
port this branch was adapted from, and are kept so the two layouts are not confused.

The T1000-E board belongs to a Meshtastic node, so a run is bracketed by a backup and a restore:

1. Stop any host software holding the node's serial port, then export the node's config and
   `--info`.
2. Enter the bootloader and copy its `CURRENT.UF2` and `INFO_UF2.TXT`. `CURRENT.UF2` covers
   `0x1000`-`0xEA000` under family `0x28860057`, and its application range matched the official
   2.7.26 release block for block.
3. Build a family-`0xADA52840` UF2 of that exact `0x27000`-`0xEA000` region (`cargo xtask uf2`). This
   UF2 is the restore image, with the official release as a second path.
4. Run the bench, then re-read `CURRENT.UF2`: the MBR and SoftDevice (`0x1000`-`0x27000`) were
   byte-identical before and after (`cargo xtask uf2 --region 0x1000 0x27000 CURRENT.UF2` prints
   the region's sha256; compare the two).
5. Copy the restore UF2 back. The node came up with its config identical (`--export-config`
   before and after), with the same node ID and channels.

The image never reaches the LittleFS at `0xED000`, so nothing needed re-importing.

Meshtastic starts the nRF52 hardware watchdog (90 s, confirmed by reading
`NRF_WDT->RUNSTATUS` and `CRV` from the first image after DFU). A started
watchdog cannot be stopped except by a watchdog, pin, or power-on reset, so
the first app image has to keep feeding it or it fires and the firmware
resets into the bootloader. This firmware snapshots `RUNSTATUS`/`CRV`/`RREN`
at boot, prints them in the start record, and reloads every enabled RR
register between timed regions when the watchdog was running. A
`wdt-probe` build feeds for a few seconds, then stops and waits, to confirm
the timeout; it is not used in published runs.
