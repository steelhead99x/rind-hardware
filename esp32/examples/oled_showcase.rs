// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! The demo for photos: the splash, then every published result page from
//! `demo/data/esp32s3.tsv` (rind-results' `cargo xtask demo-tsv`), and the
//! summary, cycling forever. Times nothing.
//! `cargo build --release --example oled_showcase --features oled`

// The showcase uses only the drawing half of the demo glue.
#[allow(dead_code)]
#[path = "../../demo/firmware/oled.rs"]
mod oled;

fn main() {
    esp_idf_svc::sys::link_patches();
    oled::init();
    oled::cycle(
        include_str!("../../demo/data/esp32s3.tsv"),
        "3 runs, median, CCOUNT",
        4,
    );
}
