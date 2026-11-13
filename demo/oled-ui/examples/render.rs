// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! Renders every demo screen to a PBM image, at the panel's 128x64, for
//! checking layouts and for docs: `cargo run --example render -- results.tsv out/`.

use std::io::Write;

use rind_bench_oled_ui::{Frame, operation, pages, progress, splash, summary, tally};

fn save(frame: &Frame, path: &std::path::Path) {
    let mut f = std::fs::File::create(path).unwrap();
    writeln!(f, "P1\n128 64").unwrap();
    for row in &frame.0 {
        let line: Vec<&str> = row.iter().map(|&p| if p { "1" } else { "0" }).collect();
        writeln!(f, "{}", line.join(" ")).unwrap();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let tsv = std::fs::read_to_string(&args[1]).unwrap();
    let out = std::path::Path::new(&args[2]);
    std::fs::create_dir_all(out).unwrap();

    let mut f = Frame::default();
    splash(&mut f, "ESP32-S3 @ 240 MHz - portable rind");
    save(&f, &out.join("00-splash.pbm"));

    let mut f = Frame::default();
    progress(&mut f, "aes_gcm", "aes128-gcm/rind/1350B", 17, 57);
    save(&f, &out.join("01-progress.pbm"));

    let all: Vec<_> = pages(&tsv).collect();
    for (i, p) in all.iter().enumerate() {
        let mut f = Frame::default();
        operation(
            &mut f,
            p.heading,
            &format!("{}/{}", i + 1, all.len()),
            p.bars(),
        );
        save(&f, &out.join(format!("{:02}-op.pbm", i + 2)));
    }

    let (wins, n, g) = tally(&tsv);
    let mut f = Frame::default();
    summary(&mut f, wins, n, g, "3 runs, median, CCOUNT");
    save(&f, &out.join(format!("{:02}-summary.pbm", all.len() + 2)));
}
