<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# rind-hardware: EFR32MG24 (Seeed XIAO MG24)

The same rind-bench bench files as [the ESP32-S3 port](../README.md) and
[the nRF52840 port](../nrf52840/README.md), built for `thumbv8m.main-none-eabihf` and run bare
metal on a Silicon Labs EFR32MG24B220F1536IM48 (Cortex-M33 with FPU and DSP extension, 78 MHz,
1536 KB flash, 256 KB RAM) on a Seeed Studio XIAO MG24. Results go to
[rind-results](https://github.com/steelhead99x/rind-results).

## What runs

- **No OS, `no_std` + `alloc`**, as on the nRF52840: `patches/0003` gives M-profile Arm rind's
  portable backend, and `build.rs` (the nRF52840's, unchanged) ports each bench file's `std::`
  paths on the way in.
- **Known-answer checks first** (`../esp32/src/vectors.rs`), then the benchmarks; a library failing
  a check is not raced.
- **Libraries**: rind, ring 0.17.14, RustCrypto, dalek and libcrux, as on the nRF52840. ring uses
  the same `patches/ring-0.17.14-thumbv7em.patch` (`.tools/ring-nrf`): ARMv8-M Mainline is
  Thumb-2, so its 32-bit Arm perlasm assembles and runs here too. aws-lc-rs needs an OS, graviola
  has no 32-bit Arm code, and there is no hardware baseline: the chip's AES and SHA engines sit
  behind the Secure Engine's mailbox.
- **Randomness**: the EFR32MG24's TRNG is also behind the Secure Engine. `getrandom` 0.4 and 0.2
  (ring's `SystemRandom`) are served from the seeded SplitMix64 the RP2040 image uses, so every
  library that draws randomness does the same work. It is not a source of entropy, and the
  benchmarks do not depend on one.

## Clock

At reset the core runs from FSRCO (20 MHz). `src/board.rs`:

1. switches back to FSRCO and stops DPLL0 if a debugger reset left them otherwise,
2. starts the 39 MHz HFXO with its reset configuration,
3. sets flash to one wait state (up to 80 MHz) and loads DEVINFO's HFRCODPLL calibration for the
   80 MHz band,
4. locks HFRCODPLL to HFXO x 2000 / 1000 = 78 MHz with DPLL0 in frequency-lock mode (Silicon Labs'
   `CMU_DPLL_HFXO_TO_78MHZ`),
5. runs the core from HFRCODPLL with HCLK = SYSCLK and PCLK = HCLK / 2.

The start record carries an independent check from the CMU's calibration counter: HFXO edges
during 400,001 HCLK cycles (`hclk_hz_vs_hfxo`, taking HFXO as 39 MHz), the DWT cycles over the
same window (`window_dwt`), HFRCODPLL against HFXO, and HFXO against FSRCO. An image that does not
lock the DPLL reports `dpll-not-locked` and times nothing. The instruction cache (ICACHE0) is on,
as at reset.

## Output and timing

- `RB:` JSON lines over SEGGER RTT (`src/rtt.rs`, one 4 KB up channel, no dependency) through the
  board's on-board SAMD11 CMSIS-DAP probe: no UART pins or USB stack on the EFR32MG24. Each line
  is drained by the host before the next benchmark starts; the host still polls the control block
  over SWD between lines.
- `DWT_CYCCNT` at 78 MHz, the nRF52840's criterion settings (0.2 s warm-up, 2 ms samples, 11 to
  51 samples, at most 2 s per benchmark) scaled to the clock. No interrupt is enabled.
- Fat LTO and one codegen unit, as on the nRF52840 and RP2040; `libcrux-p256` at `opt-level = "s"`.
- 96 KB heap (`embedded-alloc`); `flip-link` puts the stack below the data.
- A panic or fault prints one `panic` record and halts; the probe takes the next image.

## Build, flash, run

```sh
cargo xtask apply-patches && cargo xtask vendor-ring   # once
cargo xtask build-images mg24                          # efr32mg24/images/<bench>.elf
probe-rs run --chip EFR32MG24B220F1536IM48 efr32mg24/images/sha2.elf
```

`probe-rs run` erases and programs the whole image from `0x08000000` and prints the RTT channel.
This overwrites whatever the board ran (the XIAO MG24 ships an Arduino sketch, with no
bootloader). Save the whole flash first and restore it afterwards:

```sh
probe-rs read --chip EFR32MG24B220F1536IM48 -f binary b32 0x08000000 393216 -o backup.bin
probe-rs download --chip EFR32MG24B220F1536IM48 --binary-format bin --base-address 0x08000000 backup.bin
```
