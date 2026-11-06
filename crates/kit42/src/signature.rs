//! Signature and stamp library shared by PDF42, DOC42, XLS42, IMG42, PPT42 and SCN42.
//!
//! Items are stored as recipes in %APPDATA%\Suite42\Library, so every insertion renders a
//! fresh impression: a drawn or generated signature written again with slightly different
//! movement and ink, a stamp pressed again with a different angle, pad pressure and grain.

use crate::raster;
use crate::t;
use image::{Rgba, RgbaImage};
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ───────────────────────────── model ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pen {
    Ballpoint,
    Gel,
    Fountain,
    Felt,
}

impl Pen {
    pub const ALL: [Pen; 4] = [Pen::Ballpoint, Pen::Gel, Pen::Fountain, Pen::Felt];
    pub fn name(self) -> &'static str {
        match self {
            Pen::Ballpoint => t("Ballpoint", "Шариковая"),
            Pen::Gel => t("Gel", "Гелевая"),
            Pen::Fountain => t("Fountain", "Перьевая"),
            Pen::Felt => t("Felt-tip", "Фетровая"),
        }
    }
    fn width(self) -> f32 {
        match self {
            Pen::Ballpoint => 2.2,
            Pen::Gel => 3.0,
            Pen::Fountain => 3.4,
            Pen::Felt => 5.0,
        }
    }
}

/// A stroke point: position (px in a 900×300 box), time (s) and pressure.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
    pub t: f32,
    pub p: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SigForm {
    FullName,
    InitialSurname,
    Surname,
    InitialsScribble,
    CapitalScribble,
}

