// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! USB CDC-ACM for the `RB:` lines. The USB stack runs on an interrupt-priority
//! executor, so it answers the host while a benchmark runs in thread mode, but
//! [`write_and_flush`] returns only once the host has every byte, so no
//! transfer overlaps a timed region. Closing the port after opening it at 1200
//! baud enters the Arduino BOSSA bootloader; doing the
//! same at 300 baud restarts the firmware, which is how a capture starts each
//! run from a fresh boot. The run starts only once the host sends [`GO`]:
//! opening the port is not enough, since ModemManager and the like open new
//! serial ports to probe them and would swallow the first lines.

use core::sync::atomic::{AtomicBool, Ordering};
use embassy_executor::InterruptExecutor;
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::{bind_interrupts, peripherals};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::pipe::Pipe;
use embassy_usb::Builder;
use embassy_usb::class::cdc_acm::{CdcAcmClass, ControlChanged, Receiver, Sender, State};
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
});

type UsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

static EXECUTOR: InterruptExecutor = InterruptExecutor::new();
static OUT: Pipe<CriticalSectionRawMutex, 2048> = Pipe::new();
static CONNECTED: AtomicBool = AtomicBool::new(false);
/// Set while the host holds DTR, that is, has the port open.
static DTR: AtomicBool = AtomicBool::new(false);
/// What the capture host sends to start the run.
pub const GO: &[u8] = b"rind-bench:go\n";
static GO_SEEN: AtomicBool = AtomicBool::new(false);
/// Set while the writer holds bytes it has taken from `OUT` but not sent.
static SENDING: AtomicBool = AtomicBool::new(false);

#[pac_interrupt]
unsafe fn EGU0_SWI0() {
    // SAFETY: the executor's interrupt, as `InterruptExecutor` requires.
    unsafe { EXECUTOR.on_interrupt() }
}

use embassy_nrf::pac::interrupt as pac_interrupt;

pub fn start(p: embassy_nrf::Peripherals) {
    interrupt::EGU0_SWI0.set_priority(Priority::P3);
    interrupt::USBD.set_priority(Priority::P2);
    let spawner = EXECUTOR.start(interrupt::EGU0_SWI0);
    spawner.spawn(usb_main(p.USBD).unwrap());
}

/// Builds the device on the USB executor itself, since its parts are not
/// `Send`, then runs it, the writer, and the 1200-baud watch together.
#[embassy_executor::task]
async fn usb_main(usbd: embassy_nrf::Peri<'static, peripherals::USBD>) {
    static VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
    // USB-powered: VBUS is present whenever this runs.
    let vbus = VBUS.init(SoftwareVbusDetect::new(true, true));
    let driver = Driver::new(usbd, Irqs, &*vbus);

    let mut config = embassy_usb::Config::new(0x1209, 0x0001);
    config.manufacturer = Some("rind-bench");
    config.product = Some("rind-bench Nano 33 BLE");
    config.serial_number = Some("nano33ble");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static STATE: StaticCell<State<'static>> = StaticCell::new();
    let mut builder = Builder::new(
        driver,
        config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        &mut [],
        CONTROL_BUF.init([0; 64]),
    );
    let class = CdcAcmClass::new(&mut builder, STATE.init(State::new()), 64);
    let mut usb = builder.build();
    let (sender, receiver, control) = class.split_with_control();
    embassy_futures::join::join3(usb.run(), writer(sender), touch(receiver, control)).await;
}

async fn writer(mut sender: Sender<'static, UsbDriver>) {
    let mut buf = [0u8; 64];
    loop {
        sender.wait_connection().await;
        CONNECTED.store(true, Ordering::Release);
        loop {
            let n = OUT.read(&mut buf).await;
            SENDING.store(true, Ordering::Release);
            let ok = sender.write_packet(&buf[..n]).await.is_ok();
            // A full packet leaves the transfer open; end it if nothing follows.
            if ok && n == 64 && OUT.is_empty() {
                let _ = sender.write_packet(&[]).await;
            }
            SENDING.store(false, Ordering::Release);
            if !ok {
                CONNECTED.store(false, Ordering::Release);
                break;
            }
        }
    }
}

/// Tracks DTR, and when the host drops it enters the bootloader if the port is
/// at 1200 baud or restarts the firmware if it is at 300 baud. Also watches
/// what the host sends for [`GO`].
async fn touch(mut receiver: Receiver<'static, UsbDriver>, control: ControlChanged<'static>) {
    use embassy_futures::select::{Either, select};
    let mut buf = [0u8; 64];
    let mut matched = 0;
    loop {
        let read = match select(control.control_changed(), receiver.read_packet(&mut buf)).await {
            Either::First(()) => None,
            Either::Second(r) => Some(r),
        };
        match read {
            None => {
                let dtr = control.dtr();
                DTR.store(dtr, Ordering::Release);
                match (receiver.line_coding().data_rate(), dtr) {
                    (1200, false) => crate::board::enter_bootloader(),
                    (300, false) => cortex_m::peripheral::SCB::sys_reset(),
                    _ => {}
                }
            }
            Some(Ok(n)) => {
                for &b in &buf[..n] {
                    matched = if b == GO[matched] {
                        matched + 1
                    } else {
                        usize::from(b == GO[0])
                    };
                    if matched == GO.len() {
                        GO_SEEN.store(true, Ordering::Release);
                        matched = 0;
                    }
                }
            }
            Some(Err(_)) => receiver.wait_connection().await,
        }
    }
}

fn host_ready() -> bool {
    CONNECTED.load(Ordering::Acquire)
        && DTR.load(Ordering::Acquire)
        && GO_SEEN.load(Ordering::Acquire)
}

/// Blocks until a host has the port open and has sent [`GO`].
pub fn wait_for_host() {
    while !host_ready() {
        crate::board::feed_wdt();
        if crate::board::wdt_runstatus() {
            // Poll rather than `wfi`: if the host is late with GO, a sleep
            // might outlast the watchdog, which a software reset does not stop.
            let start = crate::board::cycles();
            while crate::board::cycles().wrapping_sub(start) < crate::board::CPU_HZ as u32 / 10
                && !host_ready()
            {}
        } else {
            cortex_m::asm::wfi();
        }
    }
}

/// Queues `s` and returns once the host has taken all of it.
pub fn write_and_flush(s: &str) {
    crate::board::feed_wdt();
    let mut bytes = s.as_bytes();
    while !bytes.is_empty() {
        match OUT.try_write(bytes) {
            Ok(n) => bytes = &bytes[n..],
            Err(_) => cortex_m::asm::wfi(),
        }
    }
    while !OUT.is_empty() || SENDING.load(Ordering::Acquire) {
        cortex_m::asm::wfi();
    }
}

/// For the panic handler: queue what fits and give USB a moment to send it.
pub fn emergency_write(msg: &str) {
    let _ = OUT.try_write(b"RB:{\"type\":\"panic\",\"message\":\"");
    let _ = OUT.try_write(msg.as_bytes());
    let _ = OUT.try_write(b"\"}\n");
    let start = crate::board::cycles();
    while !OUT.is_empty() && crate::board::cycles().wrapping_sub(start) < 64_000_000 {}
    let start = crate::board::cycles();
    while crate::board::cycles().wrapping_sub(start) < 32_000_000 {}
}
