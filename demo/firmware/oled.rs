// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! The demo display: an SSD1306 128x64 on I2C0 (SDA GPIO5, SCL GPIO6,
//! address 0x3C), the panel on Seeed's XIAO expansion board.
//!
//! It draws only from criterion-esp's observer, which runs between
//! benchmarks: the progress screen goes out over I2C before a benchmark's
//! setup and warm-up begin, and nothing is sent until its last sample is
//! taken. The panel refreshes itself from its own RAM, so a shown frame
//! costs the CPU nothing. Published results come from images built without
//! the `oled` feature; see demo/README.md.

use std::fmt::Write;
use std::sync::Mutex;

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::units::Hertz;
use rind_bench_oled_ui as ui;
use ssd1306::mode::BufferedGraphicsMode;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

type Display = Ssd1306<
    I2CInterface<I2cDriver<'static>>,
    DisplaySize128x64,
    BufferedGraphicsMode<DisplaySize128x64>,
>;

struct State {
    display: Display,
    file: &'static str,
    total: usize,
    done: usize,
    /// Each result: its ID and median ns.
    results: Vec<(String, f64)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn draw(f: impl FnOnce(&mut Display)) {
    if let Some(s) = STATE.lock().unwrap().as_mut() {
        s.display.clear_buffer();
        f(&mut s.display);
        let _ = s.display.flush();
    }
}

/// Opens the display and shows the splash screen. Without a display the
/// benchmarks still run; every later call does nothing.
pub fn init() {
    let Ok(p) = Peripherals::take() else { return };
    let config = I2cConfig::new().baudrate(Hertz(400_000));
    let Ok(i2c) = I2cDriver::new(p.i2c0, p.pins.gpio5, p.pins.gpio6, &config) else {
        return;
    };
    let mut display = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate0,
    )
    .into_buffered_graphics_mode();
    let ok = display.init().is_ok();
    criterion::emit(&format!("{{\"type\":\"oled\",\"ok\":{ok}}}"));
    if !ok {
        return;
    }
    *STATE.lock().unwrap() = Some(State {
        display,
        file: "",
        total: 0,
        done: 0,
        results: Vec::new(),
    });
    draw(|d| ui::splash(d, "240 MHz - portable rind"));
    FreeRtos::delay_ms(2000);
}

/// Counts a bench file's benchmarks with a dry run, for the progress bar.
pub fn begin(file: &'static str, benches: fn()) {
    let total = {
        static COUNT: Mutex<usize> = Mutex::new(0);
        *COUNT.lock().unwrap() = 0;
        criterion::set_observer(|e| {
            if let criterion::Event::Listed(_) = e {
                *COUNT.lock().unwrap() += 1;
            }
        });
        criterion::set_dry_run(true);
        benches();
        criterion::set_dry_run(false);
        *COUNT.lock().unwrap()
    };
    if let Some(s) = STATE.lock().unwrap().as_mut() {
        s.file = file;
        s.total = total;
        s.done = 0;
    }
    criterion::set_observer(observe);
}

fn observe(event: criterion::Event<'_>) {
    match event {
        criterion::Event::Listed(_) => {}
        criterion::Event::Start(id) => {
            let (file, done, total) = match STATE.lock().unwrap().as_ref() {
                Some(s) => (s.file, s.done, s.total.max(s.done + 1)),
                None => return,
            };
            draw(|d| ui::progress(d, file, id, done, total));
        }
        criterion::Event::Result(id, ns) => {
            if let Some(s) = STATE.lock().unwrap().as_mut() {
                s.done += 1;
                s.results.push((id.to_string(), ns));
            }
        }
    }
}

/// The library an ID names, and the ID without it: the page it belongs on.
fn split(id: &str) -> Option<(&str, String)> {
    const LIBRARIES: &[&str] = &[
        "rind",
        "ring",
        "rustcrypto",
        "dalek",
        "p256-rustcrypto",
        "libcrux",
        "libcrux-ml-kem",
        "esp32s3-hw",
    ];
    let segments: Vec<&str> = id.split('/').collect();
    let i = segments.iter().position(|s| LIBRARIES.contains(s))?;
    let mut rest = segments.clone();
    rest.remove(i);
    Some((segments[i], rest.join(" ")))
}

/// This run's results as the demo pages' table, one page per operation
/// where rind and at least one other library ran.
fn table(results: &[(String, f64)]) -> String {
    let mut pages: Vec<(String, Vec<(&str, f64)>)> = Vec::new();
    for (id, ns) in results {
        let Some((lib, page)) = split(id) else {
            continue;
        };
        match pages.iter_mut().find(|(p, _)| *p == page) {
            Some((_, bars)) => bars.push((lib, *ns)),
            None => pages.push((page, vec![(lib, *ns)])),
        }
    }
    let mut tsv = String::new();
    for (page, bars) in pages
        .iter()
        .filter(|(_, b)| b.len() > 1 && b.iter().any(|(l, _)| *l == "rind"))
    {
        for (lib, ns) in bars {
            let _ = writeln!(tsv, "{page}\t{lib}\t{ns}");
        }
    }
    tsv
}

/// Cycles a results table forever: each operation's page, then the summary.
pub fn cycle(tsv: &str, note: &str, seconds: u32) -> ! {
    let pages: Vec<ui::Page<'_>> = ui::pages(tsv).collect();
    let (wins, n, geomean) = ui::tally(tsv);
    loop {
        for (i, p) in pages.iter().enumerate() {
            let label = format!("{}/{}", i + 1, pages.len());
            draw(|d| ui::operation(d, p.heading, &label, p.bars()));
            FreeRtos::delay_ms(seconds * 1000);
        }
        draw(|d| ui::summary(d, wins, n, geomean, note));
        FreeRtos::delay_ms(seconds * 1000);
    }
}

/// After the run: this image's results, cycled for the demo.
pub fn finish() -> ! {
    let results = STATE
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.results.clone())
        .unwrap_or_default();
    let tsv = table(&results);
    if tsv.is_empty() {
        loop {
            FreeRtos::delay_ms(1000);
        }
    }
    cycle(&tsv, "this run, median, CCOUNT", 4)
}
