// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! The nRF52840 underneath the benchmarks: clocks, the cycle counter, the
//! hardware RNG behind `getrandom`, and the ways back into the bootloader.
//!
//! `embassy_nrf::init` is deliberately not called: it writes UICR (the reset
//! pin, NFC pins and APPROTECT) when that differs from its defaults, and UICR
//! can only be restored by erasing it with a debug probe. This firmware never
//! writes UICR, flash, or the SoftDevice; it only reads POWER.RESETREAS and
//! NRF_WDT (RUNSTATUS, CRV, RREN), writes the watchdog reload registers when
//! Meshtastic left the hardware watchdog running, and writes POWER.GPREGRET,
//! which the bootloader reads after a reset.

use core::sync::atomic::{AtomicU32, Ordering};
use embassy_nrf::pac;

/// The nRF52840's CPU clock, fixed.
pub const CPU_HZ: u64 = 64_000_000;

/// Adafruit's nRF52 bootloader enters UF2 mass-storage mode after a reset
/// with this in GPREGRET (`DFU_MAGIC_UF2_RESET`).
const DFU_MAGIC_UF2_RESET: u32 = 0x57;

/// RESETREAS bits: watchdog, CPU lockup.
const RESETREAS_DOG: u32 = 1 << 1;
const RESETREAS_LOCKUP: u32 = 1 << 3;

/// Value written to an enabled RR register to reload the watchdog counter
/// (nRF52840 Product Specification, WDT.RR[n]).
const WDT_RR_RELOAD: u32 = 0x6E52_4635;

static RESET_REASON: AtomicU32 = AtomicU32::new(0);
static WDT_RUNSTATUS: AtomicU32 = AtomicU32::new(0);
static WDT_CRV: AtomicU32 = AtomicU32::new(0);
static WDT_RREN: AtomicU32 = AtomicU32::new(0);
static WDT_CONFIG: AtomicU32 = AtomicU32::new(0);
/// 1 while `feed_wdt` should reload the watchdog. The probe turns this off
/// after it has shown that a reload keeps the board alive, so the timeout
/// can fire.
static WDT_FEED: AtomicU32 = AtomicU32::new(1);

/// Reads and clears why the chip last reset, and snapshots the hardware
/// watchdog. A lockup (a fault inside the fault handler, as a stack overflow
/// below flip-link's stack gives) goes straight to the bootloader. A watchdog
/// reset is reported in the start record first, then the bootloader is entered
/// from `main`: Meshtastic starts the nRF52 watchdog, which a software reset
/// does not stop, and the first image after it has to either feed the watchdog
/// or be reset by it.
pub fn check_reset_reason() {
    let reason = pac::POWER.resetreas().read().0;
    pac::POWER
        .resetreas()
        .write_value(pac::power::regs::Resetreas(reason));
    RESET_REASON.store(reason, Ordering::Relaxed);
    snapshot_wdt();
    if reason & RESETREAS_LOCKUP != 0 {
        enter_bootloader();
    }
}

pub fn reset_reason() -> u32 {
    RESET_REASON.load(Ordering::Relaxed)
}

/// Whether the last reset was a watchdog timeout (`RESETREAS.DOG`).
pub fn dog_reset() -> bool {
    reset_reason() & RESETREAS_DOG != 0
}

fn snapshot_wdt() {
    let wdt = pac::WDT;
    WDT_RUNSTATUS.store(
        u32::from(wdt.runstatus().read().runstatus()),
        Ordering::Relaxed,
    );
    WDT_CRV.store(wdt.crv().read(), Ordering::Relaxed);
    WDT_RREN.store(wdt.rren().read().0, Ordering::Relaxed);
    WDT_CONFIG.store(wdt.config().read().0, Ordering::Relaxed);
}

pub fn wdt_runstatus() -> bool {
    WDT_RUNSTATUS.load(Ordering::Relaxed) != 0
}

