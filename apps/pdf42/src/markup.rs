//! Markup objects drawn over pages. They stay editable objects until saved; then text markup,
//! ink, shapes, lines, notes and links become real PDF annotations with appearance streams,
//! while text boxes, whiteout and signature/stamp pictures are flattened into page content.

use crate::pdfops::{self, Geom};
use egui::{Color32, Pos2, Rect, Stroke};
use kit42::pdfgen;
use kit42::t;
use lopdf::{dictionary, Document, Object, Stream};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Highlight,
    Underline,
    Strike,
    Pen,
    Rect,
    Ellipse,
    Line,
    Arrow,
    Text,
    Note,
    Whiteout,
    Redact,
    Link,
    FieldText,
    FieldCheck,
    Crop,
    EditText,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => t("Select", "Выбор"),
            Tool::Highlight => t("Highlight", "Выделение цветом"),
            Tool::Underline => t("Underline", "Подчёркивание"),
            Tool::Strike => t("Strikethrough", "Зачёркивание"),
            Tool::Pen => t("Pencil", "Карандаш"),
            Tool::Rect => t("Rectangle", "Прямоугольник"),
            Tool::Ellipse => t("Oval", "Овал"),
            Tool::Line => t("Line", "Линия"),
            Tool::Arrow => t("Arrow", "Стрелка"),
            Tool::Text => t("Text Box", "Текстовое поле"),
            Tool::Note => t("Note", "Заметка"),
            Tool::Whiteout => t("Whiteout", "Замазка"),
            Tool::Redact => t("Redact", "Скрыть (редактирование)"),
            Tool::Link => t("Link", "Ссылка"),
            Tool::FieldText => t("Text Field", "Текстовое поле формы"),
            Tool::FieldCheck => t("Checkbox", "Флажок формы"),
            Tool::Crop => t("Crop", "Обрезка"),
            Tool::EditText => t("Edit Text", "Правка текста"),
        }
    }

    pub fn icon(self) -> &'static str {
        use kit42::icons as i;
        match self {
            Tool::Select => i::SELECT,
            Tool::Highlight => i::HIGHLIGHT,
            Tool::Underline => i::UNDERLINE,
            Tool::Strike => "\u{EDE0}",
            Tool::Pen => i::PEN,
            Tool::Rect => "\u{E739}",
            Tool::Ellipse => "\u{EA3A}",
            Tool::Line => "\u{E738}",
            Tool::Arrow => "\u{E72A}",
            Tool::Text => i::FONT,
            Tool::Note => i::COMMENT,
            Tool::Whiteout => i::ERASE,
            Tool::Redact => "\u{E73F}",
            Tool::Link => i::LINK,
            Tool::FieldText => "\u{E8D3}",
            Tool::FieldCheck => i::CHECK,
            Tool::Crop => i::CROP,
            Tool::EditText => "\u{E8D2}",
        }
    }

    pub fn default_color(self) -> [u8; 3] {
        match self {
            Tool::Highlight => [255, 230, 0],
            Tool::Underline | Tool::Strike => [220, 30, 30],
            Tool::Pen => [30, 30, 200],
            Tool::Whiteout => [255, 255, 255],
            Tool::Redact => [0, 0, 0],
            Tool::Text => [0, 0, 0],
            Tool::Note => [255, 210, 60],
            _ => [220, 30, 30],
        }
    }
}

#[derive(Clone, Debug)]
pub enum Kind {
    Highlight,
    Underline,
    Strike,
    Ink(Vec<Vec<[f32; 2]>>),
    Rect,
    Ellipse,
    Line([f32; 2], [f32; 2]),
    Arrow([f32; 2], [f32; 2]),
    Text(String),
    Note(String),
    Whiteout,
    Redact,
    Image(Arc<image::RgbaImage>),
    Link(String),
    FieldText(String),
    FieldCheck(String),
}

#[derive(Clone, Debug)]
pub struct Mark {
    pub id: u64,
    pub page: usize,
    /// x0, y0, x1, y1 in displayed page points, origin top-left.
    pub rect: [f32; 4],
    pub kind: Kind,
    pub color: [u8; 3],
    pub fill: Option<[u8; 3]>,
    pub width: f32,
    pub font_size: f32,
    pub opacity: f32,
    /// Text marks: installed font family ("" = Arial) and style.
    pub font: String,
    pub bold: bool,
    pub italic: bool,
    /// Edited page text: the original line's box, hidden behind the new text.
    pub cover: Option<[f32; 4]>,
    /// The original is pixels (scan or OCR layer), so it is always painted over.
    pub cover_always: bool,
}

