// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! rind-bench's demo screens for a 128x64 monochrome OLED, drawn with
//! embedded-graphics onto any `DrawTarget<Color = BinaryColor>`: the
//! SSD1306 on the board, or [`Frame`] for rendering on a host.
//!
//! None of this is part of the benchmark port. The firmware draws only
//! between benchmarks, never inside a timed region; see the README.

#![no_std]

use core::fmt::Write;

use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::{FONT_4X6, FONT_5X8, FONT_6X10, FONT_9X15_BOLD};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle, RoundedRectangle};
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};

pub const WIDTH: u32 = 128;
pub const HEIGHT: u32 = 64;

/// A 128x64 1-bit frame buffer, row-major, one byte per pixel, for rendering
/// screens on a host.
pub struct Frame(pub [[bool; WIDTH as usize]; HEIGHT as usize]);

impl Default for Frame {
    fn default() -> Self {
        Self([[false; WIDTH as usize]; HEIGHT as usize])
    }
}

impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        Size::new(WIDTH, HEIGHT)
    }
}

impl DrawTarget for Frame {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<BinaryColor>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Self::Error> {
        for Pixel(p, c) in pixels {
            if (0..WIDTH as i32).contains(&p.x) && (0..HEIGHT as i32).contains(&p.y) {
                self.0[p.y as usize][p.x as usize] = c.is_on();
            }
        }
        Ok(())
    }
}

/// A short, fixed-size string, for labels drawn without allocating.
pub struct Label<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> Default for Label<N> {
    fn default() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }
}

impl<const N: usize> Label<N> {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for Label<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len == N {
                return Ok(());
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
        Ok(())
    }
}

/// A time per operation, with three significant figures: `912ns`, `1.71ms`.
pub fn time(ns: f64) -> Label<8> {
    let mut l = Label::default();
    let (v, unit) = if ns < 1e3 {
        (ns, "ns")
    } else if ns < 1e6 {
        (ns / 1e3, "us")
    } else if ns < 1e9 {
        (ns / 1e6, "ms")
    } else {
        (ns / 1e9, "s")
    };
    let _ = if v < 10.0 {
        write!(l, "{v:.2}{unit}")
    } else if v < 100.0 {
        write!(l, "{v:.1}{unit}")
    } else {
        write!(l, "{v:.0}{unit}")
    };
    l
}

fn on() -> PrimitiveStyle<BinaryColor> {
    PrimitiveStyle::with_fill(BinaryColor::On)
}

fn outline() -> PrimitiveStyle<BinaryColor> {
    PrimitiveStyle::with_stroke(BinaryColor::On, 1)
}

fn text<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    s: &str,
    at: Point,
    font: &'static embedded_graphics::mono_font::MonoFont<'static>,
    color: BinaryColor,
    align: Alignment,
) {
    let style = MonoTextStyle::new(font, color);
    let ts = TextStyleBuilder::new()
        .alignment(align)
        .baseline(Baseline::Top)
        .build();
    let _ = Text::with_text_style(s, at, style, ts).draw(d);
}

/// An inverted title bar across the top 9 rows.
fn title<D: DrawTarget<Color = BinaryColor>>(d: &mut D, s: &str, right: &str) {
    let _ = Rectangle::new(Point::zero(), Size::new(WIDTH, 9))
        .into_styled(on())
        .draw(d);
    text(
        d,
        s,
        Point::new(2, 1),
        &FONT_5X8,
        BinaryColor::Off,
        Alignment::Left,
    );
    if !right.is_empty() {
        text(
            d,
            right,
            Point::new(126, 1),
            &FONT_5X8,
            BinaryColor::Off,
            Alignment::Right,
        );
    }
}

/// The boot screen: the name, what it measures, and the board.
pub fn splash<D: DrawTarget<Color = BinaryColor>>(d: &mut D, subtitle: &str) {
    let _ = d.clear(BinaryColor::Off);
    let _ = RoundedRectangle::with_equal_corners(
        Rectangle::new(Point::new(0, 0), Size::new(WIDTH, HEIGHT)),
        Size::new(6, 6),
    )
    .into_styled(outline())
    .draw(d);
    text(
        d,
        "rind-bench",
        Point::new(64, 6),
        &FONT_9X15_BOLD,
        BinaryColor::On,
        Alignment::Center,
    );
    text(
        d,
        "pure-Rust crypto",
        Point::new(64, 24),
        &FONT_6X10,
        BinaryColor::On,
        Alignment::Center,
    );
    text(
        d,
        "on an ESP32-S3",
        Point::new(64, 35),
        &FONT_6X10,
        BinaryColor::On,
        Alignment::Center,
    );
    text(
        d,
        subtitle,
        Point::new(64, 52),
        &FONT_4X6,
        BinaryColor::On,
        Alignment::Center,
    );
}

