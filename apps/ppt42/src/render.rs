//! Slide rendering: on screen with egui, to bitmaps with tiny-skia, and to vector PDF.

use crate::model::*;
use egui::{Color32, Pos2, Rect, Stroke, Vec2};
use image::RgbaImage;
use kit42::richtext::{list_numbers, Align2, CharFmt, Para};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub const INSET: (f32, f32) = (7.2, 3.6);

/// Decoded pictures, cached by their data pointer.
pub fn decoded(data: &Arc<Vec<u8>>) -> Option<Arc<RgbaImage>> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Arc<RgbaImage>>>> = OnceLock::new();
    let key = Arc::as_ptr(data) as usize;
    let cache = CACHE.get_or_init(Default::default);
    if let Some(i) = cache.lock().unwrap().get(&key) {
        return Some(i.clone());
    }
    let img = image::load_from_memory(data).ok().map(|i| Arc::new(kit42::raster::fit_within(&i.to_rgba8(), 3000)))?;
    cache.lock().unwrap().insert(key, img.clone());
    Some(img)
}

pub fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// A positioned piece of text in shape-local points.
pub struct Piece {
    pub text: String,
    pub fmt: CharFmt,
    pub x: f32,
    pub baseline: f32,
    pub width: f32,
}

/// Lays out paragraphs inside a box (shape-local points) using a measuring function.
pub fn layout_text(paras: &[Para], w: f32, h: f32, anchor: Anchor, measure: &mut dyn FnMut(&CharFmt, &str) -> f32) -> Vec<Piece> {
    let markers = list_numbers(paras);
    let inner_w = (w - 2.0 * INSET.0).max(4.0);
    let mut lines: Vec<(Vec<Piece>, f32, f32)> = vec![]; // pieces (x relative), line height, ascent
    for (pi, p) in paras.iter().enumerate() {
        let indent = p.list.as_ref().map(|l| 22.0 + l.level as f32 * 22.0).unwrap_or(0.0) + p.indent_left;
        let avail = (inner_w - indent).max(4.0);
        let base_size = p.runs.first().map(|r| r.fmt.size).unwrap_or(18.0);
        let mut cur: Vec<Piece> = vec![];
        let mut x = 0.0;
        let mut words: Vec<(String, CharFmt, bool)> = vec![];
        for r in &p.runs {
            let mut word = String::new();
            for ch in r.text.chars() {
                if ch == ' ' || ch == '\n' {
                    if !word.is_empty() {
                        words.push((std::mem::take(&mut word), r.fmt.clone(), false));
                    }
                    words.push((ch.to_string(), r.fmt.clone(), true));
                } else {
                    word.push(ch);
                }
            }
            if !word.is_empty() {
                words.push((word, r.fmt.clone(), false));
            }
        }
        let mut first_line = true;
        let flush = |cur: &mut Vec<Piece>, lines: &mut Vec<(Vec<Piece>, f32, f32)>, first: &mut bool| {
            while cur.last().map(|p| p.text == " ").unwrap_or(false) {
                cur.pop();
            }
            let size = cur.iter().map(|p| p.fmt.size).fold(base_size, f32::max);
            let mut line = std::mem::take(cur);
            let used: f32 = line.iter().map(|p| p.width).sum();
            let off = indent + match p.align {
                Align2::Center => (avail - used) / 2.0,
                Align2::Right => avail - used,
                _ => 0.0,
            };
            for piece in &mut line {
                piece.x += off;
            }
            if *first {
                if let Some(m) = &markers[pi] {
                    let f = p.runs.first().map(|r| r.fmt.clone()).unwrap_or_default();
                    line.insert(0, Piece { text: m.clone(), fmt: f, x: indent - 20.0, baseline: 0.0, width: 0.0 });
                }
            }
            *first = false;
            lines.push((line, size * 1.2 * p.line_spacing.max(0.5), size * 0.9));
        };
        for (text, fmt, is_space) in words {
            if text == "\n" {
                flush(&mut cur, &mut lines, &mut first_line);
                x = 0.0;
                continue;
            }
            let wd = measure(&fmt, &text);
            if !is_space && x + wd > avail && !cur.is_empty() {
                flush(&mut cur, &mut lines, &mut first_line);
                x = 0.0;
            }
            if is_space && cur.is_empty() && !first_line {
                continue;
            }
            cur.push(Piece { text, fmt, x, baseline: 0.0, width: wd });
            x += wd;
        }
        flush(&mut cur, &mut lines, &mut first_line);
        if let Some(last) = lines.last_mut() {
            last.1 += p.space_after.min(24.0);
        }
    }
    let total: f32 = lines.iter().map(|l| l.1).sum();
    let mut y = match anchor {
        Anchor::Top => INSET.1,
        Anchor::Middle => (h - total) / 2.0,
        Anchor::Bottom => h - INSET.1 - total,
    };
    let mut out = vec![];
    for (line, lh, asc) in lines {
        for mut piece in line {
            piece.x += INSET.0;
            piece.baseline = y + asc;
            out.push(piece);
        }
        y += lh;
    }
    out
}