impl Mark {
    pub fn new(page: usize, rect: [f32; 4], kind: Kind, color: [u8; 3], width: f32) -> Self {
        Self {
            id: rand::random(),
            page,
            rect: norm(rect),
            kind,
            color,
            fill: None,
            width,
            font_size: 12.0,
            opacity: 1.0,
            font: String::new(),
            bold: false,
            italic: false,
            cover: None,
            cover_always: false,
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.rect = [self.rect[0] + dx, self.rect[1] + dy, self.rect[2] + dx, self.rect[3] + dy];
        match &mut self.kind {
            Kind::Ink(paths) => paths.iter_mut().flatten().for_each(|p| {
                p[0] += dx;
                p[1] += dy;
            }),
            Kind::Line(a, b) | Kind::Arrow(a, b) => {
                for p in [a, b] {
                    p[0] += dx;
                    p[1] += dy;
                }
            }
            _ => {}
        }
    }

    /// Resizes to a new rectangle, scaling contained geometry.
    pub fn set_rect(&mut self, r: [f32; 4]) {
        let r = norm(r);
        let old = self.rect;
        let sx = (r[2] - r[0]) / (old[2] - old[0]).max(0.01);
        let sy = (r[3] - r[1]) / (old[3] - old[1]).max(0.01);
        let map = |p: &mut [f32; 2]| {
            p[0] = r[0] + (p[0] - old[0]) * sx;
            p[1] = r[1] + (p[1] - old[1]) * sy;
        };
        match &mut self.kind {
            Kind::Ink(paths) => paths.iter_mut().flatten().for_each(map),
            Kind::Line(a, b) | Kind::Arrow(a, b) => {
                map(a);
                map(b);
            }
            _ => {}
        }
        self.rect = r;
    }

    pub fn is_flattened(&self) -> bool {
        matches!(self.kind, Kind::Text(_) | Kind::Whiteout | Kind::Image(_))
    }

    /// Rotates a mark with its page by 90° clockwise. `h` is the old displayed page height.
    pub fn rotate_cw(&mut self, h: f32) {
        let f = |p: [f32; 2]| [h - p[1], p[0]];
        let a = f([self.rect[0], self.rect[1]]);
        let b = f([self.rect[2], self.rect[3]]);
        match &mut self.kind {
            Kind::Ink(paths) => paths.iter_mut().flatten().for_each(|p| *p = f(*p)),
            Kind::Line(x, y) | Kind::Arrow(x, y) => {
                *x = f(*x);
                *y = f(*y);
            }
            _ => {}
        }
        self.rect = norm([a[0], a[1], b[0], b[1]]);
    }
}

/// The egui family for a text mark's font.
pub fn text_family(ctx: &egui::Context, m: &Mark) -> egui::FontFamily {
    if m.font.is_empty() {
        egui::FontFamily::Proportional
    } else {
        kit42::fonts::doc_family(ctx, &m.font, m.bold, m.italic)
    }
}

pub fn norm(r: [f32; 4]) -> [f32; 4] {
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

pub fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Paints a mark. `to_screen` maps page points to screen; `s` is pixels per point.
pub fn paint(
    painter: &egui::Painter,
    m: &Mark,
    to_screen: &dyn Fn([f32; 2]) -> Pos2,
    s: f32,
    image_tex: Option<&egui::TextureHandle>,
    selected: bool,
) {
    let r = Rect::from_two_pos(to_screen([m.rect[0], m.rect[1]]), to_screen([m.rect[2], m.rect[3]]));
    let col = c32(m.color);
    let w = (m.width * s).max(1.0);
    match &m.kind {
        Kind::Highlight => {
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(m.color[0], m.color[1], m.color[2], 110));
        }
        Kind::Underline => {
            painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new((1.5 * s).max(1.0), col));
        }
        Kind::Strike => {
            painter.line_segment([r.left_center(), r.right_center()], Stroke::new((1.5 * s).max(1.0), col));
        }
        Kind::Ink(paths) => {
            for p in paths {
                let pts: Vec<Pos2> = p.iter().map(|q| to_screen(*q)).collect();
                if pts.len() > 1 {
                    painter.add(egui::Shape::line(pts, Stroke::new(w, col)));
                }
            }
        }
        Kind::Rect => {
            if let Some(f) = m.fill {
                painter.rect_filled(r, 0.0, c32(f));
            }
            painter.rect_stroke(r, 0.0, Stroke::new(w, col), egui::StrokeKind::Middle);
        }
        Kind::Ellipse => {
            let shape = egui::epaint::EllipseShape {
                center: r.center(),
                radius: r.size() / 2.0,
                fill: m.fill.map(c32).unwrap_or(Color32::TRANSPARENT),
                stroke: Stroke::new(w, col),
            };
            painter.add(shape);
        }
        Kind::Line(a, b) => {
            painter.line_segment([to_screen(*a), to_screen(*b)], Stroke::new(w, col));
        }
        Kind::Arrow(a, b) => {
            let (pa, pb) = (to_screen(*a), to_screen(*b));
            painter.line_segment([pa, pb], Stroke::new(w, col));
            let dir = (pb - pa).normalized();
            let len = (w * 4.0).max(10.0);
            let n = egui::vec2(-dir.y, dir.x);
            painter.add(egui::Shape::convex_polygon(
                vec![pb, pb - dir * len + n * len * 0.45, pb - dir * len - n * len * 0.45],
                col,
                Stroke::NONE,
            ));
        }
        Kind::Text(text) => {
            if let Some(f) = m.fill {
                if let Some(c) = m.cover {
                    painter.rect_filled(Rect::from_two_pos(to_screen([c[0], c[1]]), to_screen([c[2], c[3]])), 0.0, c32(f));
                }
                painter.rect_filled(r, 0.0, c32(f));
            }
            let galley = painter.layout(
                text.clone(),
                egui::FontId::new((m.font_size * s).max(4.0), text_family(painter.ctx(), m)),
                col,
                if m.cover.is_some() { f32::INFINITY } else { r.width().max(10.0) },
            );
            painter.galley(r.min, galley, col);
            if selected || text.is_empty() {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_gray(150)), egui::StrokeKind::Outside);
            }
        }
        Kind::Note(_) => {
            let icon = Rect::from_min_size(r.min, egui::vec2(22.0 * s.max(0.6), 22.0 * s.max(0.6)));
            painter.rect_filled(icon, 3.0, col);
            painter.text(
                icon.center(),
                egui::Align2::CENTER_CENTER,
                kit42::icons::COMMENT,
                egui::FontId::new(14.0 * s.max(0.6), egui::FontFamily::Name(kit42::fonts::ICON_FAMILY.into())),
                Color32::BLACK,
            );
        }
        Kind::Whiteout => {
            painter.rect_filled(r, 0.0, c32(m.fill.unwrap_or([255, 255, 255])));
            if selected {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_gray(160)), egui::StrokeKind::Outside);
            }
        }
        Kind::Redact => {
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(m.color[0], m.color[1], m.color[2], 200));
            painter.rect_stroke(r, 0.0, Stroke::new(1.5, Color32::RED), egui::StrokeKind::Inside);
        }
        Kind::Image(_) => {
            if let Some(tex) = image_tex {
                painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
        }
        Kind::Link(_) => {
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(0, 102, 204)), egui::StrokeKind::Inside);
        }
        Kind::FieldText(name) | Kind::FieldCheck(name) => {
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(210, 225, 255, 160));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(60, 100, 200)), egui::StrokeKind::Inside);
            painter.text(r.left_top() + egui::vec2(2.0, 1.0), egui::Align2::LEFT_TOP, name, egui::FontId::proportional(10.0), Color32::from_rgb(60, 100, 200));
        }
    }
    if selected {
        let acc = Color32::from_rgb(0, 120, 215);
        painter.rect_stroke(r.expand(2.0), 0.0, Stroke::new(1.0, acc), egui::StrokeKind::Outside);
        for h in handles(r) {
            painter.rect_filled(Rect::from_center_size(h, egui::vec2(8.0, 8.0)), 1.0, Color32::WHITE);
            painter.rect_stroke(Rect::from_center_size(h, egui::vec2(8.0, 8.0)), 1.0, Stroke::new(1.0, acc), egui::StrokeKind::Inside);
        }
    }
}