/// The live screen during a run: the bench file, the benchmark about to be
/// timed, and a bar of how many of the image's benchmarks are done.
pub fn progress<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    file: &str,
    id: &str,
    done: usize,
    total: usize,
) {
    let _ = d.clear(BinaryColor::Off);
    let mut count: Label<12> = Label::default();
    let _ = write!(count, "{done}/{total}");
    title(d, "TIMING", count.as_str());
    text(
        d,
        file,
        Point::new(2, 12),
        &FONT_6X10,
        BinaryColor::On,
        Alignment::Left,
    );
    // The ID, split at its first two slashes: operation, library, size.
    let mut parts = id.splitn(3, '/');
    let op = parts.next().unwrap_or("");
    let lib = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    text(
        d,
        op,
        Point::new(2, 24),
        &FONT_5X8,
        BinaryColor::On,
        Alignment::Left,
    );
    text(
        d,
        lib,
        Point::new(2, 33),
        &FONT_6X10,
        BinaryColor::On,
        Alignment::Left,
    );
    text(
        d,
        rest,
        Point::new(126, 33),
        &FONT_5X8,
        BinaryColor::On,
        Alignment::Right,
    );
    let _ = Rectangle::new(Point::new(2, 50), Size::new(124, 9))
        .into_styled(outline())
        .draw(d);
    let fill = (120 * done).checked_div(total).unwrap_or(0) as u32;
    if fill > 0 {
        let _ = Rectangle::new(Point::new(4, 52), Size::new(fill, 5))
            .into_styled(on())
            .draw(d);
    }
}

/// One library's time on an operation, for a bar chart.
#[derive(Clone, Copy)]
pub struct Bar<'a> {
    pub library: &'a str,
    pub ns: f64,
}

/// The four-letter tags the bars carry.
fn tag(library: &str) -> &str {
    match library {
        "rind" => "rind",
        "ring" => "ring",
        "rustcrypto" | "dalek" => "RC",
        "libcrux" | "libcrux-ml-kem" => "crux",
        "esp32s3-hw" => "HW",
        "aws-lc-rs" => "aws",
        other => other,
    }
}

/// One operation's result page: a bar per library, longest for the slowest,
/// rind's filled and the others outlined, and a footer comparing rind with
/// the fastest other software library (the hardware row is shown, never
/// compared against).
pub fn operation<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    heading: &str,
    page: &str,
    bars: &[Bar<'_>],
) {
    let _ = d.clear(BinaryColor::Off);
    title(d, heading, page);
    let slowest = bars.iter().map(|b| b.ns).fold(0.0, f64::max);
    for (i, b) in bars.iter().take(5).enumerate() {
        let y = 11 + 9 * i as i32;
        text(
            d,
            tag(b.library),
            Point::new(1, y + 1),
            &FONT_5X8,
            BinaryColor::On,
            Alignment::Left,
        );
        let width = if slowest > 0.0 {
            ((b.ns / slowest) * 70.0).max(1.0) as u32
        } else {
            1
        };
        let rect = Rectangle::new(Point::new(24, y + 1), Size::new(width, 7));
        let style = if b.library == "rind" { on() } else { outline() };
        let _ = rect.into_styled(style).draw(d);
        text(
            d,
            time(b.ns).as_str(),
            Point::new(127, y + 1),
            &FONT_5X8,
            BinaryColor::On,
            Alignment::Right,
        );
    }
    let rind = bars.iter().find(|b| b.library == "rind");
    let best = bars
        .iter()
        .filter(|b| b.library != "rind" && b.library != "esp32s3-hw")
        .min_by(|a, b| a.ns.total_cmp(&b.ns));
    let mut foot: Label<40> = Label::default();
    match (rind, best) {
        (Some(r), Some(o)) if r.ns <= o.ns => {
            let _ = write!(
                foot,
                "rind {:.2}x faster than {}",
                o.ns / r.ns,
                tag(o.library)
            );
        }
        (Some(r), Some(o)) => {
            let _ = write!(
                foot,
                "{} {:.2}x faster than rind",
                tag(o.library),
                r.ns / o.ns
            );
        }
        _ => {}
    }
    text(
        d,
        foot.as_str(),
        Point::new(64, 57),
        &FONT_4X6,
        BinaryColor::On,
        Alignment::Center,
    );
}

