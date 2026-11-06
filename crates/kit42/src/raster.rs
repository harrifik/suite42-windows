//! Software text and image helpers shared by stamps, slide export, image markup and scans.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};

/// Width of a single line of text in pixels.
pub fn measure(font: &[u8], text: &str, px: f32) -> f32 {
    let Ok(font) = FontRef::try_from_slice(font) else { return 0.0 };
    let sf = font.as_scaled(PxScale::from(px));
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Ascent of the font at a pixel size.
pub fn ascent(font: &[u8], px: f32) -> f32 {
    FontRef::try_from_slice(font)
        .map(|f| f.as_scaled(PxScale::from(px)).ascent())
        .unwrap_or(px * 0.8)
}

/// Calls `plot(x, y, coverage)` for every covered pixel of a text line whose baseline starts at (x, y).
pub fn rasterize_line(
    font: &[u8],
    text: &str,
    px: f32,
    x: f32,
    baseline: f32,
    mut plot: impl FnMut(i32, i32, f32),
) {
    let Ok(font) = FontRef::try_from_slice(font) else { return };
    let scale = PxScale::from(px);
    let sf = font.as_scaled(scale);
    let mut caret = x;
    let mut prev = None;
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(caret, baseline));
        caret += sf.h_advance(id);
        prev = Some(id);
        if let Some(outlined) = font.outline_glyph(glyph) {
            let b = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| plot(b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32, cov));
        }
    }
}

/// Calls `plot` for a line rotated by `angle` radians around its start point.
pub fn rasterize_line_rotated(
    font: &[u8],
    text: &str,
    px: f32,
    origin: (f32, f32),
    angle: f32,
    mut plot: impl FnMut(i32, i32, f32),
) {
    // Render upright into a scratch buffer, then rotate with bilinear sampling.
    let w = measure(font, text, px).ceil() as i32 + 4;
    let h = (px * 1.4).ceil() as i32 + 4;
    let asc = ascent(font, px);
    let mut buf = vec![0f32; (w * h) as usize];
    rasterize_line(font, text, px, 2.0, asc + 2.0, |x, y, c| {
        if x >= 0 && y >= 0 && x < w && y < h {
            let i = (y * w + x) as usize;
            buf[i] = (buf[i] + c).min(1.0);
        }
    });
    let (s, c) = angle.sin_cos();
    let corners = [(0.0, 0.0), (w as f32, 0.0), (0.0, h as f32), (w as f32, h as f32)];
    let mut minx = f32::MAX;
    let mut miny = f32::MAX;
    let mut maxx = f32::MIN;
    let mut maxy = f32::MIN;
    for (cx, cy) in corners {
        let cy = cy - asc - 2.0;
        let rx = origin.0 + cx * c - cy * s;
        let ry = origin.1 + cx * s + cy * c;
        minx = minx.min(rx);
        miny = miny.min(ry);
        maxx = maxx.max(rx);
        maxy = maxy.max(ry);
    }
    for py in miny.floor() as i32..=maxy.ceil() as i32 {
        for px_ in minx.floor() as i32..=maxx.ceil() as i32 {
            let dx = px_ as f32 + 0.5 - origin.0;
            let dy = py as f32 + 0.5 - origin.1;
            let ux = dx * c + dy * s;
            let uy = -dx * s + dy * c + asc + 2.0;
            let v = sample(&buf, w, h, ux - 0.5, uy - 0.5);
            if v > 0.003 {
                plot(px_, py, v);
            }
        }
    }
}

fn sample(buf: &[f32], w: i32, h: i32, x: f32, y: f32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let get = |xx: i32, yy: i32| {
        if xx < 0 || yy < 0 || xx >= w || yy >= h {
            0.0
        } else {
            buf[(yy * w + xx) as usize]
        }
    };
    let a = get(x0, y0) * (1.0 - fx) + get(x0 + 1, y0) * fx;
    let b = get(x0, y0 + 1) * (1.0 - fx) + get(x0 + 1, y0 + 1) * fx;
    a * (1.0 - fy) + b * fy
}