/// Corner and edge handles of a selection: TL, T, TR, R, BR, B, BL, L.
pub fn handles(r: Rect) -> [Pos2; 8] {
    [
        r.left_top(),
        r.center_top(),
        r.right_top(),
        r.right_center(),
        r.right_bottom(),
        r.center_bottom(),
        r.left_bottom(),
        r.left_center(),
    ]
}

/// Applies a handle drag to a rect (page points).
pub fn drag_handle(r: [f32; 4], handle: usize, dx: f32, dy: f32) -> [f32; 4] {
    let mut r = r;
    match handle {
        0 => {
            r[0] += dx;
            r[1] += dy;
        }
        1 => r[1] += dy,
        2 => {
            r[2] += dx;
            r[1] += dy;
        }
        3 => r[2] += dx,
        4 => {
            r[2] += dx;
            r[3] += dy;
        }
        5 => r[3] += dy,
        6 => {
            r[0] += dx;
            r[3] += dy;
        }
        _ => r[0] += dx,
    }
    r
}

fn n(v: f32) -> String {
    format!("{:.3}", v).trim_end_matches('0').trim_end_matches('.').to_string()
}

fn rgb(c: [u8; 3]) -> String {
    format!("{} {} {}", n(c[0] as f32 / 255.0), n(c[1] as f32 / 255.0), n(c[2] as f32 / 255.0))
}

