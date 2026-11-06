//! Pixel operations of IMG42: colour adjustments (live, non-destructive), looks, auto enhance,
//! instant alpha, background removal, pixelate, resize and markup flattening.

use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum Look {
    #[default]
    None,
    Mono,
    Noir,
    Sepia,
    Invert,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Adjust {
    pub exposure: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub temperature: f32,
    pub tint: f32,
    pub sharpness: f32,
    pub vignette: f32,
    pub look: Look,
}

impl Default for Adjust {
    fn default() -> Self {
        Self { exposure: 0.0, brightness: 0.0, contrast: 0.0, highlights: 0.0, shadows: 0.0, saturation: 0.0, vibrance: 0.0, temperature: 0.0, tint: 0.0, sharpness: 0.0, vignette: 0.0, look: Look::None }
    }
}

impl Adjust {
    pub fn is_identity(&self) -> bool {
        *self == Adjust::default()
    }
}

#[inline]
fn lum(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Applies the adjustment pipeline (values roughly −1..1; exposure in stops).
pub fn apply(src: &RgbaImage, a: &Adjust) -> RgbaImage {
    if a.is_identity() {
        return src.clone();
    }
    let (w, h) = src.dimensions();
    let mut out = src.clone();
    let exp = 2f32.powf(a.exposure);
    let contrast = 1.0 + a.contrast;
    let (tr, tb) = (1.0 + a.temperature * 0.25, 1.0 - a.temperature * 0.25);
    let tg = 1.0 - a.tint * 0.15;
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let maxd = (cx * cx + cy * cy).sqrt();
    out.as_mut().par_chunks_mut((w * 4) as usize).enumerate().for_each(|(y, row)| {
        for x in 0..w as usize {
            let p = &mut row[x * 4..x * 4 + 4];
            let mut r = p[0] as f32 / 255.0;
            let mut g = p[1] as f32 / 255.0;
            let mut b = p[2] as f32 / 255.0;
            r *= exp * tr;
            g *= exp * tg;
            b *= exp * tb;
            r += a.brightness * 0.5;
            g += a.brightness * 0.5;
            b += a.brightness * 0.5;
            r = (r - 0.5) * contrast + 0.5;
            g = (g - 0.5) * contrast + 0.5;
            b = (b - 0.5) * contrast + 0.5;
            let l = lum(r, g, b).clamp(0.0, 1.0);
            // Shadows lift dark tones, highlights compress or boost bright tones.
            let sh = a.shadows * (1.0 - l).powi(2) * 0.6;
            let hi = a.highlights * l.powi(2) * 0.6;
            r += sh + hi;
            g += sh + hi;
            b += sh + hi;
            let l = lum(r, g, b);
            let sat_now = r.max(g).max(b) - r.min(g).min(b);
            let s = 1.0 + a.saturation + a.vibrance * (1.0 - sat_now.clamp(0.0, 1.0));
            r = l + (r - l) * s;
            g = l + (g - l) * s;
            b = l + (b - l) * s;
            match a.look {
                Look::Mono => {
                    let v = lum(r, g, b);
                    r = v;
                    g = v;
                    b = v;
                }
                Look::Noir => {
                    let v = ((lum(r, g, b) - 0.5) * 1.6 + 0.5).clamp(0.0, 1.0);
                    r = v;
                    g = v;
                    b = v;
                }
                Look::Sepia => {
                    let (rr, gg, bb) = (r, g, b);
                    r = rr * 0.393 + gg * 0.769 + bb * 0.189;
                    g = rr * 0.349 + gg * 0.686 + bb * 0.168;
                    b = rr * 0.272 + gg * 0.534 + bb * 0.131;
                }
                Look::Invert => {
                    r = 1.0 - r;
                    g = 1.0 - g;
                    b = 1.0 - b;
                }
                Look::None => {}
            }
            if a.vignette != 0.0 {
                let d = (((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / maxd).powi(2);
                let f = 1.0 - a.vignette * d;
                r *= f;
                g *= f;
                b *= f;
            }
            p[0] = (r.clamp(0.0, 1.0) * 255.0).round() as u8;
            p[1] = (g.clamp(0.0, 1.0) * 255.0).round() as u8;
            p[2] = (b.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    });
    if a.sharpness.abs() > 0.01 {
        out = sharpen(&out, a.sharpness);
    }
    out
}

/// Unsharp mask (negative amount blurs).
pub fn sharpen(img: &RgbaImage, amount: f32) -> RgbaImage {
    let blur = image::imageops::blur(img, 1.2);
    let mut out = img.clone();
    out.as_mut().par_chunks_mut(4).zip(blur.as_raw().par_chunks(4)).for_each(|(p, b)| {
        for k in 0..3 {
            let v = p[k] as f32 + (p[k] as f32 - b[k] as f32) * amount * 1.5;
            p[k] = v.clamp(0.0, 255.0) as u8;
        }
    });
    out
}

/// Auto enhance: stretch levels and add a little saturation, derived from the histogram.
pub fn auto_adjust(img: &RgbaImage) -> Adjust {
    let mut hist = [0u32; 256];
    for p in img.pixels().step_by(7) {
        hist[lum(p[0] as f32, p[1] as f32, p[2] as f32).round().clamp(0.0, 255.0) as usize] += 1;
    }
    let total: u32 = hist.iter().sum();
    let pct = |q: f32| {
        let mut acc = 0;
        for (i, c) in hist.iter().enumerate() {
            acc += c;
            if acc as f32 >= total as f32 * q {
                return i as f32 / 255.0;
            }
        }
        1.0
    };
    let (lo, mid, hi) = (pct(0.01), pct(0.5), pct(0.99));
    Adjust {
        exposure: ((0.45 / mid.max(0.05)).log2() * 0.5).clamp(-1.0, 1.0),
        contrast: ((1.0 / (hi - lo).max(0.2)) - 1.0).clamp(0.0, 0.6),
        shadows: if lo > 0.08 { 0.0 } else { 0.15 },
        saturation: 0.1,
        vibrance: 0.15,
        ..Adjust::default()
    }
}

/// Flood-fills similar colours from a seed into a mask (Instant Alpha).
pub fn flood_mask(img: &RgbaImage, seed: (u32, u32), tolerance: f32) -> Vec<bool> {
    let (w, h) = img.dimensions();
    let mut mask = vec![false; (w * h) as usize];
    if seed.0 >= w || seed.1 >= h {
        return mask;
    }
    let s = *img.get_pixel(seed.0, seed.1);
    let dist = |p: &Rgba<u8>| ((p[0] as f32 - s[0] as f32).powi(2) + (p[1] as f32 - s[1] as f32).powi(2) + (p[2] as f32 - s[2] as f32).powi(2)).sqrt();
    let mut stack = vec![seed];
    while let Some((x, y)) = stack.pop() {
        let i = (y * w + x) as usize;
        if mask[i] || dist(img.get_pixel(x, y)) > tolerance {
            continue;
        }
        mask[i] = true;
        if x > 0 {
            stack.push((x - 1, y));
        }
        if y > 0 {
            stack.push((x, y - 1));
        }
        if x + 1 < w {
            stack.push((x + 1, y));
        }
        if y + 1 < h {
            stack.push((x, y + 1));
        }
    }
    mask
}

/// Removes a uniform background by flooding from all borders.
pub fn remove_background(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut mask = vec![false; (w * h) as usize];
    let step = (w.max(h) / 64).max(1);
    let mut seeds = vec![];
    for x in (0..w).step_by(step as usize) {
        seeds.push((x, 0));
        seeds.push((x, h - 1));
    }
    for y in (0..h).step_by(step as usize) {
        seeds.push((0, y));
        seeds.push((w - 1, y));
    }
    for s in seeds {
        if mask[(s.1 * w + s.0) as usize] {
            continue;
        }
        let m = flood_mask(img, s, 38.0);
        for (a, b) in mask.iter_mut().zip(m) {
            *a |= b;
        }
    }
    let mut out = img.clone();
    // Soft edge: pixels next to the background get partial alpha.
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if mask[i] {
                out.get_pixel_mut(x, y)[3] = 0;
            } else {
                let near = [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)]
                    .iter()
                    .filter(|(a, b)| *a < w && *b < h && mask[(b * w + a) as usize])
                    .count();
                if near > 0 {
                    let p = out.get_pixel_mut(x, y);
                    p[3] = (p[3] as u32 * (4 - near as u32) / 4).max(60) as u8;
                }
            }
        }
    }
    out
}

pub fn pixelate(img: &mut RgbaImage, r: (u32, u32, u32, u32), block: u32) {
    let (x0, y0, x1, y1) = r;
    let block = block.max(2);
    let mut by = y0;
    while by < y1 {
        let mut bx = x0;
        while bx < x1 {
            let (ex, ey) = ((bx + block).min(x1), (by + block).min(y1));
            let mut acc = [0u64; 4];
            let mut n = 0u64;
            for y in by..ey {
                for x in bx..ex {
                    let p = img.get_pixel(x, y);
                    for k in 0..4 {
                        acc[k] += p[k] as u64;
                    }
                    n += 1;
                }
            }
            if n > 0 {
                let c = Rgba([(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8, (acc[3] / n) as u8]);
                for y in by..ey {
                    for x in bx..ex {
                        img.put_pixel(x, y, c);
                    }
                }
            }
            bx += block;
        }
        by += block;
    }
}

pub fn has_alpha(img: &RgbaImage) -> bool {
    img.pixels().any(|p| p[3] < 255)
}

/// Flattens onto white (for JPEG and other opaque formats).
pub fn flatten_white(img: &RgbaImage) -> image::RgbImage {
    let mut out = image::RgbImage::new(img.width(), img.height());
    for (o, p) in out.pixels_mut().zip(img.pixels()) {
        let a = p[3] as f32 / 255.0;
        for k in 0..3 {
            o[k] = (p[k] as f32 * a + 255.0 * (1.0 - a)).round() as u8;
        }
    }
    out
}