impl SigForm {
    pub const ALL: [SigForm; 5] =
        [SigForm::FullName, SigForm::InitialSurname, SigForm::Surname, SigForm::InitialsScribble, SigForm::CapitalScribble];
    pub fn name(self) -> &'static str {
        match self {
            SigForm::FullName => t("Full name", "Полное имя"),
            SigForm::InitialSurname => t("Initial and surname", "Инициал и фамилия"),
            SigForm::Surname => t("Surname", "Фамилия"),
            SigForm::InitialsScribble => t("Initials and a scribble", "Инициалы и росчерк"),
            SigForm::CapitalScribble => t("Capital and a scribble", "Заглавная и росчерк"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SigStyle {
    pub font: String,
    pub slant: f32,
    pub climb: f32,
    pub flourish: u8,
    pub seed: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StampShape {
    Round,
    Oval,
    Rect,
    RoundedRect,
    Square,
    Triangle,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StampRecipe {
    pub template: String,
    pub shape: StampShape,
    pub ring_text: String,
    pub inner_ring_text: String,
    pub center_lines: Vec<String>,
    pub date: String,
    pub color: [u8; 3],
    pub double_border: bool,
    pub star: bool,
    pub vertical: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MarkKind {
    /// A picture (imported scan or photo), background removed.
    Image { file: String },
    /// Strokes drawn on the pad.
    Drawn { strokes: Vec<Vec<Pt>>, pen: Pen, color: [u8; 3] },
    /// A signature written from a name.
    Generated { text: String, style: SigStyle, pen: Pen, color: [u8; 3] },
    Stamp(StampRecipe),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mark {
    pub id: String,
    pub name: String,
    pub kind: MarkKind,
}

impl Mark {
    pub fn is_stamp(&self) -> bool {
        matches!(self.kind, MarkKind::Stamp(_))
    }
}

pub fn library_dir() -> PathBuf {
    let d = crate::settings::data_dir().join("Library");
    std::fs::create_dir_all(&d).ok();
    d
}

#[derive(Default, Serialize, Deserialize)]
pub struct Library {
    pub items: Vec<Mark>,
}

impl Library {
    pub fn load() -> Self {
        std::fs::read(library_dir().join("library.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            let _ = crate::write_atomic(&library_dir().join("library.json"), &b);
        }
    }

    pub fn add(&mut self, name: String, kind: MarkKind) {
        let id = format!("{:x}", rand::random::<u64>());
        self.items.push(Mark { id, name, kind });
        self.save();
    }

    pub fn remove(&mut self, id: &str) {
        if let Some(pos) = self.items.iter().position(|m| m.id == id) {
            let m = self.items.remove(pos);
            if let MarkKind::Image { file } = m.kind {
                std::fs::remove_file(library_dir().join(file)).ok();
            }
            self.save();
        }
    }
}

// ───────────────────────────── rendering ─────────────────────────────

/// Renders a fresh impression of a mark (random variation each call).
pub fn render(mark: &Mark) -> Option<RgbaImage> {
    let seed = rand::random::<u64>();
    render_seeded(mark, seed)
}

pub fn render_seeded(mark: &Mark, seed: u64) -> Option<RgbaImage> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let img = match &mark.kind {
        MarkKind::Image { file } => {
            let img = image::open(library_dir().join(file)).ok()?.to_rgba8();
            // Image stamps also vary slightly: angle and ink density.
            let angle = rng.gen_range(-1.5f32..1.5).to_radians();
            let rotated = rotate(&img, angle);
            let mut out = rotated;
            for p in out.pixels_mut() {
                p[3] = (p[3] as f32 * rng.gen_range(0.9..1.0)) as u8;
            }
            out
        }
        MarkKind::Drawn { strokes, pen, color } => render_strokes(strokes, *pen, *color, &mut rng, true),
        MarkKind::Generated { text, style, pen, color } => render_generated(text, style, *pen, *color, &mut rng),
        MarkKind::Stamp(r) => render_stamp(r, &mut rng),
    };
    Some(raster::trim_transparent(&img))
}

fn rotate(img: &RgbaImage, angle: f32) -> RgbaImage {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (s, c) = angle.sin_cos();
    let nw = (w * c.abs() + h * s.abs()).ceil() as u32 + 2;
    let nh = (w * s.abs() + h * c.abs()).ceil() as u32 + 2;
    let mut out = RgbaImage::new(nw, nh);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (ncx, ncy) = (nw as f32 / 2.0, nh as f32 / 2.0);
    for y in 0..nh {
        for x in 0..nw {
            let dx = x as f32 + 0.5 - ncx;
            let dy = y as f32 + 0.5 - ncy;
            let sx = dx * c + dy * s + cx - 0.5;
            let sy = -dx * s + dy * c + cy - 0.5;
            if sx >= 0.0 && sy >= 0.0 && sx < w - 1.0 && sy < h - 1.0 {
                out.put_pixel(x, y, bilinear(img, sx, sy));
            }
        }
    }
    out
}

fn bilinear(img: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let p = |xx: u32, yy: u32| img.get_pixel(xx.min(img.width() - 1), yy.min(img.height() - 1));
    let (a, b, c, d) = (p(x0, y0), p(x0 + 1, y0), p(x0, y0 + 1), p(x0 + 1, y0 + 1));
    let mut out = [0u8; 4];
    // Interpolate premultiplied colours.
    let mut alpha = 0.0;
    let mut col = [0.0f32; 3];
    for (q, wt) in [(a, (1.0 - fx) * (1.0 - fy)), (b, fx * (1.0 - fy)), (c, (1.0 - fx) * fy), (d, fx * fy)] {
        let qa = q[3] as f32 / 255.0 * wt;
        alpha += qa;
        for i in 0..3 {
            col[i] += q[i] as f32 * qa;
        }
    }
    if alpha > 0.0 {
        for i in 0..3 {
            out[i] = (col[i] / alpha).round().min(255.0) as u8;
        }
    }
    out[3] = (alpha * 255.0).round().min(255.0) as u8;
    Rgba(out)
}

/// Cheap value noise in 0..1.
fn noise(seed: u64, x: f32, y: f32, scale: f32) -> f32 {
    let hash = |ix: i64, iy: i64| {
        let mut h = (ix as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ seed;
        h ^= h >> 31;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 29;
        (h & 0xFFFF) as f32 / 65535.0
    };
    let (fx, fy) = (x / scale, y / scale);
    let (ix, iy) = (fx.floor() as i64, fy.floor() as i64);
    let (tx, ty) = (fx - ix as f32, fy - iy as f32);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let a = hash(ix, iy) * (1.0 - sx) + hash(ix + 1, iy) * sx;
    let b = hash(ix, iy + 1) * (1.0 - sx) + hash(ix + 1, iy + 1) * sx;
    a * (1.0 - sy) + b * sy
}

/// Paints a variable-width stroke into a coverage buffer.
fn stamp_disc(buf: &mut [f32], w: i32, h: i32, x: f32, y: f32, r: f32, strength: f32) {
    let r = r.max(0.35);
    let (x0, x1) = ((x - r - 1.0).floor() as i32, (x + r + 1.0).ceil() as i32);
    let (y0, y1) = ((y - r - 1.0).floor() as i32, (y + r + 1.0).ceil() as i32);
    for py in y0.max(0)..=y1.min(h - 1) {
        for px in x0.max(0)..=x1.min(w - 1) {
            let d = ((px as f32 + 0.5 - x).powi(2) + (py as f32 + 0.5 - y).powi(2)).sqrt();
            let cov = (r - d + 0.5).clamp(0.0, 1.0) * strength;
            let i = (py * w + px) as usize;
            buf[i] = buf[i].max(cov);
        }
    }
}

/// Converts a coverage buffer into ink with pen-specific texture.
fn ink(buf: &[f32], w: u32, h: u32, color: [u8; 3], pen: Pen, seed: u64) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = buf[(y * w + x) as usize];
            if c <= 0.0 {
                continue;
            }
            let fibre = noise(seed, x as f32, y as f32, 1.3);
            let density = match pen {
                Pen::Ballpoint => 0.78 + 0.22 * fibre,
                Pen::Gel => 0.9 + 0.1 * fibre,
                Pen::Fountain => 0.82 + 0.18 * noise(seed ^ 7, x as f32, y as f32, 6.0),
                Pen::Felt => 0.85 + 0.15 * noise(seed ^ 3, x as f32, y as f32, 3.0),
            };
            let a = (c * density).clamp(0.0, 1.0);
            img.put_pixel(x, y, Rgba([color[0], color[1], color[2], (a * 255.0) as u8]));
        }
    }
    img
}

/// Renders pad strokes: width follows speed (slow = wider), with small per-copy variation.
pub fn render_strokes(strokes: &[Vec<Pt>], pen: Pen, color: [u8; 3], rng: &mut impl Rng, vary: bool) -> RgbaImage {
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in strokes {
        for p in s {
            minx = minx.min(p.x);
            miny = miny.min(p.y);
            maxx = maxx.max(p.x);
            maxy = maxy.max(p.y);
        }
    }
    if minx > maxx {
        return RgbaImage::new(1, 1);
    }
    let scale = 2.0;
    let pad = 12.0;
    let w = ((maxx - minx) * scale + pad * 2.0).ceil() as u32;
    let h = ((maxy - miny) * scale + pad * 2.0).ceil() as u32;
    let mut buf = vec![0f32; (w * h) as usize];
    let seed = rng.gen::<u64>();
    // Copy variation: slight slant, scale and baseline wobble.
    let shear = if vary { rng.gen_range(-0.05f32..0.05) } else { 0.0 };
    let sx = if vary { rng.gen_range(0.96f32..1.04) } else { 1.0 };
    let wobble = if vary { rng.gen_range(0.0f32..2.0) } else { 0.0 };
    let base_w = pen.width() * scale * if vary { rng.gen_range(0.9f32..1.1) } else { 1.0 };
    for s in strokes {
        if s.is_empty() {
            continue;
        }
        let tx = |p: &Pt| {
            let x = (p.x - minx) * scale * sx + pad;
            let y = (p.y - miny) * scale + pad;
            let x = x + (y - h as f32 / 2.0) * shear;
            let y = y + (x / 60.0 + seed as f32 % 7.0).sin() * wobble;
            (x, y)
        };
        let n = s.len();
        for i in 0..n {
            let a = &s[i];
            let b = if i + 1 < n { &s[i + 1] } else { a };
            let (ax, ay) = tx(a);
            let (bx, by) = tx(b);
            let dist = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
            let dt = (b.t - a.t).max(0.004);
            let speed = dist / dt / scale;
            let speed_factor = (1.25 - (speed / 1800.0).min(0.6)).max(0.6);
            // Taper the first and last few points like a touch-down and lift.
            let taper = ((i.min(n - 1 - i) as f32 + 1.0) / 4.0).min(1.0);
            let steps = (dist / 0.6).ceil().max(1.0) as i32;
            for k in 0..steps {
                let tt = k as f32 / steps as f32;
                let x = ax + (bx - ax) * tt;
                let y = ay + (by - ay) * tt;
                let mut r = base_w * 0.5 * speed_factor * taper * (0.6 + 0.4 * a.p.clamp(0.2, 1.0) * 1.6).min(1.3);
                if pen == Pen::Fountain {
                    let angle = (by - ay).atan2(bx - ax);
                    r *= 0.45 + 0.9 * (angle - 0.6).sin().abs();
                }
                let strength = if pen == Pen::Ballpoint && speed > 1500.0 && noise(seed, x, y, 4.0) < 0.12 { 0.35 } else { 1.0 };
                stamp_disc(&mut buf, w as i32, h as i32, x, y, r, strength);
            }
        }
    }
    ink(&buf, w, h, color, pen, seed)
}

/// Script fonts installed with Windows that suit signatures.
pub fn signature_fonts() -> Vec<String> {
    let idx = crate::fonts::index();
    ["Segoe Script", "Ink Free", "Lucida Handwriting", "Segoe Print", "Gabriola", "Mistral", "Brush Script MT", "Freestyle Script", "Kristen ITC", "Monotype Corsiva"]
        .iter()
        .filter(|f| idx.contains_key(**f))
        .map(|s| s.to_string())
        .collect()
}

/// Text actually written for a signature form.
pub fn signature_text(names: &str, form: SigForm) -> String {
    let parts: Vec<&str> = names.split_whitespace().collect();
    if parts.is_empty() {
        return String::new();
    }
    let surname = *parts.last().unwrap();
    let initial = |s: &str| s.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    match form {
        SigForm::FullName => parts.join(" "),
        SigForm::InitialSurname if parts.len() > 1 => format!("{}. {}", initial(parts[0]), surname),
        SigForm::InitialSurname | SigForm::Surname => surname.to_string(),
        SigForm::InitialsScribble => {
            let mut s: String = parts.iter().map(|p| initial(p)).collect();
            s.push('~');
            s
        }
        SigForm::CapitalScribble => format!("{}~", initial(surname)),
    }
}

/// Renders a name signature: the script font is written along a climbing, slanted baseline,
/// letters shrink towards the end, then a flourish; '~' means a scribble.
pub fn render_generated(text: &str, style: &SigStyle, pen: Pen, color: [u8; 3], rng: &mut impl Rng) -> RgbaImage {
    let font = crate::fonts::load_bytes(&style.font, false, false).unwrap_or_default();
    let px = 120.0;
    let scribble = text.ends_with('~');
    let body = text.trim_end_matches('~');
    let chars: Vec<char> = body.chars().collect();
    let mut sizes = Vec::new();
    let mut widths = 0.0;
    for (i, c) in chars.iter().enumerate() {
        let shrink = 1.0 - 0.25 * (i as f32 / chars.len().max(1) as f32);
        let jitter = rng.gen_range(0.95f32..1.05);
        let size = px * shrink * jitter * if i == 0 { 1.25 } else { 1.0 };
        let w = raster::measure(&font, &c.to_string(), size);
        sizes.push((size, w));
        widths += w * 0.92;
    }
    let scribble_w = if scribble { px * 2.2 } else { 0.0 };
    let w = (widths + scribble_w + px * 1.6) as u32;
    let h = (px * 2.6) as u32;
    let mut buf = vec![0f32; (w * h) as usize];
    let climb = style.climb.to_radians() + rng.gen_range(-0.01f32..0.01);
    let slant = style.slant + rng.gen_range(-0.03f32..0.03);
    let mut x = px * 0.4;
    let base_y = h as f32 * 0.66;
    let seed = rng.gen::<u64>();
    let thickness = match pen {
        Pen::Ballpoint => 0.0,
        Pen::Gel => 0.6,
        Pen::Fountain => 0.9,
        Pen::Felt => 1.8,
    };
    for (i, c) in chars.iter().enumerate() {
        let (size, cw) = sizes[i];
        let y = base_y - x * climb.tan() + rng.gen_range(-2.0f32..2.0);
        let mut cov = Vec::new();
        raster::rasterize_line(&font, &c.to_string(), size, x, y, |gx, gy, c| cov.push((gx, gy, c)));
        for (gx, gy, c) in cov {
            // Slant: shift by height above the baseline.
            let sxf = gx as f32 + (y - gy as f32) * slant;
            let r = 0.5 + thickness;
            if thickness > 0.0 && c > 0.5 {
                stamp_disc(&mut buf, w as i32, h as i32, sxf, gy as f32, r, c);
            } else {
                let xi = sxf.round() as i32;
                if xi >= 0 && gy >= 0 && xi < w as i32 && gy < h as i32 {
                    let k = (gy as u32 * w + xi as u32) as usize;
                    buf[k] = buf[k].max(c);
                }
            }
        }
        x += cw * 0.92;
    }
    let end_y = base_y - x * climb.tan();
    let lw = pen.width() * 1.1;
    if scribble {
        // A decaying zig-zag scribble.
        let mut px_ = x;
        let amp = px * 0.45;
        let turns = rng.gen_range(4..7);
        let mut pts = Vec::new();
        for k in 0..=turns * 12 {
            let tt = k as f32 / (turns * 12) as f32;
            let xx = px_ + tt * scribble_w;
            let yy = end_y - (tt * turns as f32 * std::f32::consts::PI * 2.0).sin() * amp * (1.0 - tt * 0.7)
                - xx * 0.0;
            pts.push((xx, yy));
        }
        draw_path(&mut buf, w, h, &pts, lw);
        px_ += scribble_w;
        x = px_;
    }
    match style.flourish % 4 {
        1 => {
            // Underline with a hook.
            let y0 = end_y + px * 0.18;
            let pts: Vec<(f32, f32)> = (0..=40)
                .map(|k| {
                    let tt = k as f32 / 40.0;
                    let xx = px * 0.3 + tt * (x - px * 0.1);
                    let yy = y0 + (tt * 3.0).sin() * 6.0 + (base_y - end_y) * (1.0 - tt) * 0.9;
                    (xx, yy)
                })
                .collect();
            draw_path(&mut buf, w, h, &pts, lw * 0.9);
        }
        2 => {
            // A loop back through the capital.
            let cx = px * 0.7;
            let pts: Vec<(f32, f32)> = (0..=60)
                .map(|k| {
                    let tt = k as f32 / 60.0;
                    let a = tt * std::f32::consts::PI * 1.6;
                    (cx + (x - cx) * tt, base_y + px * 0.25 - a.sin() * px * 0.35 * (1.0 - tt))
                })
                .collect();
            draw_path(&mut buf, w, h, &pts, lw * 0.85);
        }
        3 => {
            // Strike through the foot of the capital.
            let pts = vec![(px * 0.2, base_y - px * 0.05), (px * 1.3, base_y - px * 0.18)];
            draw_path(&mut buf, w, h, &pts, lw);
        }
        _ => {}
    }
    let _ = seed;
    ink(&buf, w, h, color, pen, rng.gen())
}

fn draw_path(buf: &mut [f32], w: u32, h: u32, pts: &[(f32, f32)], width: f32) {
    let n = pts.len();
    for i in 0..n.saturating_sub(1) {
        let (ax, ay) = pts[i];
        let (bx, by) = pts[i + 1];
        let d = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let steps = (d / 0.6).ceil().max(1.0) as i32;
        let taper = ((i.min(n - 1 - i) as f32 + 1.0) / 6.0).min(1.0);
        for k in 0..steps {
            let tt = k as f32 / steps as f32;
            stamp_disc(buf, w as i32, h as i32, ax + (bx - ax) * tt, ay + (by - ay) * tt, width * 0.5 * (0.4 + 0.6 * taper), 1.0);
        }
    }
}

// ───────────────────────────── stamps ─────────────────────────────

/// Built-in stamp templates (generic designs only — no state emblems, notary, court or customs seals).
pub fn stamp_templates() -> Vec<(&'static str, StampRecipe)> {
    let today = String::new();
    let mk = |template: &str, shape, ring: &str, inner: &str, center: &[&str], color, double, star, vertical| StampRecipe {
        template: template.into(),
        shape,
        ring_text: ring.into(),
        inner_ring_text: inner.into(),
        center_lines: center.iter().map(|s| s.to_string()).collect(),
        date: today.clone(),
        color,
        double_border: double,
        star,
        vertical,
    };
    let blue = [32, 60, 170];
    let violet = [90, 40, 150];
    let red = [200, 30, 30];
    vec![
        (t("Company seal (round)", "Печать организации (круглая)"),
         mk("round", StampShape::Round, "ОБЩЕСТВО С ОГРАНИЧЕННОЙ ОТВЕТСТВЕННОСТЬЮ «РОМАШКА» * МОСКВА *", "ОГРН 1234567890123 * ИНН 7701234567", &["ДЛЯ", "ДОКУМЕНТОВ"], blue, true, false, false)),
        (t("Company seal with star", "Печать со звездой"),
         mk("round-star", StampShape::Round, "ACME TRADING LIMITED * LONDON *", "", &["COMPANY", "SEAL"], blue, true, true, false)),
        (t("Oval seal", "Овальная печать"),
         mk("oval", StampShape::Oval, "ИП ИВАНОВ ИВАН ИВАНОВИЧ * ОГРНИП 312345678900012 *", "", &["ИП"], violet, false, false, false)),
        (t("APPROVED", "УТВЕРЖДАЮ"),
         mk("approved", StampShape::Rect, "", "", &["УТВЕРЖДАЮ", "Генеральный директор", "____________ / Иванов И.И. /"], blue, false, false, false)),
        (t("PAID", "ОПЛАЧЕНО"),
         mk("paid", StampShape::RoundedRect, "", "", &["ОПЛАЧЕНО"], red, true, false, false)),
        (t("COPY CERTIFIED", "КОПИЯ ВЕРНА"),
         mk("copy", StampShape::Rect, "", "", &["КОПИЯ ВЕРНА", "Должность ____________", "Подпись ______ Дата ______"], blue, false, false, false)),
        (t("Received (date stamp)", "Входящий (штамп с датой)"),
         mk("received", StampShape::Rect, "", "", &["ПОЛУЧЕНО", "Вх. № ______"], violet, true, false, false)),
        (t("Date stamp (round)", "Датер (круглый)"),
         mk("dater", StampShape::Round, "ACCOUNTS DEPARTMENT * RECEIVED *", "", &[], blue, false, false, false)),
        (t("For documents", "Для документов"),
         mk("docs", StampShape::Triangle, "", "", &["ДЛЯ", "ДОКУМЕНТОВ"], blue, false, false, false)),
        (t("Personal name stamp", "Именной штамп"),
         mk("name", StampShape::Rect, "", "", &["Иванов Иван Иванович", "Главный бухгалтер"], blue, false, false, false)),
        (t("Japanese hanko (round)", "Японский ханко"),
         mk("hanko", StampShape::Round, "", "", &["山田"], red, false, false, true)),
        (t("Chinese company chop", "Китайская печать компании"),
         mk("chop", StampShape::Round, "上海示例贸易有限公司", "", &["合同专用章"], red, false, true, false)),
        (t("Korean square seal", "Корейская квадратная печать"),
         mk("dojang", StampShape::Square, "", "", &["金", "民"], red, true, false, false)),
        (t("URGENT", "СРОЧНО"),
         mk("urgent", StampShape::RoundedRect, "", "", &["СРОЧНО"], red, false, false, false)),
        (t("REJECTED", "ОТКЛОНЕНО"),
         mk("rejected", StampShape::Rect, "", "", &["ОТКЛОНЕНО"], red, true, false, false)),
        (t("Personal seal (Turkey)", "Личная печать (Турция)"),
         mk("kase", StampShape::Rect, "", "", &["Ahmet YILMAZ", "Muhasebe Müdürü"], blue, false, false, false)),
    ]
}

pub fn render_stamp(r: &StampRecipe, rng: &mut impl Rng) -> RgbaImage {
    let font = crate::fonts::load_bytes("Arial", true, false).unwrap_or_default();
    let cjk = crate::fonts::font_file("Microsoft YaHei", true, false)
        .and_then(|p| std::fs::read(p).ok())
        .filter(|_| r.center_lines.iter().chain([&r.ring_text]).any(|s| s.chars().any(|c| c as u32 > 0x2E80)));
    let font_for = |s: &str| -> Vec<u8> {
        if s.chars().any(|c| c as u32 > 0x2E80) {
            cjk.clone().unwrap_or_else(|| font.clone())
        } else {
            font.clone()
        }
    };
    let date = if r.date.is_empty() { chrono::Local::now().format("%d.%m.%Y").to_string() } else { r.date.clone() };
    let (w, h) = match r.shape {
        StampShape::Round | StampShape::Square => (520u32, 520u32),
        StampShape::Oval => (640, 440),
        StampShape::Rect | StampShape::RoundedRect => {
            let longest = r.center_lines.iter().map(|l| raster::measure(&font, l, 46.0)).fold(0.0f32, f32::max);
            ((longest + 120.0).max(420.0) as u32, (r.center_lines.len().max(1) as u32 * 64 + 70 + if r.template == "received" { 60 } else { 0 }))
        }
        StampShape::Triangle => (560, 500),
    };
    let mut cov = vec![0f32; (w * h) as usize];
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let line = 7.0;
    let mut ring = |rx: f32, ry: f32, width: f32, cov: &mut Vec<f32>| {
        let steps = ((rx + ry) * 4.0) as i32;
        for k in 0..steps {
            let a = k as f32 / steps as f32 * std::f32::consts::TAU;
            stamp_disc(cov, w as i32, h as i32, cx + rx * a.cos(), cy + ry * a.sin(), width / 2.0, 1.0);
        }
    };
    let mut text_at = |cov: &mut Vec<f32>, s: &str, px: f32, x: f32, baseline: f32| {
        let f = font_for(s);
        raster::rasterize_line(&f, s, px, x, baseline, |gx, gy, c| {
            if gx >= 0 && gy >= 0 && gx < w as i32 && gy < h as i32 {
                let i = (gy as u32 * w + gx as u32) as usize;
                cov[i] = cov[i].max(c);
            }
        });
    };
    match r.shape {
        StampShape::Round | StampShape::Oval => {
            let (rx, ry) = (w as f32 / 2.0 - 10.0, h as f32 / 2.0 - 10.0);
            ring(rx, ry, line, &mut cov);
            if r.double_border {
                ring(rx - 12.0, ry - 12.0, 3.0, &mut cov);
            }
            let inner_r = if r.ring_text.is_empty() { (rx, ry) } else { (rx * 0.68, ry * 0.66) };
            if !r.ring_text.is_empty() {
                ring(inner_r.0, inner_r.1, 4.0, &mut cov);
                text_on_ring(&mut cov, w, h, &font_for(&r.ring_text), &r.ring_text, (cx, cy), (rx - 22.0 - if r.double_border { 10.0 } else { 0.0 }, ry - 22.0 - if r.double_border { 10.0 } else { 0.0 }), 34.0);
            }
            if !r.inner_ring_text.is_empty() {
                text_on_ring(&mut cov, w, h, &font, &r.inner_ring_text, (cx, cy), (inner_r.0 - 18.0, inner_r.1 - 18.0), 24.0);
                ring(inner_r.0 - 46.0, inner_r.1 - 46.0, 3.0, &mut cov);
            }
            if r.star {
                star(&mut cov, w, h, cx, cy - if r.center_lines.is_empty() { 0.0 } else { ry * 0.3 }, ry * 0.16);
            }
            let lines: Vec<String> = if r.template == "dater" {
                vec![date.clone()]
            } else {
                r.center_lines.clone()
            };
            if r.vertical {
                // Vertical CJK name: one glyph per line.
                let glyphs: Vec<String> = lines.join("").chars().map(|c| c.to_string()).collect();
                let px = (h as f32 * 0.7) / glyphs.len().max(1) as f32;
                for (i, g) in glyphs.iter().enumerate() {
                    let f = font_for(g);
                    let gw = raster::measure(&f, g, px);
                    text_at(&mut cov, g, px, cx - gw / 2.0, cy - h as f32 * 0.35 + px * (i as f32 + 0.85));
                }
            } else {
                let px = if r.ring_text.is_empty() { 64.0 } else { 40.0 };
                let total = lines.len() as f32 * px * 1.15;
                for (i, l) in lines.iter().enumerate() {
                    let f = font_for(l);
                    let lw = raster::measure(&f, l, px).min(inner_r.0 * 1.8);
                    let size = if raster::measure(&f, l, px) > inner_r.0 * 1.6 { px * inner_r.0 * 1.6 / raster::measure(&f, l, px) } else { px };
                    let yoff = if r.star { ry * 0.12 } else { 0.0 };
                    text_at(&mut cov, l, size, cx - lw.min(raster::measure(&f, l, size)) / 2.0, cy - total / 2.0 + px * (i as f32 + 0.85) + yoff);
                }
            }
        }
        StampShape::Rect | StampShape::RoundedRect | StampShape::Square => {
            let m = 10.0;
            let rounded = r.shape == StampShape::RoundedRect;
            rect_outline(&mut cov, w, h, m, m, w as f32 - 2.0 * m, h as f32 - 2.0 * m, line, if rounded { 30.0 } else { 0.0 });
            if r.double_border {
                rect_outline(&mut cov, w, h, m + 12.0, m + 12.0, w as f32 - 2.0 * (m + 12.0), h as f32 - 2.0 * (m + 12.0), 3.0, if rounded { 22.0 } else { 0.0 });
            }
            let mut lines = r.center_lines.clone();
            if r.template == "received" {
                lines.push(date.clone());
            }
            let n = lines.len().max(1) as f32;
            for (i, l) in lines.iter().enumerate() {
                let f = font_for(l);
                let px = if i == 0 && r.shape != StampShape::Square { 52.0 } else if r.shape == StampShape::Square { h as f32 * 0.32 } else { 34.0 };
                let lw = raster::measure(&f, l, px);
                let avail = w as f32 - 60.0;
                let size = if lw > avail { px * avail / lw } else { px };
                let lw = raster::measure(&f, l, size);
                let y = 30.0 + (h as f32 - 60.0) * (i as f32 + 0.5) / n + size * 0.35;
                text_at(&mut cov, l, size, cx - lw / 2.0, y);
            }
        }
        StampShape::Triangle => {
            let pts = [(cx, 16.0), (w as f32 - 16.0, h as f32 - 16.0), (16.0, h as f32 - 16.0), (cx, 16.0)];
            draw_path(&mut cov, w, h, &pts, line);
            for (i, l) in r.center_lines.iter().enumerate() {
                let px = 44.0;
                let lw = raster::measure(&font, l, px);
                text_at(&mut cov, l, px, cx - lw / 2.0, h as f32 * 0.62 + i as f32 * px * 1.1);
            }
        }
    }
    // Impression: uneven pad pressure, grain, skips, optional faint double strike.
    let seed = rng.gen::<u64>();
    let pressure_dir = rng.gen_range(0.0f32..std::f32::consts::TAU);
    let mut img = RgbaImage::new(w, h);
    let shade = rng.gen_range(0.85f32..1.0);
    let color = [
        (r.color[0] as f32 * shade) as u8,
        (r.color[1] as f32 * shade) as u8,
        (r.color[2] as f32 * shade) as u8,
    ];
    for y in 0..h {
        for x in 0..w {
            let c = cov[(y * w + x) as usize];
            if c <= 0.0 {
                continue;
            }
            let (dx, dy) = ((x as f32 - cx) / w as f32, (y as f32 - cy) / h as f32);
            let pressure = 0.72 + 0.28 * (0.5 + (dx * pressure_dir.cos() + dy * pressure_dir.sin()) * 1.4).clamp(0.0, 1.0);
            let grain = 0.7 + 0.3 * noise(seed, x as f32, y as f32, 1.6);
            let skip = if noise(seed ^ 0xABCD, x as f32, y as f32, 22.0) < 0.16 { 0.35 } else { 1.0 };
            let a = (c * pressure * grain * skip).clamp(0.0, 1.0) * 0.92;
            img.put_pixel(x, y, Rgba([color[0], color[1], color[2], (a * 255.0) as u8]));
        }
    }
    if rng.gen_bool(0.15) {
        let off = rng.gen_range(2..5) as i64;
        let ghost = img.clone();
        for (x, y, p) in ghost.enumerate_pixels() {
            let nx = x as i64 + off;
            if nx < w as i64 && p[3] > 0 {
                let q = img.get_pixel_mut(nx as u32, y);
                if q[3] < p[3] / 4 {
                    *q = Rgba([p[0], p[1], p[2], p[3] / 4]);
                }
            }
        }
    }
    let angle = match r.shape {
        StampShape::Round | StampShape::Oval | StampShape::Triangle => rng.gen_range(2.0f32..8.0),
        _ => rng.gen_range(0.3f32..2.0),
    } * if rng.gen_bool(0.5) { 1.0 } else { -1.0 };
    let scale = rng.gen_range(0.97f32..1.03);
    let rotated = rotate(&img, angle.to_radians());
    image::imageops::resize(
        &rotated,
        (rotated.width() as f32 * scale) as u32,
        (rotated.height() as f32 * scale) as u32,
        image::imageops::FilterType::Triangle,
    )
}

fn rect_outline(cov: &mut [f32], w: u32, h: u32, x: f32, y: f32, rw: f32, rh: f32, lw: f32, radius: f32) {
    let r = radius.min(rw / 2.0).min(rh / 2.0);
    let mut pts = Vec::new();
    let corners = [(x + rw - r, y + r, -90.0f32), (x + rw - r, y + rh - r, 0.0), (x + r, y + rh - r, 90.0), (x + r, y + r, 180.0)];
    for (ccx, ccy, start) in corners {
        for k in 0..=8 {
            let a = (start + k as f32 * 90.0 / 8.0).to_radians();
            pts.push((ccx + r * a.cos(), ccy + r * a.sin()));
        }
    }
    pts.push(pts[0]);
    draw_path(cov, w, h, &pts, lw);
}

fn star(cov: &mut [f32], w: u32, h: u32, cx: f32, cy: f32, r: f32) {
    let mut pts = Vec::new();
    for k in 0..10 {
        let rr = if k % 2 == 0 { r } else { r * 0.42 };
        let a = (k as f32 * 36.0 - 90.0).to_radians();
        pts.push((cx + rr * a.cos(), cy + rr * a.sin()));
    }
    // Fill by scanline.
    let (miny, maxy) = (cy - r, cy + r);
    for yy in miny as i32..=maxy as i32 {
        let mut xs = Vec::new();
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            let yf = yy as f32 + 0.5;
            if (a.1 <= yf && b.1 > yf) || (b.1 <= yf && a.1 > yf) {
                xs.push(a.0 + (yf - a.1) / (b.1 - a.1) * (b.0 - a.0));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for pair in xs.chunks(2) {
            if pair.len() == 2 {
                for xx in pair[0] as i32..=pair[1] as i32 {
                    if xx >= 0 && yy >= 0 && xx < w as i32 && yy < h as i32 {
                        cov[(yy as u32 * w + xx as u32) as usize] = 1.0;
                    }
                }
            }
        }
    }
}

fn text_on_ring(cov: &mut [f32], w: u32, h: u32, font: &[u8], text: &str, c: (f32, f32), r: (f32, f32), px: f32) {
    let chars: Vec<char> = text.chars().collect();
    let widths: Vec<f32> = chars.iter().map(|ch| raster::measure(font, &ch.to_string(), px)).collect();
    let total: f32 = widths.iter().sum();
    let circumference = std::f32::consts::TAU * (r.0 + r.1) / 2.0;
    let size = if total > circumference * 0.97 { px * circumference * 0.97 / total } else { px };
    let scale = size / px;
    let spacing = (circumference - total * scale) / chars.len().max(1) as f32;
    let mut angle = -std::f32::consts::FRAC_PI_2 - (total * scale + spacing * chars.len() as f32) / circumference * std::f32::consts::PI;
    for (i, ch) in chars.iter().enumerate() {
        let cw = widths[i] * scale + spacing;
        let mid = angle + cw / 2.0 / ((r.0 + r.1) / 2.0);
        let (x, y) = (c.0 + r.0 * mid.cos(), c.1 + r.1 * mid.sin());
        let rot = mid + std::f32::consts::FRAC_PI_2;
        // Start of glyph so that its centre sits on the ring, baseline facing the centre.
        let hw = widths[i] * scale / 2.0;
        let ox = x - hw * rot.cos();
        let oy = y - hw * rot.sin();
        raster::rasterize_line_rotated(font, &ch.to_string(), size, (ox, oy), rot, |gx, gy, cv| {
            if gx >= 0 && gy >= 0 && gx < w as i32 && gy < h as i32 {
                let k = (gy as u32 * w + gx as u32) as usize;
                cov[k] = cov[k].max(cv);
            }
        });
        angle += cw / ((r.0 + r.1) / 2.0);
    }
}

// ───────────────────────────── picker UI ─────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Library,
    Draw,
    Create,
    Stamp,
}

/// The "Signatures and Stamps" window. Call `show` every frame; it returns an image to insert.
pub struct MarkPicker {
    pub open: bool,
    tab: Tab,
    lib: Library,
    thumbs: std::collections::HashMap<String, egui::TextureHandle>,
    strokes: Vec<Vec<Pt>>,
    current: Vec<Pt>,
    stroke_start: Option<std::time::Instant>,
    pen: Pen,
    ink: [u8; 3],
    names: String,
    form: SigForm,
    variants: Vec<SigStyle>,
    variant_tex: Vec<Option<egui::TextureHandle>>,
    chosen_variant: usize,
    stamp_idx: usize,
    stamp: StampRecipe,
    stamp_tex: Option<egui::TextureHandle>,
    stamp_dirty: bool,
    new_name: String,
    /// Restrict to stamps or signatures (SCN42 accepts both).
    pub selected: Option<String>,
}

impl Default for MarkPicker {
    fn default() -> Self {
        let templates = stamp_templates();
        Self {
            open: false,
            tab: Tab::Library,
            lib: Library::load(),
            thumbs: Default::default(),
            strokes: vec![],
            current: vec![],
            stroke_start: None,
            pen: Pen::Ballpoint,
            ink: [20, 40, 140],
            names: String::new(),
            form: SigForm::InitialSurname,
            variants: vec![],
            variant_tex: vec![],
            chosen_variant: 0,
            stamp_idx: 0,
            stamp: templates[0].1.clone(),
            stamp_tex: None,
            stamp_dirty: true,
            new_name: String::new(),
            selected: None,
        }
    }
}

impl MarkPicker {
    fn new_variants(&mut self) {
        let fonts = signature_fonts();
        let mut rng = rand::thread_rng();
        self.variants = (0..3)
            .map(|_| SigStyle {
                font: if fonts.is_empty() { "Segoe UI".into() } else { fonts[rng.gen_range(0..fonts.len())].clone() },
                slant: rng.gen_range(0.05..0.3),
                climb: if rng.gen_bool(0.12) { 0.0 } else { rng.gen_range(2.0..8.0) },
                flourish: rng.gen_range(0..4),
                seed: rng.gen(),
            })
            .collect();
        self.variant_tex = vec![None, None, None];
        self.chosen_variant = 0;
    }

    fn thumb(&mut self, ctx: &egui::Context, m: &Mark) -> Option<egui::TextureHandle> {
        if let Some(t) = self.thumbs.get(&m.id) {
            return Some(t.clone());
        }
        let img = render_seeded(m, 42)?;
        let img = raster::fit_within(&img, 260);
        let tex = crate::ui::texture(ctx, &m.id, &img);
        self.thumbs.insert(m.id.clone(), tex.clone());
        Some(tex)
    }

    /// Shows the window; returns a freshly rendered impression when the user inserts one.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<RgbaImage> {
        if !self.open {
            return None;
        }
        let mut result = None;
        let mut open = self.open;
        egui::Window::new(t("Signatures and Stamps", "Подписи и печати"))
            .open(&mut open)
            .default_size([620.0, 460.0])
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Library, t("Library", "Библиотека"));
                    ui.selectable_value(&mut self.tab, Tab::Draw, t("My Signature", "Моя подпись"));
                    ui.selectable_value(&mut self.tab, Tab::Create, t("Create Signature", "Создать подпись"));
                    ui.selectable_value(&mut self.tab, Tab::Stamp, t("Create Stamp", "Создать печать"));
                });
                ui.separator();
                match self.tab {
                    Tab::Library => result = self.library_ui(ui),
                    Tab::Draw => self.draw_ui(ui),
                    Tab::Create => self.create_ui(ui),
                    Tab::Stamp => self.stamp_ui(ui),
                }
            });
        self.open = open && result.is_none();
        result
    }

    fn library_ui(&mut self, ui: &mut egui::Ui) -> Option<RgbaImage> {
        let mut result = None;
        ui.horizontal(|ui| {
            if ui.button(format!("{}  {}", crate::icons::PICTURE, t("Add from file…", "Добавить из файла…"))).clicked() {
                if let Some(p) = crate::ui::open_dialog(
                    t("Add signature or stamp", "Добавить подпись или печать"),
                    &[(t("Images", "Изображения"), &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp", "gif"])],
                )
                .pick_file()
                {
                    match image::open(&p) {
                        Ok(img) => {
                            let mut img = img.to_rgba8();
                            raster::remove_white_background(&mut img);
                            let img = raster::trim_transparent(&raster::fit_within(&img, 1600));
                            let file = format!("{:x}.png", rand::random::<u64>());
                            if img.save(library_dir().join(&file)).is_ok() {
                                let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                                self.lib.add(name, MarkKind::Image { file });
                            }
                        }
                        Err(e) => crate::ui::error("Suite42", &e.to_string()),
                    }
                }
            }
        });
        ui.add_space(4.0);
        if self.lib.items.is_empty() {
            ui.label(t(
                "The library is empty. Draw your signature, create one from a name, create a stamp or add a scan.",
                "Библиотека пуста. Нарисуйте подпись, создайте её по имени, создайте печать или добавьте скан.",
            ));
        }
        let items = self.lib.items.clone();
        let mut remove = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for m in &items {
                    let tex = self.thumb(ui.ctx(), m);
                    let selected = self.selected.as_deref() == Some(&m.id);
                    let frame = egui::Frame::group(ui.style()).fill(if selected {
                        ui.visuals().selection.bg_fill
                    } else {
                        egui::Color32::WHITE
                    });
                    let r = frame
                        .show(ui, |ui| {
                            ui.set_width(170.0);
                            ui.set_height(120.0);
                            ui.vertical_centered(|ui| {
                                if let Some(tex) = &tex {
                                    let s = tex.size_vec2();
                                    let k = (160.0 / s.x).min(80.0 / s.y).min(1.0);
                                    ui.image((tex.id(), s * k));
                                }
                                ui.label(egui::RichText::new(&m.name).small().color(egui::Color32::DARK_GRAY));
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if r.clicked() {
                        self.selected = Some(m.id.clone());
                    }
                    if r.double_clicked() {
                        result = render(m);
                    }
                    r.context_menu(|ui| {
                        if ui.button(t("Insert", "Вставить")).clicked() {
                            result = render(m);
                            ui.close_menu();
                        }
                        if ui.button(t("Delete", "Удалить")).clicked() {
                            remove = Some(m.id.clone());
                            ui.close_menu();
                        }
                    });
                }
            });
        });
        if let Some(id) = remove {
            self.lib.remove(&id);
            self.thumbs.remove(&id);
        }
        ui.separator();
        ui.horizontal(|ui| {
            let sel = self.selected.clone().and_then(|id| self.lib.items.iter().find(|m| m.id == id).cloned());
            if ui.add_enabled(sel.is_some(), egui::Button::new(t("Insert", "Вставить"))).clicked() {
                result = sel.and_then(|m| render(&m));
            }
            ui.label(egui::RichText::new(t("Each insertion is a new impression.", "Каждая вставка — новый оттиск.")).small());
        });
        result
    }

    fn pen_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(t("Pen:", "Ручка:"));
            for p in Pen::ALL {
                ui.selectable_value(&mut self.pen, p, p.name());
            }
            ui.label(t("Ink:", "Чернила:"));
            for (c, tip) in [([20u8, 40, 140], t("Blue", "Синие")), ([25, 25, 30], t("Black", "Чёрные")), ([90, 40, 150], t("Violet", "Фиолетовые"))] {
                let (rect, r) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                ui.painter().circle_filled(rect.center(), 8.0, crate::ui::c32(c));
                if self.ink == c {
                    ui.painter().circle_stroke(rect.center(), 10.0, egui::Stroke::new(2.0, crate::ui::accent(ui.ctx())));
                }
                if r.on_hover_text(tip).clicked() {
                    self.ink = c;
                }
            }
        });
    }

    fn draw_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(t(
            "Sign in the box with the mouse, pen or touch. Sign a few times for natural variation.",
            "Распишитесь в поле мышью, пером или пальцем.",
        ));
        self.pen_row(ui);
        let size = egui::vec2(ui.available_width().min(900.0), 240.0);
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6.0, egui::Color32::WHITE);
        painter.line_segment(
            [rect.left_bottom() + egui::vec2(30.0, -50.0), rect.right_bottom() + egui::vec2(-30.0, -50.0)],
            egui::Stroke::new(1.0, egui::Color32::from_gray(200)),
        );
        let to_screen = |p: &Pt| rect.min + egui::vec2(p.x, p.y);
        if resp.drag_started() {
            self.current.clear();
            if self.stroke_start.is_none() {
                self.stroke_start = Some(std::time::Instant::now());
            }
        }
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let tt = self.stroke_start.map(|s| s.elapsed().as_secs_f32()).unwrap_or(0.0);
                let pressure = ui.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Touch { force: Some(f), .. } => Some(*f),
                        _ => None,
                    })
                });
                self.current.push(Pt { x: pos.x - rect.min.x, y: pos.y - rect.min.y, t: tt, p: pressure.unwrap_or(0.6) });
            }
        }
        if resp.drag_stopped() && self.current.len() > 1 {
            self.strokes.push(std::mem::take(&mut self.current));
        }
        let stroke = egui::Stroke::new(2.0, crate::ui::c32(self.ink));
        for s in self.strokes.iter().chain(std::iter::once(&self.current)) {
            let pts: Vec<egui::Pos2> = s.iter().map(to_screen).collect();
            if pts.len() > 1 {
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
        ui.horizontal(|ui| {
            if ui.button(t("Clear", "Очистить")).clicked() {
                self.strokes.clear();
                self.current.clear();
                self.stroke_start = None;
            }
            ui.label(t("Name:", "Название:"));
            ui.text_edit_singleline(&mut self.new_name);
            if ui
                .add_enabled(!self.strokes.is_empty(), egui::Button::new(t("Add to Library", "Добавить в библиотеку")))
                .clicked()
            {
                let name = if self.new_name.trim().is_empty() { t("My signature", "Моя подпись").to_string() } else { self.new_name.clone() };
                self.lib.add(name, MarkKind::Drawn { strokes: std::mem::take(&mut self.strokes), pen: self.pen, color: self.ink });
                self.stroke_start = None;
                self.tab = Tab::Library;
            }
        });
    }

    fn create_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(t("Name(s):", "Имя:"));
            ui.add(egui::TextEdit::singleline(&mut self.names).hint_text(t("Ivan Petrov", "Иван Петров")).desired_width(260.0));
        });
        ui.horizontal(|ui| {
            ui.label(t("Form:", "Форма:"));
            egui::ComboBox::from_id_salt("sigform").selected_text(self.form.name()).show_ui(ui, |ui| {
                for f in SigForm::ALL {
                    if ui.selectable_value(&mut self.form, f, f.name()).changed() {
                        self.variant_tex = vec![None; self.variants.len()];
                    }
                }
            });
            if ui.button(format!("{}  {}", crate::icons::WAND, t("New Variants", "Новые варианты"))).clicked() || self.variants.is_empty() {
                self.new_variants();
            }
        });
        let before = (self.pen, self.ink);
        self.pen_row(ui);
        if before != (self.pen, self.ink) {
            self.variant_tex = vec![None; self.variants.len()];
        }
        let text = signature_text(&self.names, self.form);
        if text.trim_end_matches('~').is_empty() {
            ui.label(t("Type a name to see three styles.", "Введите имя, чтобы увидеть три варианта."));
            return;
        }
        let key = egui::Id::new(("sigtext", &text));
        if ui.ctx().data(|d| d.get_temp::<bool>(key)).is_none() {
            ui.ctx().data_mut(|d| d.insert_temp(key, true));
            self.variant_tex = vec![None; self.variants.len()];
        }
        ui.horizontal(|ui| {
            for i in 0..self.variants.len() {
                if self.variant_tex.get(i).map(|t| t.is_none()).unwrap_or(true) {
                    let mut rng = rand::rngs::StdRng::seed_from_u64(self.variants[i].seed);
                    let img = raster::trim_transparent(&render_generated(&text, &self.variants[i], self.pen, self.ink, &mut rng));
                    let img = raster::fit_within(&img, 400);
                    if self.variant_tex.len() <= i {
                        self.variant_tex.resize(i + 1, None);
                    }
                    self.variant_tex[i] = Some(crate::ui::texture(ui.ctx(), &format!("sigvar{i}"), &img));
                }
                let selected = self.chosen_variant == i;
                let r = egui::Frame::group(ui.style())
                    .fill(if selected { ui.visuals().selection.bg_fill } else { egui::Color32::WHITE })
                    .show(ui, |ui| {
                        ui.set_width(180.0);
                        ui.set_height(100.0);
                        if let Some(tex) = &self.variant_tex[i] {
                            let s = tex.size_vec2();
                            let k = (175.0 / s.x).min(90.0 / s.y);
                            ui.image((tex.id(), s * k));
                        }
                    })
                    .response
                    .interact(egui::Sense::click());
                if r.clicked() {
                    self.chosen_variant = i;
                }
            }
        });
        if ui.button(t("Add to Library", "Добавить в библиотеку")).clicked() {
            let style = self.variants[self.chosen_variant].clone();
            self.lib.add(text.trim_end_matches('~').to_string(), MarkKind::Generated { text, style, pen: self.pen, color: self.ink });
            self.tab = Tab::Library;
        }
    }

    fn stamp_ui(&mut self, ui: &mut egui::Ui) {
        let templates = stamp_templates();
        ui.horizontal(|ui| {
            ui.label(t("Style:", "Вид:"));
            egui::ComboBox::from_id_salt("stamptpl")
                .selected_text(templates[self.stamp_idx].0)
                .width(260.0)
                .show_ui(ui, |ui| {
                    for (i, (name, _)) in templates.iter().enumerate() {
                        if ui.selectable_label(self.stamp_idx == i, *name).clicked() {
                            self.stamp_idx = i;
                            self.stamp = templates[i].1.clone();
                            self.stamp_dirty = true;
                        }
                    }
                });
        });
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            let mut changed = false;
            if matches!(self.stamp.shape, StampShape::Round | StampShape::Oval) {
                ui.label(t("Text around the edge:", "Текст по кругу:"));
                changed |= ui.text_edit_multiline(&mut self.stamp.ring_text).changed();
                ui.label(t("Inner ring:", "Внутреннее кольцо:"));
                changed |= ui.text_edit_singleline(&mut self.stamp.inner_ring_text).changed();
            }
            ui.label(t("Centre lines:", "Строки в центре:"));
            let mut joined = self.stamp.center_lines.join("\n");
            if ui.text_edit_multiline(&mut joined).changed() {
                self.stamp.center_lines = joined.lines().map(|s| s.to_string()).collect();
                changed = true;
            }
            ui.horizontal(|ui| {
                ui.label(t("Date:", "Дата:"));
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut self.stamp.date).hint_text(t("today", "сегодня")).desired_width(100.0))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label(t("Ink:", "Краска:"));
                for c in [[32u8, 60, 170], [90, 40, 150], [25, 25, 30], [200, 30, 30], [20, 120, 60]] {
                    let (rect, r) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                    ui.painter().circle_filled(rect.center(), 8.0, crate::ui::c32(c));
                    if r.clicked() {
                        self.stamp.color = c;
                        changed = true;
                    }
                }
                changed |= crate::ui::color_button(ui, &mut self.stamp.color, t("Any colour", "Любой цвет"));
            });
            changed |= ui.checkbox(&mut self.stamp.double_border, t("Double border", "Двойная рамка")).changed();
            if changed {
                self.stamp_dirty = true;
            }
            let ui = &mut cols[1];
            if self.stamp_dirty {
                let mut rng = rand::rngs::StdRng::seed_from_u64(7);
                let img = raster::trim_transparent(&render_stamp(&self.stamp, &mut rng));
                let img = raster::fit_within(&img, 300);
                self.stamp_tex = Some(crate::ui::texture(ui.ctx(), "stamppreview", &img));
                self.stamp_dirty = false;
            }
            if let Some(tex) = &self.stamp_tex {
                let s = tex.size_vec2();
                let k = (260.0 / s.x).min(260.0 / s.y).min(1.0);
                ui.image((tex.id(), s * k));
            }
        });
        if ui.button(t("Add to Library", "Добавить в библиотеку")).clicked() {
            let name = templates[self.stamp_idx].0.to_string();
            self.lib.add(name, MarkKind::Stamp(self.stamp.clone()));
            self.tab = Tab::Library;
        }
    }
}
