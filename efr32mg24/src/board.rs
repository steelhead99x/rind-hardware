// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! The EFR32MG24 underneath the benchmarks: clocks, the cycle counter, and
//! a seeded `getrandom`.
//!
//! At reset the core runs from FSRCO (20 MHz). `init` starts the 39 MHz HFXO
//! with its reset configuration, loads the HFRCODPLL calibration for its
//! 80 MHz band from DEVINFO, and locks HFRCODPLL to HFXO x 2000/1000 = 78 MHz
//! with DPLL0 in frequency-lock mode, Silicon Labs' 78 MHz setting
//! (`CMU_DPLL_HFXO_TO_78MHZ`). Flash gets one wait state (up to 80 MHz) and
//! PCLK is HCLK/2 (at most 50 MHz). Register addresses and fields are the
//! EFR32MG24 reference manual's, checked against Silicon Labs' SVD.
//!
//! Timing uses the Cortex-M33's DWT cycle counter. The CMU's calibration
//! counter is the independent check: it counts HFXO edges during a fixed
//! number of HCLK cycles, and `clock_check` reports both, with the DWT count
//! over the same window.
//!
//! The EFR32MG24's TRNG sits behind the Secure Engine's mailbox. `getrandom`
//! is instead served from the seeded SplitMix64 the RP2040 and the nRF52840
//! `seeded-rng` images use, so libraries that draw randomness do the same
//! work. It is not a source of entropy.

use core::sync::atomic::{AtomicU32, Ordering};

/// The DPLL's target: HFXO (39 MHz) x 2000 / 1000.
pub const CPU_HZ: u64 = 78_000_000;
const HFXO_HZ: u64 = 39_000_000;

const CMU: usize = 0x4000_8000;
const CMU_STATUS: usize = CMU + 0x08;
const CMU_IF: usize = CMU + 0x20;
const CMU_CALCMD: usize = CMU + 0x50;
const CMU_CALCTRL: usize = CMU + 0x54;
const CMU_CALCNT: usize = CMU + 0x58;
const CMU_CLKEN0: usize = CMU + 0x64;
const CMU_CLKEN1: usize = CMU + 0x68;
const CMU_SYSCLKCTRL: usize = CMU + 0x70;
const CMU_DPLLREFCLKCTRL: usize = CMU + 0x100;

const HFXO0: usize = 0x4A00_4000;
const HFXO0_CTRL: usize = HFXO0 + 0x28;
const HFXO0_STATUS: usize = HFXO0 + 0x58;

const HFRCO0: usize = 0x4001_0000;
const HFRCO0_CAL: usize = HFRCO0 + 0x08;
const HFRCO0_STATUS: usize = HFRCO0 + 0x0C;

const DPLL0: usize = 0x4001_C000;
const DPLL0_EN: usize = DPLL0 + 0x04;
const DPLL0_CFG: usize = DPLL0 + 0x08;
const DPLL0_CFG1: usize = DPLL0 + 0x0C;
const DPLL0_IF: usize = DPLL0 + 0x10;
const DPLL0_STATUS: usize = DPLL0 + 0x18;
/// Series 2 registers' clear alias: a write of 1 clears those bits. A plain
/// write to an IF register does not clear its flags.
const CLR: usize = 0x2000;

const MSC_READCTRL: usize = 0x4003_0000 + 0x04;
const ICACHE0_CTRL: usize = 0x4003_4000 + 0x04;
/// DEVINFO.HFRCODPLLCAL[16], the 80 MHz band (emlib's table index).
const DEVINFO_HFRCODPLLCAL_80M: usize = 0x0FE0_8000 + 0x58 + 16 * 4;

// CMU calibration clock selects.
const CAL_UP_HFXO: u32 = 2;
const CAL_UP_HFRCODPLL: u32 = 4;
const CAL_DOWN_HCLK: u32 = 1;
const CAL_DOWN_HFXO: u32 = 3;
const CAL_DOWN_FSRCO: u32 = 9;
/// Down-counter cycles per measurement. The up count must fit the 20-bit
/// CALCNT: at most 2x this for the clocks compared here.
const CAL_TOP: u32 = 400_000;

fn rd(a: usize) -> u32 {
    // SAFETY: every address here is a documented EFR32MG24 register.
    unsafe { (a as *const u32).read_volatile() }
}
fn wr(a: usize, v: u32) {
    // SAFETY: as above.
    unsafe { (a as *mut u32).write_volatile(v) }
}

