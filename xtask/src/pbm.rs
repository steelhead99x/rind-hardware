// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
//! The render example's PBM screens as PNGs (the panel's colors) and as
//! half-block text for Markdown previews.

use std::io::Write;
use std::path::{Path, PathBuf};

struct Pbm {
    w: usize,
    h: usize,
    on: Vec<bool>,
}

fn read(path: &Path) -> Result<Pbm, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let t: Vec<&str> = text.split_whitespace().collect();
    let num = |i: usize| {
        t.get(i)
            .and_then(|s| s.parse().ok())
            .ok_or(format!("{}: bad PBM header", path.display()))
    };
    let (w, h) = (num(1)?, num(2)?);
    let on: Vec<bool> = t[3..].iter().map(|p| *p == "1").collect();
    if on.len() < w * h {
        return Err(format!("{}: short PBM", path.display()));
    }
    Ok(Pbm { w, h, on })
}

fn chunk(out: &mut Vec<u8>, ty: &[u8], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut c = crc32fast::Hasher::new();
    c.update(ty);
    c.update(data);
    out.extend_from_slice(ty);
    out.extend_from_slice(data);
    out.extend_from_slice(&c.finalize().to_be_bytes());
}

/// `name.png` at 4x, or `name@Nx.png` with an explicit scale (1 is the panel's 128x64).
pub fn to_png(path: &Path, scale: Option<usize>) -> Result<PathBuf, String> {
    let p = read(path)?;
    let s = scale.unwrap_or(4);
    let mut rows = vec![];
    for y in 0..p.h {
        let mut r = vec![];
        for x in 0..p.w {
            let c: &[u8] = if p.on[y * p.w + x] {
                b"\x7f\xd8\xff"
            } else {
                b"\x05\x0a\x14"
            };
            for _ in 0..s {
                r.extend_from_slice(c);
            }
        }
        for _ in 0..s {
            rows.push(0);
            rows.extend_from_slice(&r);
        }
    }
    let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::new(6));
    z.write_all(&rows).map_err(|e| e.to_string())?;
    let idat = z.finish().map_err(|e| e.to_string())?;
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = vec![];
    ihdr.extend_from_slice(&((p.w * s) as u32).to_be_bytes());
    ihdr.extend_from_slice(&((p.h * s) as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &idat);
    chunk(&mut png, b"IEND", b"");
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let suffix = scale.map_or(String::new(), |s| format!("@{s}x"));
    let out = path.with_file_name(format!("{stem}{suffix}.png"));
    std::fs::write(&out, png).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(out)
}

/// Two pixel rows per line with half blocks, in a fenced block.
pub fn to_text(path: &Path) -> Result<String, String> {
    let p = read(path)?;
    let on = |x: usize, y: usize| y < p.h && p.on[y * p.w + x];
    let mut out = String::from("```text\n");
    for y in (0..p.h).step_by(2) {
        let line: String = (0..p.w)
            .map(|x| match (on(x, y), on(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                _ => ' ',
            })
            .collect();
        let line = line.trim_end();
        out += if line.is_empty() { " " } else { line };
        out.push('\n');
    }
    out += "```\n";
    Ok(out)
}
