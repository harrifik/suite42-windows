#![windows_subsystem = "windows"]
//! SCN42 for Windows — scanning, page clean-up, text recognition and searchable PDF output.

mod cli;
mod export;
mod process;

use egui::{Color32, Context, Key, Pos2, Rect, Sense, Stroke, Vec2};
use kit42::history::History;
use kit42::icons as ic;
use kit42::ui::{self as kui, CTRL, CTRL_ALT, CTRL_SHIFT, NONE};
use kit42::winrt::{OcrResult, ScanColor, ScanSource, ScannerInfo};
use kit42::{settings, t};
use process::{ColorMode, Opts};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

pub const APP: &str = "SCN42";

#[derive(Clone)]
pub struct Page {
    pub id: u64,
    pub original: Arc<image::RgbaImage>,
    pub processed: Arc<image::RgbaImage>,
    pub dpi: f32,
    pub opts: Opts,
    pub ocr: Option<Arc<OcrResult>>,
    pub pending: bool,
}

impl Page {
    fn new(img: image::RgbaImage, dpi: f32, opts: Opts) -> Self {
        let a = Arc::new(img);
        Self { id: rand::random(), original: a.clone(), processed: a, dpi, opts, ocr: None, pending: true }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Preset {
    Document,
    Photo,
    Receipt,
    Draft,
}

impl Preset {
    const ALL: [Preset; 4] = [Preset::Document, Preset::Photo, Preset::Receipt, Preset::Draft];
    fn name(self) -> &'static str {
        match self {
            Preset::Document => t("Document", "Документ"),
            Preset::Photo => t("Photo", "Фото"),
            Preset::Receipt => t("Receipt", "Чек"),
            Preset::Draft => t("Draft", "Черновик"),
        }
    }
    fn settings(self) -> (ColorMode, u32, Opts) {
        match self {
            Preset::Document => (ColorMode::Gray, 300, Opts { crop: true, deskew: true, cleanup: true, enhance: false, color: ColorMode::Gray, rotation: 0 }),
            Preset::Photo => (ColorMode::Color, 300, Opts { crop: true, deskew: false, cleanup: false, enhance: true, color: ColorMode::Color, rotation: 0 }),
            Preset::Receipt => (ColorMode::Gray, 200, Opts { crop: true, deskew: true, cleanup: true, enhance: false, color: ColorMode::BlackWhite, rotation: 0 }),
            Preset::Draft => (ColorMode::BlackWhite, 150, Opts { crop: true, deskew: true, cleanup: true, enhance: false, color: ColorMode::BlackWhite, rotation: 0 }),
        }
    }
}

enum Msg {
    Scanned(Result<Vec<(image::RgbaImage, f32)>, String>),
    Processed { id: u64, opts: Opts, img: image::RgbaImage, turns: u8 },
    Ocr { id: u64, res: Result<OcrResult, String> },
    Scanners(Vec<ScannerInfo>),
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct HistoryEntry {
    path: PathBuf,
    date: String,
    pages: usize,
}

struct SaveDlg {
    template: String,
    folder: PathBuf,
    format: export::Format,
    size: export::Size,
    one_per_page: bool,
    ocr: bool,
    after: usize,
}

struct Placing {
    img: Arc<image::RgbaImage>,
    /// x, y, w, h in page pixels.
    rect: [f32; 4],
    drag: Option<(usize, Pos2)>,
}

struct App {
    pages: Vec<Page>,
    history: History<Vec<Page>>,
    sel: usize,
    multi: std::collections::BTreeSet<usize>,
    scanners: Vec<ScannerInfo>,
    scanner: usize,
    preset: Preset,
    source: ScanSource,
    color: ScanColor,
    dpi: u32,
    duplex: bool,
    opts: Opts,
    remove_blank: bool,
    upright: bool,
    ocr_after: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    scanning: bool,
    thumbs: HashMap<(u64, usize), egui::TextureHandle>,
    preview: Option<((u64, usize), egui::TextureHandle)>,
    picker: kit42::signature::MarkPicker,
    placing: Option<Placing>,
    save: Option<SaveDlg>,
    show_history: bool,
    show_text: bool,
    status: String,
    drag_from: Option<usize>,
    ocr_pending: usize,
    proc_gen: u64,
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = channel();
        let preset: Preset = settings::get("scn42.preset").unwrap_or(Preset::Document);
        let (color, dpi, opts) = preset.settings();
        let mut app = Self {
            pages: vec![],
            history: History::new(40),
            sel: 0,
            multi: Default::default(),
            scanners: vec![],
            scanner: 0,
            preset,
            source: ScanSource::Auto,
            color: match color {
                ColorMode::Color => ScanColor::Color,
                ColorMode::Gray => ScanColor::Gray,
                ColorMode::BlackWhite => ScanColor::BlackWhite,
            },
            dpi,
            duplex: false,
            opts,
            remove_blank: settings::get_or("scn42.remove_blank", false),
            upright: settings::get_or("scn42.upright", true),
            ocr_after: settings::get_or("scn42.ocr", true),
            tx,
            rx,
            scanning: false,
            thumbs: HashMap::new(),
            preview: None,
            picker: Default::default(),
            placing: None,
            save: None,
            show_history: false,
            show_text: false,
            status: String::new(),
            drag_from: None,
            ocr_pending: 0,
            proc_gen: 0,
        };
        app.refresh_scanners();
        let files = kit42::startup_files();
        if !files.is_empty() {
            app.import(files);
        }
        app
    }

