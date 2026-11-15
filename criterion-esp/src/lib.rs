// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! The subset of [criterion]'s API that `rind-bench/benches` calls, for a
//! target that criterion cannot run on: one with no file system, no
//! command line, and no wall clock worth the name, such as an ESP32.
//!
//! Each `bench_function` warms its routine up, then times
//! [`Config::samples`] samples of a fixed number of iterations each with the
//! cycle counter that [`set_clock`] installs, and prints one JSON line,
//! prefixed `RB:`, holding the criterion ID and the per-iteration cycle
//! counts' median, spread, and nanoseconds at [`Config::cpu_hz`]. A host
//! script turns those lines into `admin/bench-report`'s results.
//!
//! It times a flat number of iterations per sample, not criterion's linear
//! ramp, and reports the median rather than criterion's slope estimate: on a
//! core with no other work, frequency scaling, or turbo, the two agree, and
//! the median ignores the samples an interrupt lands in.
//!
//! Without the default `std` feature it is `no_std` (with `alloc`) for bare
//! metal: lines go to the function [`set_output`] installs, and its state is
//! kept for one execution context, as benchmarks run in.
//!
//! [criterion]: https://docs.rs/criterion

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Display;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "std")]
use std::sync::Mutex;

/// A lock for one execution context: bare-metal benchmarks run from one
/// thread with nothing else calling in, so a `RefCell` serves.
#[cfg(not(feature = "std"))]
struct Mutex<T>(core::cell::RefCell<T>);

// SAFETY: without std this crate is used from one execution context only
// (see the crate docs), so the cell is never shared between threads.
#[cfg(not(feature = "std"))]
unsafe impl<T> Sync for Mutex<T> {}

#[cfg(not(feature = "std"))]
impl<T> Mutex<T> {
    const fn new(value: T) -> Self {
        Self(core::cell::RefCell::new(value))
    }

    fn lock(&self) -> Result<core::cell::RefMut<'_, T>, core::convert::Infallible> {
        Ok(self.0.borrow_mut())
    }
}

pub use core::hint::black_box;

/// Criterion's measurement types; only wall time, as cycles, is offered.
pub mod measurement {
    /// Time, here counted in CPU cycles.
    pub struct WallTime;
}

/// How much work one iteration does, as criterion records it.
#[derive(Clone, Copy, Debug)]
pub enum Throughput {
    Bytes(u64),
    BytesDecimal(u64),
    Elements(u64),
}

/// How `iter_batched` groups its inputs. Every size times the same way here:
/// a sample's inputs are made before its clock starts.
#[derive(Clone, Copy, Debug)]
pub enum BatchSize {
    SmallInput,
    LargeInput,
    PerIteration,
    NumBatches(u64),
    NumIterations(u64),
}

/// A benchmark's name within its group: a function and an optional parameter.
#[derive(Clone, Debug)]
pub struct BenchmarkId {
    function: Option<String>,
    parameter: Option<String>,
}

impl BenchmarkId {
    pub fn new<S: Into<String>, P: Display>(function_name: S, parameter: P) -> Self {
        Self {
            function: Some(function_name.into()),
            parameter: Some(parameter.to_string()),
        }
    }

    pub fn from_parameter<P: Display>(parameter: P) -> Self {
        Self {
            function: None,
            parameter: Some(parameter.to_string()),
        }
    }

    fn path(&self) -> String {
        match (&self.function, &self.parameter) {
            (Some(f), Some(p)) => format!("{f}/{p}"),
            (Some(f), None) => f.clone(),
            (None, Some(p)) => p.clone(),
            (None, None) => String::new(),
        }
    }
}

/// What `bench_function` accepts as a name, as in criterion.
pub trait IntoBenchmarkId {
    fn into_benchmark_id(self) -> BenchmarkId;
}

impl IntoBenchmarkId for BenchmarkId {
    fn into_benchmark_id(self) -> BenchmarkId {
        self
    }
}

impl<S: Into<String>> IntoBenchmarkId for S {
    fn into_benchmark_id(self) -> BenchmarkId {
        BenchmarkId {
            function: Some(self.into()),
            parameter: None,
        }
    }
}

