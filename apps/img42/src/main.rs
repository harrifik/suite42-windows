#![windows_subsystem = "windows"]
//! IMG42 for Windows — image viewer and quick editor.

mod marks;
mod pixels;

use egui::{Color32, Context, Key, Pos2, Rect, Sense, Stroke, Vec2};
use kit42::history::History;
use kit42::icons as ic;
use kit42::ui::{self as kui, CTRL, CTRL_SHIFT, NONE};
use kit42::winrt::OcrResult;
use kit42::{settings, t};
use marks::{Kind, Mark, Tool};
use pixels::{Adjust, Look};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

pub const APP: &str = "IMG42";
const EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "ico", "tga", "pnm", "ppm", "pgm", "qoi", "hdr", "dds", "heic", "heif", "avif", "jxr", "wdp", "dng", "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "svg", "pdf", "jfif"];

#[derive(Clone)]
struct EditState {
    image: Arc<image::RgbaImage>,
    marks: Vec<Mark>,
    adjust: Adjust,
}

struct Loaded {
    path: PathBuf,
    frames: Vec<(Arc<image::RgbaImage>, u32)>,
    format: String,
    exif: Vec<(String, String)>,
    pages: usize,
    page: usize,
    dpi: (f32, f32),
}

#[derive(Clone, Copy, PartialEq)]
enum Bg {
    Neutral,
    Dark,
    Black,
    Light,
}

enum Drag {
    Pan,
    Create { start: [f32; 2], cur: [f32; 2], pts: Vec<[f32; 2]> },
    Move { id: u64, last: [f32; 2] },
    Resize { id: u64, corner: usize, last: [f32; 2] },
    Select { start: [f32; 2] },
    TextSel { start: Pos2 },
}

struct ResizeDlg {
    w: u32,
    h: u32,
    keep: bool,
    percent: f32,
    dpi: f32,
    resample: bool,
}

struct App {
    files: Vec<PathBuf>,
    index: usize,
    cur: Option<Loaded>,
    edit: Option<EditState>,
    history: History<EditState>,
    tex: Option<(u64, egui::TextureHandle)>,
    gen: u64,
    frame: usize,
    frame_clock: f64,
    zoom: Option<f32>,
    offset: Vec2,
    bg: Bg,
    tool: Tool,
    color: [u8; 3],
    fill: Option<[u8; 3]>,
    width: f32,
    font_size: f32,
    selected: Option<u64>,
    sel_area: Option<([f32; 4], bool)>,
    drag: Option<Drag>,
    editing_text: Option<u64>,
    show_info: bool,
    show_adjust: bool,
    show_film: bool,
    show_markup: bool,
    thumbs: HashMap<PathBuf, egui::TextureHandle>,
    thumb_rx: Option<Receiver<(PathBuf, image::RgbaImage)>>,
    thumb_tx: std::sync::mpsc::Sender<(PathBuf, image::RgbaImage)>,
    thumb_pending: std::collections::HashSet<PathBuf>,
    slideshow: Option<f64>,
    loop_folder: bool,
    sort: u8,
    ocr: Option<(u64, Arc<OcrResult>)>,
    ocr_rx: Option<Receiver<(u64, Result<OcrResult, String>)>>,
    search: String,
    text_sel: Vec<(usize, usize)>,
    picker: kit42::signature::MarkPicker,
    mark_tex: HashMap<u64, egui::TextureHandle>,
    resize: Option<ResizeDlg>,
    export: Option<(u8, u8)>,
    status: String,
    preview_cache: Option<(u64, Adjust, Arc<image::RgbaImage>)>,
    show_original: bool,
    alpha_tol: f32,
    description: String,
    fullscreen: bool,
}

fn natural_key(p: &Path) -> Vec<(String, u64)> {
    let name = p.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut out = vec![];
    let mut text = String::new();
    let mut num = String::new();
    for c in name.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push((std::mem::take(&mut text), num.parse().unwrap_or(0)));
                num.clear();
            }
            text.push(c);
        }
    }
    out.push((text, num.parse().unwrap_or(0)));
    out
}

pub fn is_image(p: &Path) -> bool {
    EXTS.contains(&kit42::ext(p).as_str())
}

fn read_exif(bytes: &[u8]) -> (Vec<(String, String)>, u32, (f32, f32)) {
    let mut fields = vec![];
    let mut orientation = 1;
    let mut dpi = (72.0, 72.0);
    if let Ok(ex) = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(bytes)) {
        for f in ex.fields() {
            if f.ifd_num != exif::In::PRIMARY && f.ifd_num != exif::In::THUMBNAIL {
                continue;
            }
            if f.ifd_num == exif::In::THUMBNAIL {
                continue;
            }
            let v = f.display_value().with_unit(&ex).to_string();
            if f.tag == exif::Tag::Orientation {
                orientation = f.value.get_uint(0).unwrap_or(1);
            }
            if f.tag == exif::Tag::XResolution {
                if let exif::Value::Rational(r) = &f.value {
                    dpi.0 = r.first().map(|x| x.to_f64() as f32).unwrap_or(72.0);
                }
            }
            if f.tag == exif::Tag::YResolution {
                if let exif::Value::Rational(r) = &f.value {
                    dpi.1 = r.first().map(|x| x.to_f64() as f32).unwrap_or(72.0);
                }
            }
            if v.len() < 200 && f.tag != exif::Tag::MakerNote {
                fields.push((f.tag.to_string(), v));
            }
        }
    }
    (fields, orientation, dpi)
}

fn orient(img: image::RgbaImage, o: u32) -> image::RgbaImage {
    use image::imageops::*;
    match o {
        2 => flip_horizontal(&img),
        3 => rotate180(&img),
        4 => flip_vertical(&img),
        5 => flip_horizontal(&rotate90(&img)),
        6 => rotate90(&img),
        7 => flip_horizontal(&rotate270(&img)),
        8 => rotate270(&img),
        _ => img,
    }
}

const MAX_PIXELS: u64 = 100_000_000;

fn bound(img: image::RgbaImage) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    if w <= 16384 && h <= 16384 && (w as u64 * h as u64) <= MAX_PIXELS {
        return img;
    }
    let s = (16384.0 / w.max(h) as f64).min((MAX_PIXELS as f64 / (w as f64 * h as f64)).sqrt());
    image::imageops::resize(&img, (w as f64 * s) as u32, (h as f64 * s) as u32, image::imageops::FilterType::Triangle)
}