fn color_arr(c: [u8; 3]) -> Object {
    Object::Array(c.iter().map(|v| Object::Real(*v as f32 / 255.0)).collect())
}

/// Builds an appearance stream in displayed-local coordinates (y up) for a mark.
fn appearance(doc: &mut Document, g: &Geom, m: &Mark) -> Option<lopdf::ObjectId> {
    let (rw, rh) = (m.rect[2] - m.rect[0], m.rect[3] - m.rect[1]);
    let local = |p: [f32; 2]| (p[0] - m.rect[0], rh - (p[1] - m.rect[1]));
    let mut ops = String::new();
    let mut res = lopdf::Dictionary::new();
    let w = m.width.max(0.5);
    match &m.kind {
        Kind::Highlight => {
            res.set("ExtGState", dictionary! { "GS0" => dictionary! { "Type" => "ExtGState", "BM" => "Multiply", "ca" => 1.0, "CA" => 1.0 } });
            ops = format!("/GS0 gs {} rg 0 0 {} {} re f", rgb(m.color), n(rw), n(rh));
        }
        Kind::Underline => ops = format!("{} RG 1.2 w 0 {} m {} {} l S", rgb(m.color), n(1.0), n(rw), n(1.0)),
        Kind::Strike => ops = format!("{} RG 1.2 w 0 {} m {} {} l S", rgb(m.color), n(rh / 2.0), n(rw), n(rh / 2.0)),
        Kind::Ink(paths) => {
            ops = format!("{} RG {} w 1 J 1 j ", rgb(m.color), n(w));
            for p in paths {
                for (i, q) in p.iter().enumerate() {
                    let (x, y) = local(*q);
                    ops.push_str(&format!("{} {} {} ", n(x), n(y), if i == 0 { "m" } else { "l" }));
                }
                ops.push_str("S ");
            }
        }
        Kind::Rect => {
            let fill = m.fill.map(|f| format!("{} rg ", rgb(f))).unwrap_or_default();
            ops = format!(
                "{fill}{} RG {} w {} {} {} {} re {}",
                rgb(m.color), n(w), n(w / 2.0), n(w / 2.0), n(rw - w), n(rh - w), if m.fill.is_some() { "B" } else { "S" }
            );
        }
        Kind::Ellipse => {
            let mut p = pdfgen::Page::new(rw, rh);
            p.ellipse(w / 2.0, w / 2.0, rw - w, rh - w, m.fill, Some((m.color, w)));
            let mut d = pdfgen::Doc::new();
            d.add_page(p);
            let bytes = d.finish();
            let lo = Document::load_mem(&bytes).ok()?;
            let pid = *lo.get_pages().values().next()?;
            ops = String::from_utf8_lossy(&lo.get_page_content(pid).ok()?).into_owned();
        }
        Kind::Line(a, b) | Kind::Arrow(a, b) => {
            let (ax, ay) = local(*a);
            let (bx, by) = local(*b);
            ops = format!("{} RG {} rg {} w 1 J {} {} m {} {} l S ", rgb(m.color), rgb(m.color), n(w), n(ax), n(ay), n(bx), n(by));
            if matches!(m.kind, Kind::Arrow(..)) {
                let (dx, dy) = (bx - ax, by - ay);
                let len = (dx * dx + dy * dy).sqrt().max(0.01);
                let (ux, uy) = (dx / len, dy / len);
                let hl = (w * 4.0).max(8.0);
                let (nx, ny) = (-uy, ux);
                ops.push_str(&format!(
                    "{} {} m {} {} l {} {} l h f",
                    n(bx), n(by),
                    n(bx - ux * hl + nx * hl * 0.45), n(by - uy * hl + ny * hl * 0.45),
                    n(bx - ux * hl - nx * hl * 0.45), n(by - uy * hl - ny * hl * 0.45)
                ));
            }
        }
        _ => return None,
    }
    let m6 = g.rotation_matrix();
    let st = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), Object::Real(rw), Object::Real(rh)],
            "Matrix" => m6.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
            "Resources" => res,
        },
        ops.into_bytes(),
    );
    Some(doc.add_object(st))
}