/// Source-over blend of one colour with coverage into an RGBA image.
#[inline]
pub fn blend(img: &mut RgbaImage, x: i32, y: i32, color: [u8; 4], coverage: f32) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let a = (color[3] as f32 / 255.0) * coverage.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let p = img.get_pixel_mut(x as u32, y as u32);
    let da = p[3] as f32 / 255.0;
    let out_a = a + da * (1.0 - a);
    if out_a <= 0.0 {
        return;
    }
    for i in 0..3 {
        let s = color[i] as f32;
        let d = p[i] as f32;
        p[i] = ((s * a + d * da * (1.0 - a)) / out_a).round() as u8;
    }
    p[3] = (out_a * 255.0).round() as u8;
}

pub fn draw_text(img: &mut RgbaImage, font: &[u8], text: &str, px: f32, x: f32, baseline: f32, color: [u8; 4]) {
    let mut pts = Vec::new();
    rasterize_line(font, text, px, x, baseline, |x, y, c| pts.push((x, y, c)));
    for (x, y, c) in pts {
        blend(img, x, y, color, c);
    }
}

/// Draws text centred horizontally at `cx` with its visual middle at `cy`.
pub fn draw_text_centered(img: &mut RgbaImage, font: &[u8], text: &str, px: f32, cx: f32, cy: f32, color: [u8; 4]) {
    let w = measure(font, text, px);
    let asc = ascent(font, px);
    draw_text(img, font, text, px, cx - w / 2.0, cy + asc * 0.36, color);
}

/// Word-wraps text to a pixel width.
pub fn wrap(font: &[u8], text: &str, px: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if measure(font, &candidate, px) > width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines
}

/// Converts an RGBA image into a tiny-skia pixmap (premultiplied).
pub fn to_pixmap(img: &RgbaImage) -> Option<tiny_skia::Pixmap> {
    let mut pm = tiny_skia::Pixmap::new(img.width(), img.height())?;
    for (dst, src) in pm.pixels_mut().iter_mut().zip(img.pixels()) {
        *dst = tiny_skia::ColorU8::from_rgba(src[0], src[1], src[2], src[3]).premultiply();
    }
    Some(pm)
}

/// Converts a tiny-skia pixmap back into straight-alpha RGBA.
pub fn from_pixmap(pm: &tiny_skia::Pixmap) -> RgbaImage {
    let mut img = RgbaImage::new(pm.width(), pm.height());
    for (dst, src) in img.pixels_mut().zip(pm.pixels()) {
        let c = src.demultiply();
        *dst = Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    img
}

/// Draws text into a pixmap at a baseline with straight RGBA colour.
pub fn pixmap_text(pm: &mut tiny_skia::Pixmap, font: &[u8], text: &str, px: f32, x: f32, baseline: f32, color: [u8; 4]) {
    let w = pm.width() as i32;
    let h = pm.height() as i32;
    let data = pm.data_mut();
    rasterize_line(font, text, px, x, baseline, |x, y, cov| {
        if x < 0 || y < 0 || x >= w || y >= h {
            return;
        }
        let i = ((y * w + x) * 4) as usize;
        let a = color[3] as f32 / 255.0 * cov.clamp(0.0, 1.0);
        for k in 0..3 {
            let s = color[k] as f32 * a;
            data[i + k] = (s + data[i + k] as f32 * (1.0 - a)).round().min(255.0) as u8;
        }
        data[i + 3] = (a * 255.0 + data[i + 3] as f32 * (1.0 - a)).round().min(255.0) as u8;
    });
}

/// Makes near-white pixels transparent (for signatures and stamps scanned on paper).
pub fn remove_white_background(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let lum = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
        let min = p[0].min(p[1]).min(p[2]) as f32;
        let v = lum.max(min);
        let alpha = if v > 235.0 {
            0.0
        } else if v > 170.0 {
            (235.0 - v) / 65.0
        } else {
            1.0
        };
        p[3] = (p[3] as f32 * alpha) as u8;
    }
}

/// Crops transparent margins.
pub fn trim_transparent(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] > 8 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x1 < x0 || y1 < y0 {
        return img.clone();
    }
    image::imageops::crop_imm(img, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image()
}

/// Converts an RgbaImage to an egui ColorImage.
pub fn to_color_image(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

/// Downscales so the longest side is at most `max` pixels.
pub fn fit_within(img: &RgbaImage, max: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w <= max && h <= max {
        return img.clone();
    }
    let s = max as f32 / w.max(h) as f32;
    image::imageops::resize(
        img,
        ((w as f32 * s).round() as u32).max(1),
        ((h as f32 * s).round() as u32).max(1),
        image::imageops::FilterType::Triangle,
    )
}