fn load(path: &Path, page: usize) -> Result<Loaded, String> {
    let bytes = kit42::ooxml::read_file(path)?;
    let ext = kit42::ext(path);
    let (exif, orientation, dpi) = read_exif(&bytes);
    let mut frames = vec![];
    let mut pages = 1;
    let format;
    if ext == "svg" {
        let tree = resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default()).map_err(|e| e.to_string())?;
        let size = tree.size();
        let s = (4096.0 / size.width().max(size.height())).min(4.0).max(0.1);
        let (w, h) = ((size.width() * s).ceil() as u32, (size.height() * s).ceil() as u32);
        let mut pm = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1)).ok_or("svg size")?;
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(s, s), &mut pm.as_mut());
        let mut img = image::RgbaImage::new(pm.width(), pm.height());
        for (d, p) in img.pixels_mut().zip(pm.pixels()) {
            let c = p.demultiply();
            *d = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
        }
        frames.push((Arc::new(img), 0));
        format = "SVG".into();
    } else if ext == "pdf" || bytes.starts_with(b"%PDF") {
        let (r, sizes) = kit42::winrt::PdfRenderer::open(bytes, None)?;
        pages = sizes.len();
        let p = page.min(pages.saturating_sub(1));
        let (w, h) = sizes[p];
        let px = (4000.0 / w.max(h)).min(300.0 / 72.0) * w;
        frames.push((Arc::new(r.render(p as u32, px as u32)?), 0));
        format = "PDF".into();
    } else if bytes.starts_with(b"GIF8") {
        use image::AnimationDecoder;
        let dec = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&bytes)).map_err(|e| e.to_string())?;
        for f in dec.into_frames().take(2000) {
            let f = f.map_err(|e| e.to_string())?;
            let (n, d) = f.delay().numer_denom_ms();
            frames.push((Arc::new(f.into_buffer()), (n / d.max(1)).max(20)));
        }
        format = "GIF".into();
    } else if ext == "tif" || ext == "tiff" {
        // Multi-page TIFF: decode the requested page with the tiff crate via image's decoder.
        let img = image::load_from_memory(&bytes).map(|i| i.to_rgba8()).or_else(|_| kit42::winrt::decode_image(bytes.clone()))?;
        frames.push((Arc::new(orient(img, orientation)), 0));
        format = "TIFF".into();
    } else {
        let fmt = image::guess_format(&bytes).ok();
        let img = match image::load_from_memory(&bytes) {
            Ok(i) => orient(i.to_rgba8(), orientation),
            // HEIC, AVIF, RAW, JPEG XR… through Windows Imaging Component (orientation already applied).
            Err(_) => kit42::winrt::decode_image(bytes.clone()).map_err(|e| format!("{}: {e}", t("This format needs a Windows codec (e.g. HEIF/RAW extensions from the Store)", "Для этого формата нужен кодек Windows (например, расширения HEIF/RAW из Store)")))?,
        };
        frames.push((Arc::new(bound(img)), 0));
        format = fmt.map(|f| format!("{f:?}").to_uppercase()).unwrap_or_else(|| ext.to_uppercase());
    }
    Ok(Loaded { path: path.to_path_buf(), frames, format, exif, pages, page, dpi })
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = channel();
        let mut app = Self {
            files: vec![],
            index: 0,
            cur: None,
            edit: None,
            history: History::new(40),
            tex: None,
            gen: 0,
            frame: 0,
            frame_clock: 0.0,
            zoom: None,
            offset: Vec2::ZERO,
            bg: Bg::Neutral,
            tool: Tool::Select,
            color: [230, 30, 30],
            fill: None,
            width: 4.0,
            font_size: 36.0,
            selected: None,
            sel_area: None,
            drag: None,
            editing_text: None,
            show_info: settings::get_or("img42.info", false),
            show_adjust: false,
            show_film: settings::get_or("img42.film", true),
            show_markup: false,
            thumbs: HashMap::new(),
            thumb_rx: Some(rx),
            thumb_tx: tx,
            thumb_pending: Default::default(),
            slideshow: None,
            loop_folder: settings::get_or("img42.loop", true),
            sort: 0,
            ocr: None,
            ocr_rx: None,
            search: String::new(),
            text_sel: vec![],
            picker: Default::default(),
            mark_tex: HashMap::new(),
            resize: None,
            export: None,
            status: String::new(),
            preview_cache: None,
            show_original: false,
            alpha_tol: 40.0,
            description: String::new(),
            fullscreen: false,
        };
        let files = kit42::startup_files();
        if !files.is_empty() {
            app.open_set(files);
        }
        app
    }

    fn dirty(&self) -> bool {
        self.history.can_undo() && self.history.is_dirty()
    }

    fn open_set(&mut self, paths: Vec<PathBuf>) {
        if !self.leave() {
            return;
        }
        let (list, start) = if paths.len() == 1 && paths[0].is_file() {
            let p = &paths[0];
            let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
            let mut list: Vec<PathBuf> = std::fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| is_image(p)).collect()).unwrap_or_default();
            self.sort_list(&mut list);
            let idx = list.iter().position(|x| x == p).unwrap_or(0);
            if list.is_empty() {
                list.push(p.clone());
            }
            (list, idx)
        } else {
            let mut list = vec![];
            for p in paths {
                if p.is_dir() {
                    if let Ok(rd) = std::fs::read_dir(&p) {
                        list.extend(rd.flatten().map(|e| e.path()).filter(|p| is_image(p)));
                    }
                } else {
                    list.push(p);
                }
            }
            self.sort_list(&mut list);
            (list, 0)
        };
        self.files = list;
        self.show(start);
    }

    fn sort_list(&self, list: &mut [PathBuf]) {
        match self.sort {
            1 => list.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()),
            2 => list.sort_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)),
            _ => list.sort_by(|a, b| natural_key(a).cmp(&natural_key(b))),
        }
    }

    fn show(&mut self, i: usize) {
        if self.files.is_empty() {
            return;
        }
        self.index = i.min(self.files.len() - 1);
        self.load_current(0);
    }

    fn load_current(&mut self, page: usize) {
        let p = self.files[self.index].clone();
        self.history.clear();
        self.selected = None;
        self.sel_area = None;
        self.ocr = None;
        self.text_sel.clear();
        self.mark_tex.clear();
        self.frame = 0;
        self.zoom = None;
        self.offset = Vec2::ZERO;
        self.preview_cache = None;
        match load(&p, page) {
            Ok(l) => {
                self.edit = Some(EditState { image: l.frames[0].0.clone(), marks: vec![], adjust: Adjust::default() });
                self.description = l.exif.iter().find(|(k, _)| k == "ImageDescription").map(|(_, v)| v.trim_matches('"').to_string()).unwrap_or_default();
                self.cur = Some(l);
                settings::add_recent("img42", &p);
                self.status.clear();
            }
            Err(e) => {
                self.cur = None;
                self.edit = None;
                self.status = format!("{}: {e}", kit42::file_name(&p));
            }
        }
        self.gen += 1;
        self.tex = None;
    }

    /// Ask to save before leaving an edited image.
    fn leave(&mut self) -> bool {
        if !self.dirty() {
            return true;
        }
        let name = self.cur.as_ref().map(|c| kit42::file_name(&c.path)).unwrap_or_default();
        match kui::ask_save(&name) {
            kui::SaveChoice::Save => self.save(false),
            kui::SaveChoice::Discard => true,
            kui::SaveChoice::Cancel => false,
        }
    }

    fn go(&mut self, delta: i64) {
        if self.files.is_empty() || !self.leave() {
            return;
        }
        let n = self.files.len() as i64;
        let mut i = self.index as i64 + delta;
        if self.loop_folder {
            i = i.rem_euclid(n);
        } else {
            i = i.clamp(0, n - 1);
        }
        self.show(i as usize);
    }

    fn checkpoint(&mut self, label: &str) {
        if let Some(e) = &self.edit {
            self.history.push(e.clone(), label);
        }
    }

    /// The edited image with adjustments (and optionally marks) applied, at full resolution.
    fn rendered(&self, with_marks: bool) -> Option<image::RgbaImage> {
        let e = self.edit.as_ref()?;
        let img = pixels::apply(&e.image, &e.adjust);
        Some(if with_marks { marks::flatten(&img, &e.marks) } else { img })
    }

    fn image_op(&mut self, label: &str, f: impl FnOnce(&image::RgbaImage) -> image::RgbaImage, map: Option<&dyn Fn([f32; 2], (u32, u32)) -> [f32; 2]>) {
        let Some(e) = &self.edit else { return };
        let dims = e.image.dimensions();
        let new = f(&e.image);
        self.checkpoint(label);
        let e = self.edit.as_mut().unwrap();
        if let Some(m) = map {
            for mk in &mut e.marks {
                mk.map_points(&|p| m(p, dims));
            }
        }
        e.image = Arc::new(new);
        self.gen += 1;
        self.ocr = None;
    }

    fn rotate(&mut self, right: bool) {
        if right {
            self.image_op(t("Rotate", "Поворот"), image::imageops::rotate90, Some(&|p, (_, h)| [h as f32 - p[1], p[0]]));
        } else {
            self.image_op(t("Rotate", "Поворот"), image::imageops::rotate270, Some(&|p, (w, _)| [p[1], w as f32 - p[0]]));
        }
    }

    fn flip(&mut self, horizontal: bool) {
        if horizontal {
            self.image_op(t("Flip", "Отражение"), image::imageops::flip_horizontal, Some(&|p, (w, _)| [w as f32 - p[0], p[1]]));
        } else {
            self.image_op(t("Flip", "Отражение"), image::imageops::flip_vertical, Some(&|p, (_, h)| [p[0], h as f32 - p[1]]));
        }
    }

    fn crop_to_selection(&mut self) {
        let Some((r, ellipse)) = self.sel_area else { return };
        let Some(e) = &self.edit else { return };
        let (w, h) = e.image.dimensions();
        let x0 = r[0].max(0.0) as u32;
        let y0 = r[1].max(0.0) as u32;
        let x1 = (r[2] as u32).min(w);
        let y1 = (r[3] as u32).min(h);
        if x1 <= x0 + 1 || y1 <= y0 + 1 {
            return;
        }
        let (fx, fy) = (x0 as f32, y0 as f32);
        self.image_op(
            t("Crop", "Обрезка"),
            |img| {
                let mut out = image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image();
                if ellipse {
                    let (cw, ch) = out.dimensions();
                    for (x, y, p) in out.enumerate_pixels_mut() {
                        let dx = (x as f32 + 0.5) / cw as f32 * 2.0 - 1.0;
                        let dy = (y as f32 + 0.5) / ch as f32 * 2.0 - 1.0;
                        if dx * dx + dy * dy > 1.0 {
                            p[3] = 0;
                        }
                    }
                }
                out
            },
            Some(&move |p, _| [p[0] - fx, p[1] - fy]),
        );
        self.sel_area = None;
    }

    fn delete_selection(&mut self) {
        let Some((r, ellipse)) = self.sel_area else { return };
        self.image_op(
            t("Delete Selection", "Удаление выделенного"),
            |img| {
                let mut out = img.clone();
                for (x, y, p) in out.enumerate_pixels_mut() {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let inside = if ellipse {
                        let dx = (fx - (r[0] + r[2]) / 2.0) / ((r[2] - r[0]) / 2.0);
                        let dy = (fy - (r[1] + r[3]) / 2.0) / ((r[3] - r[1]) / 2.0);
                        dx * dx + dy * dy <= 1.0
                    } else {
                        fx >= r[0] && fx < r[2] && fy >= r[1] && fy < r[3]
                    };
                    if inside {
                        p[3] = 0;
                    }
                }
                out
            },
            None,
        );
    }

    fn copy_image(&mut self) {
        let Some(mut img) = self.rendered(true) else { return };
        if let Some((r, _)) = self.sel_area {
            let (w, h) = img.dimensions();
            let x0 = r[0].max(0.0) as u32;
            let y0 = r[1].max(0.0) as u32;
            let x1 = (r[2] as u32).min(w);
            let y1 = (r[3] as u32).min(h);
            if x1 > x0 && y1 > y0 {
                img = image::imageops::crop_imm(&img, x0, y0, x1 - x0, y1 - y0).to_image();
            }
        }
        if let Ok(mut cb) = arboard::Clipboard::new() {
            let (w, h) = img.dimensions();
            let _ = cb.set_image(arboard::ImageData { width: w as usize, height: h as usize, bytes: std::borrow::Cow::Owned(img.into_raw()) });
            self.status = t("Copied to the clipboard.", "Скопировано в буфер обмена.").into();
        }
    }

    fn encode(&self, img: &image::RgbaImage, ext: &str, quality: u8) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut cur = std::io::Cursor::new(&mut out);
        match ext {
            "jpg" | "jpeg" | "jfif" => {
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cur, quality).encode_image(&pixels::flatten_white(img)).map_err(|e| e.to_string())?;
            }
            "png" => image::DynamicImage::ImageRgba8(img.clone()).write_to(&mut cur, image::ImageFormat::Png).map_err(|e| e.to_string())?,
            "bmp" => image::DynamicImage::ImageRgb8(pixels::flatten_white(img)).write_to(&mut cur, image::ImageFormat::Bmp).map_err(|e| e.to_string())?,
            "tif" | "tiff" => image::DynamicImage::ImageRgba8(img.clone()).write_to(&mut cur, image::ImageFormat::Tiff).map_err(|e| e.to_string())?,
            "gif" => image::DynamicImage::ImageRgba8(img.clone()).write_to(&mut cur, image::ImageFormat::Gif).map_err(|e| e.to_string())?,
            "webp" => image::DynamicImage::ImageRgba8(img.clone()).write_to(&mut cur, image::ImageFormat::WebP).map_err(|e| e.to_string())?,
            "ico" => image::DynamicImage::ImageRgba8(kit42::raster::fit_within(img, 256)).write_to(&mut cur, image::ImageFormat::Ico).map_err(|e| e.to_string())?,
            "tga" => image::DynamicImage::ImageRgba8(img.clone()).write_to(&mut cur, image::ImageFormat::Tga).map_err(|e| e.to_string())?,
            "qoi" => image::DynamicImage::ImageRgba8(img.clone()).write_to(&mut cur, image::ImageFormat::Qoi).map_err(|e| e.to_string())?,
            _ => return Err(format!("{} .{ext}", t("Cannot write", "Нельзя записать"))),
        }
        Ok(out)
    }

    fn save(&mut self, ask: bool) -> bool {
        let Some(cur) = &self.cur else { return false };
        let path = cur.path.clone();
        let ext = kit42::ext(&path);
        let writable = ["jpg", "jpeg", "jfif", "png", "bmp", "tif", "tiff", "webp", "gif", "tga", "ico", "qoi"].contains(&ext.as_str()) && cur.frames.len() == 1 && cur.pages == 1;
        if !writable {
            return self.save_as();
        }
        if ask && !kui::confirm(APP, &format!("{} «{}»?", t("Replace the original file", "Заменить исходный файл"), kit42::file_name(&path))) {
            return false;
        }
        let Some(img) = self.rendered(true) else { return false };
        match self.encode(&img, &ext, 92).and_then(|b| kit42::write_atomic(&path, &b).map_err(|e| e.to_string())) {
            Ok(()) => {
                self.history.mark_saved();
                self.status = format!("{} {}", t("Saved:", "Сохранено:"), kit42::file_name(&path));
                let page = self.cur.as_ref().map(|c| c.page).unwrap_or(0);
                self.thumbs.remove(&path);
                self.load_current(page);
                true
            }
            Err(e) => {
                kui::error(APP, &format!("{}\n{e}", t("The image was not saved; the file is unchanged.", "Изображение не сохранено; файл не изменён.")));
                false
            }
        }
    }

    fn save_as(&mut self) -> bool {
        self.export = Some((0, 92));
        false
    }

    fn do_export(&mut self, fmt: u8, quality: u8) {
        let exts = ["png", "jpg", "webp", "tif", "bmp", "gif", "ico"];
        let ext = exts[fmt as usize % exts.len()];
        let stem = self.cur.as_ref().and_then(|c| c.path.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
        let dir = self.cur.as_ref().and_then(|c| c.path.parent()).map(|p| p.to_path_buf()).unwrap_or_default();
        let Some(p) = rfd::FileDialog::new().set_directory(dir).add_filter(ext.to_uppercase(), &[ext]).set_file_name(format!("{stem}.{ext}")).save_file() else { return };
        let Some(img) = self.rendered(true) else { return };
        match self.encode(&img, ext, quality).and_then(|b| kit42::write_atomic(&p, &b).map_err(|e| e.to_string())) {
            Ok(()) => {
                self.status = format!("{} {}", t("Exported:", "Экспортировано:"), kit42::file_name(&p));
                if self.cur.as_ref().map(|c| c.path == p).unwrap_or(false) {
                    self.history.mark_saved();
                }
            }
            Err(e) => kui::error(APP, &e),
        }
    }

    fn trash(&mut self) {
        let Some(cur) = &self.cur else { return };
        let p = cur.path.clone();
        if !kui::confirm(APP, &format!("{} «{}»?", t("Move to the Recycle Bin", "Переместить в корзину"), kit42::file_name(&p))) {
            return;
        }
        match trash::delete(&p) {
            Ok(()) => {
                self.files.retain(|x| *x != p);
                self.history.clear();
                if self.files.is_empty() {
                    self.cur = None;
                    self.edit = None;
                } else {
                    let i = self.index.min(self.files.len() - 1);
                    self.show(i);
                }
            }
            Err(e) => kui::error(APP, &e.to_string()),
        }
    }

    fn start_ocr(&mut self) {
        let Some(img) = self.rendered(false) else { return };
        let gen = self.gen;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send((gen, kit42::winrt::ocr(&img, None)));
        });
        self.ocr_rx = Some(rx);
        self.status = t("Recognising text…", "Распознавание текста…").into();
    }

    fn ocr_words(&self) -> Vec<(usize, usize, Rect, String)> {
        let Some((_, o)) = &self.ocr else { return vec![] };
        let mut v = vec![];
        for (li, l) in o.lines.iter().enumerate() {
            for (wi, w) in l.words.iter().enumerate() {
                v.push((li, wi, Rect::from_min_size(Pos2::new(w.x, w.y), Vec2::new(w.w, w.h)), w.text.clone()));
            }
        }
        v
    }

    fn selected_text(&self) -> String {
        let Some((_, o)) = &self.ocr else { return String::new() };
        let mut out = String::new();
        let mut last_line = usize::MAX;
        for (li, wi) in &self.text_sel {
            if let Some(w) = o.lines.get(*li).and_then(|l| l.words.get(*wi)) {
                if last_line != usize::MAX {
                    out.push(if *li != last_line { '\n' } else { ' ' });
                }
                out.push_str(&w.text);
                last_line = *li;
            }
        }
        out
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let title = self.cur.as_ref().map(|c| format!("{} — {} / {}", kit42::file_name(&c.path), self.index + 1, self.files.len()));
        kui::set_title(ctx, APP, title.as_deref(), self.dirty());
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.open_set(dropped);
        }
        // Background results.
        if let Some(rx) = &self.thumb_rx {
            while let Ok((p, img)) = rx.try_recv() {
                self.thumb_pending.remove(&p);
                let tex = kui::texture(ctx, &p.to_string_lossy(), &img);
                self.thumbs.insert(p, tex);
            }
        }
        if let Some(rx) = &self.ocr_rx {
            if let Ok((g, res)) = rx.try_recv() {
                self.ocr_rx = None;
                match res {
                    Ok(o) if g == self.gen => {
                        let n: usize = o.lines.iter().map(|l| l.words.len()).sum();
                        self.status = format!("{} {n}", t("Words recognised:", "Распознано слов:"));
                        self.ocr = Some((g, Arc::new(o)));
                    }
                    Ok(_) => {}
                    Err(e) => self.status = format!("OCR: {e}"),
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        // Animation and slideshow.
        let now = ctx.input(|i| i.time);
        if let Some(cur) = &self.cur {
            if cur.frames.len() > 1 && self.history.can_undo() == false {
                let delay = cur.frames[self.frame].1 as f64 / 1000.0;
                if now - self.frame_clock >= delay {
                    self.frame = (self.frame + 1) % cur.frames.len();
                    self.frame_clock = now;
                    if let Some(e) = &mut self.edit {
                        e.image = cur.frames[self.frame].0.clone();
                    }
                    self.gen += 1;
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(20));
            }
        }
        if let Some(last) = self.slideshow {
            if now - last > 3.0 {
                self.slideshow = Some(now);
                self.go(1);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        self.shortcuts(ctx);
        if !self.fullscreen {
            egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu(ui));
            egui::TopBottomPanel::top("toolbar").frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(egui::Margin::symmetric(6, 3))).show(ctx, |ui| self.toolbar(ui));
            if self.show_markup {
                egui::TopBottomPanel::top("markup").frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(egui::Margin::symmetric(6, 3))).show(ctx, |ui| self.markup_bar(ui));
            }
            let status = self.status_line();
            kui::status_bar(ctx, |ui| {
                ui.label(status);
            });
            if self.show_film && self.files.len() > 1 {
                egui::TopBottomPanel::bottom("film").exact_height(92.0).show(ctx, |ui| self.filmstrip(ui));
            }
            if self.show_info {
                egui::SidePanel::right("info").default_width(270.0).show(ctx, |ui| self.info_panel(ui));
            }
            if self.show_adjust {
                egui::SidePanel::right("adjust").default_width(260.0).show(ctx, |ui| self.adjust_panel(ui));
            }
        }
        let bg = match self.bg {
            Bg::Neutral => if ctx.style().visuals.dark_mode { Color32::from_gray(36) } else { Color32::from_gray(232) },
            Bg::Dark => Color32::from_gray(48),
            Bg::Black => Color32::BLACK,
            Bg::Light => Color32::from_gray(250),
        };
        let bg = if self.fullscreen { Color32::BLACK } else { bg };
        egui::CentralPanel::default().frame(egui::Frame::new().fill(bg)).show(ctx, |ui| self.canvas(ui));
        if let Some(img) = self.picker.show(ctx) {
            if let Some(e) = &self.edit {
                let (w, h) = e.image.dimensions();
                let mw = w as f32 * 0.3;
                let mh = mw * img.height() as f32 / img.width().max(1) as f32;
                self.checkpoint(t("Signature", "Подпись"));
                let m = Mark { id: rand::random(), rect: [w as f32 * 0.6, h as f32 * 0.7, w as f32 * 0.6 + mw, h as f32 * 0.7 + mh], kind: Kind::Image(Arc::new(img)), color: [0, 0, 0], fill: None, width: 1.0, font_size: 12.0 };
                self.selected = Some(m.id);
                self.edit.as_mut().unwrap().marks.push(m);
                self.tool = Tool::Select;
            }
        }
        self.dialogs(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty() && !self.leave() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }
}

impl App {
    fn status_line(&self) -> String {
        match (&self.cur, &self.edit) {
            (Some(c), Some(e)) => {
                let (w, h) = e.image.dimensions();
                let size = std::fs::metadata(&c.path).map(|m| m.len()).unwrap_or(0);
                let mut s = format!("{} × {} · {} · {:.1} KB", w, h, c.format, size as f32 / 1024.0);
                if c.pages > 1 {
                    s.push_str(&format!(" · {} {}/{}", t("page", "стр."), c.page + 1, c.pages));
                }
                if c.frames.len() > 1 {
                    s.push_str(&format!(" · {} {}", c.frames.len(), t("frames", "кадров")));
                }
                if !self.status.is_empty() {
                    s.push_str(" · ");
                    s.push_str(&self.status);
                }
                s
            }
            _ => {
                if self.status.is_empty() {
                    t("Open an image or a folder, or drop files here.", "Откройте изображение или папку либо перетащите файлы сюда.").into()
                } else {
                    self.status.clone()
                }
            }
        }
    }

    fn shortcuts(&mut self, ctx: &Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        if kui::shortcut(ctx, CTRL, Key::O) {
            self.open_dialog();
        }
        if kui::shortcut(ctx, CTRL, Key::S) {
            self.save(true);
        }
        if kui::shortcut(ctx, CTRL_SHIFT, Key::S) {
            self.save_as();
        }
        if kui::shortcut(ctx, CTRL, Key::Z) {
            if let Some(e) = self.edit.clone() {
                if let Some(s) = self.history.undo(e) {
                    self.edit = Some(s);
                    self.gen += 1;
                }
            }
        }
        if kui::shortcut(ctx, CTRL, Key::Y) || kui::shortcut(ctx, CTRL_SHIFT, Key::Z) {
            if let Some(e) = self.edit.clone() {
                if let Some(s) = self.history.redo(e) {
                    self.edit = Some(s);
                    self.gen += 1;
                }
            }
        }
        if kui::shortcut(ctx, CTRL, Key::R) {
            self.rotate(true);
        }
        if kui::shortcut(ctx, CTRL, Key::L) {
            self.rotate(false);
        }
        if kui::shortcut(ctx, CTRL, Key::K) {
            self.crop_to_selection();
        }
        if kui::shortcut(ctx, CTRL, Key::C) {
            if !self.text_sel.is_empty() {
                ctx.copy_text(self.selected_text());
            } else {
                self.copy_image();
            }
        }
        if kui::shortcut(ctx, CTRL, Key::P) {
            self.print();
        }
        if kui::shortcut(ctx, CTRL, Key::Num0) {
            self.zoom = Some(1.0);
        }
        if kui::shortcut(ctx, CTRL, Key::Plus) || kui::shortcut(ctx, CTRL, Key::Equals) {
            self.zoom = Some(self.zoom.unwrap_or(1.0) * 1.25);
        }
        if kui::shortcut(ctx, CTRL, Key::Minus) {
            self.zoom = Some(self.zoom.unwrap_or(1.0) / 1.25);
        }
        if kui::shortcut(ctx, CTRL, Key::I) {
            self.show_info = !self.show_info;
            settings::set("img42.info", self.show_info);
        }
        if kui::shortcut(ctx, CTRL_SHIFT, Key::A) {
            self.show_markup = !self.show_markup;
        }
        if kui::shortcut(ctx, CTRL_SHIFT, Key::K) {
            self.picker.open = true;
        }
        if kui::shortcut(ctx, NONE, Key::F11) || kui::shortcut(ctx, NONE, Key::F) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        if kui::shortcut(ctx, NONE, Key::Escape) {
            if self.slideshow.is_some() || self.fullscreen {
                self.slideshow = None;
                self.fullscreen = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            self.sel_area = None;
            self.selected = None;
            self.text_sel.clear();
            self.tool = Tool::Select;
        }
        if kui::shortcut(ctx, NONE, Key::Delete) || kui::shortcut(ctx, NONE, Key::Backspace) {
            if let Some(id) = self.selected {
                self.checkpoint(t("Delete", "Удаление"));
                if let Some(e) = &mut self.edit {
                    e.marks.retain(|m| m.id != id);
                }
                self.selected = None;
            } else if self.sel_area.is_some() {
                self.delete_selection();
            }
        }
        if kui::shortcut(ctx, CTRL, Key::Delete) {
            self.trash();
        }
        let pages = self.cur.as_ref().map(|c| c.pages).unwrap_or(1);
        if pages > 1 && ctx.input(|i| i.modifiers.alt) {
            let p = self.cur.as_ref().map(|c| c.page).unwrap_or(0);
            if kui::shortcut(ctx, egui::Modifiers::ALT, Key::ArrowRight) && p + 1 < pages {
                self.load_current(p + 1);
            }
            if kui::shortcut(ctx, egui::Modifiers::ALT, Key::ArrowLeft) && p > 0 {
                self.load_current(p - 1);
            }
        }
        if self.selected.is_none() && self.sel_area.is_none() {
            if kui::shortcut(ctx, NONE, Key::ArrowRight) || kui::shortcut(ctx, NONE, Key::Space) || kui::shortcut(ctx, NONE, Key::PageDown) {
                self.go(1);
            }
            if kui::shortcut(ctx, NONE, Key::ArrowLeft) || kui::shortcut(ctx, egui::Modifiers::SHIFT, Key::Space) || kui::shortcut(ctx, NONE, Key::PageUp) {
                self.go(-1);
            }
            if kui::shortcut(ctx, NONE, Key::Home) {
                self.go(-(self.index as i64));
            }
            if kui::shortcut(ctx, NONE, Key::End) {
                let n = self.files.len() as i64;
                self.go(n - 1 - self.index as i64);
            }
        }
    }

    fn open_dialog(&mut self) {
        let exts: Vec<&str> = EXTS.to_vec();
        if let Some(p) = kui::open_dialog(t("Open", "Открыть"), &[(t("Images", "Изображения"), &exts)]).pick_files() {
            if let Some(f) = p.first() {
                kui::remember_dir(f);
            }
            self.open_set(p);
        }
    }

    fn print(&mut self) {
        let Some(img) = self.rendered(true) else { return };
        let tmp = kit42::shell::temp_file("print", "png");
        if img.save(&tmp).is_ok() {
            kit42::shell::print(&tmp);
        }
    }

    fn menu(&mut self, ui: &mut egui::Ui) {
        let has = self.edit.is_some();
        egui::menu::bar(ui, |ui| {
            ui.menu_button(t("File", "Файл"), |ui| {
                if kui::menu_item(ui, t("Open…", "Открыть…"), "Ctrl+O") {
                    self.open_dialog();
                }
                if kui::menu_item(ui, t("Open Folder…", "Открыть папку…"), "") {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        self.open_set(vec![d]);
                    }
                }
                ui.menu_button(t("Open Recent", "Последние файлы"), |ui| {
                    for p in settings::recent("img42") {
                        if ui.button(kit42::file_name(&p)).clicked() {
                            self.open_set(vec![p]);
                            ui.close_menu();
                        }
                    }
                });
                ui.separator();
                if kui::menu_item_enabled(ui, has, t("Save", "Сохранить"), "Ctrl+S") {
                    self.save(true);
                }
                if kui::menu_item_enabled(ui, has, t("Export / Save As…", "Экспорт / Сохранить как…"), "Ctrl+Shift+S") {
                    self.save_as();
                }
                if kui::menu_item_enabled(ui, has, t("Revert to Saved", "Вернуть сохранённое"), "") {
                    let page = self.cur.as_ref().map(|c| c.page).unwrap_or(0);
                    self.load_current(page);
                }
                ui.separator();
                if kui::menu_item_enabled(ui, has, t("Copy Image", "Копировать изображение"), "Ctrl+C") {
                    self.copy_image();
                }
                if kui::menu_item_enabled(ui, has, t("Show in Explorer", "Показать в проводнике"), "") {
                    if let Some(c) = &self.cur {
                        kit42::shell::reveal(&c.path);
                    }
                }
                if kui::menu_item_enabled(ui, has, t("Open in Default App", "Открыть в другой программе"), "") {
                    if let Some(c) = &self.cur {
                        kit42::shell::open_path(&c.path);
                    }
                }
                if kui::menu_item_enabled(ui, has, t("Print…", "Печать…"), "Ctrl+P") {
                    self.print();
                }
                if kui::menu_item_enabled(ui, has, t("Move to Recycle Bin", "Переместить в корзину"), "Ctrl+Del") {
                    self.trash();
                }
                ui.separator();
                if kui::menu_item(ui, t("Exit", "Выход"), "Alt+F4") {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button(t("Edit", "Правка"), |ui| {
                if kui::menu_item_enabled(ui, self.history.can_undo(), t("Undo", "Отменить"), "Ctrl+Z") {
                    if let Some(e) = self.edit.clone() {
                        if let Some(s) = self.history.undo(e) {
                            self.edit = Some(s);
                            self.gen += 1;
                        }
                    }
                }
                if kui::menu_item_enabled(ui, self.history.can_redo(), t("Redo", "Повторить"), "Ctrl+Y") {
                    if let Some(e) = self.edit.clone() {
                        if let Some(s) = self.history.redo(e) {
                            self.edit = Some(s);
                            self.gen += 1;
                        }
                    }
                }
                ui.separator();
                if kui::menu_item_enabled(ui, self.sel_area.is_some(), t("Crop to Selection", "Обрезать по выделению"), "Ctrl+K") {
                    self.crop_to_selection();
                }
                if kui::menu_item_enabled(ui, self.sel_area.is_some(), t("Delete Selection", "Удалить выделенное"), "Del") {
                    self.delete_selection();
                }
                if kui::menu_item_enabled(ui, has, t("Select All", "Выделить всё"), "") {
                    if let Some(e) = &self.edit {
                        let (w, h) = e.image.dimensions();
                        self.sel_area = Some(([0.0, 0.0, w as f32, h as f32], false));
                    }
                }
                if kui::menu_item_enabled(ui, self.ocr.is_some(), t("Copy Text in Image", "Копировать текст на изображении"), "Ctrl+Shift+C") {
                    if let Some((_, o)) = &self.ocr {
                        ui.ctx().copy_text(o.text());
                    }
                }
            });
            ui.menu_button(t("View", "Вид"), |ui| {
                if kui::menu_item(ui, t("Fit to Window", "По размеру окна"), "") {
                    self.zoom = None;
                }
                if kui::menu_item(ui, t("Actual Size", "Реальный размер"), "Ctrl+0") {
                    self.zoom = Some(1.0);
                }
                ui.separator();
                ui.checkbox(&mut self.show_markup, t("Markup Toolbar", "Панель разметки"));
                if ui.checkbox(&mut self.show_film, t("Filmstrip", "Лента миниатюр")).changed() {
                    settings::set("img42.film", self.show_film);
                }
                if ui.checkbox(&mut self.show_info, t("Info Panel", "Панель сведений")).changed() {
                    settings::set("img42.info", self.show_info);
                }
                ui.checkbox(&mut self.show_adjust, t("Adjust Colour", "Коррекция цвета"));
                if ui.checkbox(&mut self.loop_folder, t("Loop Through Folder", "Листать по кругу")).changed() {
                    settings::set("img42.loop", self.loop_folder);
                }
                ui.menu_button(t("Sort By", "Сортировка"), |ui| {
                    for (i, l) in [t("Name", "Имя"), t("Date Modified", "Дата изменения"), t("Size", "Размер")].iter().enumerate() {
                        if ui.radio(self.sort == i as u8, *l).clicked() {
                            self.sort = i as u8;
                            let cur = self.files.get(self.index).cloned();
                            let mut list = std::mem::take(&mut self.files);
                            self.sort_list(&mut list);
                            self.files = list;
                            if let Some(c) = cur {
                                self.index = self.files.iter().position(|x| *x == c).unwrap_or(0);
                            }
                            ui.close_menu();
                        }
                    }
                });
                ui.menu_button(t("Background", "Фон"), |ui| {
                    for (b, l) in [(Bg::Neutral, t("Neutral", "Нейтральный")), (Bg::Dark, t("Dark", "Тёмный")), (Bg::Black, t("Black", "Чёрный")), (Bg::Light, t("Light", "Светлый"))] {
                        if ui.radio(self.bg == b, l).clicked() {
                            self.bg = b;
                        }
                    }
                });
                if kui::menu_item(ui, t("Full Screen", "Во весь экран"), "F") {
                    self.fullscreen = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                }
                if kui::menu_item(ui, t("Slideshow", "Слайд-шоу"), "") {
                    self.slideshow = Some(ui.ctx().input(|i| i.time));
                    self.fullscreen = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                }
            });
            ui.menu_button(t("Image", "Изображение"), |ui| {
                ui.add_enabled_ui(has, |ui| {
                    if kui::menu_item(ui, t("Rotate Left", "Повернуть влево"), "Ctrl+L") {
                        self.rotate(false);
                    }
                    if kui::menu_item(ui, t("Rotate Right", "Повернуть вправо"), "Ctrl+R") {
                        self.rotate(true);
                    }
                    if kui::menu_item(ui, t("Flip Horizontal", "Отразить по горизонтали"), "") {
                        self.flip(true);
                    }
                    if kui::menu_item(ui, t("Flip Vertical", "Отразить по вертикали"), "") {
                        self.flip(false);
                    }
                    ui.separator();
                    if kui::menu_item(ui, t("Adjust Size…", "Изменить размер…"), "") {
                        if let (Some(e), Some(c)) = (&self.edit, &self.cur) {
                            let (w, h) = e.image.dimensions();
                            self.resize = Some(ResizeDlg { w, h, keep: true, percent: 100.0, dpi: c.dpi.0, resample: true });
                        }
                    }
                    if kui::menu_item(ui, t("Remove Background", "Удалить фон"), "") {
                        self.image_op(t("Remove Background", "Удаление фона"), pixels::remove_background, None);
                    }
                    if kui::menu_item(ui, t("Auto Enhance", "Автоулучшение"), "") {
                        if let Some(e) = &self.edit {
                            let a = pixels::auto_adjust(&e.image);
                            self.checkpoint(t("Auto Enhance", "Автоулучшение"));
                            self.edit.as_mut().unwrap().adjust = a;
                        }
                    }
                    if kui::menu_item(ui, t("Recognise Text (Live Text)", "Распознать текст (Live Text)"), "") {
                        self.start_ocr();
                    }
                    if kui::menu_item(ui, t("Signatures and Stamps…", "Подписи и печати…"), "Ctrl+Shift+K") {
                        self.picker.open = true;
                    }
                });
            });
            ui.menu_button(t("Help", "Справка"), |ui| {
                kui::language_menu(ui);
                if kui::menu_item(ui, t("About IMG42", "О программе IMG42"), "") {
                    kui::about(APP, t("image viewer and editor", "просмотр и правка изображений"));
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let has = self.edit.is_some();
        ui.horizontal(|ui| {
            if kui::tool_button(ui, ic::BACK, t("Previous", "Предыдущее"), self.files.len() > 1).clicked() {
                self.go(-1);
            }
            if kui::tool_button(ui, ic::FORWARD, t("Next", "Следующее"), self.files.len() > 1).clicked() {
                self.go(1);
            }
            ui.separator();
            if kui::tool_button(ui, ic::ZOOM_OUT, t("Zoom Out", "Уменьшить"), has).clicked() {
                self.zoom = Some(self.zoom.unwrap_or(1.0) / 1.25);
            }
            if ui.add_enabled(has, egui::Button::new("1:1").frame(false)).on_hover_text(t("Actual Size", "Реальный размер")).clicked() {
                self.zoom = Some(1.0);
            }
            if kui::tool_button(ui, ic::ZOOM_IN, t("Zoom In", "Увеличить"), has).clicked() {
                self.zoom = Some(self.zoom.unwrap_or(1.0) * 1.25);
            }
            if kui::tool_button(ui, ic::FIT, t("Fit", "По размеру окна"), has).clicked() {
                self.zoom = None;
            }
            ui.separator();
            if kui::tool_button(ui, "\u{E7AD}", t("Rotate Right", "Повернуть вправо"), has).clicked() {
                self.rotate(true);
            }
            if kui::tool_toggle(ui, self.show_markup, ic::EDIT, t("Markup", "Разметка")).clicked() {
                self.show_markup = !self.show_markup;
            }
            if kui::tool_toggle(ui, self.show_adjust, "\u{E9E9}", t("Adjust Colour", "Коррекция цвета")).clicked() {
                self.show_adjust = !self.show_adjust;
            }
            if kui::tool_toggle(ui, self.show_info, ic::INFO, t("Info", "Сведения")).clicked() {
                self.show_info = !self.show_info;
                settings::set("img42.info", self.show_info);
            }
            if kui::tool_button(ui, ic::SHARE, t("Copy", "Копировать"), has).clicked() {
                self.copy_image();
            }
            ui.separator();
            if kui::tool_button(ui, ic::OCR, t("Recognise Text", "Распознать текст"), has).clicked() {
                self.start_ocr();
            }
            let r = ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(t("Search text in image", "Поиск текста на изображении")).desired_width(200.0));
            if r.changed() && self.ocr.is_none() && !self.search.is_empty() && self.ocr_rx.is_none() {
                self.start_ocr();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if kui::tool_button(ui, ic::DELETE, t("Move to Recycle Bin", "В корзину"), has).clicked() {
                    self.trash();
                }
                if kui::tool_button(ui, ic::SAVE, t("Save", "Сохранить"), has).clicked() {
                    self.save(true);
                }
            });
        });
    }

    fn markup_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for tool in [Tool::Select, Tool::RectSel, Tool::EllipseSel, Tool::Alpha, Tool::Crop, Tool::Pen, Tool::Line, Tool::Arrow, Tool::Rect, Tool::RoundRect, Tool::Ellipse, Tool::Star, Tool::Bubble, Tool::Highlight, Tool::Text, Tool::Pixelate] {
                if kui::tool_toggle(ui, self.tool == tool, tool.icon(), tool.name()).clicked() {
                    self.tool = tool;
                    if tool == Tool::Highlight {
                        self.color = [255, 230, 0];
                    }
                }
            }
            if kui::tool_button(ui, ic::SIGNATURE, t("Sign", "Подпись"), true).clicked() {
                self.picker.open = true;
            }
            ui.separator();
            let mut c = self.color;
            if kui::color_button(ui, &mut c, t("Border Colour", "Цвет линии")) {
                self.color = c;
                self.restyle(|m| m.color = c);
            }
            let mut f = self.fill.unwrap_or([255, 255, 255]);
            if kui::color_button(ui, &mut f, t("Fill Colour", "Цвет заливки")) {
                self.fill = Some(f);
                self.restyle(|m| m.fill = Some(f));
            }
            if ui.small_button(t("No fill", "Без заливки")).clicked() {
                self.fill = None;
                self.restyle(|m| m.fill = None);
            }
            ui.label(t("Width", "Толщина"));
            if ui.add(egui::DragValue::new(&mut self.width).range(1.0..=60.0)).changed() {
                let w = self.width;
                self.restyle(|m| m.width = w);
            }
            ui.label(t("Text", "Текст"));
            if ui.add(egui::DragValue::new(&mut self.font_size).range(6.0..=400.0)).changed() {
                let s = self.font_size;
                self.restyle(|m| m.font_size = s);
            }
            if self.tool == Tool::Alpha {
                ui.add(egui::Slider::new(&mut self.alpha_tol, 5.0..=150.0).text(t("Tolerance", "Допуск")));
            }
            if self.sel_area.is_some() {
                if ui.button(t("Crop", "Обрезать")).clicked() {
                    self.crop_to_selection();
                }
                if ui.button(t("Delete", "Удалить")).clicked() {
                    self.delete_selection();
                }
            }
        });
    }

    fn restyle(&mut self, f: impl Fn(&mut Mark)) {
        if let Some(id) = self.selected {
            self.checkpoint(t("Style", "Стиль"));
            if let Some(m) = self.edit.as_mut().and_then(|e| e.marks.iter_mut().find(|m| m.id == id)) {
                f(m);
            }
        }
    }

    fn display_image(&mut self, ctx: &Context) -> Option<egui::TextureHandle> {
        let e = self.edit.as_ref()?;
        let key = self.gen ^ ((e.adjust.exposure.to_bits() as u64) << 7) ^ ((e.adjust.contrast.to_bits() as u64) << 13) ^ (self.show_original as u64) << 3;
        let key = key ^ hash_adjust(&e.adjust);
        if let Some((k, t)) = &self.tex {
            if *k == key {
                return Some(t.clone());
            }
        }
        // Adjustments preview on a reduced copy for speed.
        let img = if e.adjust.is_identity() || self.show_original {
            kit42::raster::fit_within(&e.image, 4096)
        } else {
            let small = match &self.preview_cache {
                Some((g, _, img)) if *g == self.gen => img.clone(),
                _ => {
                    let s = Arc::new(kit42::raster::fit_within(&e.image, 2048));
                    self.preview_cache = Some((self.gen, e.adjust, s.clone()));
                    s
                }
            };
            pixels::apply(&small, &e.adjust)
        };
        let tex = kui::texture(ctx, "img", &img);
        self.tex = Some((key, tex.clone()));
        Some(tex)
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(tex) = self.display_image(&ctx) else {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(self.status_line()).size(16.0).weak());
                    if ui.button(format!("{}  {}", ic::OPEN, t("Open…", "Открыть…"))).clicked() {
                        self.open_dialog();
                    }
                });
            });
            return;
        };
        let (iw, ih) = self.edit.as_ref().map(|e| e.image.dimensions()).unwrap_or((1, 1));
        let avail = ui.max_rect();
        let fit = (avail.width() / iw as f32).min(avail.height() / ih as f32).min(1.0);
        let ppp = ctx.pixels_per_point();
        let scale = self.zoom.map(|z| z / ppp).unwrap_or(fit);
        // Ctrl+wheel zoom around the pointer, plain wheel pans when zoomed (or flips images when fitted).
        let (wheel, ctrl, pointer) = ctx.input(|i| (i.raw_scroll_delta, i.modifiers.command, i.pointer.hover_pos()));
        if pointer.map(|p| avail.contains(p)).unwrap_or(false) && wheel != Vec2::ZERO {
            if ctrl {
                let f = if wheel.y > 0.0 { 1.15 } else { 1.0 / 1.15 };
                self.zoom = Some((scale * ppp * f).clamp(0.02, 32.0));
            } else if self.zoom.is_none() {
                if wheel.y.abs() > 20.0 {
                    self.go(if wheel.y < 0.0 { 1 } else { -1 });
                }
            } else {
                self.offset += wheel;
            }
        }
        let size = Vec2::new(iw as f32 * scale, ih as f32 * scale);
        let rect = Rect::from_center_size(avail.center() + self.offset, size);
        let painter = ui.painter_at(avail);
        if self.edit.as_ref().map(|e| pixels::has_alpha(&kit42::raster::fit_within(&e.image, 64))).unwrap_or(false) {
            kui::checkerboard(&painter, rect.intersect(avail), 10.0);
        }
        painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        let to_screen = |p: [f32; 2]| Pos2::new(rect.left() + p[0] * scale, rect.top() + p[1] * scale);
        let to_img = |p: Pos2| [((p.x - rect.left()) / scale), ((p.y - rect.top()) / scale)];
        // Marks.
        let marks: Vec<Mark> = self.edit.as_ref().map(|e| e.marks.clone()).unwrap_or_default();
        for m in &marks {
            let tex = if let Kind::Image(img) = &m.kind {
                Some(self.mark_tex.entry(m.id).or_insert_with(|| kui::texture(&ctx, &format!("mk{}", m.id), img)).clone())
            } else {
                None
            };
            if self.editing_text == Some(m.id) {
                continue;
            }
            marks::paint(&painter, m, &to_screen, scale, tex.as_ref(), self.selected == Some(m.id));
        }
        // Live Text: matches and selection.
        let words = self.ocr_words();
        let q = self.search.to_lowercase();
        for (li, wi, r, text) in &words {
            let sr = Rect::from_min_max(to_screen([r.min.x, r.min.y]), to_screen([r.max.x, r.max.y]));
            if !q.is_empty() && text.to_lowercase().contains(&q) {
                painter.rect_filled(sr.expand(2.0), 2.0, Color32::from_rgba_unmultiplied(255, 220, 0, 110));
            }
            if self.text_sel.contains(&(*li, *wi)) {
                painter.rect_filled(sr.expand(1.0), 1.0, Color32::from_rgba_unmultiplied(0, 120, 215, 90));
            }
        }
        // Selection area.
        if let Some((r, ellipse)) = self.sel_area {
            let sr = Rect::from_min_max(to_screen([r[0], r[1]]), to_screen([r[2], r[3]]));
            let st = Stroke::new(1.5, Color32::WHITE);
            if ellipse {
                painter.add(egui::epaint::EllipseShape { center: sr.center(), radius: sr.size() / 2.0, fill: Color32::TRANSPARENT, stroke: st });
            } else {
                painter.rect_stroke(sr, 0.0, st, egui::StrokeKind::Middle);
            }
            painter.rect_stroke(sr, 0.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
        }
        // Text editing overlay.
        if let Some(id) = self.editing_text {
            if let Some(m) = marks.iter().find(|m| m.id == id) {
                let r = Rect::from_min_max(to_screen([m.rect[0], m.rect[1]]), to_screen([m.rect[2], m.rect[3]]));
                let mut text = if let Kind::Text(s) = &m.kind { s.clone() } else { String::new() };
                let resp = ui.put(r, egui::TextEdit::multiline(&mut text).font(egui::FontId::proportional((m.font_size * scale).max(6.0))).text_color(marks::c32(m.color)).desired_width(r.width()));
                if resp.changed() {
                    if let Some(mm) = self.edit.as_mut().and_then(|e| e.marks.iter_mut().find(|x| x.id == id)) {
                        mm.kind = Kind::Text(text);
                    }
                }
                if !resp.has_focus() {
                    if ctx.data(|d| d.get_temp::<bool>(egui::Id::new(("tfocus", id)))).unwrap_or(false) {
                        self.editing_text = None;
                    } else {
                        resp.request_focus();
                        ctx.data_mut(|d| d.insert_temp(egui::Id::new(("tfocus", id)), true));
                    }
                }
            }
        }
        // Interaction.
        let resp = ui.interact(avail, egui::Id::new("canvas"), Sense::click_and_drag());
        let pos = resp.interact_pointer_pos();
        if resp.double_clicked() && self.tool == Tool::Select {
            if let Some(p) = pos {
                let ip = to_img(p);
                if let Some(m) = marks.iter().rev().find(|m| inside(m.rect, ip)) {
                    if matches!(m.kind, Kind::Text(_)) {
                        self.checkpoint(t("Edit Text", "Правка текста"));
                        self.editing_text = Some(m.id);
                    }
                } else if !words.is_empty() {
                    if let Some((li, wi, ..)) = words.iter().find(|(_, _, r, _)| r.contains(Pos2::new(ip[0], ip[1]))) {
                        self.text_sel = vec![(*li, *wi)];
                    }
                } else {
                    self.zoom = if self.zoom.is_some() { None } else { Some(1.0) };
                    self.offset = Vec2::ZERO;
                }
            }
            return;
        }
        if resp.clicked() {
            if let Some(p) = pos {
                let ip = to_img(p);
                match self.tool {
                    Tool::Select => {
                        self.selected = marks.iter().rev().find(|m| inside(m.rect, ip)).map(|m| m.id);
                        if self.selected.is_none() {
                            self.text_sel.clear();
                            self.sel_area = None;
                        }
                    }
                    Tool::Alpha => {
                        let tol = self.alpha_tol;
                        let seed = (ip[0].max(0.0) as u32, ip[1].max(0.0) as u32);
                        self.image_op(t("Instant Alpha", "Мгновенная альфа"), |img| {
                            let mask = pixels::flood_mask(img, seed, tol);
                            let mut out = img.clone();
                            for (p, m) in out.pixels_mut().zip(mask) {
                                if m {
                                    p[3] = 0;
                                }
                            }
                            out
                        }, None);
                    }
                    Tool::Text => {
                        self.checkpoint(t("Text", "Текст"));
                        let m = Mark { id: rand::random(), rect: [ip[0], ip[1], ip[0] + self.font_size * 8.0, ip[1] + self.font_size * 1.4], kind: Kind::Text(String::new()), color: self.color, fill: self.fill, width: 1.0, font_size: self.font_size };
                        self.editing_text = Some(m.id);
                        self.selected = Some(m.id);
                        self.edit.as_mut().unwrap().marks.push(m);
                        self.tool = Tool::Select;
                    }
                    _ => {}
                }
            }
        }
        if resp.drag_started() {
            if let Some(p) = pos {
                let ip = to_img(p);
                let middle = ctx.input(|i| i.pointer.middle_down());
                self.drag = Some(if middle {
                    Drag::Pan
                } else {
                    match self.tool {
                        Tool::Select => {
                            let sel = self.selected.and_then(|id| marks.iter().find(|m| m.id == id));
                            let corner = sel.and_then(|m| {
                                let r = Rect::from_min_max(to_screen([m.rect[0], m.rect[1]]), to_screen([m.rect[2], m.rect[3]]));
                                [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()].iter().position(|c| c.distance(p) < 8.0).map(|k| (m.id, k))
                            });
                            if let Some((id, corner)) = corner {
                                self.checkpoint(t("Resize", "Размер"));
                                Drag::Resize { id, corner, last: ip }
                            } else if let Some(m) = marks.iter().rev().find(|m| inside(m.rect, ip)) {
                                self.selected = Some(m.id);
                                self.checkpoint(t("Move", "Перемещение"));
                                Drag::Move { id: m.id, last: ip }
                            } else if !words.is_empty() {
                                Drag::TextSel { start: p }
                            } else if self.zoom.is_some() {
                                Drag::Pan
                            } else {
                                Drag::Select { start: ip }
                            }
                        }
                        Tool::RectSel | Tool::EllipseSel | Tool::Crop | Tool::Pixelate => Drag::Select { start: ip },
                        _ => Drag::Create { start: ip, cur: ip, pts: vec![ip] },
                    }
                });
            }
        }
        if resp.dragged() {
            if let Some(p) = pos {
                let ip = to_img(p);
                match &mut self.drag {
                    Some(Drag::Pan) => self.offset += resp.drag_delta(),
                    Some(Drag::Create { cur, pts, .. }) => {
                        *cur = ip;
                        pts.push(ip);
                    }
                    Some(Drag::Move { id, last }) => {
                        let (dx, dy) = (ip[0] - last[0], ip[1] - last[1]);
                        let id = *id;
                        *last = ip;
                        if let Some(m) = self.edit.as_mut().and_then(|e| e.marks.iter_mut().find(|m| m.id == id)) {
                            m.translate(dx, dy);
                        }
                    }
                    Some(Drag::Resize { id, corner, last }) => {
                        let (dx, dy) = (ip[0] - last[0], ip[1] - last[1]);
                        let (id, corner) = (*id, *corner);
                        *last = ip;
                        if let Some(m) = self.edit.as_mut().and_then(|e| e.marks.iter_mut().find(|m| m.id == id)) {
                            let mut r = m.rect;
                            match corner {
                                0 => {
                                    r[0] += dx;
                                    r[1] += dy;
                                }
                                1 => {
                                    r[2] += dx;
                                    r[1] += dy;
                                }
                                2 => {
                                    r[0] += dx;
                                    r[3] += dy;
                                }
                                _ => {
                                    r[2] += dx;
                                    r[3] += dy;
                                }
                            }
                            if matches!(m.kind, Kind::Image(_)) {
                                let ratio = (m.rect[3] - m.rect[1]) / (m.rect[2] - m.rect[0]).max(0.01);
                                if corner >= 2 {
                                    r[3] = r[1] + (r[2] - r[0]) * ratio;
                                } else {
                                    r[1] = r[3] - (r[2] - r[0]) * ratio;
                                }
                            }
                            if (r[2] - r[0]).abs() > 3.0 && (r[3] - r[1]).abs() > 3.0 {
                                m.set_rect(r);
                            }
                        }
                    }
                    Some(Drag::Select { start }) => {
                        let mut r = marks::norm([start[0], start[1], ip[0], ip[1]]);
                        r[0] = r[0].max(0.0);
                        r[1] = r[1].max(0.0);
                        r[2] = r[2].min(iw as f32);
                        r[3] = r[3].min(ih as f32);
                        self.sel_area = Some((r, self.tool == Tool::EllipseSel));
                    }
                    Some(Drag::TextSel { start }) => {
                        let a = to_img(*start);
                        let sel = marks::norm([a[0], a[1], ip[0], ip[1]]);
                        self.text_sel = words.iter().filter(|(_, _, r, _)| r.intersects(Rect::from_min_max(Pos2::new(sel[0], sel[1]), Pos2::new(sel[2], sel[3])))).map(|(l, w, ..)| (*l, *w)).collect();
                    }
                    None => {}
                }
            }
        }
        // Live preview of shapes being drawn.
        if let Some(Drag::Create { start, cur, pts }) = &self.drag {
            let st = Stroke::new((self.width * scale).max(1.0), marks::c32(self.color));
            let r = Rect::from_two_pos(to_screen(*start), to_screen(*cur));
            match self.tool {
                Tool::Pen => {
                    painter.add(egui::Shape::line(pts.iter().map(|q| to_screen(*q)).collect(), st));
                }
                Tool::Line | Tool::Arrow => {
                    painter.line_segment([to_screen(*start), to_screen(*cur)], st);
                }
                Tool::Ellipse => {
                    painter.add(egui::epaint::EllipseShape { center: r.center(), radius: r.size() / 2.0, fill: Color32::TRANSPARENT, stroke: st });
                }
                _ => {
                    painter.rect_stroke(r, 0.0, st, egui::StrokeKind::Middle);
                }
            }
        }
        if resp.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Create { start, cur, pts }) => {
                    let r = marks::norm([start[0], start[1], cur[0], cur[1]]);
                    let big = (r[2] - r[0]) > 3.0 || (r[3] - r[1]) > 3.0;
                    let kind = match self.tool {
                        Tool::Pen if pts.len() > 1 => Some(Kind::Pen(pts.clone())),
                        Tool::Line => Some(Kind::Line(start, cur)),
                        Tool::Arrow => Some(Kind::Arrow(start, cur)),
                        Tool::Rect => Some(Kind::Rect),
                        Tool::RoundRect => Some(Kind::RoundRect),
                        Tool::Ellipse => Some(Kind::Ellipse),
                        Tool::Star => Some(Kind::Star),
                        Tool::Bubble => Some(Kind::Bubble),
                        Tool::Highlight => Some(Kind::Highlight),
                        Tool::Text => Some(Kind::Text(String::new())),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        if big || matches!(kind, Kind::Pen(_)) {
                            let rect = match &kind {
                                Kind::Pen(p) => {
                                    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
                                    for q in p {
                                        b = [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])];
                                    }
                                    b
                                }
                                _ => r,
                            };
                            self.checkpoint(self.tool.name());
                            let is_text = matches!(kind, Kind::Text(_));
                            let m = Mark { id: rand::random(), rect, kind, color: self.color, fill: self.fill, width: self.width, font_size: self.font_size };
                            if is_text {
                                self.editing_text = Some(m.id);
                            }
                            self.selected = Some(m.id);
                            self.edit.as_mut().unwrap().marks.push(m);
                        }
                    }
                }
                Some(Drag::Select { .. }) => {
                    if self.tool == Tool::Crop {
                        self.crop_to_selection();
                    } else if self.tool == Tool::Pixelate {
                        if let Some((r, _)) = self.sel_area.take() {
                            let rr = (r[0] as u32, r[1] as u32, r[2] as u32, r[3] as u32);
                            let block = ((r[2] - r[0]).max(r[3] - r[1]) / 12.0).max(6.0) as u32;
                            self.image_op(t("Pixelate", "Пикселизация"), |img| {
                                let mut out = img.clone();
                                pixels::pixelate(&mut out, rr, block);
                                out
                            }, None);
                        }
                    }
                }
                _ => {}
            }
        }
        let _ = Sense::hover();
    }

    fn filmstrip(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut goto = None;
        egui::ScrollArea::horizontal().auto_shrink(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, p) in self.files.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(82.0, 76.0), Sense::click());
                    if ui.is_rect_visible(r) {
                        if let Some(tex) = self.thumbs.get(p) {
                            let s = tex.size_vec2();
                            let k = (78.0 / s.x).min(72.0 / s.y);
                            ui.painter().image(tex.id(), Rect::from_center_size(r.center(), s * k), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                        } else if !self.thumb_pending.contains(p) && self.thumb_pending.len() < 6 {
                            self.thumb_pending.insert(p.clone());
                            let (tx, path) = (self.thumb_tx.clone(), p.clone());
                            std::thread::spawn(move || {
                                let img = image::open(&path).map(|i| i.thumbnail(160, 160).to_rgba8()).or_else(|_| load(&path, 0).map(|l| kit42::raster::fit_within(&l.frames[0].0, 160))).unwrap_or_else(|_| image::RgbaImage::new(1, 1));
                                let _ = tx.send((path, img));
                            });
                        }
                        if i == self.index {
                            ui.painter().rect_stroke(r, 3.0, Stroke::new(2.5, kui::accent(&ctx)), egui::StrokeKind::Inside);
                        }
                    }
                    if resp.clicked() {
                        goto = Some(i);
                    }
                    if i == self.index && resp.rect.is_positive() {
                        let _ = resp;
                    }
                }
            });
        });
        if let Some(i) = goto {
            if self.leave() {
                self.show(i);
            }
        }
    }

    fn info_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading(t("Info", "Сведения"));
        ui.separator();
        let Some(c) = &self.cur else { return };
        let (w, h) = self.edit.as_ref().map(|e| e.image.dimensions()).unwrap_or((0, 0));
        let meta = std::fs::metadata(&c.path).ok();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("info").num_columns(2).striped(true).show(ui, |ui| {
                let mut row = |k: &str, v: String| {
                    ui.label(egui::RichText::new(k).weak());
                    ui.add(egui::Label::new(v).wrap());
                    ui.end_row();
                };
                row(t("File", "Файл"), kit42::file_name(&c.path));
                row(t("Folder", "Папка"), c.path.parent().map(|p| p.display().to_string()).unwrap_or_default());
                row(t("Format", "Формат"), c.format.clone());
                row(t("Dimensions", "Размер"), format!("{w} × {h} px ({:.1} MP)", (w as f64 * h as f64) / 1e6));
                row(t("Resolution", "Разрешение"), format!("{:.0} × {:.0} dpi", c.dpi.0, c.dpi.1));
                if let Some(m) = &meta {
                    row(t("File size", "Размер файла"), format!("{:.1} KB", m.len() as f64 / 1024.0));
                    if let Ok(t2) = m.modified() {
                        let dt: chrono::DateTime<chrono::Local> = t2.into();
                        row(t("Modified", "Изменён"), dt.format("%d.%m.%Y %H:%M").to_string());
                    }
                }
                for key in ["Make", "Model", "LensModel", "ExposureTime", "FNumber", "PhotographicSensitivity", "FocalLength", "DateTimeOriginal", "GPSLatitude", "GPSLongitude"] {
                    if let Some((_, v)) = c.exif.iter().find(|(k, _)| k == key) {
                        row(key, v.clone());
                    }
                }
            });
            let lat = c.exif.iter().find(|(k, _)| k == "GPSLatitude").map(|x| x.1.clone());
            if lat.is_some() && ui.link(t("Show in Maps", "Показать на карте")).clicked() {
                let (la, lo) = (gps(&c.exif, "GPSLatitude", "GPSLatitudeRef"), gps(&c.exif, "GPSLongitude", "GPSLongitudeRef"));
                if let (Some(la), Some(lo)) = (la, lo) {
                    kit42::shell::open(&format!("https://www.openstreetmap.org/?mlat={la}&mlon={lo}#map=15/{la}/{lo}"));
                }
            }
            ui.separator();
            ui.label(t("Image Description", "Описание изображения"));
            ui.text_edit_multiline(&mut self.description);
            egui::CollapsingHeader::new(t("All Metadata", "Все метаданные")).show(ui, |ui| {
                for (k, v) in &c.exif {
                    ui.label(format!("{k}: {v}"));
                }
            });
        });
    }

    fn adjust_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading(t("Adjust Colour", "Коррекция цвета"));
        ui.separator();
        let Some(e) = &self.edit else { return };
        let mut a = e.adjust;
        let before = a;
        let mut changed_drag = false;
        let mut slider = |ui: &mut egui::Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>, label: &str| {
            let r = ui.add(egui::Slider::new(v, range).text(label));
            if r.double_clicked() {
                *v = 0.0;
            }
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                changed_drag = true;
            }
        };
        slider(ui, &mut a.exposure, -3.0..=3.0, t("Exposure", "Экспозиция"));
        slider(ui, &mut a.brightness, -1.0..=1.0, t("Brightness", "Яркость"));
        slider(ui, &mut a.contrast, -1.0..=1.0, t("Contrast", "Контраст"));
        slider(ui, &mut a.highlights, -1.0..=1.0, t("Highlights", "Светлые участки"));
        slider(ui, &mut a.shadows, -1.0..=1.0, t("Shadows", "Тени"));
        slider(ui, &mut a.saturation, -1.0..=1.0, t("Saturation", "Насыщенность"));
        slider(ui, &mut a.vibrance, -1.0..=1.0, t("Vibrance", "Красочность"));
        slider(ui, &mut a.temperature, -1.0..=1.0, t("Temperature", "Температура"));
        slider(ui, &mut a.tint, -1.0..=1.0, t("Tint", "Оттенок"));
        slider(ui, &mut a.sharpness, -1.0..=1.0, t("Sharpness", "Резкость"));
        slider(ui, &mut a.vignette, -1.0..=1.0, t("Vignette", "Виньетка"));
        ui.horizontal_wrapped(|ui| {
            for (l, look) in [(t("None", "Нет"), Look::None), (t("B&W", "Ч/Б"), Look::Mono), ("Noir", Look::Noir), (t("Sepia", "Сепия"), Look::Sepia), (t("Invert", "Инверсия"), Look::Invert)] {
                if ui.selectable_label(a.look == look, l).clicked() {
                    a.look = look;
                    changed_drag = true;
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.button(t("Auto Enhance", "Автоулучшение")).clicked() {
                a = pixels::auto_adjust(&e.image);
                changed_drag = true;
            }
            if ui.button(t("Reset All", "Сбросить всё")).clicked() {
                a = Adjust::default();
                changed_drag = true;
            }
        });
        let orig = ui.button(t("Hold to Show Original", "Удерживайте, чтобы увидеть оригинал"));
        self.show_original = orig.is_pointer_button_down_on();
        if a != before {
            if changed_drag || !self.history.can_undo() || self.history.undo_label() != Some(t("Adjust Colour", "Коррекция цвета")) {
                self.checkpoint(t("Adjust Colour", "Коррекция цвета"));
            }
            self.edit.as_mut().unwrap().adjust = a;
        }
    }

    fn dialogs(&mut self, ctx: &Context) {
        if let Some(mut d) = self.resize.take() {
            let mut open = true;
            let mut done = false;
            let (ow, oh) = self.edit.as_ref().map(|e| e.image.dimensions()).unwrap_or((1, 1));
            egui::Window::new(t("Adjust Size", "Изменить размер")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.checkbox(&mut d.resample, t("Resample image", "Изменять число пикселей"));
                ui.add_enabled_ui(d.resample, |ui| {
                    egui::Grid::new("rs").show(ui, |ui| {
                        ui.label(t("Width", "Ширина"));
                        if ui.add(egui::DragValue::new(&mut d.w).range(1..=16384).suffix(" px")).changed() && d.keep {
                            d.h = ((d.w as f32 * oh as f32 / ow as f32).round() as u32).max(1);
                            d.percent = d.w as f32 / ow as f32 * 100.0;
                        }
                        ui.end_row();
                        ui.label(t("Height", "Высота"));
                        if ui.add(egui::DragValue::new(&mut d.h).range(1..=16384).suffix(" px")).changed() && d.keep {
                            d.w = ((d.h as f32 * ow as f32 / oh as f32).round() as u32).max(1);
                            d.percent = d.h as f32 / oh as f32 * 100.0;
                        }
                        ui.end_row();
                        ui.label("%");
                        if ui.add(egui::DragValue::new(&mut d.percent).range(1.0..=1000.0)).changed() {
                            d.w = ((ow as f32 * d.percent / 100.0).round() as u32).max(1);
                            d.h = ((oh as f32 * d.percent / 100.0).round() as u32).max(1);
                        }
                        ui.end_row();
                    });
                    ui.checkbox(&mut d.keep, t("Keep proportions", "Сохранять пропорции"));
                });
                ui.horizontal(|ui| {
                    ui.label(t("Resolution", "Разрешение"));
                    ui.add(egui::DragValue::new(&mut d.dpi).range(1.0..=4800.0).suffix(" dpi"));
                });
                ui.label(format!("{} {:.1} × {:.1} cm", t("Print size:", "Размер печати:"), d.w as f32 / d.dpi * 2.54, d.h as f32 / d.dpi * 2.54));
                let valid = d.w > 0 && d.h > 0 && (d.w as u64 * d.h as u64) <= MAX_PIXELS;
                if ui.add_enabled(valid, egui::Button::new(t("OK", "ОК"))).clicked() {
                    if d.resample && (d.w, d.h) != (ow, oh) {
                        let (w, h) = (d.w, d.h);
                        let (sx, sy) = (w as f32 / ow as f32, h as f32 / oh as f32);
                        self.image_op(t("Resize", "Изменение размера"), |img| image::imageops::resize(img, w, h, image::imageops::FilterType::Lanczos3), Some(&move |p, _| [p[0] * sx, p[1] * sy]));
                    }
                    if let Some(c) = &mut self.cur {
                        c.dpi = (d.dpi, d.dpi);
                    }
                    done = true;
                }
            });
            if open && !done {
                self.resize = Some(d);
            }
        }
        if let Some((mut fmt, mut q)) = self.export.take() {
            let mut open = true;
            let mut done = false;
            egui::Window::new(t("Export", "Экспорт")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (i, n) in ["PNG", "JPEG", "WebP", "TIFF", "BMP", "GIF", "ICO"].iter().enumerate() {
                        ui.selectable_value(&mut fmt, i as u8, *n);
                    }
                });
                if fmt == 1 {
                    ui.add(egui::Slider::new(&mut q, 30..=100).text(t("Quality", "Качество")));
                }
                if fmt == 1 || fmt == 4 {
                    ui.label(egui::RichText::new(t("Transparency is flattened onto white.", "Прозрачность заменяется белым.")).small());
                }
                if ui.button(t("Export…", "Экспорт…")).clicked() {
                    done = true;
                }
            });
            if done {
                self.do_export(fmt, q);
            } else if open {
                self.export = Some((fmt, q));
            }
        }
    }
}

