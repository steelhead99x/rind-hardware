<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# Minimal reproductions

- [`xtensa-overalign/`](xtensa-overalign/README.md): LLVM-Xtensa realigns the stack
  without `movsp` for locals aligned past 32 bytes. ring's Poly1305 state hit it on
  the ESP32-S3.
