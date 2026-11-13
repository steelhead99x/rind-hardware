// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! Minimal repro: an over-aligned (> 16 B) local plus a stack-passed argument.
//! Build: cargo +esp rustc --release -Zbuild-std=core \
//!          --target xtensa-esp32s3-none-elf -- --emit asm
#![no_std]

#[repr(C, align(64))]
pub struct Aligned([u32; 16]);

unsafe extern "C" {
    fn sink(p: *mut Aligned);
}

/// Seven words of arguments: a2..a7 hold the first six, `g` goes on the stack.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn takes_stack_arg(a: u32, b: u32, c: u32, d: u32, e: u32, f: u32, g: u32) -> u32 {
    let mut s = Aligned([a ^ b ^ c ^ d ^ e ^ f; 16]);
    // SAFETY: `sink` only reads and writes `s`.
    unsafe { sink(&mut s) };
    s.0[0].wrapping_add(g)
}
