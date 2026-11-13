// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! Layout-shift padding, for measuring how much code placement moves the
//! results. `RIND_BENCH_PAD=<bytes>` at build time puts that many zero bytes,
//! never executed, at the start of the firmware's code, ahead of every
//! library, so all library code moves by about that much and nothing else
//! changes. On the ESP32-S3 a linker section-ordering file (`build.rs`) puts
//! the padding first in `.flash.text`; on the nRF52840 it goes in `.Reset`,
//! which cortex-m-rt's `link.x` places first in `.text`. Check the shift with
//! `nm -n`: until 2026-11-12 the padding was an ordinary `.text.*` section and
//! landed after the libraries, which moved them by only a few bytes. Unset or
//! 0 (the published images): no padding. Shared by the ESP32-S3 and nRF52840
//! firmware.

include!(concat!(env!("OUT_DIR"), "/pad.rs"));
