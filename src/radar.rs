//! The radar picture: a crop of the map's overview with where to stand and where it lands.
//!
//! Needs the map's radar image and its overview file (Valve's `<map>.txt` with `pos_x`, `pos_y`
//! and `scale`) in the radar folder: `$XDG_DATA_HOME/nadecue/radar/<map>.png` + `<map>.txt`.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{ImageFormat, Rgba, RgbaImage, imageops};

/// Output size (square). The card shows it at half size.
const SIZE: u32 = 240;

const ORANGE: Rgba<u8> = Rgba([255, 140, 26, 255]);
const GOLD: Rgba<u8> = Rgba([245, 165, 36, 255]);
const EMBER: Rgba<u8> = Rgba([255, 77, 31, 255]);
const DARK: Rgba<u8> = Rgba([18, 13, 10, 255]);

pub fn dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NADECUE_RADAR_DIR") {
        return PathBuf::from(dir);
    }
    crate::library::root().parent().map(|p| p.join("radar")).unwrap_or_else(|| PathBuf::from("radar"))
}

/// World → radar pixel transform from an overview file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overview {
    pub pos_x: f64,
    pub pos_y: f64,
    pub scale: f64,
}

impl Overview {
    pub fn parse(text: &str) -> Option<Overview> {
        let value = |key: &str| -> Option<f64> {
            text.lines().find_map(|line| {
                let mut parts = line.split('"').skip(1).step_by(2);
                (parts.next()? == key).then_some(())?;
                parts.next()?.trim().parse().ok()
            })
        };
        Some(Overview { pos_x: value("pos_x")?, pos_y: value("pos_y")?, scale: value("scale")? })
    }

    pub fn pixel(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.pos_x) / self.scale, (self.pos_y - y) / self.scale)
    }
}

pub fn load(dir: &Path, map: &str) -> Option<(Overview, RgbaImage)> {
    let overview = Overview::parse(&std::fs::read_to_string(dir.join(format!("{map}.txt"))).ok()?)?;
    let image = image::open(dir.join(format!("{map}.png"))).ok()?.to_rgba8();
    Some((overview, image))
}

fn disc(img: &mut RgbaImage, cx: f64, cy: f64, r: f64, color: Rgba<u8>) {
    let (w, h) = (img.width() as i64, img.height() as i64);
    for y in (cy - r).floor() as i64..=(cy + r).ceil() as i64 {
        for x in (cx - r).floor() as i64..=(cx + r).ceil() as i64 {
            if x >= 0
                && y >= 0
                && x < w
                && y < h
                && ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)) <= r * r
            {
                img.put_pixel(x as u32, y as u32, color);
            }
        }
    }
}

fn line(img: &mut RgbaImage, a: (f64, f64), b: (f64, f64), width: f64, color: Rgba<u8>) {
    let steps = ((b.0 - a.0).hypot(b.1 - a.1) * 2.0).ceil().max(1.0) as usize;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        disc(img, a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, width / 2.0, color);
    }
}

/// PNG of the radar around `from` (and `to`), marked. None without radar files for the map.
pub fn render(dir: &Path, map: &str, from: [f64; 3], to: Option<[f64; 2]>) -> Option<Vec<u8>> {
    let (overview, radar) = load(dir, map)?;
    let a = overview.pixel(from[0], from[1]);
    let b = to.map(|t| overview.pixel(t[0], t[1]));
    // A square around both points with some room, at least 300 px of the 1024 radar.
    let (cx, cy, span) = match b {
        Some(b) => ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0, (a.0 - b.0).abs().max((a.1 - b.1).abs()) + 140.0),
        None => (a.0, a.1, 300.0),
    };
    let span = span.max(300.0).min(radar.width().min(radar.height()) as f64);
    let x0 = (cx - span / 2.0).clamp(0.0, radar.width() as f64 - span);
    let y0 = (cy - span / 2.0).clamp(0.0, radar.height() as f64 - span);
    let mut crop = imageops::crop_imm(&radar, x0 as u32, y0 as u32, span as u32, span as u32).to_image();
    // Dark backdrop under the radar's transparent parts.
    for p in crop.pixels_mut() {
        let alpha = p[3] as f64 / 255.0;
        for c in 0..3 {
            p[c] = (p[c] as f64 * alpha + DARK[c] as f64 * (1.0 - alpha)) as u8;
        }
        p[3] = 255;
    }
    let k = span / SIZE as f64; // keeps marks the same size whatever the zoom
    let a = (a.0 - x0, a.1 - y0);
    if let Some(b) = b {
        let b = (b.0 - x0, b.1 - y0);
        line(&mut crop, a, b, 5.0 * k, DARK);
        line(&mut crop, a, b, 2.5 * k, ORANGE);
        disc(&mut crop, b.0, b.1, 9.0 * k, DARK);
        disc(&mut crop, b.0, b.1, 7.0 * k, EMBER);
    }
    disc(&mut crop, a.0, a.1, 8.0 * k, DARK);
    disc(&mut crop, a.0, a.1, 6.0 * k, GOLD);
    let small = imageops::resize(&crop, SIZE, SIZE, imageops::FilterType::Triangle);
    let mut png = Vec::new();
    small.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).ok()?;
    Some(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_overviews() {
        let text = "\"de_mirage\"\n{\n\t\"material\"\t\"overviews/de_mirage\"\n\t\"pos_x\"\t\t\"-3230\"\t// X\n\t\"pos_y\"\t\t\"1713\"\n\t\"scale\"\t\t\"5.00\" \t// s\n}";
        let o = Overview::parse(text).unwrap();
        assert_eq!(o, Overview { pos_x: -3230.0, pos_y: 1713.0, scale: 5.0 });
        // Tetris on Mirage lands near pixel (640, 626) of the 1024 radar.
        let (x, y) = o.pixel(-31.915405, -1418.03125);
        assert!((x - 639.6).abs() < 0.1 && (y - 626.2).abs() < 0.1);
    }

    #[test]
    fn renders_a_marked_crop() {
        let dir = std::env::temp_dir().join(format!("nadecue-radar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("de_x.txt"), "\"pos_x\" \"-1000\"\n\"pos_y\" \"1000\"\n\"scale\" \"2\"\n")
            .unwrap();
        RgbaImage::new(1024, 1024).save(dir.join("de_x.png")).unwrap();
        let png = render(&dir, "de_x", [0.0, 0.0, 0.0], Some([200.0, -200.0])).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (SIZE, SIZE));
        assert!(img.pixels().any(|p| *p == GOLD) && img.pixels().any(|p| *p == ORANGE));
        assert!(render(&dir, "de_missing", [0.0; 3], None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
