//! Markup objects drawn over the image (editable until saved) and their flattening with
//! tiny-skia (shapes) and the kit42 rasterizer (text).

use egui::{Color32, Pos2, Rect, Stroke};
use image::RgbaImage;
use std::sync::Arc;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    RectSel,
    EllipseSel,
    Alpha,
    Pen,
    Line,
    Arrow,
    Rect,
    RoundRect,
    Ellipse,
    Star,
    Bubble,
    Highlight,
    Text,
    Pixelate,
    Crop,
}

impl Tool {
    pub fn name(self) -> &'static str {
        use kit42::t;
        match self {
            Tool::Select => t("Select", "Выбор"),
            Tool::RectSel => t("Rectangular Selection", "Прямоугольное выделение"),
            Tool::EllipseSel => t("Elliptical Selection", "Овальное выделение"),
            Tool::Alpha => t("Instant Alpha", "Мгновенная альфа"),
            Tool::Pen => t("Draw", "Рисование"),
            Tool::Line => t("Line", "Линия"),
            Tool::Arrow => t("Arrow", "Стрелка"),
            Tool::Rect => t("Rectangle", "Прямоугольник"),
            Tool::RoundRect => t("Rounded Rectangle", "Скруглённый прямоугольник"),
            Tool::Ellipse => t("Oval", "Овал"),
            Tool::Star => t("Star", "Звезда"),
            Tool::Bubble => t("Speech Bubble", "Выноска"),
            Tool::Highlight => t("Highlight", "Маркер"),
            Tool::Text => t("Text", "Текст"),
            Tool::Pixelate => t("Pixelate", "Пикселизация"),
            Tool::Crop => t("Crop", "Обрезка"),
        }
    }
    pub fn icon(self) -> &'static str {
        use kit42::icons as i;
        match self {
            Tool::Select => i::SELECT,
            Tool::RectSel => "\u{E739}",
            Tool::EllipseSel => "\u{EA3A}",
            Tool::Alpha => i::WAND,
            Tool::Pen => i::PEN,
            Tool::Line => "\u{E738}",
            Tool::Arrow => "\u{E72A}",
            Tool::Rect => "\u{F16B}",
            Tool::RoundRect => "\u{E7FB}",
            Tool::Ellipse => "\u{EA3A}",
            Tool::Star => "\u{E734}",
            Tool::Bubble => i::COMMENT,
            Tool::Highlight => i::HIGHLIGHT,
            Tool::Text => i::FONT,
            Tool::Pixelate => "\u{E80A}",
            Tool::Crop => i::CROP,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Kind {
    Pen(Vec<[f32; 2]>),
    Line([f32; 2], [f32; 2]),
    Arrow([f32; 2], [f32; 2]),
    Rect,
    RoundRect,
    Ellipse,
    Star,
    Bubble,
    Highlight,
    Text(String),
    Image(Arc<RgbaImage>),
}

#[derive(Clone, Debug)]
pub struct Mark {
    pub id: u64,
    /// x0, y0, x1, y1 in image pixels.
    pub rect: [f32; 4],
    pub kind: Kind,
    pub color: [u8; 3],
    pub fill: Option<[u8; 3]>,
    pub width: f32,
    pub font_size: f32,
}

pub fn norm(r: [f32; 4]) -> [f32; 4] {
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

impl Mark {
    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.rect = [self.rect[0] + dx, self.rect[1] + dy, self.rect[2] + dx, self.rect[3] + dy];
        match &mut self.kind {
            Kind::Pen(p) => p.iter_mut().for_each(|q| {
                q[0] += dx;
                q[1] += dy;
            }),
            Kind::Line(a, b) | Kind::Arrow(a, b) => {
                for q in [a, b] {
                    q[0] += dx;
                    q[1] += dy;
                }
            }
            _ => {}
        }
    }

    pub fn set_rect(&mut self, r: [f32; 4]) {
        let r = norm(r);
        let o = self.rect;
        let sx = (r[2] - r[0]) / (o[2] - o[0]).max(0.01);
        let sy = (r[3] - r[1]) / (o[3] - o[1]).max(0.01);
        let map = |q: &mut [f32; 2]| {
            q[0] = r[0] + (q[0] - o[0]) * sx;
            q[1] = r[1] + (q[1] - o[1]) * sy;
        };
        match &mut self.kind {
            Kind::Pen(p) => p.iter_mut().for_each(map),
            Kind::Line(a, b) | Kind::Arrow(a, b) => {
                map(a);
                map(b);
            }
            _ => {}
        }
        self.rect = r;
    }

    /// Re-maps geometry through an image transform (rotation/flip/crop/resize).
    pub fn map_points(&mut self, f: &dyn Fn([f32; 2]) -> [f32; 2]) {
        let a = f([self.rect[0], self.rect[1]]);
        let b = f([self.rect[2], self.rect[3]]);
        match &mut self.kind {
            Kind::Pen(p) => p.iter_mut().for_each(|q| *q = f(*q)),
            Kind::Line(x, y) | Kind::Arrow(x, y) => {
                *x = f(*x);
                *y = f(*y);
            }
            _ => {}
        }
        self.rect = norm([a[0], a[1], b[0], b[1]]);
    }
}

fn star_points(r: Rect) -> Vec<Pos2> {
    let c = r.center();
    (0..10)
        .map(|k| {
            let rad = if k % 2 == 0 { 1.0 } else { 0.42 };
            let a = (k as f32 * 36.0 - 90.0).to_radians();
            Pos2::new(c.x + r.width() / 2.0 * rad * a.cos(), c.y + r.height() / 2.0 * rad * a.sin())
        })
        .collect()
}

fn bubble_points(r: Rect) -> Vec<Pos2> {
    let body_h = r.height() * 0.78;
    vec![
        r.left_top(),
        r.right_top(),
        Pos2::new(r.right(), r.top() + body_h),
        Pos2::new(r.left() + r.width() * 0.45, r.top() + body_h),
        Pos2::new(r.left() + r.width() * 0.2, r.bottom()),
        Pos2::new(r.left() + r.width() * 0.28, r.top() + body_h),
        Pos2::new(r.left(), r.top() + body_h),
    ]
}

pub fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Draws a mark on screen; `map` converts image pixels to screen points, `s` = screen px per image px.
pub fn paint(p: &egui::Painter, m: &Mark, map: &dyn Fn([f32; 2]) -> Pos2, s: f32, tex: Option<&egui::TextureHandle>, selected: bool) {
    let r = Rect::from_two_pos(map([m.rect[0], m.rect[1]]), map([m.rect[2], m.rect[3]]));
    let st = Stroke::new((m.width * s).max(1.0), c32(m.color));
    let fill = m.fill.map(c32).unwrap_or(Color32::TRANSPARENT);
    match &m.kind {
        Kind::Pen(pts) => {
            p.add(egui::Shape::line(pts.iter().map(|q| map(*q)).collect(), st));
        }
        Kind::Line(a, b) => {
            p.line_segment([map(*a), map(*b)], st);
        }
        Kind::Arrow(a, b) => {
            let (pa, pb) = (map(*a), map(*b));
            p.line_segment([pa, pb], st);
            let dir = (pb - pa).normalized();
            let len = (st.width * 4.0).max(10.0);
            let n = egui::vec2(-dir.y, dir.x);
            p.add(egui::Shape::convex_polygon(vec![pb, pb - dir * len + n * len * 0.45, pb - dir * len - n * len * 0.45], st.color, Stroke::NONE));
        }
        Kind::Rect => {
            p.rect_filled(r, 0.0, fill);
            p.rect_stroke(r, 0.0, st, egui::StrokeKind::Middle);
        }
        Kind::RoundRect => {
            let cr = (r.width().min(r.height()) * 0.15) as u8;
            p.rect_filled(r, cr, fill);
            p.rect_stroke(r, cr, st, egui::StrokeKind::Middle);
        }
        Kind::Ellipse => {
            p.add(egui::epaint::EllipseShape { center: r.center(), radius: r.size() / 2.0, fill, stroke: st });
        }
        Kind::Star => {
            let pts = star_points(r);
            p.add(egui::Shape::closed_line(pts, st));
        }
        Kind::Bubble => {
            let pts = bubble_points(r);
            p.add(egui::Shape::closed_line(pts, st));
        }
        Kind::Highlight => {
            p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(m.color[0], m.color[1], m.color[2], 100));
        }
        Kind::Text(t) => {
            if let Some(f) = m.fill {
                p.rect_filled(r, 0.0, c32(f));
            }
            let g = p.layout(t.clone(), egui::FontId::proportional((m.font_size * s).max(4.0)), c32(m.color), r.width().max(8.0));
            p.galley(r.min, g, c32(m.color));
        }
        Kind::Image(_) => {
            if let Some(t) = tex {
                p.image(t.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
        }
    }
    if selected {
        let acc = Color32::from_rgb(0, 120, 215);
        p.rect_stroke(r.expand(2.0), 0.0, Stroke::new(1.0, acc), egui::StrokeKind::Outside);
        for h in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
            p.rect_filled(Rect::from_center_size(h, egui::vec2(8.0, 8.0)), 1.0, Color32::WHITE);
            p.rect_stroke(Rect::from_center_size(h, egui::vec2(8.0, 8.0)), 1.0, Stroke::new(1.0, acc), egui::StrokeKind::Inside);
        }
    }
}

fn paint_of(c: [u8; 3], a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], a);
    p.anti_alias = true;
    p
}

