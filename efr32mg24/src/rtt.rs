// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! SEGGER RTT, one up channel: the `RB:` JSON lines go to the host through
//! the debug probe's memory reads, as the nRF52840 and RP2040 images send
//! them over USB. The XIAO MG24's SAMD11 is a CMSIS-DAP probe, so this needs
//! no UART pins and no USB stack on the EFR32MG24 itself.
//!
//! `write_and_flush` returns only once the host has read every byte, so no
//! probe read of the buffer overlaps a timed region more than a polling host
//! must (it still reads the control block between lines). A write blocks
//! while the buffer is full: a host must be attached.

use core::ptr::{addr_of, addr_of_mut};
use core::sync::atomic::{Ordering, compiler_fence};

const UP_SIZE: usize = 4096;
const DOWN_SIZE: usize = 16;

#[repr(C)]
struct Channel {
    name: *const u8,
    buffer: *mut u8,
    size: u32,
    write: u32,
    read: u32,
    flags: u32,
}

#[repr(C)]
struct ControlBlock {
    id: [u8; 16],
    max_up: u32,
    max_down: u32,
    up: Channel,
    down: Channel,
}

/// probe-rs finds this by its ELF symbol, or by scanning RAM for the ID,
/// which `init` writes last so a half-initialised block is never matched.
#[unsafe(no_mangle)]
static mut _SEGGER_RTT: ControlBlock = ControlBlock {
    id: [0; 16],
    max_up: 0,
    max_down: 0,
    up: Channel {
        name: core::ptr::null(),
        buffer: core::ptr::null_mut(),
        size: 0,
        write: 0,
        read: 0,
        flags: 0,
    },
    down: Channel {
        name: core::ptr::null(),
        buffer: core::ptr::null_mut(),
        size: 0,
        write: 0,
        read: 0,
        flags: 0,
    },
};
static mut UP: [u8; UP_SIZE] = [0; UP_SIZE];
static mut DOWN: [u8; DOWN_SIZE] = [0; DOWN_SIZE];
static NAME: &[u8] = b"Terminal\0";

/// Sets up the control block. Call once, before any write.
pub fn init() {
    // SAFETY: single-threaded start-up; nothing else touches the block yet.
    unsafe {
        let cb = addr_of_mut!(_SEGGER_RTT);
        (*cb).max_up = 1;
        (*cb).max_down = 1;
        (*cb).up = Channel {
            name: NAME.as_ptr(),
            buffer: addr_of_mut!(UP) as *mut u8,
            size: UP_SIZE as u32,
            write: 0,
            read: 0,
            // SEGGER_RTT_MODE_BLOCK_IF_FIFO_FULL
            flags: 2,
        };
        (*cb).down = Channel {
            name: NAME.as_ptr(),
            buffer: addr_of_mut!(DOWN) as *mut u8,
            size: DOWN_SIZE as u32,
            write: 0,
            read: 0,
            flags: 0,
        };
        compiler_fence(Ordering::SeqCst);
        let id = b"SEGGER RTT\0\0\0\0\0\0";
        // The ID last, back to front, as SEGGER's own code does.
        for i in (0..16).rev() {
            addr_of_mut!((*cb).id[i]).write_volatile(id[i]);
        }
        compiler_fence(Ordering::SeqCst);
    }
}

fn up() -> *mut Channel {
    // SAFETY: a raw pointer to the static block; accesses below are volatile.
    unsafe { addr_of_mut!(_SEGGER_RTT.up) }
}

fn write_bytes(bytes: &[u8]) {
    let ch = up();
    for &b in bytes {
        // SAFETY: `write` is ours, `read` is the host's; both are word
        // accesses the probe also makes. The buffer index stays in bounds.
        unsafe {
            let w = addr_of!((*ch).write).read_volatile();
            let next = (w + 1) % UP_SIZE as u32;
            while addr_of!((*ch).read).read_volatile() == next {}
            (addr_of_mut!(UP) as *mut u8)
                .add(w as usize)
                .write_volatile(b);
            compiler_fence(Ordering::SeqCst);
            addr_of_mut!((*ch).write).write_volatile(next);
        }
    }
}

/// Waits until the host has read everything written so far.
pub fn flush() {
    let ch = up();
    // SAFETY: as in `write_bytes`.
    unsafe {
        while addr_of!((*ch).read).read_volatile() != addr_of!((*ch).write).read_volatile() {}
    }
}

/// One `RB:` line (criterion's `emit` adds the newline), then waits for the
/// host to drain the buffer.
pub fn write_and_flush(s: &str) {
    write_bytes(s.as_bytes());
    flush();
}

/// For the panic and fault handlers: the same path; RTT needs no allocation.
pub fn emergency_write(msg: &str) {
    write_bytes(b"RB:{\"type\":\"panic\",\"message\":\"");
    write_bytes(msg.as_bytes());
    write_bytes(b"\"}\n");
    flush();
}