/// What `init` saw and did, for the start record.
pub struct ClockReport {
    pub sysclkctrl_at_reset: u32,
    pub hfxo_ready: bool,
    pub dpll_if: u32,
    pub hfrco_cal: u32,
    pub sysclkctrl: u32,
    pub readctrl: u32,
}

static LOCKED: AtomicU32 = AtomicU32::new(0);

/// Waits for `done` for up to `limit` DWT cycles; false on timeout.
fn wait(limit: u32, done: impl Fn() -> bool) -> bool {
    let start = cycles();
    while !done() {
        if cycles().wrapping_sub(start) > limit {
            return false;
        }
    }
    true
}

/// Starts the cycle counter and brings the core to 78 MHz (see the module
/// docs). On any timeout the core stays on FSRCO and `locked` is false.
pub fn init() -> ClockReport {
    // SAFETY: called once, before anything else takes core peripherals.
    let mut core = unsafe { cortex_m::Peripherals::steal() };
    core.DCB.enable_trace();
    core.DWT.set_cycle_count(0);
    core.DWT.enable_cycle_counter();

    let sysclkctrl_at_reset = rd(CMU_SYSCLKCTRL);
    // Bus clocks for DPLL0, HFRCO0 and HFXO0; MSC and ICACHE0.
    wr(
        CMU_CLKEN0,
        rd(CMU_CLKEN0) | (1 << 17) | (1 << 18) | (1 << 20),
    );
    wr(CMU_CLKEN1, rd(CMU_CLKEN1) | (1 << 15) | (1 << 16));

    // A debugger's reset leaves the CMU as the last image set it: start from
    // FSRCO with DPLL0 off, as after a power-on reset.
    if sysclkctrl_at_reset & 0x7 != 1 {
        wr(CMU_SYSCLKCTRL, (rd(CMU_SYSCLKCTRL) & !0x7) | 1);
    }
    wr(DPLL0_EN, 0);
    wait(2_000_000, || {
        rd(DPLL0_STATUS) & 0b10 == 0 && rd(DPLL0_EN) & 0b10 == 0
    });

    // HFXO with its reset configuration, forced on. 20 MHz x 0.1 s.
    wr(HFXO0_CTRL, rd(HFXO0_CTRL) | (1 << 16));
    let hfxo_ready = wait(2_000_000, || rd(HFXO0_STATUS) & 1 != 0);
    let mut report = ClockReport {
        sysclkctrl_at_reset,
        hfxo_ready,
        dpll_if: 0,
        hfrco_cal: rd(HFRCO0_CAL),
        sysclkctrl: rd(CMU_SYSCLKCTRL),
        readctrl: rd(MSC_READCTRL),
    };
    if !hfxo_ready {
        return report;
    }

    // Flash wait states before any faster clock: WS1 (bits 21:20).
    wr(MSC_READCTRL, (rd(MSC_READCTRL) & !(3 << 20)) | (1 << 20));

    // HFRCODPLL into its 80 MHz band; it is not the system clock yet.
    wr(HFRCO0_CAL, rd(DEVINFO_HFRCODPLLCAL_80M));
    wait(200_000, || rd(HFRCO0_STATUS) & 0b110 == 0);

    // DPLL0: reference HFXO, FLL mode, falling edge, auto-recover,
    // N = 1999, M = 999.
    wr(CMU_DPLLREFCLKCTRL, 1);
    wr(DPLL0_CFG, 1 << 2);
    wr(DPLL0_CFG1, (1999 << 16) | 999);
    wr(DPLL0_IF + CLR, 0b111);
    wr(DPLL0_EN, 1);
    wait(2_000_000, || rd(DPLL0_IF) & 0b111 != 0);
    report.dpll_if = rd(DPLL0_IF);
    report.hfrco_cal = rd(HFRCO0_CAL);
    if report.dpll_if & 0b111 != 1 {
        return report;
    }

    // SYSCLK = HFRCODPLL, HCLK = SYSCLK, PCLK = HCLK / 2.
    let v = rd(CMU_SYSCLKCTRL) & !(0x7 | (1 << 10) | (0xF << 12));
    wr(CMU_SYSCLKCTRL, v | 2 | (1 << 10));
    report.sysclkctrl = rd(CMU_SYSCLKCTRL);
    report.readctrl = rd(MSC_READCTRL);
    LOCKED.store(1, Ordering::Relaxed);
    report
}