fn rotate_pt(p: Pos2, c: Pos2, a: f32) -> Pos2 {
    let (s, co) = a.sin_cos();
    let d = p - c;
    c + Vec2::new(d.x * co - d.y * s, d.x * s + d.y * co)
}

/// Screen polygon for a shape outline (slide points → screen via `map`).
fn screen_outline(sh: &Shape, map: &dyn Fn(f32, f32) -> Pos2) -> Vec<Pos2> {
    let c = map(sh.x + sh.w / 2.0, sh.y + sh.h / 2.0);
    let a = sh.rot.to_radians();
    let geom = match &sh.content {
        Content::Shape(g) => *g,
        _ => Geom::Rect,
    };
    let unit: Vec<(f32, f32)> = match geom.outline() {
        Some(v) => v,
        None => (0..48).map(|k| {
            let t = k as f32 / 48.0 * std::f32::consts::TAU;
            (0.5 + 0.5 * t.cos(), 0.5 + 0.5 * t.sin())
        }).collect(),
    };
    unit.into_iter()
        .map(|(u, v)| {
            let u = if sh.flip_h { 1.0 - u } else { u };
            let v = if sh.flip_v { 1.0 - v } else { v };
            rotate_pt(map(sh.x + u * sh.w, sh.y + v * sh.h), c, a)
        })
        .collect()
}

fn fill_polygon(p: &egui::Painter, pts: &[Pos2], fill: Color32) {
    if pts.len() < 3 || fill == Color32::TRANSPARENT {
        return;
    }
    // Triangulate as a fan from the centroid (good for the star-shaped presets used here).
    let c = pts.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2()) / pts.len() as f32;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c.to_pos2(), fill);
    for q in pts {
        mesh.colored_vertex(*q, fill);
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    p.add(egui::Shape::mesh(mesh));
}

