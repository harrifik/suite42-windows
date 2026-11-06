//! Page clean-up after scanning: crop to the page, deskew, paper whitening, adaptive black &
//! white, photo auto-levels, blank detection, upright detection and ink-like mark printing.

use image::{GrayImage, Luma, Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMode {
    Color,
    Gray,
    BlackWhite,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Opts {
    pub crop: bool,
    pub deskew: bool,
    pub cleanup: bool,
    pub enhance: bool,
    pub color: ColorMode,
    /// Quarter turns clockwise.
    pub rotation: u8,
}

impl Default for Opts {
    fn default() -> Self {
        Self { crop: true, deskew: true, cleanup: true, enhance: false, color: ColorMode::Color, rotation: 0 }
    }
}

pub fn gray(img: &RgbaImage) -> GrayImage {
    let (w, h) = img.dimensions();
    let mut g = GrayImage::new(w, h);
    g.as_mut()
        .par_iter_mut()
        .zip(img.as_raw().par_chunks(4))
        .for_each(|(d, p)| *d = ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8);
    g
}

pub fn rotate_quarter(img: &RgbaImage, q: u8) -> RgbaImage {
    match q % 4 {
        1 => image::imageops::rotate90(img),
        2 => image::imageops::rotate180(img),
        3 => image::imageops::rotate270(img),
        _ => img.clone(),
    }
}

fn downscale_gray(g: &GrayImage, max: u32) -> (GrayImage, f32) {
    let (w, h) = g.dimensions();
    let s = (max as f32 / w.max(h) as f32).min(1.0);
    if s >= 1.0 {
        return (g.clone(), 1.0);
    }
    (image::imageops::resize(g, ((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1), image::imageops::FilterType::Triangle), s)
}

/// Otsu threshold of a grayscale image.
pub fn otsu(g: &GrayImage) -> u8 {
    let mut hist = [0u64; 256];
    for p in g.as_raw() {
        hist[*p as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    let sum: f64 = hist.iter().enumerate().map(|(i, c)| i as f64 * *c as f64).sum();
    let (mut sum_b, mut w_b, mut best, mut thr) = (0.0, 0u64, 0.0, 127u8);
    for (t, &c) in hist.iter().enumerate() {
        w_b += c;
        if w_b == 0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0 {
            break;
        }
        sum_b += t as f64 * c as f64;
        let m_b = sum_b / w_b as f64;
        let m_f = (sum - sum_b) / w_f as f64;
        let between = w_b as f64 * w_f as f64 * (m_b - m_f).powi(2);
        if between > best {
            best = between;
            thr = t as u8;
        }
    }
    thr
}

/// Finds the page on the glass: the region that differs from the border colour.
pub fn find_page(img: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let g = gray(img);
    let (small, s) = downscale_gray(&g, 600);
    let (w, h) = small.dimensions();
    if w < 20 || h < 20 {
        return None;
    }
    // Border statistics.
    let mut border: Vec<u8> = Vec::new();
    for x in 0..w {
        border.push(small.get_pixel(x, 0)[0]);
        border.push(small.get_pixel(x, h - 1)[0]);
    }
    for y in 0..h {
        border.push(small.get_pixel(0, y)[0]);
        border.push(small.get_pixel(w - 1, y)[0]);
    }
    border.sort_unstable();
    let bg = border[border.len() / 2] as i32;
    let spread = (border[border.len() * 9 / 10] as i32 - border[border.len() / 10] as i32).max(6);
    let thr = (spread * 2).clamp(18, 60);
    // Columns/rows with many pixels unlike the background.
    let diff = |x: u32, y: u32| (small.get_pixel(x, y)[0] as i32 - bg).abs() > thr;
    let col: Vec<u32> = (0..w).map(|x| (0..h).filter(|&y| diff(x, y)).count() as u32).collect();
    let row: Vec<u32> = (0..h).map(|y| (0..w).filter(|&x| diff(x, y)).count() as u32).collect();
    let span = |v: &[u32], len: u32| -> Option<(u32, u32)> {
        let lim = (len as f32 * 0.12) as u32;
        let a = v.iter().position(|&c| c > lim)?;
        let b = v.len() - 1 - v.iter().rev().position(|&c| c > lim)?;
        Some((a as u32, b as u32))
    };
    let (x0, x1) = span(&col, h)?;
    let (y0, y1) = span(&row, w)?;
    // Nothing to crop when the page fills the glass.
    let area = ((x1 - x0) * (y1 - y0)) as f32 / (w * h) as f32;
    if !(0.05..0.97).contains(&area) {
        return None;
    }
    // A page on contrasting glass is a filled region; sparse text on paper is not.
    let mut filled = 0u32;
    let mut total = 0u32;
    for y in (y0..=y1).step_by(2) {
        for x in (x0..=x1).step_by(2) {
            total += 1;
            if diff(x, y) {
                filled += 1;
            }
        }
    }
    if (filled as f32) < total as f32 * 0.6 {
        return None;
    }
    let inv = 1.0 / s;
    Some(((x0 as f32 * inv) as u32, (y0 as f32 * inv) as u32, ((x1 + 1) as f32 * inv) as u32, ((y1 + 1) as f32 * inv) as u32))
}

/// Skew angle in degrees from the horizontal projection profile of dark pixels.
pub fn skew_angle(img: &RgbaImage) -> f32 {
    let g = gray(img);
    let (small, _) = downscale_gray(&g, 1000);
    // Only dark ink: keeps grey scanner glass and shading out of the profile.
    let t = otsu(&small).min(100);
    let pts: Vec<(f32, f32)> = small
        .enumerate_pixels()
        .filter(|(_, _, p)| p[0] < t)
        .map(|(x, y, _)| (x as f32, y as f32))
        .collect();
    if pts.len() < 200 || pts.len() > small.as_raw().len() / 2 {
        return 0.0;
    }
    let h = small.height() as f32;
    let score = |deg: f32| -> f64 {
        let (s, c) = deg.to_radians().sin_cos();
        let bins = (h * 1.5) as usize + 4;
        let mut prof = vec![0u32; bins];
        for (x, y) in &pts {
            let yr = -x * s + y * c + h * 0.25;
            if yr >= 0.0 && (yr as usize) < bins {
                prof[yr as usize] += 1;
            }
        }
        prof.iter().map(|&v| (v as f64).powi(2)).sum()
    };
    let coarse: Vec<f32> = (-30..=30).map(|i| i as f32 * 0.25).collect();
    let best = coarse.par_iter().map(|&a| (a, score(a))).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).map(|x| x.0).unwrap_or(0.0);
    let fine: Vec<f32> = (-5..=5).map(|i| best + i as f32 * 0.05).collect();
    fine.par_iter().map(|&a| (a, score(a))).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).map(|x| x.0).unwrap_or(best)
}

/// Rotates by an arbitrary angle (degrees, counter-clockwise positive) with white fill.
pub fn rotate_deg(img: &RgbaImage, deg: f32, fill: [u8; 4]) -> RgbaImage {
    if deg.abs() < 0.01 {
        return img.clone();
    }
    let (w, h) = img.dimensions();
    let (s, c) = deg.to_radians().sin_cos();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut out = RgbaImage::new(w, h);
    let src = img.as_raw();
    out.as_mut().par_chunks_mut((w * 4) as usize).enumerate().for_each(|(y, row)| {
        for x in 0..w as usize {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let sx = dx * c - dy * s + cx - 0.5;
            let sy = dx * s + dy * c + cy - 0.5;
            let px = if sx >= 0.0 && sy >= 0.0 && sx < (w - 1) as f32 && sy < (h - 1) as f32 {
                let (x0, y0) = (sx as usize, sy as usize);
                let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
                let mut v = [0u8; 4];
                for k in 0..4 {
                    let p = |xx: usize, yy: usize| src[(yy * w as usize + xx) * 4 + k] as f32;
                    let a = p(x0, y0) * (1.0 - fx) + p(x0 + 1, y0) * fx;
                    let b = p(x0, y0 + 1) * (1.0 - fx) + p(x0 + 1, y0 + 1) * fx;
                    v[k] = (a * (1.0 - fy) + b * fy).round() as u8;
                }
                v
            } else {
                fill
            };
            row[x * 4..x * 4 + 4].copy_from_slice(&px);
        }
    });
    out
}

/// Estimates the paper background (shadows, yellowing, uneven light) and divides it out.
pub fn whiten(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let block = ((w.max(h) / 48).max(8)) as usize;
    let bw = (w as usize).div_ceil(block);
    let bh = (h as usize).div_ceil(block);
    let raw = img.as_raw();
    // Per-block bright percentile = local paper colour.
    let mut bg = vec![[255f32; 3]; bw * bh];
    bg.par_iter_mut().enumerate().for_each(|(i, cell)| {
        let (bx, by) = (i % bw, i / bw);
        for k in 0..3 {
            let mut vals: Vec<u8> = Vec::with_capacity(block * block / 4);
            let mut yy = by * block;
            while yy < ((by + 1) * block).min(h as usize) {
                let mut xx = bx * block;
                while xx < ((bx + 1) * block).min(w as usize) {
                    vals.push(raw[(yy * w as usize + xx) * 4 + k]);
                    xx += 2;
                }
                yy += 2;
            }
            if vals.is_empty() {
                continue;
            }
            vals.sort_unstable();
            cell[k] = vals[vals.len() * 9 / 10].max(40) as f32;
        }
    });
    // Smooth the background map (dilate then blur) so text blocks don't darken it.
    let mut sm = bg.clone();
    for _ in 0..2 {
        let prev = sm.clone();
        for y in 0..bh {
            for x in 0..bw {
                let mut best = prev[y * bw + x];
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < bw && (ny as usize) < bh {
                        let q = prev[ny as usize * bw + nx as usize];
                        for k in 0..3 {
                            best[k] = best[k].max(q[k] * 0.98);
                        }
                    }
                }
                sm[y * bw + x] = best;
            }
        }
    }
    let mut out = img.clone();
    out.as_mut().par_chunks_mut((w * 4) as usize).enumerate().for_each(|(y, row)| {
        let fy = (y as f32 / block as f32 - 0.5).clamp(0.0, (bh - 1) as f32);
        let y0 = fy as usize;
        let y1 = (y0 + 1).min(bh - 1);
        let ty = fy - y0 as f32;
        for x in 0..w as usize {
            let fx = (x as f32 / block as f32 - 0.5).clamp(0.0, (bw - 1) as f32);
            let x0 = fx as usize;
            let x1 = (x0 + 1).min(bw - 1);
            let tx = fx - x0 as f32;
            for k in 0..3 {
                let a = sm[y0 * bw + x0][k] * (1.0 - tx) + sm[y0 * bw + x1][k] * tx;
                let b = sm[y1 * bw + x0][k] * (1.0 - tx) + sm[y1 * bw + x1][k] * tx;
                let bgv = a * (1.0 - ty) + b * ty;
                let v = row[x * 4 + k] as f32 / bgv * 255.0;
                // Gentle contrast curve keeps text dark.
                let v = if v > 235.0 { 255.0 } else { v * 0.96 };
                row[x * 4 + k] = v.clamp(0.0, 255.0) as u8;
            }
        }
    });
    out
}

/// Sauvola adaptive threshold → black/white (as RGBA for display).
pub fn binarize(img: &RgbaImage, dpi: f32) -> RgbaImage {
    let g = gray(img);
    let (w, h) = g.dimensions();
    let win = ((dpi / 300.0 * 31.0) as i64).max(9) | 1;
    let r = win / 2;
    let (wi, hi) = (w as usize + 1, h as usize + 1);
    let mut sum = vec![0f64; wi * hi];
    let mut sq = vec![0f64; wi * hi];
    for y in 0..h as usize {
        let mut rs = 0f64;
        let mut rq = 0f64;
        for x in 0..w as usize {
            let v = g.as_raw()[y * w as usize + x] as f64;
            rs += v;
            rq += v * v;
            sum[(y + 1) * wi + x + 1] = sum[y * wi + x + 1] + rs;
            sq[(y + 1) * wi + x + 1] = sq[y * wi + x + 1] + rq;
        }
    }
    let mut out = RgbaImage::new(w, h);
    out.as_mut().par_chunks_mut((w * 4) as usize).enumerate().for_each(|(y, row)| {
        let y0 = (y as i64 - r).max(0) as usize;
        let y1 = (y as i64 + r + 1).min(h as i64) as usize;
        for x in 0..w as usize {
            let x0 = (x as i64 - r).max(0) as usize;
            let x1 = (x as i64 + r + 1).min(w as i64) as usize;
            let n = ((x1 - x0) * (y1 - y0)) as f64;
            let s = sum[y1 * wi + x1] - sum[y0 * wi + x1] - sum[y1 * wi + x0] + sum[y0 * wi + x0];
            let q = sq[y1 * wi + x1] - sq[y0 * wi + x1] - sq[y1 * wi + x0] + sq[y0 * wi + x0];
            let mean = s / n;
            let sd = (q / n - mean * mean).max(0.0).sqrt();
            let t = mean * (1.0 + 0.2 * (sd / 128.0 - 1.0));
            let v = g.as_raw()[y * w as usize + x] as f64;
            // Keep solid black ink black even on already binary pages.
            let black = v < t.min(200.0) || v < 60.0;
            let c = if black { 0 } else { 255 };
            row[x * 4..x * 4 + 4].copy_from_slice(&[c, c, c, 255]);
        }
    });
    out
}

pub fn to_gray_rgba(img: &RgbaImage) -> RgbaImage {
    let g = gray(img);
    let mut out = RgbaImage::new(img.width(), img.height());
    out.as_mut().par_chunks_mut(4).zip(g.as_raw().par_iter()).for_each(|(o, v)| o.copy_from_slice(&[*v, *v, *v, 255]));
    out
}

/// Photo auto-levels: stretch each channel between its 0.5 % and 99.5 % percentiles.
pub fn auto_levels(img: &RgbaImage) -> RgbaImage {
    let mut lo = [0u8; 3];
    let mut hi = [255u8; 3];
    for k in 0..3 {
        let mut hist = [0u64; 256];
        for p in img.pixels() {
            hist[p[k] as usize] += 1;
        }
        let total: u64 = hist.iter().sum();
        let mut acc = 0;
        for (i, c) in hist.iter().enumerate() {
            acc += c;
            if acc * 200 >= total {
                lo[k] = i as u8;
                break;
            }
        }
        acc = 0;
        for (i, c) in hist.iter().enumerate().rev() {
            acc += c;
            if acc * 200 >= total {
                hi[k] = i as u8;
                break;
            }
        }
    }
    let mut out = img.clone();
    out.as_mut().par_chunks_mut(4).for_each(|p| {
        for k in 0..3 {
            let range = (hi[k] as f32 - lo[k] as f32).max(20.0);
            p[k] = ((p[k] as f32 - lo[k] as f32) / range * 255.0).clamp(0.0, 255.0) as u8;
        }
        // A touch more saturation.
        let l = (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0;
        for k in 0..3 {
            p[k] = (l + (p[k] as f32 - l) * 1.12).clamp(0.0, 255.0) as u8;
        }
    });
    out
}

/// Fraction of ink (dark pixels) inside the page, ignoring a 4 % border.
pub fn ink_coverage(img: &RgbaImage) -> f32 {
    let g = gray(img);
    let (small, _) = downscale_gray(&g, 800);
    let (w, h) = small.dimensions();
    let (mx, my) = (w / 25, h / 25);
    let mut dark = 0u64;
    let mut total = 0u64;
    for y in my..h.saturating_sub(my) {
        for x in mx..w.saturating_sub(mx) {
            total += 1;
            if small.get_pixel(x, y)[0] < 140 {
                dark += 1;
            }
        }
    }
    dark as f32 / total.max(1) as f32
}

pub fn is_blank(img: &RgbaImage) -> bool {
    ink_coverage(img) < 0.0025
}

/// Detects pages scanned sideways or upside down using the Windows OCR word yield.
pub fn upright_turns(img: &RgbaImage) -> u8 {
    let small = kit42::raster::fit_within(img, 1400);
    let mut best = (0usize, 0u8);
    for q in 0..4u8 {
        let r = rotate_quarter(&small, q);
        let n = kit42::winrt::ocr(&r, None)
            .map(|o| o.lines.iter().flat_map(|l| l.words.iter()).filter(|w| w.text.chars().filter(|c| c.is_alphabetic()).count() >= 3).count())
            .unwrap_or(0);
        if n > best.0 * 3 / 2 + 2 {
            best = (n, q);
        }
    }
    best.1
}

/// The full pipeline for one page.
pub fn process(original: &RgbaImage, o: &Opts, dpi: f32) -> RgbaImage {
    let mut img = rotate_quarter(original, o.rotation);
    if o.deskew {
        let a = skew_angle(&img);
        if a.abs() > 0.1 {
            // Fill with the border colour so page detection still sees the glass.
            let c = *img.get_pixel(0, 0);
            img = rotate_deg(&img, a, [c[0], c[1], c[2], 255]);
        }
    }
    if o.crop {
        if let Some((x0, y0, x1, y1)) = find_page(&img) {
            // Trim a hair inside so no glass shows at the edges.
            let m = ((x1 - x0).min(y1 - y0) / 150).max(2);
            img = image::imageops::crop_imm(&img, x0 + m, y0 + m, (x1 - x0).saturating_sub(2 * m).max(1), (y1 - y0).saturating_sub(2 * m).max(1)).to_image();
        }
    }
    if o.cleanup {
        img = whiten(&img);
    }
    if o.enhance {
        img = auto_levels(&img);
    }
    match o.color {
        ColorMode::Color => img,
        ColorMode::Gray => to_gray_rgba(&img),
        ColorMode::BlackWhite => binarize(&img, dpi),
    }
}

/// Prints a mark into the page with multiply blending, like ink on paper.
pub fn print_mark(page: &mut RgbaImage, mark: &RgbaImage, x: i64, y: i64, w: u32, h: u32, bw: bool) {
    let scaled = image::imageops::resize(mark, w.max(1), h.max(1), image::imageops::FilterType::Lanczos3);
    for (mx, my, p) in scaled.enumerate_pixels() {
        let (px, py) = (x + mx as i64, y + my as i64);
        if px < 0 || py < 0 || px >= page.width() as i64 || py >= page.height() as i64 {
            continue;
        }
        let a = p[3] as f32 / 255.0;
        if a <= 0.0 {
            continue;
        }
        let d = page.get_pixel_mut(px as u32, py as u32);
        for k in 0..3 {
            let m = 1.0 - a + a * p[k] as f32 / 255.0;
            d[k] = (d[k] as f32 * m).round() as u8;
        }
        if bw {
            let l = (d[0] as u32 + d[1] as u32 + d[2] as u32) / 3;
            let v = if l < 160 { 0 } else { 255 };
            *d = Rgba([v, v, v, 255]);
        }
    }
}

/// Packs a black/white RGBA page into a 1-bit-per-pixel friendly gray image.
pub fn to_luma(img: &RgbaImage) -> GrayImage {
    let mut g = GrayImage::new(img.width(), img.height());
    for (x, y, p) in img.enumerate_pixels() {
        g.put_pixel(x, y, Luma([p[0]]));
    }
    g
}

/// True when every pixel is pure black or white.
pub fn is_bilevel(img: &RgbaImage) -> bool {
    img.as_raw().par_chunks(4).all(|p| (p[0] == 0 || p[0] == 255) && p[0] == p[1] && p[1] == p[2])
}

/// Test inputs like the macOS `--make-test-inputs`: a skewed page on grey glass and feeder pages.
pub fn make_test_inputs(dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let font = kit42::fonts::load_bytes("Times New Roman", false, false).or_else(|| kit42::fonts::load_bytes("Arial", false, false)).unwrap_or_default();
    let mut page = RgbaImage::from_pixel(1240, 1754, Rgba([250, 246, 236, 255]));
    let lines = [
        "SCAN42 TEST PAGE — ТЕСТОВАЯ СТРАНИЦА",
        "Lorem ipsum dolor sit amet, consectetur adipiscing elit.",
        "Съешь же ещё этих мягких французских булок, да выпей чаю.",
        "The quick brown fox jumps over the lazy dog 0123456789.",
    ];
    for i in 0..34 {
        let l = lines[i % lines.len()];
        kit42::raster::draw_text(&mut page, &font, l, if i == 0 { 40.0 } else { 26.0 }, 110.0, 160.0 + i as f32 * 44.0, [25, 25, 30, 255]);
    }
    // Uneven light: a shadow on the left.
    for (x, _, p) in page.enumerate_pixels_mut() {
        let k = (x as f32 / 300.0).min(1.0) * 0.25 + 0.75;
        for c in 0..3 {
            p[c] = (p[c] as f32 * k) as u8;
        }
    }
    let skewed = rotate_deg(&page, 3.2, [0, 0, 0, 0]);
    let mut glass = RgbaImage::from_pixel(1500, 2000, Rgba([120, 122, 126, 255]));
    image::imageops::overlay(&mut glass, &skewed, 120, 110);
    glass.save(dir.join("glass-page.png")).map_err(|e| e.to_string())?;
    image::imageops::rotate180(&page).save(dir.join("feed-upside-down.png")).map_err(|e| e.to_string())?;
    let mut blank = RgbaImage::from_pixel(1240, 1754, Rgba([248, 248, 246, 255]));
    for (x, y, p) in blank.enumerate_pixels_mut() {
        if (x * 7 + y * 13) % 997 == 0 {
            *p = Rgba([200, 200, 200, 255]);
        }
    }
    blank.save(dir.join("feed-blank.png")).map_err(|e| e.to_string())?;
    image::imageops::rotate90(&page).save(dir.join("feed-sideways.png")).map_err(|e| e.to_string())?;
    Ok(())
}