pub fn wdt_crv() -> u32 {
    WDT_CRV.load(Ordering::Relaxed)
}

pub fn wdt_rren() -> u32 {
    WDT_RREN.load(Ordering::Relaxed)
}

pub fn wdt_config() -> u32 {
    WDT_CONFIG.load(Ordering::Relaxed)
}

/// Reloads every enabled RR register when the watchdog was running at boot.
/// Once started, the nRF52 watchdog cannot be stopped or reconfigured except
/// by a watchdog, pin, or power-on reset, so the first image after Meshtastic
/// has to keep feeding it or it fires after `CRV/32768` seconds.
pub fn set_wdt_feed(enable: bool) {
    WDT_FEED.store(u32::from(enable), Ordering::Relaxed);
}

pub fn feed_wdt() {
    if !wdt_runstatus() || WDT_FEED.load(Ordering::Relaxed) == 0 {
        return;
    }
    let rren = WDT_RREN.load(Ordering::Relaxed);
    for n in 0..8 {
        if rren & (1 << n) != 0 {
            pac::WDT
                .rr(n)
                .write_value(pac::wdt::regs::Rr(WDT_RR_RELOAD));
        }
    }
}

/// Resets into the UF2 bootloader, whose drive takes the next image.
pub fn enter_bootloader() -> ! {
    pac::POWER
        .gpregret()
        .write_value(pac::power::regs::Gpregret(DFU_MAGIC_UF2_RESET));
    cortex_m::peripheral::SCB::sys_reset()
}

/// Starts the crystal oscillator USB needs, turns on the flash instruction
/// cache (as Meshtastic and Nordic's SDK do), and starts the cycle counter.
pub fn init() -> embassy_nrf::Peripherals {
    pac::CLOCK.events_hfclkstarted().write_value(0);
    pac::CLOCK.tasks_hfclkstart().write_value(1);
    while pac::CLOCK.events_hfclkstarted().read() == 0 {}
    pac::NVMC.icachecnf().modify(|w| w.set_cacheen(true));

    // SAFETY: called once, before anything else takes core peripherals.
    let mut core = unsafe { cortex_m::Peripherals::steal() };
    core.DCB.enable_trace();
    core.DWT.set_cycle_count(0);
    core.DWT.enable_cycle_counter();

    // SAFETY: `embassy_nrf::init` is never called (see above), so nothing else
    // holds these singletons; only USBD and SWI0 are used.
    unsafe { embassy_nrf::Peripherals::steal() }
}

pub fn icache_enabled() -> bool {
    pac::NVMC.icachecnf().read().cacheen()
}

/// DWT_CYCCNT, which counts every CPU cycle and wraps at 2^32 (67 s).
pub fn cycles() -> u32 {
    cortex_m::peripheral::DWT::cycle_count()
}

/// `getrandom` 0.4's custom backend (its symbol keeps the 0.3 name): the
/// nRF52840's RNG peripheral, with
/// its bias correction on.
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    let start = cycles();
    // SAFETY: getrandom passes a writable buffer of `len` bytes.
    let dest = unsafe { core::slice::from_raw_parts_mut(dest, len) };
    fill_random(dest);
    RNG_BYTES.fetch_add(len as u32, Ordering::Relaxed);
    RNG_CYCLES.fetch_add(cycles().wrapping_sub(start), Ordering::Relaxed);
    Ok(())
}

/// Bytes and cycles spent in getrandom since start-up, for `rng-diag`.
pub static RNG_BYTES: AtomicU32 = AtomicU32::new(0);
pub static RNG_CYCLES: AtomicU32 = AtomicU32::new(0);

#[cfg(not(feature = "seeded-rng"))]
fn fill_random(dest: &mut [u8]) {
    pac::RNG.config().write(|w| w.set_dercen(true));
    pac::RNG.tasks_start().write_value(1);
    for byte in dest {
        while pac::RNG.events_valrdy().read() == 0 {}
        pac::RNG.events_valrdy().write_value(0);
        *byte = pac::RNG.value().read().value();
    }
    pac::RNG.tasks_stop().write_value(1);
}

