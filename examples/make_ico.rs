//! Regenerates `assets/ember.ico` (embedded in the Windows .exe files) from the same
//! drawing code as the tray icon:  cargo run --example make_ico
//!
//! Each image is stored as a 32-bit BGRA DIB with an all-zero AND mask, which every
//! Windows version since XP understands.

use std::io::Write;

const SIZES: &[u32] = &[16, 20, 24, 32, 40, 48, 64, 128, 256];

fn dib(size: u32) -> Vec<u8> {
    let rgba = ember::tray_model::icon_rgba(size, true);
    let mask_row = size.div_ceil(32) * 4;
    let mut out = Vec::new();
    // BITMAPINFOHEADER; height is doubled to cover the XOR + AND masks.
    for v in [40u32, size, size * 2] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bpp
    out.extend_from_slice(&[0u8; 24]); // compression, image size, ppm x/y, colours
    // Pixels, bottom-up, BGRA.
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    out.extend(std::iter::repeat_n(0u8, (mask_row * size) as usize));
    out
}

fn main() -> std::io::Result<()> {
    let images: Vec<Vec<u8>> = SIZES.iter().map(|&s| dib(s)).collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len() as u32;
    for (&s, img) in SIZES.iter().zip(&images) {
        let dim = if s >= 256 { 0 } else { s as u8 }; // 0 means 256
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in &images {
        ico.extend_from_slice(img);
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/ember.ico");
    std::fs::File::create(path)?.write_all(&ico)?;
    println!("wrote {path} ({} bytes)", ico.len());
    Ok(())
}