/// The summary page: on how many operations rind is fastest of the Rust
/// libraries, and the geometric mean of its time over the fastest other's.
pub fn summary<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    wins: usize,
    total: usize,
    geomean: f64,
    note: &str,
) {
    let _ = d.clear(BinaryColor::Off);
    title(d, "SUMMARY", "");
    let mut big: Label<16> = Label::default();
    let _ = write!(big, "{wins}/{total}");
    text(
        d,
        big.as_str(),
        Point::new(64, 12),
        &FONT_9X15_BOLD,
        BinaryColor::On,
        Alignment::Center,
    );
    text(
        d,
        "ops where rind is fastest",
        Point::new(64, 28),
        &FONT_4X6,
        BinaryColor::On,
        Alignment::Center,
    );
    let mut g: Label<32> = Label::default();
    if geomean <= 1.0 {
        let _ = write!(g, "{:.2}x faster (geomean)", 1.0 / geomean);
    } else {
        let _ = write!(g, "{:.2}x slower (geomean)", geomean);
    }
    text(
        d,
        g.as_str(),
        Point::new(64, 37),
        &FONT_5X8,
        BinaryColor::On,
        Alignment::Center,
    );
    text(
        d,
        "vs the fastest other Rust lib",
        Point::new(64, 46),
        &FONT_4X6,
        BinaryColor::On,
        Alignment::Center,
    );
    text(
        d,
        note,
        Point::new(64, 56),
        &FONT_4X6,
        BinaryColor::On,
        Alignment::Center,
    );
}

/// One result page's data: a heading and up to six bars.
#[derive(Clone, Copy)]
pub struct Page<'a> {
    pub heading: &'a str,
    pub bars: [Bar<'a>; 6],
    pub len: usize,
}

impl<'a> Page<'a> {
    pub fn bars(&self) -> &[Bar<'a>] {
        &self.bars[..self.len]
    }

    /// rind's time over the fastest other software library's, if both ran.
    pub fn ratio(&self) -> Option<f64> {
        let rind = self.bars().iter().find(|b| b.library == "rind")?.ns;
        let best = self
            .bars()
            .iter()
            .filter(|b| b.library != "rind" && b.library != "esp32s3-hw")
            .map(|b| b.ns)
            .reduce(f64::min)?;
        Some(rind / best)
    }
}

/// The pages in a results table: one `heading<TAB>library<TAB>ns` line per
/// bar, with a page's lines adjacent. `#` starts a comment line.
pub fn pages(tsv: &str) -> impl Iterator<Item = Page<'_>> {
    let mut lines = tsv
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .peekable();
    core::iter::from_fn(move || {
        let first = lines.peek()?;
        let heading = first.split('\t').next()?;
        let mut page = Page {
            heading,
            bars: [Bar {
                library: "",
                ns: 0.0,
            }; 6],
            len: 0,
        };
        while let Some(l) = lines.peek() {
            let mut f = l.split('\t');
            if f.next() != Some(heading) {
                break;
            }
            let library = f.next().unwrap_or("");
            let ns = f.next().and_then(|n| n.trim().parse().ok()).unwrap_or(0.0);
            if page.len < page.bars.len() {
                page.bars[page.len] = Bar { library, ns };
                page.len += 1;
            }
            lines.next();
        }
        Some(page)
    })
}

/// The summary over a results table: pages where rind is fastest, pages
/// compared, and the geometric mean of [`Page::ratio`].
pub fn tally(tsv: &str) -> (usize, usize, f64) {
    let (mut wins, mut n, mut log) = (0, 0, 0.0);
    for r in pages(tsv).filter_map(|p| p.ratio()) {
        n += 1;
        wins += usize::from(r <= 1.0);
        log += libm_ln(r);
    }
    let geomean = if n == 0 {
        1.0
    } else {
        libm_exp(log / n as f64)
    };
    (wins, n, geomean)
}

// `f64::ln` and `exp` need std; these series are exact enough for a label.
fn libm_ln(x: f64) -> f64 {
    // ln(x) = 2 atanh((x-1)/(x+1)), after scaling x into [0.5, 2).
    let (mut x, mut k) = (x, 0.0);
    while x > 2.0 {
        x /= 2.0;
        k += 1.0;
    }
    while x < 0.5 {
        x *= 2.0;
        k -= 1.0;
    }
    let y = (x - 1.0) / (x + 1.0);
    let (mut term, mut sum) = (y, 0.0);
    for i in 0..40 {
        sum += term / (2 * i + 1) as f64;
        term *= y * y;
    }
    2.0 * sum + k * core::f64::consts::LN_2
}

fn libm_exp(x: f64) -> f64 {
    let (mut term, mut sum) = (1.0, 1.0);
    for i in 1..60 {
        term *= x / i as f64;
        sum += term;
    }
    sum
}