/// Renders marks into the image pixels.
pub fn flatten(img: &RgbaImage, marks: &[Mark]) -> RgbaImage {
    if marks.is_empty() {
        return img.clone();
    }
    let Some(mut pm) = kit42::raster::to_pixmap(img) else { return img.clone() };
    let font = kit42::fonts::load_bytes("Segoe UI", false, false).unwrap_or_default();
    for m in marks {
        let st = tiny_skia::Stroke { width: m.width.max(0.5), line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
        let r = m.rect;
        let poly = |pts: &[Pos2]| -> Option<tiny_skia::Path> {
            let mut pb = PathBuilder::new();
            pb.move_to(pts.first()?.x, pts[0].y);
            for q in &pts[1..] {
                pb.line_to(q.x, q.y);
            }
            pb.close();
            pb.finish()
        };
        let rr = Rect::from_min_max(Pos2::new(r[0], r[1]), Pos2::new(r[2], r[3]));
        match &m.kind {
            Kind::Pen(pts) if pts.len() > 1 => {
                let mut pb = PathBuilder::new();
                pb.move_to(pts[0][0], pts[0][1]);
                for q in &pts[1..] {
                    pb.line_to(q[0], q[1]);
                }
                if let Some(path) = pb.finish() {
                    pm.stroke_path(&path, &paint_of(m.color, 255), &st, Transform::identity(), None);
                }
            }
            Kind::Line(a, b) | Kind::Arrow(a, b) => {
                let mut pb = PathBuilder::new();
                pb.move_to(a[0], a[1]);
                pb.line_to(b[0], b[1]);
                if let Some(path) = pb.finish() {
                    pm.stroke_path(&path, &paint_of(m.color, 255), &st, Transform::identity(), None);
                }
                if matches!(m.kind, Kind::Arrow(..)) {
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    let len = (dx * dx + dy * dy).sqrt().max(0.01);
                    let (ux, uy) = (dx / len, dy / len);
                    let hl = (m.width * 4.0).max(10.0);
                    let pts = [Pos2::new(b[0], b[1]), Pos2::new(b[0] - ux * hl - uy * hl * 0.45, b[1] - uy * hl + ux * hl * 0.45), Pos2::new(b[0] - ux * hl + uy * hl * 0.45, b[1] - uy * hl - ux * hl * 0.45)];
                    if let Some(path) = poly(&pts) {
                        pm.fill_path(&path, &paint_of(m.color, 255), FillRule::Winding, Transform::identity(), None);
                    }
                }
            }
            Kind::Rect | Kind::RoundRect | Kind::Highlight => {
                let path = if matches!(m.kind, Kind::RoundRect) {
                    let rad = (r[2] - r[0]).min(r[3] - r[1]) * 0.15;
                    let mut pb = PathBuilder::new();
                    pb.move_to(r[0] + rad, r[1]);
                    pb.line_to(r[2] - rad, r[1]);
                    pb.quad_to(r[2], r[1], r[2], r[1] + rad);
                    pb.line_to(r[2], r[3] - rad);
                    pb.quad_to(r[2], r[3], r[2] - rad, r[3]);
                    pb.line_to(r[0] + rad, r[3]);
                    pb.quad_to(r[0], r[3], r[0], r[3] - rad);
                    pb.line_to(r[0], r[1] + rad);
                    pb.quad_to(r[0], r[1], r[0] + rad, r[1]);
                    pb.close();
                    pb.finish()
                } else {
                    tiny_skia::Rect::from_ltrb(r[0], r[1], r[2], r[3]).map(PathBuilder::from_rect)
                };
                if let Some(path) = path {
                    if matches!(m.kind, Kind::Highlight) {
                        pm.fill_path(&path, &paint_of(m.color, 100), FillRule::Winding, Transform::identity(), None);
                    } else {
                        if let Some(f) = m.fill {
                            pm.fill_path(&path, &paint_of(f, 255), FillRule::Winding, Transform::identity(), None);
                        }
                        pm.stroke_path(&path, &paint_of(m.color, 255), &st, Transform::identity(), None);
                    }
                }
            }
            Kind::Ellipse => {
                if let Some(path) = tiny_skia::Rect::from_ltrb(r[0], r[1], r[2], r[3]).and_then(PathBuilder::from_oval) {
                    if let Some(f) = m.fill {
                        pm.fill_path(&path, &paint_of(f, 255), FillRule::Winding, Transform::identity(), None);
                    }
                    pm.stroke_path(&path, &paint_of(m.color, 255), &st, Transform::identity(), None);
                }
            }
            Kind::Star | Kind::Bubble => {
                let pts = if matches!(m.kind, Kind::Star) { star_points(rr) } else { bubble_points(rr) };
                if let Some(path) = poly(&pts) {
                    if let Some(f) = m.fill {
                        pm.fill_path(&path, &paint_of(f, 255), FillRule::Winding, Transform::identity(), None);
                    }
                    pm.stroke_path(&path, &paint_of(m.color, 255), &st, Transform::identity(), None);
                }
            }
            Kind::Text(text) => {
                if let Some(f) = m.fill {
                    if let Some(rect) = tiny_skia::Rect::from_ltrb(r[0], r[1], r[2], r[3]) {
                        pm.fill_rect(rect, &paint_of(f, 255), Transform::identity(), None);
                    }
                }
                let lines = kit42::raster::wrap(&font, text, m.font_size, (r[2] - r[0]).max(8.0));
                let asc = kit42::raster::ascent(&font, m.font_size);
                for (i, l) in lines.iter().enumerate() {
                    kit42::raster::pixmap_text(&mut pm, &font, l, m.font_size, r[0], r[1] + asc + i as f32 * m.font_size * 1.25, [m.color[0], m.color[1], m.color[2], 255]);
                }
            }
            Kind::Image(src) => {
                let w = (r[2] - r[0]).max(1.0) as u32;
                let h = (r[3] - r[1]).max(1.0) as u32;
                let scaled = image::imageops::resize(&**src, w, h, image::imageops::FilterType::Lanczos3);
                if let Some(spm) = kit42::raster::to_pixmap(&scaled) {
                    pm.draw_pixmap(r[0] as i32, r[1] as i32, spm.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
                }
            }
            _ => {}
        }
    }
    let _ = Pixmap::new(1, 1);
    kit42::raster::from_pixmap(&pm)
}