/// `seeded-rng`: SplitMix64 from a fixed seed. Not random at all, and only
/// for timing what the libraries do with randomness apart from drawing it.
#[cfg(feature = "seeded-rng")]
fn fill_random(dest: &mut [u8]) {
    use core::cell::Cell;
    use cortex_m::interrupt::{Mutex, free};
    static STATE: Mutex<Cell<u64>> = Mutex::new(Cell::new(0x5EED_0F41_DBE4));
    for chunk in dest.chunks_mut(8) {
        let mut z = free(|cs| {
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

/// Gives usage, bus and memory faults their own handlers below the USB
/// interrupts' priority, so a fault in a benchmark can still report itself
/// over USB before entering the bootloader. (A HardFault outranks USB, so it
/// goes to the bootloader silently.)
pub fn enable_fault_reports() {
    use cortex_m::peripheral::scb::SystemHandler;
    // SAFETY: called once at start-up, before any fault can be taken.
    let mut core = unsafe { cortex_m::Peripherals::steal() };
    // Priority 4 of 8 (the top three bits): below USB (2) and its executor (3).
    unsafe {
        core.SCB.set_priority(SystemHandler::UsageFault, 4 << 5);
        core.SCB.set_priority(SystemHandler::BusFault, 4 << 5);
        core.SCB
            .set_priority(SystemHandler::MemoryManagement, 4 << 5);
    }
    // SHCSR: MEMFAULTENA, BUSFAULTENA, USGFAULTENA.
    unsafe { core.SCB.shcsr.modify(|v| v | (0b111 << 16)) };
}

// The three configurable faults share a handler that takes the stacked frame.
// The benchmarks run in thread mode on the main stack, so the frame is at MSP.
core::arch::global_asm!(
    ".section .text.rind_bench_fault, \"ax\"",
    ".global UsageFault",
    ".global BusFault",
    ".global MemoryManagement",
    ".type UsageFault, %function",
    ".type BusFault, %function",
    ".type MemoryManagement, %function",
    ".thumb_func",
    "UsageFault:",
    ".thumb_func",
    "BusFault:",
    ".thumb_func",
    "MemoryManagement:",
    "mrs r0, msp",
    "b {report}",
    report = sym fault_report,
);

unsafe extern "C" fn fault_report(frame: *const u32) -> ! {
    use core::fmt::Write;
    // SAFETY: `frame` is the exception frame the core stacked on entry.
    let (lr, pc) = unsafe { (frame.add(5).read(), frame.add(6).read()) };
    // SAFETY: SCB's fault status and address registers, read-only here.
    let (cfsr, mmfar, bfar) = unsafe {
        (
            (0xE000_ED28 as *const u32).read_volatile(),
            (0xE000_ED34 as *const u32).read_volatile(),
            (0xE000_ED38 as *const u32).read_volatile(),
        )
    };
    let mut msg = crate::heapless_line::Line::new();
    let _ = write!(
        msg,
        "fault: pc={pc:#010x} lr={lr:#010x} cfsr={cfsr:#010x} mmfar={mmfar:#010x} bfar={bfar:#010x}"
    );
    crate::usb::emergency_write(msg.as_str());
    enter_bootloader()
}

/// getrandom 0.2's custom backend, which ring's `SystemRandom` uses: the same RNG.
fn getrandom02(dest: &mut [u8]) -> Result<(), getrandom02::Error> {
    // SAFETY: `dest` is a writable buffer of its own length.
    unsafe { __getrandom_v03_custom(dest.as_mut_ptr(), dest.len()) }
        .map_err(|_| getrandom02::Error::UNSUPPORTED)
}
getrandom02::register_custom_getrandom!(getrandom02);