/// Draws one slide into `rect` (screen). `hide` hides shapes (animations not yet shown).
pub fn paint_slide(
    p: &egui::Painter,
    ctx: &egui::Context,
    rect: Rect,
    pres: &Presentation,
    slide: &Slide,
    texes: &mut HashMap<usize, egui::TextureHandle>,
    hide: &dyn Fn(&Shape) -> f32,
    skip_text: Option<u64>,
) {
    let s = rect.width() / pres.width;
    let map = |x: f32, y: f32| Pos2::new(rect.left() + x * s, rect.top() + y * s);
    match pres.background_of(slide) {
        Background::Solid(c) => {
            p.rect_filled(rect, 0.0, c32(c));
        }
        Background::Gradient(a, b) => {
            let mut mesh = egui::Mesh::default();
            mesh.colored_vertex(rect.left_top(), c32(a));
            mesh.colored_vertex(rect.right_top(), c32(a));
            mesh.colored_vertex(rect.right_bottom(), c32(b));
            mesh.colored_vertex(rect.left_bottom(), c32(b));
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            p.add(egui::Shape::mesh(mesh));
        }
        Background::Picture(d) => {
            p.rect_filled(rect, 0.0, Color32::WHITE);
            if let Some(tex) = picture_tex(ctx, texes, &d) {
                p.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
        }
    }
    for sh in &slide.shapes {
        let alpha = hide(sh);
        if alpha <= 0.0 {
            continue;
        }
        let tint = |c: Color32| if alpha >= 1.0 { c } else { c.gamma_multiply(alpha) };
        let center = map(sh.x + sh.w / 2.0, sh.y + sh.h / 2.0);
        let a = sh.rot.to_radians();
        match &sh.content {
            Content::Shape(g) if sh.is_line() => {
                let (x0, y0, x1, y1) = (sh.x, sh.y, sh.x + sh.w, sh.y + sh.h);
                let (x0, x1) = if sh.flip_h { (x1, x0) } else { (x0, x1) };
                let (y0, y1) = if sh.flip_v { (y1, y0) } else { (y0, y1) };
                let (pa, pb) = (rotate_pt(map(x0, y0), center, a), rotate_pt(map(x1, y1), center, a));
                let (col, w) = sh.line.unwrap_or((pres.theme.text, 2.0));
                p.line_segment([pa, pb], Stroke::new((w * s).max(1.0), tint(c32(col))));
                if *g == Geom::Arrow {
                    let dir = (pb - pa).normalized();
                    let len = (w * s * 4.0).max(8.0);
                    let n = Vec2::new(-dir.y, dir.x);
                    p.add(egui::Shape::convex_polygon(vec![pb, pb - dir * len + n * len * 0.45, pb - dir * len - n * len * 0.45], tint(c32(col)), Stroke::NONE));
                }
                continue;
            }
            Content::Shape(g) => {
                let pts = screen_outline(sh, &map);
                if let Some(f) = sh.fill {
                    if *g == Geom::RoundRect && a == 0.0 {
                        let r = Rect::from_min_size(map(sh.x, sh.y), Vec2::new(sh.w * s, sh.h * s));
                        p.rect_filled(r, (sh.w.min(sh.h) * s * 0.16) as u8, tint(c32(f)));
                    } else {
                        fill_polygon(p, &pts, tint(c32(f)));
                    }
                }
                if let Some((c, w)) = sh.line {
                    p.add(egui::Shape::closed_line(pts, Stroke::new((w * s).max(1.0), tint(c32(c)))));
                }
            }
            Content::Picture(d) => {
                if let Some(tex) = picture_tex(ctx, texes, d) {
                    let r = Rect::from_min_size(map(sh.x, sh.y), Vec2::new(sh.w * s, sh.h * s));
                    let uv = Rect::from_min_max(Pos2::new(if sh.flip_h { 1.0 } else { 0.0 }, if sh.flip_v { 1.0 } else { 0.0 }), Pos2::new(if sh.flip_h { 0.0 } else { 1.0 }, if sh.flip_v { 0.0 } else { 1.0 }));
                    if a == 0.0 {
                        p.image(tex.id(), r, uv, tint(Color32::WHITE));
                    } else {
                        let mut mesh = egui::Mesh::with_texture(tex.id());
                        let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].map(|q| rotate_pt(q, center, a));
                        let uvs = [uv.left_top(), uv.right_top(), uv.right_bottom(), uv.left_bottom()];
                        for i in 0..4 {
                            mesh.vertices.push(egui::epaint::Vertex { pos: corners[i], uv: uvs[i], color: tint(Color32::WHITE) });
                        }
                        mesh.add_triangle(0, 1, 2);
                        mesh.add_triangle(0, 2, 3);
                        p.add(egui::Shape::mesh(mesh));
                    }
                    if let Some((c, w)) = sh.line {
                        p.rect_stroke(r, 0.0, Stroke::new((w * s).max(1.0), c32(c)), egui::StrokeKind::Middle);
                    }
                }
            }
            Content::Table(rows) => {
                let n_r = rows.len().max(1);
                let n_c = rows.first().map(|r| r.len()).unwrap_or(1).max(1);
                let (cw, rh) = (sh.w / n_c as f32, sh.h / n_r as f32);
                for (ri, row) in rows.iter().enumerate() {
                    for (ci, cell) in row.iter().enumerate() {
                        let r = Rect::from_min_size(map(sh.x + ci as f32 * cw, sh.y + ri as f32 * rh), Vec2::new(cw * s, rh * s));
                        let bg = if ri == 0 { pres.theme.accents[0] } else if ri % 2 == 0 { [233, 237, 244] } else { [207, 213, 234] };
                        p.rect_filled(r, 0.0, tint(c32(bg)));
                        p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Inside);
                        if skip_text != Some(sh.id) {
                            draw_paras(p, ctx, std::slice::from_ref(cell), r, s, Anchor::Middle, 0.0, r.center(), alpha, if ri == 0 { Some([255, 255, 255]) } else { None });
                        }
                    }
                }
                continue;
            }
        }
        if !sh.text.is_empty() && skip_text != Some(sh.id) {
            let r = Rect::from_min_size(map(sh.x, sh.y), Vec2::new(sh.w * s, sh.h * s));
            draw_paras(p, ctx, &sh.text, r, s, sh.anchor, a, center, alpha, None);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_paras(p: &egui::Painter, ctx: &egui::Context, paras: &[Para], r: Rect, s: f32, anchor: Anchor, angle: f32, center: Pos2, alpha: f32, color_override: Option<[u8; 3]>) {
    let mut measure = |f: &CharFmt, t: &str| -> f32 {
        let fam = kit42::fonts::doc_family(ctx, &f.font, f.bold, f.italic);
        ctx.fonts(|fo| fo.layout_no_wrap(t.to_string(), egui::FontId::new((f.size * s).max(1.0), fam), Color32::BLACK).size().x) / s
    };
    let pieces = layout_text(paras, r.width() / s, r.height() / s, anchor, &mut measure);
    for pc in pieces {
        let f = &pc.fmt;
        let size = (f.size * s * if f.script != 0 { 0.65 } else { 1.0 }).max(1.0);
        let fam = kit42::fonts::doc_family(ctx, &f.font, f.bold, f.italic);
        let col = c32(color_override.unwrap_or(f.color));
        let col = if alpha >= 1.0 { col } else { col.gamma_multiply(alpha) };
        let fmt = egui::TextFormat {
            font_id: egui::FontId::new(size, fam),
            color: col,
            underline: if f.underline { Stroke::new((size / 14.0).max(1.0), col) } else { Stroke::NONE },
            strikethrough: if f.strike { Stroke::new((size / 14.0).max(1.0), col) } else { Stroke::NONE },
            italics: f.italic && !kit42::fonts::has_doc_family(&f.font, f.bold, true),
            ..Default::default()
        };
        let galley = ctx.fonts(|fo| fo.layout_job(egui::text::LayoutJob::single_section(pc.text.clone(), fmt)));
        let top_left = Pos2::new(r.left() + pc.x * s, r.top() + pc.baseline * s - size * 0.9);
        if angle == 0.0 {
            p.galley(top_left, galley, col);
        } else {
            let pos = rotate_pt(top_left, center, angle);
            p.add(egui::epaint::TextShape::new(pos, galley, col).with_angle(angle));
        }
    }
}

fn picture_tex(ctx: &egui::Context, texes: &mut HashMap<usize, egui::TextureHandle>, d: &Arc<Vec<u8>>) -> Option<egui::TextureHandle> {
    let key = Arc::as_ptr(d) as usize;
    if let Some(t) = texes.get(&key) {
        return Some(t.clone());
    }
    let img = decoded(d)?;
    let tex = kit42::ui::texture(ctx, &format!("pic{key}"), &kit42::raster::fit_within(&img, 2048));
    texes.insert(key, tex.clone());
    Some(tex)
}

// ───────────────────────── raster ─────────────────────────

fn font_bytes(f: &CharFmt) -> Arc<Vec<u8>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, bool, bool), Arc<Vec<u8>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (f.font.clone(), f.bold, f.italic);
    if let Some(b) = cache.lock().unwrap().get(&key) {
        return b.clone();
    }
    let b = Arc::new(kit42::fonts::load_bytes(&f.font, f.bold, f.italic).unwrap_or_default());
    cache.lock().unwrap().insert(key, b.clone());
    b
}