fn hash_adjust(a: &Adjust) -> u64 {
    let v = [a.exposure, a.brightness, a.contrast, a.highlights, a.shadows, a.saturation, a.vibrance, a.temperature, a.tint, a.sharpness, a.vignette];
    let mut h: u64 = 1469598103934665603;
    for x in v {
        h = (h ^ x.to_bits() as u64).wrapping_mul(1099511628211);
    }
    h ^ (a.look as u64) << 40
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    let pad = 4.0;
    p[0] >= r[0] - pad && p[0] <= r[2] + pad && p[1] >= r[1] - pad && p[1] <= r[3] + pad
}

fn gps(exif: &[(String, String)], key: &str, rkey: &str) -> Option<f64> {
    let v = exif.iter().find(|(k, _)| k == key)?.1.clone();
    let nums: Vec<f64> = v.split(|c: char| !(c.is_ascii_digit() || c == '.')).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
    let d = nums.first()? + nums.get(1).unwrap_or(&0.0) / 60.0 + nums.get(2).unwrap_or(&0.0) / 3600.0;
    let r = exif.iter().find(|(k, _)| k == rkey).map(|x| x.1.clone()).unwrap_or_default();
    Some(if r.contains('S') || r.contains('W') { -d } else { d })
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a.starts_with("--")).unwrap_or(false) {
        kit42::attach_console();
        kit42::settings::load_global();
        let res: Result<String, String> = (|| match args[0].as_str() {
            "--info" => {
                let mut out = String::new();
                for f in &args[1..] {
                    let l = load(Path::new(f), 0)?;
                    out.push_str(&format!("{}: {} {}x{} frames {} pages {}\n", f, l.format, l.frames[0].0.width(), l.frames[0].0.height(), l.frames.len(), l.pages));
                }
                Ok(out)
            }
            "--edit" => {
                let (i, o) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?);
                let l = load(Path::new(i), 0)?;
                let mut img = (*l.frames[0].0).clone();
                let mut a = Adjust::default();
                let mut k = 3;
                while k < args.len() {
                    let v = args.get(k + 1).cloned().unwrap_or_default();
                    match args[k].as_str() {
                        "--brightness" => a.brightness = v.parse().map_err(|_| "bad value")?,
                        "--contrast" => a.contrast = v.parse().map_err(|_| "bad value")?,
                        "--exposure" => a.exposure = v.parse().map_err(|_| "bad value")?,
                        "--saturation" => a.saturation = v.parse().map_err(|_| "bad value")?,
                        "--look" => a.look = match v.as_str() { "mono" => Look::Mono, "noir" => Look::Noir, "sepia" => Look::Sepia, "invert" => Look::Invert, _ => Look::None },
                        "--auto" => {
                            a = pixels::auto_adjust(&img);
                            k -= 1;
                        }
                        "--rotate" => img = if v == "left" { image::imageops::rotate270(&img) } else { image::imageops::rotate90(&img) },
                        "--flip" => img = if v == "v" { image::imageops::flip_vertical(&img) } else { image::imageops::flip_horizontal(&img) },
                        "--resize" => {
                            let (w, h) = v.split_once('x').ok_or("WxH")?;
                            img = image::imageops::resize(&img, w.parse().map_err(|_| "bad w")?, h.parse().map_err(|_| "bad h")?, image::imageops::FilterType::Lanczos3);
                        }
                        "--remove-background" => {
                            img = pixels::remove_background(&img);
                            k -= 1;
                        }
                        other => return Err(format!("unknown option {other}")),
                    }
                    k += 2;
                }
                let img = pixels::apply(&img, &a);
                let ext = kit42::ext(Path::new(o));
                let tmp_app_encode = |img: &image::RgbaImage| -> Result<Vec<u8>, String> {
                    let mut out = Vec::new();
                    let mut cur = std::io::Cursor::new(&mut out);
                    match ext.as_str() {
                        "jpg" | "jpeg" => image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cur, 92).encode_image(&pixels::flatten_white(img)).map_err(|e| e.to_string())?,
                        _ => image::DynamicImage::ImageRgba8(img.clone()).write_to(&mut cur, image::ImageFormat::from_extension(&ext).ok_or("unsupported extension")?).map_err(|e| e.to_string())?,
                    }
                    Ok(out)
                };
                let b = tmp_app_encode(&img)?;
                kit42::write_atomic(Path::new(o), &b).map_err(|e| e.to_string())?;
                Ok(format!("{}x{} -> {o}", img.width(), img.height()))
            }
            "--ocr" => {
                let l = load(Path::new(args.get(1).ok_or("usage")?), 0)?;
                Ok(kit42::winrt::ocr(&l.frames[0].0, None)?.text())
            }
            _ => Err("unknown command".into()),
        })();
        match res {
            Ok(m) => {
                println!("{m}");
                std::process::exit(0)
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1)
            }
        }
    }
    kit42::run(kit42::AppSpec { id: "img42", title: APP, size: [1200.0, 860.0] }, App::new)
}