/// The timing every benchmark gets.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// The cycle counter's frequency, to turn cycles into nanoseconds.
    pub cpu_hz: u64,
    /// Cycles of warm-up before sampling.
    pub warm_up_cycles: u64,
    /// The cycles each sample aims for, so that one counter read's cost and
    /// an interrupt are small against it.
    pub sample_cycles: u64,
    /// Samples per benchmark, unless the whole set would exceed
    /// `max_measure_cycles`, when fewer, down to `min_samples`, are taken.
    pub samples: usize,
    pub min_samples: usize,
    pub max_measure_cycles: u64,
}

impl Config {
    /// 240 MHz, 0.2 s of warm-up, 2 ms samples, 51 samples, at most 2 s.
    pub const ESP32S3_240MHZ: Self = Self {
        cpu_hz: 240_000_000,
        warm_up_cycles: 48_000_000,
        sample_cycles: 480_000,
        samples: 51,
        min_samples: 11,
        max_measure_cycles: 480_000_000,
    };
}

/// The installed cycle counter, a `fn() -> u32` that wraps.
static CLOCK: AtomicUsize = AtomicUsize::new(0);
static CONFIG: Mutex<Config> = Mutex::new(Config::ESP32S3_240MHZ);
/// Only IDs starting with one of these run, when any are set.
static FILTER: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// (ID prefix, library) pairs that are skipped: a library that failed a
/// known-answer check is not raced at that operation.
static EXCLUDE: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Installs the cycle counter that every benchmark is timed with. It must
/// count every cycle of the core the benchmarks run on, and may wrap at 2^32.
pub fn set_clock(clock: fn() -> u32) {
    CLOCK.store(clock as usize, Ordering::Relaxed);
}

/// The optional wall clock, a `fn() -> u64` in nanoseconds, independent of
/// the cycle counter.
static WALL: AtomicUsize = AtomicUsize::new(0);

/// Installs a wall clock (nanoseconds, any epoch) independent of the cycle
/// counter. With one, each result also records the wall time its samples
/// took and the cycles they counted, so a host can check the counter's rate
/// against it and catch a wrapped or stalled counter.
pub fn set_wall_clock(clock: fn() -> u64) {
    WALL.store(clock as usize, Ordering::Relaxed);
}

fn wall() -> Option<u64> {
    let clock = WALL.load(Ordering::Relaxed);
    if clock == 0 {
        return None;
    }
    // SAFETY: WALL only ever holds a `fn() -> u64`, stored by `set_wall_clock`.
    let clock: fn() -> u64 = unsafe { core::mem::transmute(clock) };
    Some(clock())
}

/// Sets the timing every later benchmark gets.
pub fn set_config(config: Config) {
    *CONFIG.lock().unwrap() = config;
}

/// Runs only the benchmarks whose ID starts with one of `prefixes`.
pub fn set_filter(prefixes: Vec<String>) {
    *FILTER.lock().unwrap() = prefixes;
}

/// Skips every benchmark whose ID starts with `prefix` and has `library` as
/// one of its path segments, printing a `skipped` line in place of its result.
pub fn exclude(prefix: &str, library: &str) {
    EXCLUDE
        .lock()
        .unwrap()
        .push((prefix.to_string(), library.to_string()));
}