/// True once the core runs at `CPU_HZ` from the locked DPLL.
pub fn locked() -> bool {
    LOCKED.load(Ordering::Relaxed) == 1
}

/// The CMU's calibration counter: `up` clock edges while `down` counts
/// `CAL_TOP` cycles down; with the DWT cycles over the same window.
fn calibrate(up: u32, down: u32) -> (u32, u32) {
    // CALCTRL takes a write only while no calibration runs (STATUS.CALRDY).
    while rd(CMU_STATUS) & 1 == 0 {}
    wr(CMU_CALCTRL, (down << 28) | (up << 24) | CAL_TOP);
    wr(CMU_IF + CLR, 0b11);
    let c0 = cycles();
    wr(CMU_CALCMD, 1);
    while rd(CMU_IF) & 1 == 0 {}
    let dwt = cycles().wrapping_sub(c0);
    let count = rd(CMU_CALCNT);
    while rd(CMU_STATUS) & 1 == 0 {}
    (count, dwt)
}

/// The clock cross-check for the start record: HCLK measured against HFXO
/// (in Hz, taking HFXO as 39 MHz), the DWT cycles over that window against
/// its HCLK cycles, and HFXO against FSRCO (x1000) as a check on HFXO itself.
pub struct ClockCheck {
    pub hclk_hz: u64,
    pub window_hclk: u32,
    pub window_dwt: u32,
    pub hfxo_per_fsrco_milli: u64,
    pub hfrcodpll_hz: u64,
}

pub fn clock_check() -> ClockCheck {
    let (hfxo_edges, dwt) = calibrate(CAL_UP_HFXO, CAL_DOWN_HCLK);
    let (hfxo_vs_fsrco, _) = calibrate(CAL_UP_HFXO, CAL_DOWN_FSRCO);
    let (dpll_vs_hfxo, _) = calibrate(CAL_UP_HFRCODPLL, CAL_DOWN_HFXO);
    let top = CAL_TOP as u64 + 1;
    ClockCheck {
        hclk_hz: HFXO_HZ * top / (hfxo_edges.max(1) as u64),
        window_hclk: CAL_TOP + 1,
        window_dwt: dwt,
        hfxo_per_fsrco_milli: hfxo_vs_fsrco as u64 * 1000 / top,
        hfrcodpll_hz: HFXO_HZ * dpll_vs_hfxo as u64 / top,
    }
}

/// ICACHE0 is on unless CTRL.CACHEDIS is set (it is clear at reset).
pub fn icache_enabled() -> bool {
    rd(ICACHE0_CTRL) & 1 == 0
}

/// DWT_CYCCNT, which counts every CPU cycle and wraps at 2^32 (55 s at 78 MHz).
pub fn cycles() -> u32 {
    cortex_m::peripheral::DWT::cycle_count()
}

/// Nothing to return to: the SWD probe takes the next image. Spin with
/// interrupts off so the host sees the last line and stops.
pub fn halt() -> ! {
    cortex_m::interrupt::disable();
    loop {
        cortex_m::asm::nop();
    }
}

/// `getrandom` 0.4's custom backend (its symbol keeps the 0.3 name): a
/// seeded SplitMix64. Not random. The seed matches the RP2040 image and the
/// nRF52840 `seeded-rng` image.
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

/// getrandom 0.2's custom backend, which ring's `SystemRandom` uses: the same generator.
fn getrandom02(dest: &mut [u8]) -> Result<(), getrandom02::Error> {
    fill_seeded(dest);
    Ok(())
}
getrandom02::register_custom_getrandom!(getrandom02);

#[cortex_m_rt::exception]
unsafe fn HardFault(frame: &cortex_m_rt::ExceptionFrame) -> ! {
    use core::fmt::Write;
    // SAFETY: SCB's fault status and address registers, read-only here.
    let (cfsr, hfsr, bfar) = unsafe {
        (
            (0xE000_ED28 as *const u32).read_volatile(),
            (0xE000_ED2C as *const u32).read_volatile(),
            (0xE000_ED38 as *const u32).read_volatile(),
        )
    };
    let mut msg = crate::heapless_line::Line::new();
    let _ = write!(
        msg,
        "fault: pc={:#010x} lr={:#010x} cfsr={cfsr:#010x} hfsr={hfsr:#010x} bfar={bfar:#010x}",
        frame.pc(),
        frame.lr()
    );
    crate::rtt::emergency_write(msg.as_str());
    halt()
}