    fn refresh_scanners(&mut self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut list = kit42::winrt::list_scanners().unwrap_or_default();
            if let Ok(dir) = std::env::var("K42_FAKE_SCANNER") {
                list.insert(0, ScannerInfo { id: format!("fake:{dir}"), name: t("Virtual Scanner", "Виртуальный сканер").into() });
            }
            let _ = tx.send(Msg::Scanners(list));
        });
    }

    fn checkpoint(&mut self, label: &str) {
        self.history.push(self.pages.clone(), label);
    }

    fn dirty(&self) -> bool {
        self.history.is_dirty() && !self.pages.is_empty()
    }

    /// Starts background processing of a page (crop, deskew, clean-up, colour, upright).
    fn process_page(&mut self, id: u64, auto_upright: bool) {
        let Some(p) = self.pages.iter_mut().find(|p| p.id == id) else { return };
        p.pending = true;
        let (orig, opts, dpi, tx) = (p.original.clone(), p.opts.clone(), p.dpi, self.tx.clone());
        std::thread::spawn(move || {
            let mut opts = opts;
            let mut turns = 0;
            if auto_upright {
                turns = process::upright_turns(&process::rotate_quarter(&orig, opts.rotation));
                opts.rotation = (opts.rotation + turns) % 4;
            }
            let img = process::process(&orig, &opts, dpi);
            let _ = tx.send(Msg::Processed { id, opts, img, turns });
        });
    }

    fn run_ocr(&mut self, ids: Vec<u64>) {
        for id in ids {
            let Some(p) = self.pages.iter().find(|p| p.id == id) else { continue };
            let (img, tx) = (p.processed.clone(), self.tx.clone());
            self.ocr_pending += 1;
            std::thread::spawn(move || {
                let res = kit42::winrt::ocr(&img, None);
                let _ = tx.send(Msg::Ocr { id, res });
            });
        }
    }

    fn add_pages(&mut self, imgs: Vec<(image::RgbaImage, f32)>, label: &str) {
        if imgs.is_empty() {
            return;
        }
        self.checkpoint(label);
        let at = if self.pages.is_empty() { 0 } else { self.sel + 1 };
        let mut ids = vec![];
        for (k, (img, dpi)) in imgs.into_iter().enumerate() {
            let p = Page::new(img, dpi, self.opts.clone());
            ids.push(p.id);
            self.pages.insert((at + k).min(self.pages.len()), p);
        }
        self.sel = at.min(self.pages.len() - 1);
        for id in ids {
            self.process_page(id, self.upright);
        }
    }

    fn import(&mut self, files: Vec<PathBuf>) {
        let mut imgs = vec![];
        for f in &files {
            let ext = kit42::ext(f);
            let res: Result<(), String> = (|| {
                let bytes = kit42::ooxml::read_file(f)?;
                if ext == "pdf" {
                    let (r, sizes) = kit42::winrt::PdfRenderer::open(bytes, None)?;
                    for (i, (w, h)) in sizes.iter().enumerate().take(2000) {
                        let dpi = (250.0f32).min(6000.0 / (w.max(*h) / 72.0));
                        let img = r.render(i as u32, (w / 72.0 * dpi) as u32)?;
                        imgs.push((img, dpi));
                    }
                } else {
                    let img = match image::load_from_memory(&bytes) {
                        Ok(i) => i.to_rgba8(),
                        Err(_) => kit42::winrt::decode_image(bytes.clone())?,
                    };
                    let img = apply_exif_orientation(img, &bytes);
                    let long = img.width().max(img.height());
                    let scale = (6000.0 / long as f32).min(1.0);
                    let img = if scale < 1.0 { kit42::raster::fit_within(&img, 6000) } else { img };
                    // Assume 300 dpi for scans, 150 dpi for photos.
                    let dpi = if long > 2500 { 300.0 } else { 150.0 } * scale;
                    imgs.push((img, dpi));
                }
                Ok(())
            })();
            if let Err(e) = res {
                kui::error(APP, &format!("{}: {e}", kit42::file_name(f)));
            }
        }
        self.add_pages(imgs, t("Import", "Импорт"));
    }

    fn scan(&mut self) {
        let Some(sc) = self.scanners.get(self.scanner).cloned() else {
            if kui::confirm(APP, t("No scanner is available. Open the Windows Scan app?", "Сканер не найден. Открыть приложение «Сканер» Windows?")) {
                kit42::winrt::open_windows_scan();
            }
            return;
        };
        self.scanning = true;
        self.status = t("Scanning…", "Сканирование…").into();
        let (tx, source, color, dpi, duplex) = (self.tx.clone(), self.source, self.color, self.dpi, self.duplex);
        std::thread::spawn(move || {
            let res = (|| -> Result<Vec<(image::RgbaImage, f32)>, String> {
                let files: Vec<PathBuf> = if let Some(dir) = sc.id.strip_prefix("fake:") {
                    let prefix = if source == ScanSource::Feeder { "feed" } else { "glass" };
                    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
                        .map_err(|e| e.to_string())?
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| kit42::file_name(p).starts_with(prefix) || source == ScanSource::Auto)
                        .collect();
                    v.sort();
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    v
                } else {
                    let folder = std::env::temp_dir().join("Suite42").join(format!("scan-{}", rand::random::<u32>()));
                    kit42::winrt::scan(sc.id.clone(), source, color, dpi, duplex, folder)?
                };
                let mut out = vec![];
                for f in files {
                    let b = std::fs::read(&f).map_err(|e| e.to_string())?;
                    let img = image::load_from_memory(&b).map(|i| i.to_rgba8()).or_else(|_| kit42::winrt::decode_image(b))?;
                    out.push((img, dpi as f32));
                }
                if out.is_empty() {
                    return Err(t("The scanner returned no pages (is the feeder empty?).", "Сканер не вернул страниц (лоток пуст?).").into());
                }
                Ok(out)
            })();
            let _ = tx.send(Msg::Scanned(res));
        });
    }

    fn poll(&mut self, ctx: &Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Scanners(list) => {
                    let last: Option<String> = settings::get("scn42.scanner");
                    self.scanner = list.iter().position(|s| Some(&s.id) == last.as_ref()).unwrap_or(0);
                    self.scanners = list;
                }
                Msg::Scanned(res) => {
                    self.scanning = false;
                    match res {
                        Ok(imgs) => {
                            let n = imgs.len();
                            self.add_pages(imgs, t("Scan", "Сканирование"));
                            self.status = format!("{} {n}", t("Pages scanned:", "Отсканировано страниц:"));
                        }
                        Err(e) => {
                            self.status.clear();
                            kui::error(APP, &e);
                        }
                    }
                }
                Msg::Processed { id, opts, img, turns } => {
                    let remove_blank = self.remove_blank;
                    let ocr_after = self.ocr_after;
                    if let Some(p) = self.pages.iter_mut().find(|p| p.id == id) {
                        let mut expect = p.opts.clone();
                        expect.rotation = (expect.rotation + turns) % 4;
                        if expect == opts {
                            p.opts = opts;
                            p.processed = Arc::new(img);
                            p.pending = false;
                            p.ocr = None;
                        }
                    }
                    self.proc_gen += 1;
                    if remove_blank {
                        if let Some(i) = self.pages.iter().position(|p| p.id == id && !p.pending) {
                            if process::is_blank(&self.pages[i].processed) {
                                self.pages.remove(i);
                                self.sel = self.sel.min(self.pages.len().saturating_sub(1));
                                self.status = t("A blank page was removed.", "Пустая страница удалена.").into();
                                continue;
                            }
                        }
                    }
                    if ocr_after {
                        self.run_ocr(vec![id]);
                    }
                }
                Msg::Ocr { id, res } => {
                    self.ocr_pending = self.ocr_pending.saturating_sub(1);
                    match res {
                        Ok(o) => {
                            if let Some(p) = self.pages.iter_mut().find(|p| p.id == id) {
                                p.ocr = Some(Arc::new(o));
                            }
                        }
                        Err(e) => self.status = format!("OCR: {e}"),
                    }
                }
            }
            ctx.request_repaint();
        }
        if self.scanning || self.ocr_pending > 0 || self.pages.iter().any(|p| p.pending) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn thumb(&mut self, ctx: &Context, i: usize) -> Option<egui::TextureHandle> {
        let p = self.pages.get(i)?;
        let key = (p.id, Arc::as_ptr(&p.processed) as usize);
        if let Some(t) = self.thumbs.get(&key) {
            return Some(t.clone());
        }
        let small = kit42::raster::fit_within(&p.processed, 240);
        let tex = kui::texture(ctx, &format!("th{}", p.id), &small);
        self.thumbs.retain(|k, _| k.0 != p.id);
        self.thumbs.insert(key, tex.clone());
        Some(tex)
    }

    fn preview_tex(&mut self, ctx: &Context) -> Option<egui::TextureHandle> {
        let p = self.pages.get(self.sel)?;
        let key = (p.id, Arc::as_ptr(&p.processed) as usize);
        if let Some((k, t)) = &self.preview {
            if *k == key {
                return Some(t.clone());
            }
        }
        let img = kit42::raster::fit_within(&p.processed, 2200);
        let tex = kui::texture(ctx, "preview", &img);
        self.preview = Some((key, tex.clone()));
        Some(tex)
    }

    fn selection(&self) -> Vec<usize> {
        if self.multi.is_empty() {
            if self.pages.is_empty() {
                vec![]
            } else {
                vec![self.sel]
            }
        } else {
            self.multi.iter().cloned().collect()
        }
    }

    fn change_opts(&mut self, label: &str, f: impl Fn(&mut Opts)) {
        let sel = self.selection();
        if sel.is_empty() {
            return;
        }
        self.checkpoint(label);
        let mut ids = vec![];
        for i in sel {
            if let Some(p) = self.pages.get_mut(i) {
                f(&mut p.opts);
                ids.push(p.id);
            }
        }
        for id in ids {
            self.process_page(id, false);
        }
    }

    fn delete_pages(&mut self) {
        let mut sel = self.selection();
        if sel.is_empty() {
            return;
        }
        self.checkpoint(t("Delete Page", "Удаление страницы"));
        sel.sort_by(|a, b| b.cmp(a));
        for i in sel {
            if i < self.pages.len() {
                self.pages.remove(i);
            }
        }
        self.multi.clear();
        self.sel = self.sel.min(self.pages.len().saturating_sub(1));
    }

    fn move_page(&mut self, dir: i32) {
        let i = self.sel;
        let j = i as i32 + dir;
        if j < 0 || j as usize >= self.pages.len() {
            return;
        }
        self.checkpoint(t("Move Page", "Перемещение страницы"));
        self.pages.swap(i, j as usize);
        self.sel = j as usize;
    }

    fn all_text(&self) -> String {
        self.pages.iter().filter_map(|p| p.ocr.as_ref().map(|o| export::page_text(o))).collect::<Vec<_>>().join("\n\x0C\n")
    }

    fn open_save(&mut self) {
        if self.pages.is_empty() {
            return;
        }
        let folder: PathBuf = settings::get("scn42.folder").unwrap_or_else(|| kit42::shell::documents_dir().join("Scans"));
        self.save = Some(SaveDlg {
            template: settings::get_or("scn42.template", "Scan YYYY-MM-DD HH-mm".to_string()),
            folder,
            format: settings::get_or("scn42.format", export::Format::Pdf),
            size: settings::get_or("scn42.size", export::Size::Balanced),
            one_per_page: false,
            ocr: self.ocr_after,
            after: settings::get_or("scn42.after", 0usize),
        });
    }

    fn do_save(&mut self, d: &SaveDlg) -> bool {
        if d.ocr {
            // Recognise missing pages synchronously so the text layer is complete.
            for p in self.pages.iter_mut().filter(|p| p.ocr.is_none()) {
                if let Ok(o) = kit42::winrt::ocr(&p.processed, None) {
                    p.ocr = Some(Arc::new(o));
                }
            }
        }
        let title = self.pages.iter().find_map(|p| p.ocr.as_ref().and_then(|o| o.lines.first().map(|l| l.text.clone()))).unwrap_or_default();
        let title: String = title.chars().take(40).collect();
        std::fs::create_dir_all(&d.folder).ok();
        let base = export::expand_name(&d.template, self.history_list().len() + 1, &title);
        let mut path = d.folder.join(format!("{base}.{}", d.format.ext()));
        let mut k = 2;
        while path.exists() {
            path = d.folder.join(format!("{base} ({k}).{}", d.format.ext()));
            k += 1;
        }
        match export::save(&self.pages, d.format, d.size, &path, d.one_per_page) {
            Ok(files) => {
                settings::set("scn42.folder", &d.folder);
                settings::set("scn42.template", &d.template);
                settings::set("scn42.format", d.format);
                settings::set("scn42.size", d.size);
                settings::set("scn42.after", d.after);
                let mut h = self.history_list();
                h.insert(0, HistoryEntry { path: files[0].clone(), date: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(), pages: self.pages.len() });
                h.truncate(200);
                settings::set("scn42.history", h);
                self.history.mark_saved();
                self.status = format!("{} {}", t("Saved:", "Сохранено:"), files[0].display());
                match d.after {
                    1 if matches!(d.format, export::Format::Pdf | export::Format::PdfA) => kit42::shell::open_in("pdf42", &files[0]),
                    1 => kit42::shell::open_in("img42", &files[0]),
                    2 => kit42::shell::reveal(&files[0]),
                    3 => {
                        kit42::shell::open_path(&files[0]);
                    }
                    _ => {}
                }
                true
            }
            Err(e) => {
                kui::error(APP, &e);
                false
            }
        }
    }

    fn history_list(&self) -> Vec<HistoryEntry> {
        settings::get("scn42.history").unwrap_or_default()
    }
}

