//! The aim picture: a screenshot of the middle of the screen (crosshair in the centre).

use std::io::Cursor;
use std::process::{Command, Stdio};

use image::{ImageFormat, imageops};

/// Share of the screen width kept around the crosshair.
const CROP: f64 = 0.375;
/// Stored picture width (the card shows it at half size, so it stays sharp on HiDPI).
const WIDTH: u32 = 760;

/// Screenshot `output` (empty = all) with grim. Returns two PNGs: the crosshair close-up
/// (where to aim) and the whole view scaled down (where you stand).
pub fn pictures(output: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut grim = Command::new("grim");
    if !output.is_empty() {
        grim.args(["-o", output]);
    }
    let shot = grim
        .args(["-t", "ppm", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("grim: {e}"))?;
    if !shot.status.success() {
        return Err(format!("grim: {}", String::from_utf8_lossy(&shot.stderr).trim()));
    }
    let full = image::load_from_memory(&shot.stdout).map_err(|e| e.to_string())?;
    Ok((crop_png(&full)?, view_png(&full)?))
}

pub fn view_png(full: &image::DynamicImage) -> Result<Vec<u8>, String> {
    let (w, h) = (full.width(), full.height());
    let out_w = WIDTH.min(w);
    let small =
        imageops::resize(&full.to_rgb8(), out_w, (out_w * h / w).max(1), imageops::FilterType::Triangle);
    let mut png = Vec::new();
    small.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(png)
}

pub fn crop_png(full: &image::DynamicImage) -> Result<Vec<u8>, String> {
    let (w, h) = (full.width(), full.height());
    let cw = ((w as f64 * CROP) as u32).max(16).min(w);
    let ch = (cw * 9 / 16).min(h);
    let piece = full.crop_imm((w - cw) / 2, (h - ch) / 2, cw, ch);
    let out_w = WIDTH.min(cw);
    let small = imageops::resize(&piece.to_rgb8(), out_w, out_w * ch / cw, imageops::FilterType::Triangle);
    let mut png = Vec::new();
    small.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_the_centre() {
        let mut img = image::RgbImage::new(2560, 1440);
        img.put_pixel(1280, 720, image::Rgb([255, 0, 0]));
        let png = crop_png(&image::DynamicImage::ImageRgb8(img)).unwrap();
        let out = image::load_from_memory(&png).unwrap();
        assert_eq!((out.width(), out.height()), (760, 427));
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(2560, 1440));
        let view = image::load_from_memory(&view_png(&img).unwrap()).unwrap();
        assert_eq!((view.width(), view.height()), (760, 427));
    }
}
