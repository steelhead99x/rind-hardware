// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! The two calls the boards' known-answer checks make into criterion-esp:
//! on a host they print the line and count exclusions.

use std::sync::atomic::{AtomicUsize, Ordering};

static EXCLUDED: AtomicUsize = AtomicUsize::new(0);

/// Prints one JSON line, as the firmware does over serial.
pub fn emit(line: &str) {
    println!("{line}");
}

/// Records that `lib` failed a check for the operations under `prefix`.
pub fn exclude(prefix: &str, lib: &str) {
    EXCLUDED.fetch_add(1, Ordering::Relaxed);
    eprintln!("excluded: {prefix} for {lib}");
}

/// How many exclusions were recorded.
pub fn excluded() -> usize {
    EXCLUDED.load(Ordering::Relaxed)
}
