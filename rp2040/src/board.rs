// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! The RP2040 underneath the benchmarks: clocks, the cycle counter, and the
//! ways back into the bootloader.
//!
//! Cortex-M0+ has no DWT cycle counter. Timing uses SysTick at the processor
//! clock (125 MHz with the official Pico 12 MHz crystal): 1 count is 1 CPU
//! cycle. The hardware counter is 24 bits (wraps every 134.2 ms at 125 MHz);
//! a software extension and `cycles` together give a 32-bit count that wraps
//! every 34.4 s, the same width as DWT_CYCCNT on the nRF52840. The 1 MHz
//! TIMER peripheral is an independent check of that count at boot, not the
//! timed clock: its 1 µs resolution is too coarse for the short rows.
//!
//! The RP2040 has no TRNG. `getrandom` is served from a seeded SplitMix64,
//! the same generator the nRF52840 `seeded-rng` image uses, so libraries
//! that draw randomness do the same work. It is not a source of entropy.
//! Core 1 is left parked.

use core::cell::Cell;
use core::sync::atomic::{AtomicU32, Ordering};
use critical_section::Mutex;

/// The official Pico default: 12 MHz crystal, PLL to 125 MHz.
/// `init` overwrites this with `embassy_rp::clocks::clk_sys_freq`.
pub static CPU_HZ: AtomicU32 = AtomicU32::new(125_000_000);

/// SysTick overflows since start-up (each is 0x0100_0000 processor cycles).
static OVERFLOWS: Mutex<Cell<u32>> = Mutex::new(Cell::new(0));

/// SYS, CVR and CSR of the SysTick module (ARMv6-M ARM).
const SYST_CSR: *mut u32 = 0xE000_E010 as *mut u32;
const SYST_RVR: *mut u32 = 0xE000_E014 as *mut u32;
const SYST_CVR: *mut u32 = 0xE000_E018 as *mut u32;
const SYST_CSR_ENABLE: u32 = 1;
const SYST_CSR_TICKINT: u32 = 1 << 1;
const SYST_CSR_CLKSOURCE: u32 = 1 << 2;
const SYST_CSR_COUNTFLAG: u32 = 1 << 16;

/// Resets into the USB UF2 bootloader (BOOTSEL), whose drive takes the next
/// image. `gpio_activity_pin_mask` and `disable_interface_mask` are 0, as on
/// a cold BOOTSEL boot.
pub fn enter_bootloader() -> ! {
    embassy_rp::rom_data::reset_to_usb_boot(0, 0);
    loop {
        cortex_m::asm::wfi();
    }
}

/// Starts the 12 MHz crystal and 125 MHz system PLL, the 1 MHz TIMER, and
/// SysTick at the processor clock. Leaves core 1 parked.
pub fn init() -> embassy_rp::Peripherals {
    let p = embassy_rp::init(Default::default());
    CPU_HZ.store(embassy_rp::clocks::clk_sys_freq(), Ordering::Relaxed);

    // SAFETY: called once, after embassy has taken the HAL peripherals but
    // before anything else programs SysTick.
    let mut core = unsafe { cortex_m::Peripherals::steal() };
    // Lowest of the RP2040's four priority levels, below USB (P2) and its
    // executor (P3), so a tick does not delay a transfer.
    unsafe {
        core.SCB
            .set_priority(cortex_m::peripheral::scb::SystemHandler::SysTick, 3 << 6)
    };

    // SAFETY: SysTick is programmed once, at start-up, with the processor
    // clock as the source and the exception enabled.
    unsafe {
        SYST_CSR.write_volatile(0);
        SYST_RVR.write_volatile(0x00FF_FFFF);
        SYST_CVR.write_volatile(0);
        SYST_CSR.write_volatile(SYST_CSR_ENABLE | SYST_CSR_TICKINT | SYST_CSR_CLKSOURCE);
    }
    p
}

pub fn cpu_hz() -> u64 {
    CPU_HZ.load(Ordering::Relaxed) as u64
}

/// The XIP cache is enabled by the second-stage bootloader (boot2-w25q080).
pub fn xip_cache_enabled() -> bool {
    // XIP CTRL bit 0 is EN: 1 when the 16 KB cache is on.
    // SAFETY: a read of the XIP control register, which boot2 has already set.
    unsafe { (0x1400_0000 as *const u32).read_volatile() & 1 != 0 }
}

/// Processor cycles since start-up, wrapping at 2^32. 1 count is 1 cycle.
pub fn cycles() -> u32 {
    critical_section::with(|cs| {
        // SAFETY: SysTick CVR and CSR, the latter's COUNTFLAG read also
        // clears that bit; the NVIC pending bit is separate and the
        // handler still runs on exit, so OVERFLOWS catches up.
        let current = unsafe { SYST_CVR.read_volatile() };
        let flag = unsafe { SYST_CSR.read_volatile() } & SYST_CSR_COUNTFLAG != 0;
        let mut high = OVERFLOWS.borrow(cs).get();
        if flag {
            high = high.wrapping_add(1);
        }
        high.wrapping_mul(0x0100_0000)
            .wrapping_add(0x00FF_FFFF - current)
    })
}

/// Microseconds from the 1 MHz TIMER, for the boot cross-check only.
pub fn timer_us() -> u64 {
    embassy_time::Instant::now().as_micros()
}

/// Compares SysTick with the 1 MHz TIMER over about 10 ms of busy-wait.
/// Returns (systick cycles, timer microseconds). At 125 MHz the two should
/// agree to within a few microseconds (the TIMER's resolution).
pub fn cross_check() -> (u32, u64) {
    let t0 = timer_us();
    let c0 = cycles();
    let wait = (cpu_hz() / 100) as u32;
    while cycles().wrapping_sub(c0) < wait {}
    (cycles().wrapping_sub(c0), timer_us().saturating_sub(t0))
}

#[cortex_m_rt::exception]
fn SysTick() {
    critical_section::with(|cs| {
        let c = OVERFLOWS.borrow(cs);
        c.set(c.get().wrapping_add(1));
    });
}

/// `getrandom` 0.4's custom backend (its symbol keeps the 0.3 name): a
/// seeded SplitMix64. Not random, and only here because the RP2040 has no
/// TRNG. The seed matches the nRF52840 `seeded-rng` image.
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    // SAFETY: getrandom passes a writable buffer of `len` bytes.
    let dest = unsafe { core::slice::from_raw_parts_mut(dest, len) };
    fill_seeded(dest);
    Ok(())
}

fn fill_seeded(dest: &mut [u8]) {
    static STATE: Mutex<Cell<u64>> = Mutex::new(Cell::new(0x5EED_0F41_DBE4));
    for chunk in dest.chunks_mut(8) {
        let mut z = critical_section::with(|cs| {
            let s = STATE.borrow(cs);
            s.set(s.get().wrapping_add(0x9E37_79B9_7F4A_7C15));
            s.get()
        });
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
    }
}

#[cortex_m_rt::exception]
unsafe fn HardFault(_frame: &cortex_m_rt::ExceptionFrame) -> ! {
    enter_bootloader()
}