fn apply_exif_orientation(img: image::RgbaImage, bytes: &[u8]) -> image::RgbaImage {
    // Minimal EXIF orientation reader for JPEG (APP1 'Exif').
    let o = (|| {
        let i = bytes.windows(6).position(|w| w == b"Exif\0\0")? + 6;
        let tiff = &bytes[i..];
        let le = tiff.get(0..2)? == b"II";
        let rd16 = |p: usize| -> Option<u16> {
            let b = tiff.get(p..p + 2)?;
            Some(if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
        };
        let rd32 = |p: usize| -> Option<u32> {
            let b = tiff.get(p..p + 4)?;
            Some(if le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) })
        };
        let ifd = rd32(4)? as usize;
        let n = rd16(ifd)? as usize;
        for k in 0..n {
            let e = ifd + 2 + k * 12;
            if rd16(e)? == 0x0112 {
                return rd16(e + 8);
            }
        }
        None
    })()
    .unwrap_or(1);
    match o {
        3 => image::imageops::rotate180(&img),
        6 => image::imageops::rotate90(&img),
        8 => image::imageops::rotate270(&img),
        2 => image::imageops::flip_horizontal(&img),
        4 => image::imageops::flip_vertical(&img),
        _ => img,
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        let name = if self.pages.is_empty() { None } else { Some(format!("{} {}", self.pages.len(), t("pages", "стр."))) };
        kui::set_title(ctx, APP, name.as_deref(), self.dirty());
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.import(dropped);
        }
        self.shortcuts(ctx);
        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu(ui));
        let status = format!(
            "{} · {}{}",
            if self.pages.is_empty() { t("No pages", "Нет страниц").to_string() } else { format!("{} {} / {}", t("Page", "Страница"), self.sel + 1, self.pages.len()) },
            self.pages.get(self.sel).map(|p| format!("{} × {} px, {:.0} dpi", p.processed.width(), p.processed.height(), p.dpi)).unwrap_or_default(),
            if self.status.is_empty() { String::new() } else { format!(" · {}", self.status) }
        );
        kui::status_bar(ctx, |ui| {
            ui.label(status);
            if self.ocr_pending > 0 {
                ui.spinner();
                ui.label(t("recognising text…", "распознавание текста…"));
            }
        });
        egui::SidePanel::right("settings").resizable(false).exact_width(270.0).show(ctx, |ui| self.settings_panel(ui));
        egui::SidePanel::left("pages").resizable(true).default_width(170.0).show(ctx, |ui| self.page_list(ui));
        if self.show_text {
            egui::TopBottomPanel::bottom("text").resizable(true).default_height(180.0).show(ctx, |ui| {
                let mut text = self.pages.get(self.sel).and_then(|p| p.ocr.as_ref().map(|o| export::page_text(o))).unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.strong(t("Recognised text", "Распознанный текст"));
                    if ui.small_button(t("Copy", "Копировать")).clicked() {
                        ui.ctx().copy_text(text.clone());
                    }
                });
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut text).desired_width(f32::INFINITY).interactive(true));
                });
            });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(if ctx.style().visuals.dark_mode { Color32::from_gray(38) } else { Color32::from_gray(222) }))
            .show(ctx, |ui| self.preview_ui(ui));
        if let Some(img) = self.picker.show(ctx) {
            if let Some(p) = self.pages.get(self.sel) {
                let w = p.processed.width() as f32 * 0.3;
                let h = w * img.height() as f32 / img.width().max(1) as f32;
                let (pw, ph) = (p.processed.width() as f32, p.processed.height() as f32);
                self.placing = Some(Placing { img: Arc::new(img), rect: [pw * 0.6 - w / 2.0, ph * 0.8 - h / 2.0, w, h], drag: None });
            }
        }
        self.save_dialog(ctx);
        self.history_window(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty() {
            match kui::ask_save(&format!("{} {}", self.pages.len(), t("scanned pages", "отсканированных страниц"))) {
                kui::SaveChoice::Save => {
                    self.open_save();
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
                kui::SaveChoice::Discard => {}
                kui::SaveChoice::Cancel => ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose),
            }
        }
    }
}

