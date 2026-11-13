<!-- SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 -->
# The ESP32-S3 ChaCha20-Poly1305 crash: ring hits an LLVM-Xtensa bug

**Summary.** The `chacha20poly1305` and `xchacha20poly1305` images crashed on the
ESP32-S3 with `StoreProhibited` right after the `start` record. The crash was in
**ring**, not rind. More precisely, it was in the code LLVM-Xtensa generates for
`ring::aead::chacha20_poly1305::{seal,open}`.

ring's fallback Poly1305 state is `#[repr(C, align(64))]`. Any function with a local
aligned to more than 32 bytes gets this prologue from LLVM-Xtensa:

```
entry  a1, N
and    a8, a1, 63
sub    a8, 64, a8
add.n  a1, a1, a8     ; SP moved after `entry` with a plain add, not movsp
```

The windowed ABI needs `movsp` for that step. Without it, two things go wrong:

- **Window spills land in live slots.** On ESP-IDF, every interrupt spills all
  windows. A spill uses the realigned SP, so it writes into the wrong slots.
- **Stack arguments are read from the wrong place.** The function reads its
  stack-passed arguments at `a1 + N` after the shift (`l32i a9, a1, 160` in the
  repro). That address is the caller's SP + 16…64, not the caller's SP.

This is espressif/llvm-project#140 (found after the diagnosis, not filed by us). The
bug is the same in upstream LLVM's `XtensaFrameLowering::emitPrologue`.

## Evidence

1. **Hardware panic.** Both images, all 3 runs, failed the same way
   (`rind-results/results/raw/{chacha20poly1305,xchacha20poly1305}/run-*.log`).
   - The registers were identical across runs: `EXCCAUSE 29`, `EXCVADDR 0xc4ade18e`.
     `LBEG/LEND` were in ROM `memcpy`.
   - In the backtrace, one 0x600-byte frame sits above the TLSF allocator. That
     matches `vectors::chacha_all`, which has `entry a1, 1536`. So the crash came
     during the first known-answer check, when growing a `Vec` after a seal.
   - The free-list pointer was garbage (`0xc4ade182`), so the heap was corrupted.
2. **Emulation.** The images ran in Espressif's QEMU (`qemu-system-xtensa
   -machine esp32s3`), with ADC calibration skipped through gdb.
   - Unpatched, the checks passed, and then ring's first bench call (32 B) faulted:
     `LoadProhibited` at `0x3fdf0000` in `ring::aead::chacha::fallback::ChaCha20_ctr32`.
     That is safe Rust walking off the end of its buffer.
   - The `Overlapping` passed to that call had `len = 0x8200d6e3`. That value is a
     spilled `call8` return address into `benches`. A register-window spill had
     overwritten a live local.
   - In the linked image, the only functions that realign the stack are
     `ring::aead::chacha20_poly1305::seal` and `::open` (found by scanning every
     function prologue for `and aX, a1, …`).
3. **The patch.** `patches/ring-0.17.14-xtensa-poly1305-align.patch` keeps the
   state at 16 bytes on Xtensa, in both the Rust struct and the C struct. Nothing in
   `poly1305.c` needs 64.
   - With the patch, the image has 0 realigning functions.
   - It runs all 56 ChaCha20-Poly1305 results in QEMU with no fault.
   - On the ESP32-S3 itself (2026-11-12 09:09-09:11 PST) the patched images
     (`chacha20poly1305.elf` `b5a7e512…`, `xchacha20poly1305.elf` `9e07e2b5…`) passed
     every known-answer check and finished 3 of 3 runs each with no fault.
   - The known-answer checks still pass for every library.
4. **This minimal repro.** `cargo xtask repro-overalign` (from the repository root) builds `src/lib.rs` for
   `xtensa-esp32s3-none-elf` with the `esp` toolchain (rustc 1.97.0-nightly,
   esp 1.97.0.0) and prints the prologue.
   - It reports FAIL at opt-level 1, 2, 3, s and z, and at 3 with fat LTO and one
     codegen unit.

## Ruled out

- **rind.** rind's portable ChaCha20-Poly1305 passes its checks on the ESP32-S3
  (in QEMU) and on the nRF52840. No rind function in the image realigns the stack.
- **Stack overflow.** The deepest frames on the ChaCha path are under 2 KB:
  `chacha_all` is 1536 B and ring's `seal` is 672 B. The task stack is 96 KB or
  160 KB, and both sizes fail the same way in QEMU. The 10 KB frames in the image
  belong to std's panic-time backtrace symbolizer.
- **Unaligned u32 loads.** None of the faults was `LoadStoreAlignment` (cause 9).
  The causes were 29 (StoreProhibited) on hardware and 15 (LoadProhibited) in QEMU,
  both at wild addresses.
- **Heap poisoning and stack canaries.** `esp32/sdkconfig.debug` enables them and
  print-halt. Its first build silently didn't apply (see the note in that file). The
  fixed build was ready, but the board then dropped off USB before it could be
  flashed. QEMU found the cause first.

## Fix status

- The ring workaround is in `patches/` and wired in through `esp32/Cargo.toml`
  `[patch.crates-io]`; run `cargo xtask vendor-ring` to set it up.
- The compiler fix belongs in LLVM: compute the aligned SP in a scratch register and
  `movsp` it, and address incoming stack arguments from the pre-realignment SP (or a
  frame pointer set before the shift). That needs a rebuilt esp toolchain, so it is
  not attempted here. No upstream issue or PR has been opened.