fn paint(c: [u8; 3], a: f32) -> tiny_skia::Paint<'static> {
    let mut p = tiny_skia::Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], (a * 255.0) as u8);
    p.anti_alias = true;
    p
}

/// Renders a slide to an image `width` pixels wide.
pub fn raster(pres: &Presentation, slide: &Slide, width: u32) -> RgbaImage {
    use tiny_skia::*;
    let s = width as f32 / pres.width;
    let height = (pres.height * s).round() as u32;
    let mut pm = Pixmap::new(width.max(1), height.max(1)).unwrap();
    match pres.background_of(slide) {
        Background::Solid(c) => pm.fill(Color::from_rgba8(c[0], c[1], c[2], 255)),
        Background::Gradient(a, b) => {
            if let Some(sh) = LinearGradient::new(Point::from_xy(0.0, 0.0), Point::from_xy(0.0, height as f32), vec![GradientStop::new(0.0, Color::from_rgba8(a[0], a[1], a[2], 255)), GradientStop::new(1.0, Color::from_rgba8(b[0], b[1], b[2], 255))], SpreadMode::Pad, Transform::identity()) {
                let mut p = Paint::default();
                p.shader = sh;
                pm.fill_rect(Rect::from_xywh(0.0, 0.0, width as f32, height as f32).unwrap(), &p, Transform::identity(), None);
            }
        }
        Background::Picture(d) => {
            pm.fill(Color::WHITE);
            if let Some(img) = decoded(&d) {
                let scaled = image::imageops::resize(&*img, width, height, image::imageops::FilterType::Triangle);
                if let Some(sp) = kit42::raster::to_pixmap(&scaled) {
                    pm.draw_pixmap(0, 0, sp.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
                }
            }
        }
    }
    for sh in &slide.shapes {
        let (lw, lh) = ((sh.w * s).ceil().max(1.0) as u32 + 2, (sh.h * s).ceil().max(1.0) as u32 + 2);
        if lw > 16384 || lh > 16384 {
            continue;
        }
        let Some(mut local) = Pixmap::new(lw, lh) else { continue };
        let (w, h) = (sh.w * s, sh.h * s);
        match &sh.content {
            Content::Shape(g) if sh.is_line() => {
                let mut pb = PathBuilder::new();
                pb.move_to(1.0, 1.0);
                pb.line_to(1.0 + w, 1.0 + h);
                let (c, lwid) = sh.line.unwrap_or((pres.theme.text, 2.0));
                if let Some(path) = pb.finish() {
                    local.stroke_path(&path, &paint(c, 1.0), &tiny_skia::Stroke { width: lwid * s, ..Default::default() }, Transform::identity(), None);
                }
                if *g == Geom::Arrow {
                    let len = (lwid * s * 4.0).max(8.0);
                    let d = (w * w + h * h).sqrt().max(0.01);
                    let (ux, uy) = (w / d, h / d);
                    let mut pb = PathBuilder::new();
                    pb.move_to(1.0 + w, 1.0 + h);
                    pb.line_to(1.0 + w - ux * len - uy * len * 0.45, 1.0 + h - uy * len + ux * len * 0.45);
                    pb.line_to(1.0 + w - ux * len + uy * len * 0.45, 1.0 + h - uy * len - ux * len * 0.45);
                    pb.close();
                    if let Some(path) = pb.finish() {
                        local.fill_path(&path, &paint(c, 1.0), FillRule::Winding, Transform::identity(), None);
                    }
                }
            }
            Content::Shape(g) => {
                let path = match g.outline() {
                    None => Rect::from_xywh(1.0, 1.0, w, h).and_then(PathBuilder::from_oval),
                    Some(pts) => {
                        let mut pb = PathBuilder::new();
                        for (i, (u, v)) in pts.iter().enumerate() {
                            let (x, y) = (1.0 + u * w, 1.0 + v * h);
                            if i == 0 {
                                pb.move_to(x, y);
                            } else {
                                pb.line_to(x, y);
                            }
                        }
                        pb.close();
                        pb.finish()
                    }
                };
                if let Some(path) = path {
                    if let Some(f) = sh.fill {
                        local.fill_path(&path, &paint(f, 1.0), FillRule::Winding, Transform::identity(), None);
                    }
                    if let Some((c, lwid)) = sh.line {
                        local.stroke_path(&path, &paint(c, 1.0), &tiny_skia::Stroke { width: lwid * s, ..Default::default() }, Transform::identity(), None);
                    }
                }
            }
            Content::Picture(d) => {
                if let Some(img) = decoded(d) {
                    let scaled = image::imageops::resize(&*img, (w as u32).max(1), (h as u32).max(1), image::imageops::FilterType::Lanczos3);
                    if let Some(sp) = kit42::raster::to_pixmap(&scaled) {
                        local.draw_pixmap(1, 1, sp.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
                    }
                }
            }
            Content::Table(rows) => {
                let n_r = rows.len().max(1);
                let n_c = rows.first().map(|r| r.len()).unwrap_or(1).max(1);
                let (cw, rh) = (w / n_c as f32, h / n_r as f32);
                for (ri, row) in rows.iter().enumerate() {
                    for (ci, cell) in row.iter().enumerate() {
                        let bg = if ri == 0 { pres.theme.accents[0] } else if ri % 2 == 0 { [233, 237, 244] } else { [207, 213, 234] };
                        if let Some(r) = Rect::from_xywh(1.0 + ci as f32 * cw, 1.0 + ri as f32 * rh, cw - 1.0, rh - 1.0) {
                            local.fill_rect(r, &paint(bg, 1.0), Transform::identity(), None);
                        }
                        raster_text(&mut local, std::slice::from_ref(cell), 1.0 + ci as f32 * cw, 1.0 + ri as f32 * rh, cw, rh, s, Anchor::Middle, if ri == 0 { Some([255, 255, 255]) } else { None });
                    }
                }
            }
        }
        if !sh.text.is_empty() {
            raster_text(&mut local, &sh.text, 1.0, 1.0, w, h, s, sh.anchor, None);
        }
        let (cx, cy) = (sh.x * s + w / 2.0, sh.y * s + h / 2.0);
        let mut tr = Transform::from_translate(-(lw as f32) / 2.0, -(lh as f32) / 2.0);
        if sh.flip_h && !sh.is_line() {
            tr = tr.post_scale(-1.0, 1.0);
        }
        if sh.flip_v && !sh.is_line() {
            tr = tr.post_scale(1.0, -1.0);
        }
        tr = tr.post_rotate(sh.rot).post_translate(cx, cy);
        pm.draw_pixmap(0, 0, local.as_ref(), &PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() }, tr, None);
    }
    kit42::raster::from_pixmap(&pm)
}

#[allow(clippy::too_many_arguments)]
fn raster_text(pm: &mut tiny_skia::Pixmap, paras: &[Para], x: f32, y: f32, w: f32, h: f32, s: f32, anchor: Anchor, color: Option<[u8; 3]>) {
    let mut measure = |f: &CharFmt, t: &str| kit42::raster::measure(&font_bytes(f), t, f.size);
    let pieces = layout_text(paras, w / s, h / s, anchor, &mut measure);
    for pc in pieces {
        let f = &pc.fmt;
        let col = color.unwrap_or(f.color);
        let size = f.size * s * if f.script != 0 { 0.65 } else { 1.0 };
        let font = font_bytes(f);
        let (px, py) = (x + pc.x * s, y + pc.baseline * s);
        kit42::raster::pixmap_text(pm, &font, &pc.text, size, px, py, [col[0], col[1], col[2], 255]);
        if f.underline || f.strike {
            let tw = kit42::raster::measure(&font, &pc.text, size);
            let yy = if f.underline { py + size * 0.12 } else { py - size * 0.3 };
            if let Some(r) = tiny_skia::Rect::from_xywh(px, yy, tw, (size / 16.0).max(1.0)) {
                pm.fill_rect(r, &paint(col, 1.0), tiny_skia::Transform::identity(), None);
            }
        }
    }
}

// ───────────────────────── PDF ─────────────────────────

pub fn pdf(pres: &Presentation, include_hidden: bool, notes: bool) -> Vec<u8> {
    use kit42::pdfgen::{Doc, Page, TextStyle};
    let mut d = Doc::new();
    d.title = pres.title.clone();
    d.author = pres.author.clone();
    let mut fonts: HashMap<(String, bool, bool), kit42::pdfgen::FontId> = HashMap::new();
    for (si, slide) in pres.slides.iter().enumerate() {
        if slide.hidden && !include_hidden {
            continue;
        }
        let (pw, ph) = if notes { (595.0, 842.0) } else { (pres.width * 0.75, pres.height * 0.75) };
        let s = if notes { 500.0 / pres.width } else { 0.75 };
        let (ox, oy) = if notes { ((pw - pres.width * s) / 2.0, 40.0) } else { (0.0, 0.0) };
        let mut page = Page::new(pw, ph);
        match pres.background_of(slide) {
            Background::Solid(c) => page.fill_rect(ox, oy, pres.width * s, pres.height * s, c),
            Background::Gradient(a, b) => {
                for k in 0..32 {
                    let t = k as f32 / 31.0;
                    let c = [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8);
                    page.fill_rect(ox, oy + pres.height * s * k as f32 / 32.0, pres.width * s, pres.height * s / 32.0 + 0.5, c);
                }
            }
            Background::Picture(dt) => {
                if let Some(img) = decoded(&dt) {
                    let id = d.add_image_rgba(&img);
                    page.image(id, ox, oy, pres.width * s, pres.height * s);
                }
            }
        }
        for sh in &slide.shapes {
            let (x, y, w, h) = (ox + sh.x * s, oy + sh.y * s, sh.w * s, sh.h * s);
            page.save();
            if sh.rot != 0.0 {
                page.rotate_about(x + w / 2.0, y + h / 2.0, sh.rot);
            }
            match &sh.content {
                Content::Shape(g) if sh.is_line() => {
                    let (c, lw) = sh.line.unwrap_or((pres.theme.text, 2.0));
                    let (x0, x1) = if sh.flip_h { (x + w, x) } else { (x, x + w) };
                    let (y0, y1) = if sh.flip_v { (y + h, y) } else { (y, y + h) };
                    page.line(x0, y0, x1, y1, c, lw * s);
                    if *g == Geom::Arrow {
                        let len = (lw * s * 4.0).max(6.0);
                        let dd = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt().max(0.01);
                        let (ux, uy) = ((x1 - x0) / dd, (y1 - y0) / dd);
                        page.polygon(&[(x1, y1), (x1 - ux * len - uy * len * 0.45, y1 - uy * len + ux * len * 0.45), (x1 - ux * len + uy * len * 0.45, y1 - uy * len - ux * len * 0.45)], true, Some(c), None);
                    }
                }
                Content::Shape(g) => {
                    let stroke = sh.line.map(|(c, lw)| (c, lw * s));
                    match g.outline() {
                        None => page.ellipse(x, y, w, h, sh.fill, stroke),
                        Some(pts) => {
                            let pts: Vec<(f32, f32)> = pts.iter().map(|(u, v)| (x + if sh.flip_h { 1.0 - u } else { *u } * w, y + if sh.flip_v { 1.0 - v } else { *v } * h)).collect();
                            if sh.fill.is_some() || stroke.is_some() {
                                page.polygon(&pts, true, sh.fill, stroke);
                            }
                        }
                    }
                }
                Content::Picture(dt) => {
                    if let Some(img) = decoded(dt) {
                        let id = d.add_image_rgba(&img);
                        page.image(id, x, y, w, h);
                    }
                }
                Content::Table(rows) => {
                    let n_r = rows.len().max(1);
                    let n_c = rows.first().map(|r| r.len()).unwrap_or(1).max(1);
                    let (cw, rh) = (w / n_c as f32, h / n_r as f32);
                    for (ri, row) in rows.iter().enumerate() {
                        for (ci, cell) in row.iter().enumerate() {
                            let bg = if ri == 0 { pres.theme.accents[0] } else if ri % 2 == 0 { [233, 237, 244] } else { [207, 213, 234] };
                            page.fill_rect(x + ci as f32 * cw, y + ri as f32 * rh, cw, rh, bg);
                            page.stroke_rect(x + ci as f32 * cw, y + ri as f32 * rh, cw, rh, [255, 255, 255], 0.75);
                            pdf_text(&mut d, &mut page, &mut fonts, std::slice::from_ref(cell), x + ci as f32 * cw, y + ri as f32 * rh, cw, rh, s, Anchor::Middle, if ri == 0 { Some([255, 255, 255]) } else { None });
                        }
                    }
                }
            }
            if !sh.text.is_empty() {
                pdf_text(&mut d, &mut page, &mut fonts, &sh.text, x, y, w, h, s, sh.anchor, None);
            }
            page.restore();
        }
        if notes {
            page.stroke_rect(ox, oy, pres.width * s, pres.height * s, [160, 160, 160], 0.5);
            let f = *fonts.entry(("Calibri".into(), false, false)).or_insert_with(|| d.font("Calibri", false, false));
            let lines = d.wrap(f, 12.0, &slide.notes, pw - 100.0);
            let mut yy = oy + pres.height * s + 40.0;
            let mut st = TextStyle::new(f, 9.0);
            st.color = [120, 120, 120];
            d.text(&mut page, st, 50.0, yy - 18.0, &format!("{} {}", kit42::t("Slide", "Слайд"), si + 1));
            for l in lines {
                d.text(&mut page, TextStyle::new(f, 12.0), 50.0, yy, &l);
                yy += 15.0;
            }
        }
        d.add_page(page);
    }
    d.finish()
}

#[allow(clippy::too_many_arguments)]
fn pdf_text(
    d: &mut kit42::pdfgen::Doc,
    page: &mut kit42::pdfgen::Page,
    fonts: &mut HashMap<(String, bool, bool), kit42::pdfgen::FontId>,
    paras: &[Para],
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    s: f32,
    anchor: Anchor,
    color: Option<[u8; 3]>,
) {
    let mut font_of = |d: &mut kit42::pdfgen::Doc, f: &CharFmt| *fonts.entry((f.font.clone(), f.bold, f.italic)).or_insert_with(|| d.font(&f.font, f.bold, f.italic));
    let mut cache: Vec<(CharFmt, kit42::pdfgen::FontId)> = vec![];
    let mut measure = |f: &CharFmt, t: &str| -> f32 {
        let id = match cache.iter().find(|(c, _)| c.font == f.font && c.bold == f.bold && c.italic == f.italic) {
            Some((_, id)) => *id,
            None => {
                let id = font_of(d, f);
                cache.push((f.clone(), id));
                id
            }
        };
        d.measure(id, f.size, t)
    };
    let pieces = layout_text(paras, w / s, h / s, anchor, &mut measure);
    for pc in pieces {
        let f = &pc.fmt;
        let id = font_of(d, f);
        let mut st = kit42::pdfgen::TextStyle::new(id, f.size * s * if f.script != 0 { 0.65 } else { 1.0 });
        st.color = color.unwrap_or(f.color);
        st.underline = f.underline;
        st.strike = f.strike;
        d.text(page, st, x + pc.x * s, y + pc.baseline * s, &pc.text);
    }
}