impl App {
    fn shortcuts(&mut self, ctx: &Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        if self.placing.is_some() {
            if kui::shortcut(ctx, NONE, Key::Enter) {
                self.print_mark();
            }
            if kui::shortcut(ctx, NONE, Key::Escape) {
                self.placing = None;
            }
            return;
        }
        if kui::shortcut(ctx, CTRL, Key::S) {
            self.open_save();
        }
        if kui::shortcut(ctx, CTRL, Key::O) {
            self.import_dialog();
        }
        if kui::shortcut(ctx, CTRL, Key::Z) {
            if let Some(p) = self.history.undo(self.pages.clone()) {
                self.pages = p;
                self.sel = self.sel.min(self.pages.len().saturating_sub(1));
            }
        }
        if kui::shortcut(ctx, CTRL, Key::Y) || kui::shortcut(ctx, CTRL_SHIFT, Key::Z) {
            if let Some(p) = self.history.redo(self.pages.clone()) {
                self.pages = p;
                self.sel = self.sel.min(self.pages.len().saturating_sub(1));
            }
        }
        if kui::shortcut(ctx, CTRL, Key::L) {
            self.change_opts(t("Rotate", "Поворот"), |o| o.rotation = (o.rotation + 3) % 4);
        }
        if kui::shortcut(ctx, CTRL, Key::R) {
            self.change_opts(t("Rotate", "Поворот"), |o| o.rotation = (o.rotation + 1) % 4);
        }
        if kui::shortcut(ctx, NONE, Key::Delete) {
            self.delete_pages();
        }
        if kui::shortcut(ctx, CTRL_ALT, Key::ArrowUp) {
            self.move_page(-1);
        }
        if kui::shortcut(ctx, CTRL_ALT, Key::ArrowDown) {
            self.move_page(1);
        }
        if kui::shortcut(ctx, CTRL_ALT, Key::R) {
            let ids = self.pages.iter().map(|p| p.id).collect();
            self.run_ocr(ids);
        }
        if kui::shortcut(ctx, CTRL_SHIFT, Key::K) {
            self.picker.open = true;
        }
        if kui::shortcut(ctx, NONE, Key::ArrowDown) || kui::shortcut(ctx, NONE, Key::PageDown) {
            self.sel = (self.sel + 1).min(self.pages.len().saturating_sub(1));
        }
        if kui::shortcut(ctx, NONE, Key::ArrowUp) || kui::shortcut(ctx, NONE, Key::PageUp) {
            self.sel = self.sel.saturating_sub(1);
        }
    }

