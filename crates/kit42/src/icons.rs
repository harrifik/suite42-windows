//! Segoe Fluent Icons code points and generated app icons.

pub const OPEN: &str = "\u{E8E5}";
pub const SAVE: &str = "\u{E74E}";
pub const SAVE_AS: &str = "\u{E792}";
pub const PRINT: &str = "\u{E749}";
pub const UNDO: &str = "\u{E7A7}";
pub const REDO: &str = "\u{E7A6}";
pub const ZOOM_IN: &str = "\u{E8A3}";
pub const ZOOM_OUT: &str = "\u{E71F}";
pub const DELETE: &str = "\u{E74D}";
pub const ADD: &str = "\u{E710}";
pub const SEARCH: &str = "\u{E721}";
pub const EDIT: &str = "\u{E70F}";
pub const COPY: &str = "\u{E8C8}";
pub const PASTE: &str = "\u{E77F}";
pub const CUT: &str = "\u{E8C6}";
pub const CROP: &str = "\u{E7A8}";
pub const BACK: &str = "\u{E72B}";
pub const FORWARD: &str = "\u{E72A}";
pub const UP: &str = "\u{E74A}";
pub const DOWN: &str = "\u{E74B}";
pub const PLAY: &str = "\u{E768}";
pub const PAUSE: &str = "\u{E769}";
pub const STOP: &str = "\u{E71A}";
pub const SETTINGS: &str = "\u{E713}";
pub const INFO: &str = "\u{E946}";
pub const FULLSCREEN: &str = "\u{E740}";
pub const ROTATE_RIGHT: &str = "\u{E7AD}";
pub const ROTATE_LEFT: &str = "\u{E7AD}";
pub const PICTURE: &str = "\u{E8B9}";
pub const CAMERA: &str = "\u{E722}";
pub const DOCUMENT: &str = "\u{E8A5}";
pub const BOLD: &str = "\u{E8DD}";
pub const ITALIC: &str = "\u{E8DB}";
pub const UNDERLINE: &str = "\u{E8DC}";
pub const ALIGN_LEFT: &str = "\u{E8E4}";
pub const ALIGN_CENTER: &str = "\u{E8E3}";
pub const ALIGN_RIGHT: &str = "\u{E8E2}";
pub const PEN: &str = "\u{E76D}";
pub const HIGHLIGHT: &str = "\u{E7E6}";
pub const FONT: &str = "\u{E8D2}";
pub const COLOR: &str = "\u{E790}";
pub const LIST: &str = "\u{E8FD}";
pub const FILTER: &str = "\u{E71C}";
pub const SORT: &str = "\u{E8CB}";
pub const PAGE: &str = "\u{E7C3}";
pub const SCAN: &str = "\u{E8FE}";
pub const SIGNATURE: &str = "\u{EE56}";
pub const SHARE: &str = "\u{E72D}";
pub const FOLDER: &str = "\u{E8B7}";
pub const MERGE: &str = "\u{E8C8}";
pub const TABLE: &str = "\u{E80A}";
pub const SHAPES: &str = "\u{E91B}";
pub const TEXT: &str = "\u{E8D2}";
pub const CHART: &str = "\u{E9D2}";
pub const LOCK: &str = "\u{E72E}";
pub const LINK: &str = "\u{E71B}";
pub const BOOKMARK: &str = "\u{E8A4}";
pub const SIDEBAR: &str = "\u{E8A0}";
pub const FIT: &str = "\u{E9A6}";
pub const SELECT: &str = "\u{E8B3}";
pub const ERASE: &str = "\u{E75C}";
pub const COMMENT: &str = "\u{E90A}";
pub const SLIDESHOW: &str = "\u{E786}";
pub const FUNCTION: &str = "\u{E8EF}";
pub const OCR: &str = "\u{E8C1}";
pub const WAND: &str = "\u{E7E7}";
pub const CHECK: &str = "\u{E73E}";
pub const CLOSE: &str = "\u{E711}";

/// Accent colour and monogram of each app, used for window and taskbar icons.
pub fn app_style(id: &str) -> ([u8; 3], &'static str) {
    match id {
        "pdf42" => ([0xD1, 0x34, 0x38], "PDF"),
        "doc42" => ([0x18, 0x5A, 0xBD], "DOC"),
        "xls42" => ([0x10, 0x7C, 0x41], "XLS"),
        "img42" => ([0x88, 0x17, 0x98], "IMG"),
        "ppt42" => ([0xC4, 0x3E, 0x1C], "PPT"),
        "scn42" => ([0x00, 0x78, 0x7A], "SCN"),
        _ => ([0x44, 0x44, 0x44], "42"),
    }
}

/// Renders a rounded-square icon with the app's monogram.
pub fn app_icon_image(id: &str, size: u32) -> image::RgbaImage {
    let ([r, g, b], label) = app_style(id);
    let mut img = image::RgbaImage::new(size, size);
    let s = size as f32;
    let radius = s * 0.22;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let dx = (radius - fx).max(fx - (s - radius)).max(0.0);
            let dy = (radius - fy).max(fy - (s - radius)).max(0.0);
            let d = (dx * dx + dy * dy).sqrt();
            let a = (radius - d + 0.5).clamp(0.0, 1.0);
            if a > 0.0 {
                let shade = 1.0 - 0.18 * (fy / s);
                img.put_pixel(
                    x,
                    y,
                    image::Rgba([
                        (r as f32 * shade) as u8,
                        (g as f32 * shade) as u8,
                        (b as f32 * shade) as u8,
                        (a * 255.0) as u8,
                    ]),
                );
            }
        }
    }
    if let Some(font) = crate::fonts::load_bytes("Segoe UI", true, false) {
        let px = s * if label.len() > 2 { 0.34 } else { 0.5 };
        crate::raster::draw_text_centered(&mut img, &font, label, px, s / 2.0, s * 0.42, [255, 255, 255, 255]);
        crate::raster::draw_text_centered(&mut img, &font, "42", s * 0.2, s / 2.0, s * 0.74, [255, 255, 255, 200]);
    }
    img
}

pub fn app_icon(id: &str) -> Option<egui::IconData> {
    let img = app_icon_image(id, 64);
    Some(egui::IconData { rgba: img.into_raw(), width: 64, height: 64 })
}