/// What a benchmark run tells an observer. Observers run only between
/// benchmarks: before a benchmark's setup and warm-up, and after its last
/// sample, never while it is being timed.
pub enum Event<'a> {
    /// A dry run (see [`set_dry_run`]) reached this benchmark.
    Listed(&'a str),
    /// This benchmark is about to warm up and be timed.
    Start(&'a str),
    /// This benchmark finished with this median time, in ns.
    Result(&'a str, f64),
}

static OBSERVER: Mutex<Option<fn(Event<'_>)>> = Mutex::new(None);
static DRY_RUN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Installs a function told of each benchmark, such as a progress display.
pub fn set_observer(observer: fn(Event<'_>)) {
    *OBSERVER.lock().unwrap() = Some(observer);
}

/// When set, benchmarks are listed to the observer and not run or printed, so
/// a display can count them first.
pub fn set_dry_run(dry: bool) {
    DRY_RUN.store(dry, Ordering::Relaxed);
}

fn observe(event: Event<'_>) {
    let observer = *OBSERVER.lock().unwrap();
    if let Some(observer) = observer {
        observer(event);
    }
}

fn now() -> u32 {
    let clock = CLOCK.load(Ordering::Relaxed);
    assert!(clock != 0, "criterion-esp: call set_clock first");
    // SAFETY: CLOCK only ever holds a `fn() -> u32`, stored by `set_clock`.
    let clock: fn() -> u32 = unsafe { core::mem::transmute(clock) };
    clock()
}

/// Prints one `RB:` line. Each holds a JSON object.
pub fn emit(json: &str) {
    #[cfg(all(test, feature = "std"))]
    {
        let mut cap = CAPTURE.lock().unwrap();
        if let Some(lines) = cap.as_mut() {
            lines.push(json.to_string());
            return;
        }
    }
    #[cfg(feature = "std")]
    {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "RB:{json}");
        let _ = out.flush();
    }
    #[cfg(not(feature = "std"))]
    if let Some(output) = *OUTPUT.lock().unwrap() {
        output(&format!("RB:{json}\n"));
    }
}

/// Lines captured by tests, instead of stdout. Production builds leave it empty.
#[cfg(all(test, feature = "std"))]
static CAPTURE: Mutex<Option<Vec<String>>> = Mutex::new(None);

#[cfg(not(feature = "std"))]
static OUTPUT: Mutex<Option<fn(&str)>> = Mutex::new(None);

/// Without std: installs where `RB:` lines go, such as a USB serial port.
#[cfg(not(feature = "std"))]
pub fn set_output(output: fn(&str)) {
    *OUTPUT.lock().unwrap() = Some(output);
}

/// `f64::round` and `f64::sqrt` need std; these serve the statistics here.
fn round(x: f64) -> f64 {
    let t = (x + 0.5) as i64 as f64;
    if x >= 0.0 {
        t
    } else {
        -((-x + 0.5) as i64 as f64)
    }
}

fn sqrt(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut r = if x > 1.0 { x / 2.0 } else { 1.0 };
    for _ in 0..64 {
        r = 0.5 * (r + x / r);
    }
    r
}

/// JSON's string escaping, for IDs.
fn quote(s: &str) -> String {
    let mut q = String::with_capacity(s.len() + 2);
    q.push('"');
    for c in s.chars() {
        match c {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            c if (c as u32) < 0x20 => q.push_str(&format!("\\u{:04x}", c as u32)),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

/// The benchmark manager, as criterion's.
#[derive(Default)]
pub struct Criterion {
    _private: (),
}

impl Criterion {
    /// Criterion reads its options from the command line; here they come from
    /// [`set_config`] and [`set_filter`].
    #[must_use]
    pub fn configure_from_args(self) -> Self {
        self
    }

    #[must_use]
    pub fn sample_size(self, _n: usize) -> Self {
        self
    }

    #[must_use]
    pub fn measurement_time(self, _t: core::time::Duration) -> Self {
        self
    }

    #[must_use]
    pub fn warm_up_time(self, _t: core::time::Duration) -> Self {
        self
    }

    pub fn benchmark_group<S: Into<String>>(&mut self, group_name: S) -> BenchmarkGroup<'_> {
        BenchmarkGroup {
            _criterion: self,
            name: group_name.into(),
            throughput: None,
            _measurement: PhantomData,
        }
    }

    pub fn bench_function<F: FnMut(&mut Bencher<'_>)>(&mut self, id: &str, f: F) -> &mut Self {
        run(id.to_string(), None, f);
        self
    }

    pub fn final_summary(&self) {}
}

/// A group of benchmarks sharing a name prefix and throughput, as criterion's.
pub struct BenchmarkGroup<'a, M = measurement::WallTime> {
    _criterion: &'a mut Criterion,
    name: String,
    throughput: Option<Throughput>,
    _measurement: PhantomData<M>,
}

impl<M> BenchmarkGroup<'_, M> {
    pub fn throughput(&mut self, throughput: Throughput) -> &mut Self {
        self.throughput = Some(throughput);
        self
    }

    /// Criterion's sample count is fixed by [`Config`] here, so every
    /// benchmark gets the same statistics.
    pub fn sample_size(&mut self, _n: usize) -> &mut Self {
        self
    }

    pub fn measurement_time(&mut self, _t: core::time::Duration) -> &mut Self {
        self
    }

    pub fn warm_up_time(&mut self, _t: core::time::Duration) -> &mut Self {
        self
    }

    pub fn bench_function<ID: IntoBenchmarkId, F>(&mut self, id: ID, f: F) -> &mut Self
    where
        F: FnMut(&mut Bencher<'_>),
    {
        let id = format!("{}/{}", self.name, id.into_benchmark_id().path());
        run(id, self.throughput, f);
        self
    }

    pub fn bench_with_input<ID: IntoBenchmarkId, F, I: ?Sized>(
        &mut self,
        id: ID,
        input: &I,
        mut f: F,
    ) -> &mut Self
    where
        F: FnMut(&mut Bencher<'_>, &I),
    {
        let id = format!("{}/{}", self.name, id.into_benchmark_id().path());
        run(id, self.throughput, |b| f(b, input));
        self
    }

    pub fn finish(self) {}
}

/// Per-iteration cycle counts of one benchmark's samples.
struct Samples {
    iters: u64,
    cycles: Vec<f64>,
    /// The wall time the samples took, and the cycles they counted, when a
    /// wall clock is installed.
    wall: Option<(u64, u64)>,
}

/// Times a routine, as criterion's `Bencher`.
pub struct Bencher<'a> {
    config: Config,
    samples: Option<Samples>,
    _lifetime: PhantomData<&'a ()>,
}

impl Bencher<'_> {
    /// Times `routine`, its output passed through [`black_box`] so that it
    /// cannot be optimised away.
    pub fn iter<O, R: FnMut() -> O>(&mut self, mut routine: R) {
        self.measure(|iters| {
            let start = now();
            for _ in 0..iters {
                black_box(routine());
            }
            now().wrapping_sub(start)
        });
    }

    /// Times `routine` on inputs from `setup`, made before each sample's
    /// clock starts and dropped after it stops.
    pub fn iter_batched<I, O, S, R>(&mut self, mut setup: S, mut routine: R, _size: BatchSize)
    where
        S: FnMut() -> I,
        R: FnMut(I) -> O,
    {
        self.measure(|iters| {
            let inputs: Vec<I> = (0..iters).map(|_| setup()).collect();
            let mut outputs = Vec::with_capacity(inputs.len());
            let start = now();
            for input in inputs {
                outputs.push(black_box(routine(input)));
            }
            let elapsed = now().wrapping_sub(start);
            drop(outputs);
            elapsed
        });
    }

    /// As [`Bencher::iter_batched`], with each input borrowed.
    pub fn iter_batched_ref<I, O, S, R>(&mut self, mut setup: S, mut routine: R, _size: BatchSize)
    where
        S: FnMut() -> I,
        R: FnMut(&mut I) -> O,
    {
        self.measure(|iters| {
            let mut inputs: Vec<I> = (0..iters).map(|_| setup()).collect();
            let mut outputs = Vec::with_capacity(inputs.len());
            let start = now();
            for input in &mut inputs {
                outputs.push(black_box(routine(input)));
            }
            let elapsed = now().wrapping_sub(start);
            drop(outputs);
            elapsed
        });
    }

    pub fn iter_with_setup<I, O, S, R>(&mut self, setup: S, routine: R)
    where
        S: FnMut() -> I,
        R: FnMut(I) -> O,
    {
        self.iter_batched(setup, routine, BatchSize::PerIteration);
    }

    /// Warms up, sizes the samples, then takes them. `timed(n)` runs `n`
    /// iterations and returns the cycles they took.
    fn measure(&mut self, mut timed: impl FnMut(u64) -> u32) {
        let c = self.config;
        // Warm-up, doubling the iterations until the warm-up time is spent,
        // which also estimates one iteration's cost. One timed call stays
        // under 2^31 cycles, so the 32-bit counter cannot wrap within it.
        let (mut iters, mut spent, mut last) = (1u64, 0u64, 0u64);
        while spent < c.warm_up_cycles {
            last = u64::from(timed(iters));
            spent += last;
            if last < c.warm_up_cycles / 4 {
                iters *= 2;
            }
        }
        let per_iter = (last as f64 / iters as f64).max(1.0);
        let iters = ((c.sample_cycles as f64 / per_iter) as u64).max(1);
        let sample = per_iter * iters as f64;
        let samples =
            ((c.max_measure_cycles as f64 / sample) as usize).clamp(c.min_samples, c.samples);
        let w0 = wall();
        let mut counted = 0u64;
        let cycles = (0..samples)
            .map(|_| {
                let c = timed(iters);
                counted += u64::from(c);
                f64::from(c) / iters as f64
            })
            .collect();
        let wall = w0.zip(wall()).map(|(a, b)| (b.wrapping_sub(a), counted));
        self.samples = Some(Samples {
            iters,
            cycles,
            wall,
        });
    }
}

fn run<F: FnMut(&mut Bencher<'_>)>(id: String, throughput: Option<Throughput>, mut f: F) {
    {
        let filter = FILTER.lock().unwrap();
        if !filter.is_empty() && !filter.iter().any(|p| id.starts_with(p.as_str())) {
            return;
        }
    }
    let excluded = EXCLUDE.lock().unwrap().iter().any(|(prefix, lib)| {
        id.starts_with(prefix.as_str()) && id.split('/').any(|seg| seg == lib)
    });
    if DRY_RUN.load(Ordering::Relaxed) {
        if !excluded {
            observe(Event::Listed(&id));
        }
        return;
    }
    if excluded {
        emit(&format!(
            "{{\"type\":\"skipped\",\"id\":{},\"reason\":\"failed a known-answer check\"}}",
            quote(&id)
        ));
        return;
    }
    let config = *CONFIG.lock().unwrap();
    let mut b = Bencher {
        config,
        samples: None,
        _lifetime: PhantomData,
    };
    observe(Event::Start(&id));
    f(&mut b);
    let Some(Samples {
        iters,
        mut cycles,
        wall,
    }) = b.samples
    else {
        emit(&format!(
            "{{\"type\":\"skipped\",\"id\":{},\"reason\":\"no iter call\"}}",
            quote(&id)
        ));
        return;
    };
    let stats = summarize(&mut cycles);
    let ns = |cyc: f64| cyc * 1e9 / config.cpu_hz as f64;
    let (tp, bytes) = match throughput {
        Some(Throughput::Bytes(b)) => (format!("{{\"Bytes\":{b}}}"), Some(b)),
        Some(Throughput::BytesDecimal(b)) => (format!("{{\"BytesDecimal\":{b}}}"), Some(b)),
        Some(Throughput::Elements(e)) => (format!("{{\"Elements\":{e}}}"), None),
        None => ("null".to_string(), None),
    };
    let cpb = bytes.filter(|&b| b > 0).map_or("null".to_string(), |b| {
        format!("{:.3}", stats.median / b as f64)
    });
    emit(&format!(
        concat!(
            "{{\"type\":\"result\",\"id\":{},\"ns\":{:.3},\"ns_low\":{:.3},\"ns_high\":{:.3},",
            "\"throughput\":{},\"cycles\":{{\"median\":{:.1},\"min\":{:.1},\"p5\":{:.1},",
            "\"p95\":{:.1},\"max\":{:.1},\"mean\":{:.1},\"stddev\":{:.1}}},",
            "\"cycles_per_byte\":{},\"samples\":{},\"iters_per_sample\":{}{}}}"
        ),
        quote(&id),
        ns(stats.median),
        ns(stats.p5),
        ns(stats.p95),
        tp,
        stats.median,
        stats.min,
        stats.p5,
        stats.p95,
        stats.max,
        stats.mean,
        stats.stddev,
        cpb,
        stats.n,
        iters,
        wall.map_or(String::new(), |(w, c)| format!(
            ",\"wall_ns\":{w},\"sampled_cycles\":{c}"
        ))
    ));
    observe(Event::Result(&id, ns(stats.median)));
}

/// Per-iteration cycle counts, sorted, as one result line reports them.
struct Stats {
    n: usize,
    min: f64,
    p5: f64,
    median: f64,
    p95: f64,
    max: f64,
    mean: f64,
    stddev: f64,
}

/// `cycles` is non-empty: a benchmark that took no samples does not get here.
fn summarize(cycles: &mut [f64]) -> Stats {
    cycles.sort_by(f64::total_cmp);
    let n = cycles.len();
    let pct = |p: f64| cycles[round((n - 1) as f64 * p) as usize];
    let mean = cycles.iter().sum::<f64>() / n as f64;
    let var = cycles.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / (n.max(2) - 1) as f64;
    Stats {
        n,
        min: cycles[0],
        p5: pct(0.05),
        median: pct(0.5),
        p95: pct(0.95),
        max: cycles[n - 1],
        mean,
        stddev: sqrt(var),
    }
}

/// Defines a function running each target, as criterion's.
#[macro_export]
macro_rules! criterion_group {
    (name = $name:ident; config = $config:expr; targets = $( $target:path ),+ $(,)*) => {
        pub fn $name() {
            let mut criterion: $crate::Criterion = $config.configure_from_args();
            $( $target(&mut criterion); )+
        }
    };
    ($name:ident, $( $target:path ),+ $(,)*) => {
        $crate::criterion_group! {
            name = $name;
            config = $crate::Criterion::default();
            targets = $( $target ),+
        }
    };
}

/// Criterion's `main`. The firmware calls each bench file's groups itself, so
/// this one only names them, to keep each bench file compiling unchanged.
#[macro_export]
macro_rules! criterion_main {
    ( $( $group:path ),+ $(,)* ) => {
        #[allow(dead_code)]
        pub fn main() {
            $( $group(); )+
        }
    };
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

    static HARNESS: Mutex<()> = Mutex::new(());
    static TICK: AtomicU32 = AtomicU32::new(0);
    static LISTED: AtomicUsize = AtomicUsize::new(0);
    static WALL_NS: AtomicU64 = AtomicU64::new(0);

    fn tick_clock() -> u32 {
        TICK.load(Ordering::Relaxed)
    }

    fn advance(by: u32) {
        TICK.fetch_add(by, Ordering::Relaxed);
    }

    fn wall_clock() -> u64 {
        WALL_NS.fetch_add(250, Ordering::Relaxed)
    }

    fn on_listed(event: Event<'_>) {
        if matches!(event, Event::Listed(_)) {
            LISTED.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Short enough that a test finishes in milliseconds, same shape as the
    /// boards: at least `min_samples`, at most `samples`, each sample aiming
    /// for `sample_cycles`.
    fn tiny(samples: usize, min_samples: usize, max_measure_cycles: u64) -> Config {
        Config {
            cpu_hz: 1_000_000,
            warm_up_cycles: 40,
            sample_cycles: 20,
            samples,
            min_samples,
            max_measure_cycles,
        }
    }

    fn with_harness(config: Config, f: impl FnOnce()) -> Vec<String> {
        let _guard = HARNESS.lock().unwrap_or_else(|e| e.into_inner());
        set_filter(Vec::new());
        *EXCLUDE.lock().unwrap() = Vec::new();
        *OBSERVER.lock().unwrap() = None;
        set_dry_run(false);
        WALL.store(0, Ordering::Relaxed);
        TICK.store(0, Ordering::Relaxed);
        LISTED.store(0, Ordering::Relaxed);
        WALL_NS.store(0, Ordering::Relaxed);
        set_clock(tick_clock);
        set_config(config);
        *CAPTURE.lock().unwrap() = Some(Vec::new());
        f();
        CAPTURE.lock().unwrap().take().unwrap_or_default()
    }

    fn value(line: &str) -> serde_json::Value {
        serde_json::from_str(line).unwrap_or_else(|e| panic!("{e} in {line}"))
    }

    #[test]
    fn round_and_sqrt_match_the_statistics_the_lines_use() {
        assert_eq!(round(0.5), 1.0);
        assert_eq!(round(1.5), 2.0);
        assert_eq!(round(0.49), 0.0);
        assert_eq!(round(-0.5), -1.0);
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(-1.0), 0.0);
        assert!((sqrt(11.0) - 11.0_f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn percentiles_use_nearest_rank_on_the_sorted_samples() {
        let mut cycles: Vec<f64> = (1..=11).map(f64::from).collect();
        let s = summarize(&mut cycles);
        assert_eq!(s.n, 11);
        assert_eq!(s.min, 1.0);
        assert_eq!(s.p5, 2.0);
        assert_eq!(s.median, 6.0);
        assert_eq!(s.p95, 11.0);
        assert_eq!(s.max, 11.0);
        assert_eq!(s.mean, 6.0);
        assert!((s.stddev - 11.0_f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn constant_cost_reports_median_cycles_and_nanoseconds() {
        let lines = with_harness(tiny(11, 11, 10_000), || {
            let mut c = Criterion::default();
            c.bench_function("plain", |b| b.iter(|| advance(5)));
        });
        assert_eq!(lines.len(), 1, "{lines:?}");
        let v = value(&lines[0]);
        assert_eq!(v["type"], "result");
        assert_eq!(v["id"], "plain");
        assert_eq!(v["cycles"]["median"], 5.0);
        assert_eq!(v["cycles"]["min"], 5.0);
        assert_eq!(v["cycles"]["max"], 5.0);
        assert_eq!(v["cycles"]["stddev"], 0.0);
        assert_eq!(v["ns"], 5000.0);
        assert_eq!(v["samples"], 11);
        assert_eq!(v["iters_per_sample"], 4);
        assert!(v["cycles_per_byte"].is_null());
        assert!(v["throughput"].is_null());
        assert!(v.get("wall_ns").is_none());
    }

    #[test]
    fn sample_count_stays_inside_min_and_max() {
        let capped = with_harness(tiny(7, 3, 1_000_000), || {
            let mut c = Criterion::default();
            c.bench_function("capped", |b| b.iter(|| advance(5)));
        });
        assert_eq!(value(&capped[0])["samples"], 7);

        // One iteration already costs more than the measurement budget, so
        // the count falls to min_samples instead of going below it.
        let floored = with_harness(tiny(51, 11, 10_000), || {
            let mut c = Criterion::default();
            c.bench_function("floored", |b| b.iter(|| advance(1000)));
        });
        let v = value(&floored[0]);
        assert_eq!(v["samples"], 11);
        assert_eq!(v["cycles"]["median"], 1000.0);
        assert_eq!(v["iters_per_sample"], 1);

        let mid = with_harness(tiny(51, 11, 400), || {
            let mut c = Criterion::default();
            c.bench_function("mid", |b| b.iter(|| advance(5)));
        });
        assert_eq!(value(&mid[0])["samples"], 20);
    }

    #[test]
    fn setup_is_outside_the_timed_section() {
        let lines = with_harness(tiny(11, 11, 10_000), || {
            let mut c = Criterion::default();
            c.bench_function("batched", |b| {
                b.iter_batched(|| advance(1000), |()| advance(3), BatchSize::PerIteration);
            });
        });
        let v = value(&lines[0]);
        assert_eq!(v["cycles"]["median"], 3.0);
        assert_eq!(v["ns"], 3000.0);
    }

    #[test]
    fn throughput_fills_cycles_per_byte_only_for_bytes() {
        let lines = with_harness(tiny(11, 11, 10_000), || {
            let mut c = Criterion::default();
            let mut g = c.benchmark_group("sha256");
            g.throughput(Throughput::Bytes(32));
            g.bench_function("rind/32B", |b| b.iter(|| advance(5)));
            g.throughput(Throughput::Elements(1));
            g.bench_function("once", |b| b.iter(|| advance(5)));
            g.throughput(Throughput::Bytes(0));
            g.bench_function("empty", |b| b.iter(|| advance(5)));
            g.finish();
        });
        assert_eq!(lines.len(), 3, "{lines:?}");
        let bytes = value(&lines[0]);
        assert_eq!(bytes["id"], "sha256/rind/32B");
        assert_eq!(bytes["throughput"]["Bytes"], 32);
        assert!((bytes["cycles_per_byte"].as_f64().unwrap() - 5.0 / 32.0).abs() < 0.0006);
        let elements = value(&lines[1]);
        assert_eq!(elements["throughput"]["Elements"], 1);
        assert!(elements["cycles_per_byte"].is_null());
        assert!(value(&lines[2])["cycles_per_byte"].is_null());
    }

    #[test]
    fn wall_clock_records_the_sample_window_and_the_counted_cycles() {
        let lines = with_harness(tiny(11, 11, 10_000), || {
            set_wall_clock(wall_clock);
            let mut c = Criterion::default();
            c.bench_function("walled", |b| b.iter(|| advance(5)));
        });
        let v = value(&lines[0]);
        assert_eq!(v["wall_ns"], 250);
        assert_eq!(v["sampled_cycles"], 220);
        assert_eq!(v["cycles"]["median"], 5.0);
    }

    #[test]
    fn filter_exclude_and_a_missing_iter_skip_without_a_time() {
        let lines = with_harness(tiny(11, 11, 10_000), || {
            set_filter(vec![
                "keep/".into(),
                "aes128-gcm".into(),
                "sha256/".into(),
                "quiet".into(),
            ]);
            exclude("aes128-gcm", "ring");
            let mut c = Criterion::default();
            c.bench_function("drop/no", |b| b.iter(|| advance(5)));
            c.bench_function("keep/yes", |b| b.iter(|| advance(5)));
            c.bench_function("aes128-gcm/ring/32B", |b| b.iter(|| advance(5)));
            c.bench_function("aes128-gcm/rind/32B", |b| b.iter(|| advance(5)));
            c.bench_function("sha256/ring/64B", |b| b.iter(|| advance(5)));
            c.bench_function("quiet", |_| {});
        });
        let rows: Vec<(String, String)> = lines
            .iter()
            .map(|l| {
                let v = value(l);
                (
                    v["type"].as_str().unwrap().to_string(),
                    v["id"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("result".into(), "keep/yes".into()),
                ("skipped".into(), "aes128-gcm/ring/32B".into()),
                ("result".into(), "aes128-gcm/rind/32B".into()),
                ("result".into(), "sha256/ring/64B".into()),
                ("skipped".into(), "quiet".into()),
            ]
        );
        assert!(lines[1].contains("failed a known-answer check"));
        assert!(lines[4].contains("no iter call"));
    }

    #[test]
    fn ids_are_json_strings() {
        let lines = with_harness(tiny(11, 11, 10_000), || {
            let mut c = Criterion::default();
            c.bench_function("a\"b\\c\u{1}", |b| b.iter(|| advance(5)));
        });
        assert_eq!(value(&lines[0])["id"], "a\"b\\c\u{1}");
    }

    #[test]
    fn dry_run_lists_the_id_and_does_not_run_the_body() {
        let lines = with_harness(tiny(11, 11, 10_000), || {
            set_dry_run(true);
            set_observer(on_listed);
            let mut c = Criterion::default();
            c.bench_function("counted", |_| advance(5));
            exclude("counted", "counted");
            c.bench_function("counted", |_| advance(5));
        });
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(LISTED.load(Ordering::Relaxed), 1);
        assert_eq!(TICK.load(Ordering::Relaxed), 0);
    }
}
