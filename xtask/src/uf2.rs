// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
//! UF2 images for the nRF52840 (Adafruit bootloader) and the RP2040
//! (BOOTSEL). The nRF writer refuses anything outside the application region:
//! below 0x27000 are the MBR and the S140 SoftDevice, and from 0xED000 the
//! board's LittleFS (Meshtastic's config and keys) and the bootloader. The
//! RP2040 writer covers the 2 MiB XIP region at 0x10000000, including the
//! 256-byte second-stage bootloader.

use sha2::{Digest, Sha256};
use std::path::Path;

/// A UF2 family and the flash range a raw image may occupy.
#[derive(Clone, Copy)]
pub struct Family {
    pub id: u32,
    pub start: u32,
    pub end: u32,
}

/// Adafruit nRF52 bootloader, application region only.
pub const NRF: Family = Family {
    id: 0xADA5_2840,
    start: 0x27000,
    end: 0xED000,
};
/// RP2040 BOOTSEL (official Pico family ID), the 2 MiB XIP flash.
pub const RP2040: Family = Family {
    id: 0xE48B_FF56,
    start: 0x1000_0000,
    end: 0x1020_0000,
};
const MAGIC0: u32 = 0x0A324655;
const MAGIC1: u32 = 0x9E5D5157;
const MAGIC_END: u32 = 0x0AB16F30;
const FLAG_FAMILY: u32 = 0x2000;

fn word(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}

/// A block's address, payload, flags and family.
type Block = (u32, Vec<u8>, u32, u32);

/// Every block of a UF2 file, checked.
fn blocks(path: &Path) -> Result<Vec<Block>, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if data.len() % 512 != 0 {
        return Err("not a UF2: size is not a multiple of 512".into());
    }
    let mut out = vec![];
    for (i, b) in data.chunks(512).enumerate() {
        let (m0, m1, flags, addr, size, fam) = (
            word(b, 0),
            word(b, 4),
            word(b, 8),
            word(b, 12),
            word(b, 16),
            word(b, 28),
        );
        if (m0, m1, word(b, 508)) != (MAGIC0, MAGIC1, MAGIC_END) || size > 476 {
            return Err(format!("bad magic in block {i}"));
        }
        out.push((addr, b[32..32 + size as usize].to_vec(), flags, fam));
    }
    Ok(out)
}

/// Its address range, family and block count, as uf2.py printed them.
pub fn info(path: &Path) -> Result<String, String> {
    let bl = blocks(path)?;
    let lo = bl.iter().map(|b| b.0).min().unwrap_or(0);
    let hi = bl.iter().map(|b| b.0 + b.1.len() as u32).max().unwrap_or(0);
    let mut fams: Vec<String> = bl
        .iter()
        .map(|b| {
            if b.2 & FLAG_FAMILY != 0 {
                format!("{:#x}", b.3)
            } else {
                "none".into()
            }
        })
        .collect();
    fams.sort();
    fams.dedup();
    let fams: Vec<String> = fams.iter().map(|f| format!("'{f}'")).collect();
    Ok(format!(
        "{{'blocks': {}, 'start': '{lo:#x}', 'end': '{hi:#x}', 'families': [{}]}}",
        bl.len(),
        fams.join(", ")
    ))
}

/// The bytes of `start..end` the file holds (0xFF where it holds none), their
/// sha256 and how many bytes it covers.
pub fn region(path: &Path, start: u32, end: u32) -> Result<String, String> {
    let mut img = vec![0xFFu8; (end - start) as usize];
    let mut covered = vec![false; img.len()];
    for (addr, data, _, _) in blocks(path)? {
        for (i, b) in data.iter().enumerate() {
            let a = addr + i as u32;
            if (start..end).contains(&a) {
                img[(a - start) as usize] = *b;
                covered[(a - start) as usize] = true;
            }
        }
    }
    let hash: String = Sha256::digest(&img)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let n = covered.iter().filter(|c| **c).count();
    Ok(format!(
        "{start:#x}..{end:#x}: {n} of {} bytes covered, sha256 {hash}",
        img.len()
    ))
}

/// Writes a raw image (objcopy -O binary of the loadable sections) at `base`
/// under `family`, refusing anything outside `family.start..family.end`.
pub fn write_family(image: &Path, out: &Path, base: u32, family: Family) -> Result<String, String> {
    let data = std::fs::read(image).map_err(|e| format!("{}: {e}", image.display()))?;
    let end = base as u64 + data.len() as u64;
    if base < family.start || end > family.end as u64 {
        return Err(format!(
            "refusing: {base:#x}..{end:#x} leaves {:#x}..{:#x}",
            family.start, family.end
        ));
    }
    let chunks: Vec<&[u8]> = data.chunks(256).collect();
    let mut f = Vec::with_capacity(chunks.len() * 512);
    for (n, chunk) in chunks.iter().enumerate() {
        for w in [
            MAGIC0,
            MAGIC1,
            FLAG_FAMILY,
            base + n as u32 * 256,
            256,
            n as u32,
            chunks.len() as u32,
            family.id,
        ] {
            f.extend_from_slice(&w.to_le_bytes());
        }
        f.extend_from_slice(chunk);
        f.resize(f.len() + 476 - chunk.len(), 0);
        f.extend_from_slice(&MAGIC_END.to_le_bytes());
    }
    std::fs::write(out, f).map_err(|e| format!("{}: {e}", out.display()))?;
    info(out)
}
