<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# OLED demo

Screens for photos and demos on the board's display. This is not part of the benchmark port and is
not in the upstream PR: the firmware builds it only with `--features oled`.

## Hardware

Found by I2C scan (`esp32/examples/i2c_scan.rs`) and matching an earlier project on the same
board:

| | |
|---|---|
| Display | SSD1306, 128x64 monochrome OLED (Seeed XIAO ESP32S3 expansion board) |
| Bus | I2C0 at 400 kHz |
| SDA / SCL | GPIO5 / GPIO6 |
| Address | 0x3C (0x51 on the same bus is the board's PCF8563 RTC, unused) |

The controller is taken from that project's `U8G2_SSD1306_128X64_NONAME` driver at the same
address and pins; the scan shows only that something answers at 0x3C.

## Pieces

- `oled-ui/`: `no_std` screens drawn with embedded-graphics 0.8 onto any `DrawTarget<Color =
  BinaryColor>`: `splash`, `progress`, `operation` (bar chart), `summary`, plus `pages`/`tally` to
  read a results table (`heading<TAB>library<TAB>ns` lines). It carries a 128x64 `Frame` for
  rendering on a host.
- `firmware/oled.rs`: the board glue (ssd1306 0.10 in buffered graphics mode), included by
  `esp32/src/main.rs` only under the `oled` feature.

```sh
cd esp32 && cargo build --release --features "aes_gcm oled"   # any bench file, plus oled
```

## Timing isolation

- Drawing happens only in criterion-esp's observer, which runs **between** benchmarks: the progress
  frame for a benchmark is sent over I2C and the transfer has finished before that benchmark's
  setup, warm-up and samples begin; results are only recorded (no I2C) when its last sample ends.
- The SSD1306 refreshes from its own RAM, so a frame on screen costs the ESP32 nothing: no
  interrupt, DMA or task touches the bus while a benchmark is timed.
- A dry pass over the bench file (`criterion::set_dry_run`) counts its benchmarks for the progress
  bar before any timing; it runs each bench function's setup but no benchmark.
- Still, the published numbers in rind-results come from images built **without** `oled`, and
  that matters: the display code changes the image's layout, and layout moves small-input rows.
  Checked on `aes_gcm` (all 171 rows ran with the display live, `results/diag/aes_gcm-oled`):
  every rind and hardware row was within 2% of the published image, as were all 16 KB rows; but
  ring's 32-64 B rows were up to 1.6x slower and libcrux's and RustCrypto's down to 0.6x. That is
  code placement against a 16 KB instruction cache over flash, not the display (it is idle while
  timing): the same shift appears with any relink. So the display's numbers are for show only.

## Showcase

`examples/oled_showcase.rs` cycles the published result pages (`data/esp32s3.tsv`, written by
rind-results' `cargo xtask demo-tsv`) without running any benchmark:

```sh
cd esp32 && cargo build --release --features oled --example oled_showcase
espflash flash --port <PORT> --flash-freq 80mhz target/xtensa-esp32s3-espidf/release/examples/oled_showcase
```

## Pages

1. **Splash** (2 s at boot): rounded frame, `rind-bench` in 9x15 bold, "pure-Rust crypto / on an
   ESP32-S3", the clock and backend on the bottom line.
2. **Progress** (each benchmark): inverted `TIMING` bar with `done/total`, the bench file, the
   operation, the library in 6x10 with the size right-aligned, and a progress bar.
3. **Result pages** (after the run, 4 s each, cycling forever): one per operation, a header with
   the operation and `page/pages`, then one row per library: a tag (`rind`, `ring`, `RC` for
   RustCrypto/dalek, `crux` for libcrux, `HW` for the accelerators), a bar scaled to the slowest,
   **rind's filled and the rest outlined**, and the time. The footer says who is fastest of the
   Rust libraries and by how much ("rind 1.19x faster than ring"); the hardware bar is shown but
   never compared.
4. **Summary**: on how many operations rind is the fastest Rust library, and the geometric mean of
   rind's time over the fastest other's.

To see every screen without a board, render them on the host (PBM, then 4x PNGs or Markdown text):

```sh
(cd demo/oled-ui && cargo run --example render -- ../data/esp32s3.tsv /tmp/out/)
cargo xtask pbm2png /tmp/out/*.pbm             # 4x previews, name.png
cargo xtask pbm2png --scale 1 /tmp/out/*.pbm   # native 128x64, name@1x.png
cargo xtask pbm2text /tmp/out/02-op.pbm
cargo xtask oled-media demo/data/esp32s3.tsv ../rind-results/media/oled   # all of it, as published
```

rind-results' `cargo xtask demo-tsv` writes the page table from the published results.

### Photo tips

The panel is small and bright: shoot in a dim room, exposure down 1-2 stops, the lens square to the
glass so the 0.96" panel isn't keystoned. Page 3's bar charts read best in photos; the summary page
is the one-shot "headline".