/// Writes marks into the document: redaction pages first (rasterised), then overlays, then
/// annotations. `render` produces a page raster (300 dpi) of the current document bytes.
pub fn apply(
    doc: &mut Document,
    marks: &[Mark],
    render: &dyn Fn(usize, u32) -> Result<image::RgbaImage, String>,
    author: &str,
) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    let pages = pdfops::page_ids(doc);
    let page_count = pages.len();
    // 0. Edited page text: remove the original glyphs; areas that cannot be cleaned are covered.
    let mut cover_fill: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for (pi, &pid) in pages.iter().enumerate() {
        let edits: Vec<&Mark> = marks.iter().filter(|m| m.page == pi && m.cover.is_some()).collect();
        if edits.is_empty() {
            continue;
        }
        let rects: Vec<[f32; 4]> = edits.iter().map(|m| m.cover.unwrap()).collect();
        let done = crate::textlayer::remove_text(doc, pid, &rects);
        for (m, ok) in edits.iter().zip(done) {
            if m.cover_always || !ok {
                cover_fill.insert(m.id);
            }
        }
    }
    // 1. Redactions: the page becomes a picture with the boxes burned in plus an OCR text layer.
    let mut redact_pages: Vec<usize> = marks.iter().filter(|m| matches!(m.kind, Kind::Redact)).map(|m| m.page).collect();
    redact_pages.sort();
    redact_pages.dedup();
    for &pi in &redact_pages {
        let Some(&pid) = pages.get(pi) else { continue };
        let g = pdfops::geom(doc, pid);
        let (wd, hd) = g.display();
        let px_w = (wd / 72.0 * 300.0).round() as u32;
        let mut img = render(pi, px_w)?;
        let s = img.width() as f32 / wd;
        for m in marks.iter().filter(|m| m.page == pi && matches!(m.kind, Kind::Redact)) {
            let x0 = (m.rect[0] * s).floor().max(0.0) as u32;
            let y0 = (m.rect[1] * s).floor().max(0.0) as u32;
            let x1 = ((m.rect[2] * s).ceil() as u32).min(img.width());
            let y1 = ((m.rect[3] * s).ceil() as u32).min(img.height());
            for y in y0..y1 {
                for x in x0..x1 {
                    img.put_pixel(x, y, image::Rgba([m.color[0], m.color[1], m.color[2], 255]));
                }
            }
        }
        let ocr = kit42::winrt::ocr(&img, None).ok();
        let mut d = pdfgen::Doc::new();
        let font = d.font("Arial", false, false);
        let mut page = pdfgen::Page::new(wd, hd);
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        let mut jpg = Vec::new();
        let _ = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 85).encode_image(&rgb);
        let id = d.add_image_jpeg(jpg, img.width(), img.height(), false);
        page.image(id, 0.0, 0.0, wd, hd);
        if let Some(o) = ocr {
            o.to_pdf_layer(&mut d, &mut page, font, 1.0 / s);
        }
        d.add_page(page);
        pdfops::replace_page(doc, pid, &d.finish())?;
        notes.push(format!("{} {}", t("Redacted page", "Скрыто на странице"), pi + 1));
    }
    // 2. Flattened marks per page.
    for pi in 0..page_count {
        let flat: Vec<&Mark> = marks.iter().filter(|m| m.page == pi && m.is_flattened()).collect();
        if flat.is_empty() {
            continue;
        }
        let pid = pages[pi];
        let g = pdfops::geom(doc, pid);
        let (wd, hd) = g.display();
        let mut d = pdfgen::Doc::new();
        let font = d.font("Arial", false, false);
        let mut page = pdfgen::Page::new(wd, hd);
        for m in flat {
            let (x, y, w, h) = (m.rect[0], m.rect[1], m.rect[2] - m.rect[0], m.rect[3] - m.rect[1]);
            match &m.kind {
                Kind::Whiteout => page.fill_rect(x, y, w, h, m.fill.unwrap_or([255, 255, 255])),
                Kind::Image(img) => {
                    let id = d.add_image_rgba(img);
                    page.image(id, x, y, w, h);
                }
                Kind::Text(text) if m.cover.is_some() => {
                    let font = d.font(if m.font.is_empty() { "Arial" } else { &m.font }, m.bold, m.italic);
                    if cover_fill.contains(&m.id) {
                        let f = m.fill.unwrap_or([255, 255, 255]);
                        let c = m.cover.unwrap();
                        page.fill_rect(c[0], c[1], c[2] - c[0], c[3] - c[1], f);
                        page.fill_rect(x, y, w, h, f);
                    }
                    let mut style = pdfgen::TextStyle::new(font, m.font_size);
                    style.color = m.color;
                    let asc = d.ascent(font) * m.font_size;
                    for (i, l) in text.split('\n').enumerate() {
                        d.text(&mut page, style, x, y + asc + i as f32 * m.font_size * 1.2, l);
                    }
                }
                Kind::Text(text) => {
                    let font = if m.font.is_empty() { font } else { d.font(&m.font, m.bold, m.italic) };
                    if let Some(f) = m.fill {
                        page.fill_rect(x, y, w, h, f);
                    }
                    let lines = d.wrap(font, m.font_size, text, w.max(4.0));
                    let mut style = pdfgen::TextStyle::new(font, m.font_size);
                    style.color = m.color;
                    let asc = d.ascent(font) * m.font_size;
                    for (i, l) in lines.iter().enumerate() {
                        d.text(&mut page, style, x, y + asc + i as f32 * m.font_size * 1.2, l);
                    }
                }
                _ => {}
            }
        }
        d.add_page(page);
        pdfops::stamp(doc, pid, &d.finish(), false)?;
    }
    // 3. Annotations and form fields.
    let date = Object::string_literal(chrono::Local::now().format("D:%Y%m%d%H%M%S").to_string());
    for m in marks.iter().filter(|m| !m.is_flattened() && !matches!(m.kind, Kind::Redact)) {
        let Some(&pid) = pages.get(m.page) else { continue };
        let g = pdfops::geom(doc, pid);
        let user = g.rect_to_user(m.rect);
        let rect: Vec<Object> = user.iter().map(|v| Object::Real(*v)).collect();
        let quad = || {
            let pts = [[m.rect[0], m.rect[1]], [m.rect[2], m.rect[1]], [m.rect[0], m.rect[3]], [m.rect[2], m.rect[3]]];
            let mut v = Vec::new();
            for p in pts {
                let (x, y) = g.to_user(p[0], p[1]);
                v.push(Object::Real(x));
                v.push(Object::Real(y));
            }
            Object::Array(v)
        };
        let mut d = dictionary! {
            "Type" => "Annot",
            "Rect" => rect,
            "F" => 4,
            "M" => date.clone(),
            "NM" => Object::string_literal(format!("k42-{:x}", m.id)),
            "T" => pdfops::text_string(author),
            "C" => color_arr(m.color),
            "P" => pid,
        };
        match &m.kind {
            Kind::Highlight => {
                d.set("Subtype", "Highlight");
                d.set("QuadPoints", quad());
            }
            Kind::Underline => {
                d.set("Subtype", "Underline");
                d.set("QuadPoints", quad());
            }
            Kind::Strike => {
                d.set("Subtype", "StrikeOut");
                d.set("QuadPoints", quad());
            }
            Kind::Ink(paths) => {
                d.set("Subtype", "Ink");
                let list: Vec<Object> = paths
                    .iter()
                    .map(|p| {
                        Object::Array(
                            p.iter()
                                .flat_map(|q| {
                                    let (x, y) = g.to_user(q[0], q[1]);
                                    [Object::Real(x), Object::Real(y)]
                                })
                                .collect(),
                        )
                    })
                    .collect();
                d.set("InkList", list);
                d.set("BS", dictionary! { "W" => Object::Real(m.width) });
            }
            Kind::Rect | Kind::Ellipse => {
                d.set("Subtype", if matches!(m.kind, Kind::Rect) { "Square" } else { "Circle" });
                d.set("BS", dictionary! { "W" => Object::Real(m.width) });
                if let Some(f) = m.fill {
                    d.set("IC", color_arr(f));
                }
            }
            Kind::Line(a, b) | Kind::Arrow(a, b) => {
                d.set("Subtype", "Line");
                let (ax, ay) = g.to_user(a[0], a[1]);
                let (bx, by) = g.to_user(b[0], b[1]);
                d.set("L", vec![Object::Real(ax), Object::Real(ay), Object::Real(bx), Object::Real(by)]);
                d.set("BS", dictionary! { "W" => Object::Real(m.width) });
                if matches!(m.kind, Kind::Arrow(..)) {
                    d.set("LE", vec![Object::Name(b"None".to_vec()), Object::Name(b"ClosedArrow".to_vec())]);
                    d.set("IC", color_arr(m.color));
                }
            }
            Kind::Note(text) => {
                d.set("Subtype", "Text");
                d.set("Name", "Comment");
                d.set("Contents", pdfops::text_string(text));
                let (x, y) = g.to_user(m.rect[0], m.rect[1]);
                d.set("Rect", vec![Object::Real(x), Object::Real(y - 22.0), Object::Real(x + 22.0), Object::Real(y)]);
                d.set("Open", false);
            }
            Kind::Link(target) => {
                d.set("Subtype", "Link");
                d.set("Border", vec![0.into(), 0.into(), 0.into()]);
                d.remove(b"T");
                if let Some(p) = target.strip_prefix('#').and_then(|s| s.parse::<usize>().ok()) {
                    if let Some(&dest) = pages.get(p.saturating_sub(1)) {
                        d.set("Dest", vec![Object::Reference(dest), Object::Name(b"Fit".to_vec())]);
                    }
                } else {
                    let uri = if target.contains('@') && !target.contains(':') { format!("mailto:{target}") } else if !target.contains(':') { format!("https://{target}") } else { target.clone() };
                    d.set("A", dictionary! { "S" => "URI", "URI" => Object::string_literal(uri) });
                }
            }
            Kind::FieldText(name) => {
                pdfops::add_field(doc, pid, name, &pdfops::FieldKind::Text, user)?;
                continue;
            }
            Kind::FieldCheck(name) => {
                pdfops::add_field(doc, pid, name, &pdfops::FieldKind::Check, user)?;
                continue;
            }
            _ => continue,
        }
        if m.opacity < 1.0 {
            d.set("CA", Object::Real(m.opacity));
        }
        if let Some(ap) = appearance(doc, &g, m) {
            d.set("AP", dictionary! { "N" => ap });
        }
        let aid = doc.add_object(d);
        pdfops::add_annot_ref(doc, pid, aid)?;
    }
    Ok(notes)
}

/// A text mark that replaces a recognised page line in place, in the line's font and colour.
pub fn line_edit(page: usize, l: &crate::textlayer::Line, text: &str) -> Mark {
    let asc = {
        let mut d = pdfgen::Doc::new();
        let f = d.font(&l.font, l.bold, l.italic);
        d.ascent(f)
    };
    let top = l.baseline - asc * l.size;
    let mut m = Mark::new(page, [l.rect[0], top, l.rect[2] + 1.0, top + l.size * 1.25], Kind::Text(text.to_string()), l.color, 1.0);
    m.font_size = l.size;
    m.font = l.font.clone();
    m.bold = l.bold;
    m.italic = l.italic;
    m.fill = Some(l.bg);
    m.cover = Some([l.rect[0] - 0.8, l.rect[1] - 0.8, l.rect[2] + 0.8, l.rect[3] + 0.8]);
    m.cover_always = l.hidden;
    m
}