    fn import_dialog(&mut self) {
        if let Some(f) = kui::open_dialog(t("Import", "Импорт"), &[(t("Images and PDF", "Изображения и PDF"), &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "gif", "webp", "heic", "pdf"])]).pick_files() {
            self.import(f);
        }
    }

    fn menu(&mut self, ui: &mut egui::Ui) {
        let has = !self.pages.is_empty();
        egui::menu::bar(ui, |ui| {
            ui.menu_button(t("File", "Файл"), |ui| {
                if kui::menu_item(ui, t("New Scan", "Новое сканирование"), "") {
                    if !self.dirty() || kui::confirm(APP, t("Discard the unsaved pages?", "Удалить несохранённые страницы?")) {
                        self.pages.clear();
                        self.history.clear();
                    }
                }
                if kui::menu_item(ui, t("Import…", "Импорт…"), "Ctrl+O") {
                    self.import_dialog();
                }
                ui.separator();
                if kui::menu_item_enabled(ui, has, t("Save…", "Сохранить…"), "Ctrl+S") {
                    self.open_save();
                }
                if kui::menu_item_enabled(ui, has, t("Export Text…", "Экспорт текста…"), "") {
                    self.export_text();
                }
                if kui::menu_item_enabled(ui, has, t("Print…", "Печать…"), "") {
                    if let Ok(b) = export::pdf(&self.pages, export::Size::High, false, "Scan") {
                        let tmp = kit42::shell::temp_file("scan", "pdf");
                        if std::fs::write(&tmp, b).is_ok() {
                            kit42::shell::print(&tmp);
                        }
                    }
                }
                ui.separator();
                if kui::menu_item(ui, t("History…", "История…"), "Ctrl+Alt+3") {
                    self.show_history = true;
                }
                ui.separator();
                if kui::menu_item(ui, t("Exit", "Выход"), "Alt+F4") {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button(t("Edit", "Правка"), |ui| {
                if kui::menu_item_enabled(ui, self.history.can_undo(), t("Undo", "Отменить"), "Ctrl+Z") {
                    if let Some(p) = self.history.undo(self.pages.clone()) {
                        self.pages = p;
                    }
                }
                if kui::menu_item_enabled(ui, self.history.can_redo(), t("Redo", "Повторить"), "Ctrl+Y") {
                    if let Some(p) = self.history.redo(self.pages.clone()) {
                        self.pages = p;
                    }
                }
                ui.separator();
                if kui::menu_item_enabled(ui, has, t("Select All Pages", "Выделить все страницы"), "") {
                    self.multi = (0..self.pages.len()).collect();
                }
                if kui::menu_item_enabled(ui, has, t("Delete Page", "Удалить страницу"), "Del") {
                    self.delete_pages();
                }
            });
            ui.menu_button(t("Scan", "Сканирование"), |ui| {
                if kui::menu_item(ui, t("Scan", "Сканировать"), "") {
                    self.scan();
                }
                if kui::menu_item(ui, t("Refresh Scanners", "Обновить список сканеров"), "") {
                    self.refresh_scanners();
                }
                if kui::menu_item(ui, t("Open Windows Scan", "Открыть «Сканер» Windows"), "") {
                    kit42::winrt::open_windows_scan();
                }
            });
            ui.menu_button(t("Page", "Страница"), |ui| {
                ui.add_enabled_ui(has, |ui| {
                    if kui::menu_item(ui, t("Rotate Left", "Повернуть влево"), "Ctrl+L") {
                        self.change_opts(t("Rotate", "Поворот"), |o| o.rotation = (o.rotation + 3) % 4);
                    }
                    if kui::menu_item(ui, t("Rotate Right", "Повернуть вправо"), "Ctrl+R") {
                        self.change_opts(t("Rotate", "Поворот"), |o| o.rotation = (o.rotation + 1) % 4);
                    }
                    if kui::menu_item(ui, t("Move Up", "Выше"), "Ctrl+Alt+↑") {
                        self.move_page(-1);
                    }
                    if kui::menu_item(ui, t("Move Down", "Ниже"), "Ctrl+Alt+↓") {
                        self.move_page(1);
                    }
                    ui.separator();
                    if kui::menu_item(ui, t("Crop and Straighten Again", "Обрезать и выровнять заново"), "") {
                        self.change_opts(t("Crop and Straighten", "Обрезка и выравнивание"), |o| {
                            o.crop = true;
                            o.deskew = true;
                        });
                    }
                    if kui::menu_item(ui, t("Enhance", "Улучшить"), "Ctrl+Alt+E") {
                        self.change_opts(t("Enhance", "Улучшение"), |o| o.enhance = !o.enhance);
                    }
                    if kui::menu_item(ui, t("Remove Blank Pages", "Удалить пустые страницы"), "") {
                        let blank: Vec<usize> = self.pages.iter().enumerate().filter(|(_, p)| process::is_blank(&p.processed)).map(|(i, _)| i).collect();
                        if !blank.is_empty() {
                            self.checkpoint(t("Remove Blank Pages", "Удаление пустых страниц"));
                            for i in blank.iter().rev() {
                                self.pages.remove(*i);
                            }
                            self.sel = self.sel.min(self.pages.len().saturating_sub(1));
                        }
                        self.status = format!("{} {}", t("Blank pages removed:", "Удалено пустых страниц:"), blank.len());
                    }
                    if kui::menu_item(ui, t("Split Document Here", "Разделить документ здесь"), "") {
                        let rest: Vec<Page> = self.pages.split_off(self.sel);
                        if !rest.is_empty() {
                            self.checkpoint(t("Split", "Разделение"));
                            let tmp = kit42::shell::temp_file("Split", "pdf");
                            if let Ok(b) = export::pdf(&rest, export::Size::Lossless, false, "Scan") {
                                if std::fs::write(&tmp, b).is_ok() {
                                    if let Ok(exe) = std::env::current_exe() {
                                        let _ = std::process::Command::new(exe).arg(&tmp).spawn();
                                    }
                                }
                            }
                            self.sel = self.sel.saturating_sub(1);
                        }
                    }
                });
            });
            ui.menu_button(t("Text", "Текст"), |ui| {
                if kui::menu_item_enabled(ui, has, t("Recognize Text", "Распознать текст"), "Ctrl+Alt+R") {
                    let ids = self.pages.iter().map(|p| p.id).collect();
                    self.run_ocr(ids);
                }
                if kui::menu_item_enabled(ui, has, t("Copy Text", "Копировать текст"), "Ctrl+Shift+C") {
                    ui.ctx().copy_text(self.all_text());
                }
                ui.checkbox(&mut self.show_text, t("Show Text Panel", "Показать панель текста"));
            });
            ui.menu_button(t("Tools", "Инструменты"), |ui| {
                if kui::menu_item_enabled(ui, has, t("Signature or Stamp…", "Подпись или печать…"), "Ctrl+Shift+K") {
                    self.picker.open = true;
                }
            });
            ui.menu_button(t("Help", "Справка"), |ui| {
                kui::language_menu(ui);
                if kui::menu_item(ui, t("About SCN42", "О программе SCN42"), "") {
                    kui::about(APP, t("scanning and searchable PDF", "сканирование и PDF с текстом"));
                }
            });
        });
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(t("Scanner", "Сканер"));
            if kui::tool_button(ui, "\u{E72C}", t("Refresh", "Обновить"), true).clicked() {
                self.refresh_scanners();
            }
        });
        let name = self.scanners.get(self.scanner).map(|s| s.name.clone()).unwrap_or_else(|| t("No scanner found", "Сканер не найден").into());
        egui::ComboBox::from_id_salt("scanner").width(250.0).selected_text(name).show_ui(ui, |ui| {
            for (i, s) in self.scanners.iter().enumerate() {
                if ui.selectable_value(&mut self.scanner, i, &s.name).changed() {
                    settings::set("scn42.scanner", &s.id);
                }
            }
        });
        ui.add_space(4.0);
        ui.label(t("Preset", "Пресет"));
        ui.horizontal_wrapped(|ui| {
            for p in Preset::ALL {
                if ui.selectable_label(self.preset == p, p.name()).clicked() {
                    self.preset = p;
                    let (c, dpi, opts) = p.settings();
                    self.color = match c {
                        ColorMode::Color => ScanColor::Color,
                        ColorMode::Gray => ScanColor::Gray,
                        ColorMode::BlackWhite => ScanColor::BlackWhite,
                    };
                    self.dpi = dpi;
                    self.opts = opts;
                    settings::set("scn42.preset", p);
                }
            }
        });
        ui.separator();
        egui::Grid::new("scanopts").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(t("Source", "Источник"));
            egui::ComboBox::from_id_salt("src")
                .selected_text(match self.source {
                    ScanSource::Auto => t("Automatic", "Авто"),
                    ScanSource::Flatbed => t("Flatbed", "Планшет"),
                    ScanSource::Feeder => t("Feeder", "Автоподатчик"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.source, ScanSource::Auto, t("Automatic", "Авто"));
                    ui.selectable_value(&mut self.source, ScanSource::Flatbed, t("Flatbed", "Планшет"));
                    ui.selectable_value(&mut self.source, ScanSource::Feeder, t("Feeder", "Автоподатчик"));
                });
            ui.end_row();
            ui.label(t("Colour", "Цвет"));
            egui::ComboBox::from_id_salt("col")
                .selected_text(match self.opts.color {
                    ColorMode::Color => t("Colour", "Цветной"),
                    ColorMode::Gray => t("Grey", "Оттенки серого"),
                    ColorMode::BlackWhite => t("Black & white", "Чёрно-белый"),
                })
                .show_ui(ui, |ui| {
                    for (c, sc, l) in [
                        (ColorMode::Color, ScanColor::Color, t("Colour", "Цветной")),
                        (ColorMode::Gray, ScanColor::Gray, t("Grey", "Оттенки серого")),
                        (ColorMode::BlackWhite, ScanColor::Gray, t("Black & white", "Чёрно-белый")),
                    ] {
                        if ui.selectable_label(self.opts.color == c, l).clicked() {
                            self.opts.color = c;
                            self.color = sc;
                        }
                    }
                });
            ui.end_row();
            ui.label(t("Resolution", "Разрешение"));
            egui::ComboBox::from_id_salt("dpi").selected_text(format!("{} dpi", self.dpi)).show_ui(ui, |ui| {
                for d in [75, 100, 150, 200, 300, 400, 600, 1200] {
                    ui.selectable_value(&mut self.dpi, d, format!("{d} dpi"));
                }
            });
            ui.end_row();
        });
        ui.add_enabled(self.source == ScanSource::Feeder, egui::Checkbox::new(&mut self.duplex, t("Both sides (duplex)", "Двусторонний")));
        ui.separator();
        ui.label(egui::RichText::new(t("After scanning", "После сканирования")).strong());
        ui.checkbox(&mut self.opts.crop, t("Crop to the page", "Обрезать по странице"));
        ui.checkbox(&mut self.opts.deskew, t("Straighten", "Выровнять"));
        ui.checkbox(&mut self.opts.cleanup, t("Clean up (white paper)", "Очистить (белая бумага)"));
        ui.checkbox(&mut self.opts.enhance, t("Enhance photos", "Улучшить фото"));
        if ui.checkbox(&mut self.upright, t("Turn pages upright", "Разворачивать страницы")).changed() {
            settings::set("scn42.upright", self.upright);
        }
        if ui.checkbox(&mut self.remove_blank, t("Remove blank pages", "Удалять пустые страницы")).changed() {
            settings::set("scn42.remove_blank", self.remove_blank);
        }
        if ui.checkbox(&mut self.ocr_after, t("Recognise text", "Распознавать текст")).changed() {
            settings::set("scn42.ocr", self.ocr_after);
        }
        ui.add_space(10.0);
        let scan_label = if self.scanning { t("Scanning…", "Сканирование…") } else { t("Scan", "Сканировать") };
        let b = egui::Button::new(egui::RichText::new(format!("{}   {}", ic::SCAN, scan_label)).size(18.0).color(Color32::WHITE))
            .fill(kui::accent(ui.ctx()))
            .min_size(Vec2::new(250.0, 46.0));
        if ui.add_enabled(!self.scanning, b).clicked() {
            self.scan();
        }
        ui.add_space(6.0);
        if ui.add_sized([250.0, 30.0], egui::Button::new(format!("{}  {}", ic::OPEN, t("Import Images or PDF…", "Импорт изображений или PDF…")))).clicked() {
            self.import_dialog();
        }
        if ui.add_enabled(!self.pages.is_empty(), egui::Button::new(format!("{}  {}", ic::SAVE, t("Save…", "Сохранить…"))).min_size(Vec2::new(250.0, 30.0))).clicked() {
            self.open_save();
        }
        // Selected page.
        if let Some(p) = self.pages.get(self.sel).cloned() {
            ui.separator();
            ui.label(egui::RichText::new(t("This page", "Эта страница")).strong());
            ui.horizontal(|ui| {
                for (c, l) in [(ColorMode::Color, t("Colour", "Цвет")), (ColorMode::Gray, t("Grey", "Серый")), (ColorMode::BlackWhite, t("B&W", "Ч/Б"))] {
                    if ui.selectable_label(p.opts.color == c, l).clicked() {
                        self.change_opts(t("Colour", "Цвет"), move |o| o.color = c);
                    }
                }
            });
            ui.horizontal(|ui| {
                if kui::tool_button(ui, "\u{E7AD}", t("Rotate Right", "Повернуть вправо"), true).clicked() {
                    self.change_opts(t("Rotate", "Поворот"), |o| o.rotation = (o.rotation + 1) % 4);
                }
                if kui::tool_toggle(ui, p.opts.crop, ic::CROP, t("Crop to page", "Обрезка по странице")).clicked() {
                    self.change_opts(t("Crop", "Обрезка"), |o| o.crop = !o.crop);
                }
                if kui::tool_toggle(ui, p.opts.deskew, "\u{E7AC}", t("Straighten", "Выравнивание")).clicked() {
                    self.change_opts(t("Straighten", "Выравнивание"), |o| o.deskew = !o.deskew);
                }
                if kui::tool_toggle(ui, p.opts.cleanup, ic::ERASE, t("Clean up", "Очистка")).clicked() {
                    self.change_opts(t("Clean Up", "Очистка"), |o| o.cleanup = !o.cleanup);
                }
                if kui::tool_toggle(ui, p.opts.enhance, ic::WAND, t("Enhance", "Улучшение")).clicked() {
                    self.change_opts(t("Enhance", "Улучшение"), |o| o.enhance = !o.enhance);
                }
                if kui::tool_button(ui, ic::SIGNATURE, t("Signature or Stamp", "Подпись или печать"), true).clicked() {
                    self.picker.open = true;
                }
                if kui::tool_button(ui, ic::DELETE, t("Delete Page", "Удалить страницу"), true).clicked() {
                    self.delete_pages();
                }
            });
            if p.pending {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t("Processing…", "Обработка…"));
                });
            }
        }
    }

    fn page_list(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let n = self.pages.len();
        let mut clicked = None;
        let mut rects = vec![];
        let mut drop = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for i in 0..n {
                let tex = self.thumb(&ctx, i);
                let w = (ui.available_width() - 24.0).clamp(60.0, 220.0);
                let (iw, ih) = self.pages[i].processed.dimensions();
                let h = w * ih as f32 / iw.max(1) as f32;
                ui.vertical_centered(|ui| {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click_and_drag());
                    rects.push(rect);
                    if let Some(tex) = tex {
                        ui.painter().image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    }
                    let selected = i == self.sel || self.multi.contains(&i);
                    ui.painter().rect_stroke(rect, 2.0, if selected { Stroke::new(3.0, kui::accent(&ctx)) } else { Stroke::new(1.0, Color32::from_gray(160)) }, egui::StrokeKind::Outside);
                    if self.pages[i].pending {
                        ui.painter().circle_filled(rect.right_top() + Vec2::new(-10.0, 10.0), 5.0, Color32::from_rgb(240, 160, 0));
                    }
                    if self.pages[i].ocr.is_some() {
                        ui.painter().text(rect.left_bottom() + Vec2::new(4.0, -4.0), egui::Align2::LEFT_BOTTOM, "T", egui::FontId::proportional(11.0), Color32::from_rgb(0, 120, 60));
                    }
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                    if resp.drag_started() {
                        self.drag_from = Some(i);
                    }
                    ui.label(egui::RichText::new((i + 1).to_string()).small());
                });
            }
            if let (Some(from), Some(pos)) = (self.drag_from, ctx.pointer_interact_pos()) {
                let target = rects.iter().position(|r| pos.y < r.center().y).unwrap_or(n);
                if let Some(r) = rects.get(target.min(n.saturating_sub(1))) {
                    let y = if target < n { r.top() - 4.0 } else { r.bottom() + 4.0 };
                    ui.painter().hline(r.x_range(), y, Stroke::new(3.0, kui::accent(&ctx)));
                }
                if ctx.input(|i| i.pointer.any_released()) {
                    drop = Some((from, target));
                }
            }
        });
        if let Some(i) = clicked {
            if ctx.input(|inp| inp.modifiers.command) {
                if !self.multi.insert(i) {
                    self.multi.remove(&i);
                }
            } else {
                self.multi.clear();
            }
            self.sel = i;
            self.placing = None;
        }
        if let Some((from, to)) = drop {
            self.drag_from = None;
            if to != from && to != from + 1 {
                self.checkpoint(t("Move Page", "Перемещение страницы"));
                let p = self.pages.remove(from);
                let to = if to > from { to - 1 } else { to };
                self.pages.insert(to, p);
                self.sel = to;
            }
        }
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(tex) = self.preview_tex(&ctx) else {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new(t("Press Scan, or drop images and PDFs here.", "Нажмите «Сканировать» или перетащите сюда изображения и PDF.")).size(16.0).weak());
            });
            return;
        };
        let avail = ui.available_rect_before_wrap().shrink(16.0);
        let (iw, ih) = {
            let p = &self.pages[self.sel];
            (p.processed.width() as f32, p.processed.height() as f32)
        };
        let k = (avail.width() / iw).min(avail.height() / ih);
        let rect = Rect::from_center_size(avail.center(), Vec2::new(iw * k, ih * k));
        let painter = ui.painter();
        painter.rect_filled(rect.translate(Vec2::new(2.0, 3.0)), 0.0, Color32::from_black_alpha(50));
        painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        // Mark placement: drag to move, drag a corner to resize, Enter prints.
        if let Some(pl) = &mut self.placing {
            let tex_key = egui::Id::new(("placing", Arc::as_ptr(&pl.img) as usize));
            let mtex = match ctx.data(|d| d.get_temp::<egui::TextureHandle>(tex_key)) {
                Some(t) => t,
                None => {
                    let t = kui::texture(&ctx, "placing", &pl.img);
                    ctx.data_mut(|d| d.insert_temp(tex_key, t.clone()));
                    t
                }
            };
            let r = Rect::from_min_size(rect.min + Vec2::new(pl.rect[0] * k, pl.rect[1] * k), Vec2::new(pl.rect[2] * k, pl.rect[3] * k));
            painter.image(mtex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, kui::accent(&ctx)), egui::StrokeKind::Outside);
            let corner = r.right_bottom();
            painter.circle_filled(corner, 6.0, kui::accent(&ctx));
            let resp = ui.interact(rect, egui::Id::new("place"), Sense::drag());
            if resp.drag_started() {
                if let Some(p) = resp.interact_pointer_pos() {
                    pl.drag = Some((if p.distance(corner) < 12.0 { 1 } else { 0 }, p));
                }
            }
            if let (Some((mode, last)), Some(p)) = (pl.drag, resp.interact_pointer_pos()) {
                let d = (p - last) / k;
                if mode == 0 {
                    pl.rect[0] += d.x;
                    pl.rect[1] += d.y;
                } else {
                    let ratio = pl.rect[3] / pl.rect[2].max(1.0);
                    pl.rect[2] = (pl.rect[2] + d.x).max(20.0);
                    pl.rect[3] = pl.rect[2] * ratio;
                }
                pl.drag = Some((mode, p));
            }
            if resp.drag_stopped() {
                pl.drag = None;
            }
            let bar = Rect::from_min_size(rect.center_bottom() + Vec2::new(-170.0, 10.0), Vec2::new(340.0, 34.0));
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(bar), |ui| {
                ui.horizontal(|ui| {
                    if ui.button(t("Print into Page", "Впечатать в страницу")).clicked() {
                        self.print_mark();
                    }
                    if ui.button(t("Cancel", "Отмена")).clicked() {
                        self.placing = None;
                    }
                });
            });
        }
    }

    fn print_mark(&mut self) {
        let Some(pl) = self.placing.take() else { return };
        self.checkpoint(t("Print Mark", "Впечатывание"));
        let Some(p) = self.pages.get_mut(self.sel) else { return };
        let bw = p.opts.color == ColorMode::BlackWhite;
        let mut img = (*p.processed).clone();
        process::print_mark(&mut img, &pl.img, pl.rect[0] as i64, pl.rect[1] as i64, pl.rect[2] as u32, pl.rect[3] as u32, bw);
        // The original gets the mark too (mapped through the processing crop approximately: printed on the processed page).
        let arc = Arc::new(img);
        p.processed = arc.clone();
        p.original = arc;
        p.opts = Opts { crop: false, deskew: false, cleanup: false, enhance: false, color: p.opts.color, rotation: 0 };
        p.ocr = None;
    }

    fn export_text(&mut self) {
        let text = self.all_text();
        if text.trim().is_empty() {
            kui::info(APP, t("No text yet. Use Text › Recognize Text first.", "Текста пока нет. Сначала выполните «Текст › Распознать текст»."));
            return;
        }
        if let Some(p) = rfd::FileDialog::new().add_filter(t("Text", "Текст"), &["txt"]).add_filter("RTF", &["rtf"]).set_file_name("Scan.txt").save_file() {
            let bytes = if kit42::ext(&p) == "rtf" { export::text_rtf(&text).into_bytes() } else { text.into_bytes() };
            if let Err(e) = kit42::write_atomic(&p, &bytes) {
                kui::error(APP, &e.to_string());
            }
        }
    }

    fn save_dialog(&mut self, ctx: &Context) {
        let Some(mut d) = self.save.take() else { return };
        let mut open = true;
        let mut done = false;
        egui::Window::new(t("Save Scan", "Сохранить скан")).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            egui::Grid::new("save").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label(t("Name", "Имя"));
                ui.add(egui::TextEdit::singleline(&mut d.template).desired_width(280.0));
                ui.end_row();
                ui.label("");
                ui.label(egui::RichText::new(format!("→ {}  ({{n}}, {{title}}, YYYY MM DD HH mm)", export::expand_name(&d.template, 1, "Title"))).small().weak());
                ui.end_row();
                ui.label(t("Folder", "Папка"));
                ui.horizontal(|ui| {
                    ui.label(d.folder.display().to_string());
                    if ui.small_button(t("Change…", "Изменить…")).clicked() {
                        if let Some(f) = rfd::FileDialog::new().set_directory(&d.folder).pick_folder() {
                            d.folder = f;
                        }
                    }
                });
                ui.end_row();
                ui.label(t("Format", "Формат"));
                ui.horizontal(|ui| {
                    for f in export::Format::ALL {
                        ui.selectable_value(&mut d.format, f, f.name());
                    }
                });
                ui.end_row();
                ui.label(t("Size", "Размер"));
                egui::ComboBox::from_id_salt("size").selected_text(d.size.name()).show_ui(ui, |ui| {
                    for s in export::Size::ALL {
                        ui.selectable_value(&mut d.size, s, s.name());
                    }
                });
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut d.one_per_page, t("One file per page", "Каждая страница отдельным файлом"));
                ui.end_row();
                ui.label("");
                ui.add_enabled(matches!(d.format, export::Format::Pdf | export::Format::PdfA), egui::Checkbox::new(&mut d.ocr, t("Searchable text (OCR)", "Текст для поиска (OCR)")));
                ui.end_row();
                ui.label(t("Then", "Затем"));
                egui::ComboBox::from_id_salt("after")
                    .selected_text([t("Nothing", "Ничего"), t("Open in PDF42 / IMG42", "Открыть в PDF42 / IMG42"), t("Show in Explorer", "Показать в проводнике"), t("Open in default app", "Открыть в программе по умолчанию")][d.after])
                    .show_ui(ui, |ui| {
                        for (i, l) in [t("Nothing", "Ничего"), t("Open in PDF42 / IMG42", "Открыть в PDF42 / IMG42"), t("Show in Explorer", "Показать в проводнике"), t("Open in default app", "Открыть в программе по умолчанию")].iter().enumerate() {
                            ui.selectable_value(&mut d.after, i, *l);
                        }
                    });
                ui.end_row();
            });
            ui.separator();
            if ui.button(format!("{}  {}", ic::SAVE, t("Save", "Сохранить"))).clicked() {
                done = self.do_save(&d);
            }
        });
        if open && !done {
            self.save = Some(d);
        }
    }

    fn history_window(&mut self, ctx: &Context) {
        if !self.show_history {
            return;
        }
        let mut open = true;
        let list = self.history_list();
        let mut remove = None;
        egui::Window::new(t("Scan History", "История сканирования")).open(&mut open).default_size([460.0, 400.0]).show(ctx, |ui| {
            if list.is_empty() {
                ui.label(t("Saved scans appear here.", "Здесь появятся сохранённые сканы."));
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, h) in list.iter().enumerate() {
                    let exists = h.path.exists();
                    let r = ui.add_enabled(exists, egui::Button::new(format!("{}  ·  {}  ·  {} {}", kit42::file_name(&h.path), h.date, h.pages, t("p.", "стр."))).frame(false));
                    if r.double_clicked() || r.clicked() && ui.input(|i| i.modifiers.command) {
                        kit42::shell::open_path(&h.path);
                    }
                    r.context_menu(|ui| {
                        if ui.button(t("Open", "Открыть")).clicked() {
                            kit42::shell::open_path(&h.path);
                            ui.close_menu();
                        }
                        if ui.button(t("Show in Explorer", "Показать в проводнике")).clicked() {
                            kit42::shell::reveal(&h.path);
                            ui.close_menu();
                        }
                        if ui.button(t("Remove from History", "Убрать из истории")).clicked() {
                            remove = Some(i);
                            ui.close_menu();
                        }
                    });
                }
            });
            if !list.is_empty() && ui.button(t("Clear History", "Очистить историю")).clicked() {
                settings::set::<Vec<HistoryEntry>>("scn42.history", vec![]);
            }
        });
        if let Some(i) = remove {
            let mut l = list.clone();
            l.remove(i);
            settings::set("scn42.history", l);
        }
        self.show_history = open;
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }
    kit42::run(kit42::AppSpec { id: "scn42", title: APP, size: [1240.0, 820.0] }, App::new)
}
