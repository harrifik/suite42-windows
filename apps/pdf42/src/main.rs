#![windows_subsystem = "windows"]
//! PDF42 for Windows — reading, editing, marking up and organising PDF documents.

mod cli;
mod crypt;
mod dialogs;
mod markup;
mod pageimg;
mod pdfops;
mod textlayer;
mod view;

use egui::{Context, Key};
use kit42::history::History;
use kit42::icons as ic;
use kit42::ui::{self as kui, CTRL, CTRL_ALT, CTRL_SHIFT, NONE};
use kit42::winrt::PdfRenderer;
use kit42::{t, settings};
use markup::{Kind, Mark, Tool};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

pub const APP: &str = "PDF42";

/// Finds the editable lines of a page: its text layer, or OCR when the page is a picture.
fn recognize_page(bytes: Arc<Vec<u8>>, password: Option<String>, page: usize, size: (f32, f32)) -> Result<Vec<textlayer::Line>, String> {
    let d = pdfops::load(&bytes, password.as_deref())?;
    let pid = *pdfops::page_ids(&d).get(page).ok_or("no page")?;
    let mut lines = textlayer::page_lines(&d, pid);
    let visible = lines.iter().any(|l| !l.hidden);
    let px_w = (size.0 / 72.0 * if visible { 150.0 } else { 300.0 }).round() as u32;
    let img = kit42::winrt::pdf_render_page((*bytes).clone(), page as u32, px_w, password)?;
    let k = img.width() as f32 / size.0;
    if !visible {
        // A scan: recognise the pixels (an existing invisible OCR layer is replaced by fresh OCR).
        if let Ok(res) = kit42::winrt::ocr(&img, None) {
            let ocr = textlayer::ocr_lines(&res, 1.0 / k);
            if !ocr.is_empty() {
                lines = ocr;
            }
        }
    }
    textlayer::sample_colors(&mut lines, &img, k);
    Ok(lines)
}

#[derive(Clone)]
pub struct Snap {
    pub bytes: Arc<Vec<u8>>,
    pub marks: Vec<Mark>,
}

pub struct Doc {
    pub path: Option<PathBuf>,
    pub bytes: Arc<Vec<u8>>,
    pub password: Option<String>,
    pub marks: Vec<Mark>,
    pub history: History<Snap>,
    pub sizes: Vec<(f32, f32)>,
    pub renderer: Option<PdfRenderer>,
    pub tex: HashMap<usize, (u32, egui::TextureHandle)>,
    pub pending: HashMap<usize, (u32, Receiver<Result<image::RgbaImage, String>>)>,
    pub thumbs: HashMap<usize, egui::TextureHandle>,
    pub thumb_pending: HashMap<usize, Receiver<Result<image::RgbaImage, String>>>,
    /// Editable text lines per page (text layer, else OCR) and their background jobs.
    pub lines: HashMap<usize, Vec<textlayer::Line>>,
    pub line_jobs: HashMap<usize, Receiver<Result<Vec<textlayer::Line>, String>>>,
    /// Images painted on each page, and the parsed document they were read from.
    pub images: HashMap<usize, Vec<textlayer::PageImage>>,
    pub parsed: Option<Arc<lopdf::Document>>,
    pub outline: Vec<pdfops::Bookmark>,
    pub fields: Vec<pdfops::Field>,
    pub attachments: Vec<pdfops::Attachment>,
    pub info: pdfops::Info,
    pub texts: Option<Vec<String>>,
    pub generation: u64,
}

impl Doc {
    pub fn name(&self) -> String {
        self.path.as_ref().map(|p| kit42::file_name(p)).unwrap_or_else(|| t("Untitled.pdf", "Без имени.pdf").into())
    }

    pub fn dirty(&self) -> bool {
        self.history.is_dirty() || self.path.is_none()
    }

    pub fn page_count(&self) -> usize {
        self.sizes.len()
    }

    pub fn snap(&self) -> Snap {
        Snap { bytes: self.bytes.clone(), marks: self.marks.clone() }
    }

    /// Records the current state for Undo before a change.
    pub fn checkpoint(&mut self, label: &str) {
        let s = self.snap();
        self.history.push(s, label);
    }

    pub fn lopdf(&self) -> Result<lopdf::Document, String> {
        pdfops::load(&self.bytes, None)
    }

    /// Rebuilds the renderer and the cached document structure after the bytes changed.
    pub fn reload(&mut self) -> Result<(), String> {
        let (r, sizes) = PdfRenderer::open((*self.bytes).clone(), self.password.clone())?;
        self.renderer = Some(r);
        self.sizes = sizes;
        self.tex.clear();
        self.pending.clear();
        self.thumbs.clear();
        self.thumb_pending.clear();
        self.lines.clear();
        self.line_jobs.clear();
        self.images.clear();
        self.parsed = None;
        self.texts = None;
        self.generation += 1;
        if let Ok(d) = self.lopdf() {
            self.outline = pdfops::read_outline(&d);
            self.fields = pdfops::read_fields(&d);
            self.attachments = pdfops::read_attachments(&d);
            self.info = pdfops::read_info(&d);
        }
        Ok(())
    }

    /// Replaces the document bytes with Undo.
    pub fn set_bytes(&mut self, bytes: Vec<u8>, label: &str) -> Result<(), String> {
        self.checkpoint(label);
        self.bytes = Arc::new(bytes);
        self.reload()
    }

    /// Runs a structural edit on the lopdf document and stores the result with Undo.
    pub fn edit(&mut self, label: &str, f: impl FnOnce(&mut lopdf::Document) -> Result<(), String>) -> Result<(), String> {
        let mut d = self.lopdf()?;
        f(&mut d)?;
        let bytes = pdfops::save(&mut d)?;
        self.set_bytes(bytes, label)
    }

    pub fn restore(&mut self, s: Snap) {
        let reload = !Arc::ptr_eq(&s.bytes, &self.bytes);
        self.bytes = s.bytes;
        self.marks = s.marks;
        if reload {
            let _ = self.reload();
        }
    }

    /// Text lines of a page for editing; starts recognition in the background on first use.
    pub fn page_lines(&mut self, page: usize) -> Option<&Vec<textlayer::Line>> {
        if let Some(rx) = self.line_jobs.get(&page) {
            match rx.try_recv() {
                Ok(res) => {
                    self.line_jobs.remove(&page);
                    self.lines.insert(page, res.unwrap_or_default());
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.line_jobs.remove(&page);
                    self.lines.insert(page, vec![]);
                }
                Err(_) => {}
            }
        } else if !self.lines.contains_key(&page) {
            let (tx, rx) = std::sync::mpsc::channel();
            let bytes = self.bytes.clone();
            let password = self.password.clone();
            let size = self.sizes.get(page).copied().unwrap_or((612.0, 792.0));
            std::thread::Builder::new()
                .stack_size(16 << 20)
                .spawn(move || {
                    let _ = tx.send(recognize_page(bytes, password, page, size));
                })
                .ok();
            self.line_jobs.insert(page, rx);
        }
        self.lines.get(&page)
    }

    /// The parsed document, kept until the bytes change.
    pub fn parsed(&mut self) -> Option<Arc<lopdf::Document>> {
        if self.parsed.is_none() {
            self.parsed = self.lopdf().ok().map(Arc::new);
        }
        self.parsed.clone()
    }

    /// Images painted on a page (topmost last).
    pub fn page_images(&mut self, page: usize) -> Vec<textlayer::PageImage> {
        if !self.images.contains_key(&page) {
            let v = self
                .parsed()
                .and_then(|d| pdfops::page_ids(&d).get(page).map(|&pid| textlayer::page_images(&d, pid)))
                .unwrap_or_default();
            self.images.insert(page, v);
        }
        self.images[&page].clone()
    }

    pub fn texts(&mut self) -> &Vec<String> {
        if self.texts.is_none() {
            let v = self.lopdf().map(|d| pdfops::all_text(&d)).unwrap_or_default();
            self.texts = Some(v);
        }
        self.texts.as_ref().unwrap()
    }

    /// Produces the file to save: marks written as annotations / page content.
    pub fn output(&self) -> Result<(Vec<u8>, Vec<String>), String> {
        let mut d = self.lopdf()?;
        let notes = if self.marks.is_empty() {
            vec![]
        } else {
            let renderer = self.renderer.clone();
            let render = move |page: usize, w: u32| -> Result<image::RgbaImage, String> {
                renderer.as_ref().ok_or("no renderer")?.render(page as u32, w)
            };
            let author = std::env::var("USERNAME").unwrap_or_default();
            markup::apply(&mut d, &self.marks, &render, &author)?
        };
        Ok((pdfops::save(&mut d)?, notes))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SideTab {
    Thumbnails,
    Bookmarks,
    Attachments,
    Fields,
    Search,
    Marks,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    None,
    Width,
    Page,
}

#[derive(Clone, Debug)]
pub enum Action {
    Open,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    SaveFlattened,
    Print,
    Close,
    Undo,
    Redo,
    ZoomIn,
    ZoomOut,
    ZoomActual,
    FitWidth,
    FitPage,
    Rotate(i32),
    DeletePages,
    InsertBlank,
    InsertFile,
    ExtractPages,
    DuplicatePages,
    MovePages(i32),
    NewFromImages,
    GoTo(usize),
    DeleteMark,
    ApplyRedactions,
    Ocr,
    AddBookmark,
    RemoveAnnotations,
    Signatures,
    Dialog(dialogs::DialogKind),
    About,
    ExportText(dialogs::TextFormat),
    AutoCrop,
    FlattenNow,
}

pub enum Drag {
    Create { page: usize, start: [f32; 2], cur: [f32; 2], points: Vec<[f32; 2]> },
    Move { id: u64, last: [f32; 2] },
    Resize { id: u64, handle: usize, last: [f32; 2] },
    Thumb { from: usize },
    /// Moving (handle None) or resizing an image of the page content.
    Image { handle: Option<usize>, last: [f32; 2] },
}

/// A selected image of the page content and its box while it is being dragged.
pub struct ImgSel {
    pub page: usize,
    pub img: textlayer::PageImage,
    pub rect: [f32; 4],
    pub generation: u64,
}

pub struct Job {
    pub label: String,
    pub progress: Arc<std::sync::atomic::AtomicU32>,
    pub rx: Receiver<Result<Vec<u8>, String>>,
    pub undo_label: String,
}

pub struct App {
    pub doc: Option<Doc>,
    pub zoom: f32,
    pub fit: Fit,
    pub tool: Tool,
    pub color: [u8; 3],
    pub fill: Option<[u8; 3]>,
    pub width: f32,
    pub font_size: f32,
    pub selected: Option<u64>,
    pub img_sel: Option<ImgSel>,
    pub pages_sel: BTreeSet<usize>,
    pub sidebar: Option<SideTab>,
    pub scroll_to: Option<usize>,
    pub current: usize,
    pub drag: Option<Drag>,
    pub editing: Option<u64>,
    pub picker: kit42::signature::MarkPicker,
    pub placing: Option<Arc<image::RgbaImage>>,
    pub mark_tex: HashMap<u64, egui::TextureHandle>,
    pub dialog: Option<dialogs::Dialog>,
    pub search: String,
    pub results: Vec<(usize, String)>,
    pub status: String,
    pub job: Option<Job>,
    pub actions: Vec<Action>,
    pub page_input: String,
    pub crop_rect: Option<(usize, [f32; 4])>,
    pub continuous: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self {
            doc: None,
            zoom: 1.0,
            fit: Fit::Width,
            tool: Tool::Select,
            color: [255, 230, 0],
            fill: None,
            width: 2.0,
            font_size: 12.0,
            selected: None,
            img_sel: None,
            pages_sel: BTreeSet::new(),
            sidebar: Some(SideTab::Thumbnails),
            scroll_to: None,
            current: 0,
            drag: None,
            editing: None,
            picker: Default::default(),
            placing: None,
            mark_tex: HashMap::new(),
            dialog: None,
            search: String::new(),
            results: vec![],
            status: String::new(),
            job: None,
            actions: vec![],
            page_input: String::new(),
            crop_rect: None,
            continuous: true,
        };
        let _ = cc;
        for f in kit42::startup_files() {
            app.actions.push(Action::OpenPath(f));
        }
        // Start with a tool selected (snapshots and automation), e.g. K42_TOOL=edit-text.
        if std::env::var("K42_TOOL").as_deref() == Ok("edit-text") {
            app.tool = Tool::EditText;
        }
        app
    }

    pub fn act(&mut self, a: Action) {
        self.actions.push(a);
    }

    pub fn open_bytes(&mut self, path: Option<PathBuf>, bytes: Vec<u8>, password: Option<String>) -> Result<(), String> {
        // Decrypt with lopdf so every page operation works on plain objects.
        let plain = match pdfops::load(&bytes, password.as_deref()) {
            Ok(mut d) => {
                if Document_is_plain(&bytes) {
                    bytes.clone()
                } else {
                    pdfops::save(&mut d)?
                }
            }
            Err(e) if e == "password" => return Err(e),
            Err(e) => return Err(e),
        };
        let mut doc = Doc {
            path: path.clone(),
            bytes: Arc::new(plain),
            password: None,
            marks: vec![],
            history: History::new(60),
            sizes: vec![],
            renderer: None,
            tex: HashMap::new(),
            pending: HashMap::new(),
            thumbs: HashMap::new(),
            thumb_pending: HashMap::new(),
            lines: HashMap::new(),
            line_jobs: HashMap::new(),
            images: HashMap::new(),
            parsed: None,
            outline: vec![],
            fields: vec![],
            attachments: vec![],
            info: Default::default(),
            texts: None,
            generation: 0,
        };
        doc.reload()?;
        if let Some(p) = &path {
            settings::add_recent("pdf42", p);
        }
        self.doc = Some(doc);
        self.selected = None;
        self.pages_sel.clear();
        self.current = 0;
        self.scroll_to = Some(0);
        self.fit = Fit::Width;
        self.results.clear();
        self.mark_tex.clear();
        if !self.doc.as_ref().unwrap().outline.is_empty() {
            self.sidebar = Some(SideTab::Bookmarks);
        }
        Ok(())
    }

    pub fn open_path(&mut self, path: PathBuf) {
        // A second document opens in its own window.
        if self.doc.is_some() {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).arg(&path).spawn();
            }
            return;
        }
        let ext = kit42::ext(&path);
        let res = if ["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "webp", "heic", "heif", "avif"].contains(&ext.as_str()) {
            images_to_pdf(&[path.clone()]).and_then(|b| self.open_bytes(None, b, None))
        } else {
            match kit42::ooxml::read_file(&path) {
                Ok(b) => match self.open_bytes(Some(path.clone()), b.clone(), None) {
                    Err(e) if e == "password" => {
                        self.dialog = Some(dialogs::Dialog::password_for(path.clone(), b));
                        Ok(())
                    }
                    r => r,
                },
                Err(e) => Err(e),
            }
        };
        kui::remember_dir(&path);
        if let Err(e) = res {
            kui::error(APP, &format!("{}\n{}", t("The file could not be opened.", "Не удалось открыть файл."), e));
        }
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        let Some(doc) = &mut self.doc else { return false };
        match doc.output() {
            Ok((bytes, notes)) => {
                if let Err(e) = kit42::write_atomic(&path, &bytes) {
                    kui::error(APP, &e.to_string());
                    return false;
                }
                doc.path = Some(path.clone());
                doc.bytes = Arc::new(bytes);
                doc.marks.clear();
                let _ = doc.reload();
                doc.history.mark_saved();
                settings::add_recent("pdf42", &path);
                self.selected = None;
                self.mark_tex.clear();
                self.status = format!("{} {}", t("Saved", "Сохранено:"), kit42::file_name(&path));
                if !notes.is_empty() {
                    self.status.push_str(&format!(" · {}", notes.join(", ")));
                }
                true
            }
            Err(e) => {
                kui::error(APP, &format!("{}\n{}", t("Saving failed; the file was not changed.", "Не удалось сохранить; файл не изменён."), e));
                false
            }
        }
    }

    pub fn save(&mut self) -> bool {
        let path = self.doc.as_ref().and_then(|d| d.path.clone());
        match path {
            Some(p) => self.save_to(p),
            None => self.save_as(),
        }
    }

    pub fn save_as(&mut self) -> bool {
        let name = self.doc.as_ref().map(|d| d.name()).unwrap_or_default();
        let Some(p) = kui::open_dialog(t("Save As", "Сохранить как"), &[("PDF", &["pdf"])]).set_file_name(&name).save_file() else {
            return false;
        };
        kui::remember_dir(&p);
        let p = if kit42::ext(&p) == "pdf" { p } else { p.with_extension("pdf") };
        self.save_to(p)
    }

    pub fn spawn_job(&mut self, label: &str, undo_label: &str, f: impl FnOnce(Arc<std::sync::atomic::AtomicU32>) -> Result<Vec<u8>, String> + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        let progress = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let p2 = progress.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f(p2));
        });
        self.job = Some(Job { label: label.into(), progress, rx, undo_label: undo_label.into() });
    }

    fn poll_job(&mut self, ctx: &Context) {
        let Some(job) = &self.job else { return };
        match job.rx.try_recv() {
            Ok(res) => {
                let label = job.undo_label.clone();
                self.job = None;
                match res {
                    Ok(_) if label.is_empty() => self.status = t("Done.", "Готово.").into(),
                    Ok(bytes) => {
                        if let Some(doc) = &mut self.doc {
                            if let Err(e) = doc.set_bytes(bytes, &label) {
                                kui::error(APP, &e);
                            }
                        }
                    }
                    Err(e) => kui::error(APP, &e),
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                egui::Modal::new(egui::Id::new("job")).show(ctx, |ui| {
                    ui.set_width(320.0);
                    ui.heading(&job.label);
                    let p = job.progress.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
                    ui.add(egui::ProgressBar::new(p).show_percentage().animate(true));
                });
            }
            Err(_) => self.job = None,
        }
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let pairs: [(egui::Modifiers, Key, Action); 18] = [
            (CTRL, Key::O, Action::Open),
            (CTRL, Key::S, Action::Save),
            (CTRL_SHIFT, Key::S, Action::SaveAs),
            (CTRL, Key::P, Action::Print),
            (CTRL, Key::Z, Action::Undo),
            (CTRL, Key::Y, Action::Redo),
            (CTRL_SHIFT, Key::Z, Action::Redo),
            (CTRL, Key::Plus, Action::ZoomIn),
            (CTRL, Key::Equals, Action::ZoomIn),
            (CTRL, Key::Minus, Action::ZoomOut),
            (CTRL, Key::Num0, Action::ZoomActual),
            (CTRL, Key::R, Action::Rotate(90)),
            (CTRL, Key::L, Action::Rotate(-90)),
            (NONE, Key::Delete, Action::DeleteMark),
            (CTRL, Key::D, Action::AddBookmark),
            (CTRL_ALT, Key::M, Action::Dialog(dialogs::DialogKind::Merge)),
            (CTRL_SHIFT, Key::K, Action::Signatures),
            (CTRL_ALT, Key::T, Action::Ocr),
        ];
        for (m, k, a) in pairs {
            if kui::shortcut(ctx, m, k) {
                self.act(a);
            }
        }
        if kui::shortcut(ctx, CTRL, Key::E) && self.doc.is_some() {
            self.set_tool(Tool::EditText);
        }
        if kui::shortcut(ctx, CTRL, Key::F) {
            self.sidebar = Some(SideTab::Search);
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("searchbox")));
        }
        if kui::shortcut(ctx, CTRL, Key::G) {
            self.act(Action::Dialog(dialogs::DialogKind::GoTo));
        }
        if kui::shortcut(ctx, NONE, Key::Escape) {
            self.img_sel = None;
            self.tool = Tool::Select;
            self.drag = None;
            self.placing = None;
            self.selected = None;
            self.crop_rect = None;
        }
        let n = self.doc.as_ref().map(|d| d.page_count()).unwrap_or(0);
        if n > 0 && self.selected.is_none() {
            if kui::shortcut(ctx, NONE, Key::PageDown) || kui::shortcut(ctx, NONE, Key::ArrowRight) {
                self.act(Action::GoTo((self.current + 1).min(n - 1)));
            }
            if kui::shortcut(ctx, NONE, Key::PageUp) || kui::shortcut(ctx, NONE, Key::ArrowLeft) {
                self.act(Action::GoTo(self.current.saturating_sub(1)));
            }
            if kui::shortcut(ctx, NONE, Key::Home) {
                self.act(Action::GoTo(0));
            }
            if kui::shortcut(ctx, NONE, Key::End) {
                self.act(Action::GoTo(n - 1));
            }
        }
        // Nudge the selected mark with arrows.
        if let (Some(id), Some(doc)) = (self.selected, &mut self.doc) {
            let (dx, dy) = ctx.input(|i| {
                let s = if i.modifiers.shift { 10.0 } else { 1.0 };
                (
                    (i.key_pressed(Key::ArrowRight) as i32 - i.key_pressed(Key::ArrowLeft) as i32) as f32 * s,
                    (i.key_pressed(Key::ArrowDown) as i32 - i.key_pressed(Key::ArrowUp) as i32) as f32 * s,
                )
            });
            if dx != 0.0 || dy != 0.0 {
                doc.checkpoint(t("Move", "Перемещение"));
                if let Some(m) = doc.marks.iter_mut().find(|m| m.id == id) {
                    m.translate(dx, dy);
                }
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let has = self.doc.is_some();
        let can_undo = self.doc.as_ref().map(|d| d.history.can_undo()).unwrap_or(false);
        let can_redo = self.doc.as_ref().map(|d| d.history.can_redo()).unwrap_or(false);
        egui::menu::bar(ui, |ui| {
            ui.menu_button(t("File", "Файл"), |ui| {
                if kui::menu_item(ui, t("Open…", "Открыть…"), "Ctrl+O") {
                    self.act(Action::Open);
                }
                ui.menu_button(t("Open Recent", "Последние файлы"), |ui| {
                    let recent = settings::recent("pdf42");
                    if recent.is_empty() {
                        ui.label(t("No recent files", "Нет последних файлов"));
                    }
                    for p in recent {
                        if ui.button(kit42::file_name(&p)).on_hover_text(p.display().to_string()).clicked() {
                            self.act(Action::OpenPath(p));
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button(t("Clear Menu", "Очистить список")).clicked() {
                        settings::clear_recent("pdf42");
                        ui.close_menu();
                    }
                });
                if kui::menu_item(ui, t("New PDF from Images…", "Новый PDF из изображений…"), "") {
                    self.act(Action::NewFromImages);
                }
                if kui::menu_item(ui, t("Merge Files…", "Объединить файлы…"), "Ctrl+Alt+M") {
                    self.act(Action::Dialog(dialogs::DialogKind::Merge));
                }
                ui.separator();
                if kui::menu_item_enabled(ui, has, t("Save", "Сохранить"), "Ctrl+S") {
                    self.act(Action::Save);
                }
                if kui::menu_item_enabled(ui, has, t("Save As…", "Сохранить как…"), "Ctrl+Shift+S") {
                    self.act(Action::SaveAs);
                }
                if kui::menu_item_enabled(ui, has, t("Save Flattened Copy…", "Сохранить копию без слоёв…"), "") {
                    self.act(Action::SaveFlattened);
                }
                if kui::menu_item_enabled(ui, has, t("Save Copy with Password…", "Сохранить копию с паролем…"), "") {
                    self.act(Action::Dialog(dialogs::DialogKind::Protect));
                }
                if kui::menu_item_enabled(ui, has, t("Reduce File Size…", "Уменьшить размер файла…"), "") {
                    self.act(Action::Dialog(dialogs::DialogKind::Compress));
                }
                ui.menu_button(t("Export", "Экспорт"), |ui| {
                    if kui::menu_item_enabled(ui, has, t("Pages as Images (PNG/JPEG)…", "Страницы как изображения (PNG/JPEG)…"), "") {
                        self.act(Action::Dialog(dialogs::DialogKind::ExportImages));
                    }
                    for (f, label) in [
                        (dialogs::TextFormat::Docx, t("Word Document (.docx)…", "Документ Word (.docx)…")),
                        (dialogs::TextFormat::Rtf, t("Rich Text (.rtf)…", "RTF (.rtf)…")),
                        (dialogs::TextFormat::Html, t("Web Page (.html)…", "Веб-страница (.html)…")),
                        (dialogs::TextFormat::Txt, t("Plain Text (.txt)…", "Текст (.txt)…")),
                    ] {
                        if kui::menu_item_enabled(ui, has, label, "") {
                            self.act(Action::ExportText(f));
                        }
                    }
                });
                if kui::menu_item_enabled(ui, has, t("Split…", "Разделить…"), "") {
                    self.act(Action::Dialog(dialogs::DialogKind::Split));
                }
                if kui::menu_item_enabled(ui, has, t("Compare With…", "Сравнить с…"), "") {
                    self.act(Action::Dialog(dialogs::DialogKind::Compare));
                }
                if kui::menu_item_enabled(ui, has, t("Properties…", "Свойства…"), "Ctrl+Alt+I") {
                    self.act(Action::Dialog(dialogs::DialogKind::Properties));
                }
                ui.separator();
                if kui::menu_item_enabled(ui, has, t("Print…", "Печать…"), "Ctrl+P") {
                    self.act(Action::Print);
                }
                ui.separator();
                if kui::menu_item_enabled(ui, has, t("Close", "Закрыть"), "") {
                    self.act(Action::Close);
                }
                if kui::menu_item(ui, t("Exit", "Выход"), "Alt+F4") {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button(t("Edit", "Правка"), |ui| {
                if kui::menu_item_enabled(ui, can_undo, t("Undo", "Отменить"), "Ctrl+Z") {
                    self.act(Action::Undo);
                }
                if kui::menu_item_enabled(ui, can_redo, t("Redo", "Повторить"), "Ctrl+Y") {
                    self.act(Action::Redo);
                }
                ui.separator();
                if kui::menu_item_enabled(ui, self.selected.is_some(), t("Delete Markup", "Удалить пометку"), "Del") {
                    self.act(Action::DeleteMark);
                }
                if kui::menu_item_enabled(ui, has, t("Find…", "Найти…"), "Ctrl+F") {
                    self.sidebar = Some(SideTab::Search);
                }
                if kui::menu_item_enabled(ui, has, t("Go to Page…", "Перейти к странице…"), "Ctrl+G") {
                    self.act(Action::Dialog(dialogs::DialogKind::GoTo));
                }
                if kui::menu_item_enabled(ui, has, t("Edit Page Text", "Править текст страницы"), "Ctrl+E") {
                    self.set_tool(Tool::EditText);
                }
            });
            ui.menu_button(t("View", "Вид"), |ui| {
                if kui::menu_item(ui, t("Zoom In", "Увеличить"), "Ctrl++") {
                    self.act(Action::ZoomIn);
                }
                if kui::menu_item(ui, t("Zoom Out", "Уменьшить"), "Ctrl+-") {
                    self.act(Action::ZoomOut);
                }
                if kui::menu_item(ui, t("Actual Size", "Реальный размер"), "Ctrl+0") {
                    self.act(Action::ZoomActual);
                }
                if kui::menu_item(ui, t("Fit Width", "По ширине"), "") {
                    self.act(Action::FitWidth);
                }
                if kui::menu_item(ui, t("Whole Page", "Страница целиком"), "") {
                    self.act(Action::FitPage);
                }
                ui.checkbox(&mut self.continuous, t("Continuous Scroll", "Непрерывная прокрутка"));
                ui.separator();
                for (tab, label) in [
                    (SideTab::Thumbnails, t("Thumbnails", "Миниатюры")),
                    (SideTab::Bookmarks, t("Bookmarks", "Закладки")),
                    (SideTab::Attachments, t("Attachments", "Вложения")),
                    (SideTab::Fields, t("Form Fields", "Поля формы")),
                    (SideTab::Marks, t("Markup List", "Список пометок")),
                    (SideTab::Search, t("Search", "Поиск")),
                ] {
                    if ui.radio(self.sidebar == Some(tab), label).clicked() {
                        self.sidebar = Some(tab);
                        ui.close_menu();
                    }
                }
                if ui.radio(self.sidebar.is_none(), t("Hide Sidebar", "Скрыть боковую панель")).clicked() {
                    self.sidebar = None;
                    ui.close_menu();
                }
                ui.separator();
                if kui::menu_item(ui, t("Full Screen", "Во весь экран"), "F11") {
                    let fs = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
                }
            });
            ui.menu_button(t("Page", "Страница"), |ui| {
                ui.add_enabled_ui(has, |ui| {
                    if kui::menu_item(ui, t("Rotate Left", "Повернуть влево"), "Ctrl+L") {
                        self.act(Action::Rotate(-90));
                    }
                    if kui::menu_item(ui, t("Rotate Right", "Повернуть вправо"), "Ctrl+R") {
                        self.act(Action::Rotate(90));
                    }
                    ui.separator();
                    if kui::menu_item(ui, t("Insert Blank Page", "Вставить пустую страницу"), "") {
                        self.act(Action::InsertBlank);
                    }
                    if kui::menu_item(ui, t("Insert from File…", "Вставить из файла…"), "") {
                        self.act(Action::InsertFile);
                    }
                    if kui::menu_item(ui, t("Duplicate Page", "Дублировать страницу"), "") {
                        self.act(Action::DuplicatePages);
                    }
                    if kui::menu_item(ui, t("Extract Pages…", "Извлечь страницы…"), "") {
                        self.act(Action::ExtractPages);
                    }
                    if kui::menu_item(ui, t("Delete Page", "Удалить страницу"), "") {
                        self.act(Action::DeletePages);
                    }
                    if kui::menu_item(ui, t("Move Up", "Переместить вверх"), "") {
                        self.act(Action::MovePages(-1));
                    }
                    if kui::menu_item(ui, t("Move Down", "Переместить вниз"), "") {
                        self.act(Action::MovePages(1));
                    }
                    ui.separator();
                    if kui::menu_item(ui, t("Crop…", "Обрезать…"), "Ctrl+Alt+K") {
                        self.act(Action::Dialog(dialogs::DialogKind::Crop));
                    }
                    if kui::menu_item(ui, t("Remove White Margins", "Убрать белые поля"), "") {
                        self.act(Action::AutoCrop);
                    }
                    if kui::menu_item(ui, t("Page Size…", "Размер страницы…"), "") {
                        self.act(Action::Dialog(dialogs::DialogKind::PageSize));
                    }
                    ui.separator();
                    if kui::menu_item(ui, t("Watermark…", "Водяной знак…"), "") {
                        self.act(Action::Dialog(dialogs::DialogKind::Watermark));
                    }
                    if kui::menu_item(ui, t("Page Numbers…", "Номера страниц…"), "") {
                        self.act(Action::Dialog(dialogs::DialogKind::PageNumbers));
                    }
                    if kui::menu_item(ui, t("Header & Footer / Bates…", "Колонтитулы / Бейтс…"), "") {
                        self.act(Action::Dialog(dialogs::DialogKind::HeaderFooter));
                    }
                });
            });
            ui.menu_button(t("Markup", "Пометки"), |ui| {
                for tool in [
                    Tool::Select, Tool::Highlight, Tool::Underline, Tool::Strike, Tool::Pen, Tool::Rect, Tool::Ellipse,
                    Tool::Line, Tool::Arrow, Tool::Text, Tool::EditText, Tool::Note, Tool::Whiteout, Tool::Redact, Tool::Link,
                ] {
                    if ui.radio(self.tool == tool, tool.name()).clicked() {
                        self.set_tool(tool);
                        ui.close_menu();
                    }
                }
                ui.separator();
                if kui::menu_item_enabled(ui, has, t("Signatures and Stamps…", "Подписи и печати…"), "Ctrl+Shift+K") {
                    self.act(Action::Signatures);
                }
                if kui::menu_item_enabled(ui, has, t("Apply Redactions…", "Применить скрытие…"), "") {
                    self.act(Action::ApplyRedactions);
                }
                if kui::menu_item_enabled(ui, has, t("Flatten Markup into Pages", "Впечатать пометки в страницы"), "") {
                    self.act(Action::FlattenNow);
                }
                if kui::menu_item_enabled(ui, has, t("Remove Existing Annotations", "Удалить имеющиеся аннотации"), "") {
                    self.act(Action::RemoveAnnotations);
                }
            });
            ui.menu_button(t("Form", "Форма"), |ui| {
                if ui.radio(self.tool == Tool::FieldText, t("Add Text Field", "Добавить текстовое поле")).clicked() {
                    self.set_tool(Tool::FieldText);
                    ui.close_menu();
                }
                if ui.radio(self.tool == Tool::FieldCheck, t("Add Checkbox", "Добавить флажок")).clicked() {
                    self.set_tool(Tool::FieldCheck);
                    ui.close_menu();
                }
                if kui::menu_item_enabled(ui, has, t("Fill Form…", "Заполнить форму…"), "") {
                    self.sidebar = Some(SideTab::Fields);
                }
            });
            ui.menu_button(t("Tools", "Инструменты"), |ui| {
                if kui::menu_item_enabled(ui, has, t("Recognize Text (OCR)…", "Распознать текст (OCR)…"), "Ctrl+Alt+T") {
                    self.act(Action::Ocr);
                }
                if kui::menu_item_enabled(ui, has, t("Add Bookmark", "Добавить закладку"), "Ctrl+D") {
                    self.act(Action::AddBookmark);
                }
                if kui::menu_item_enabled(ui, has, t("Bookmarks from Headings", "Закладки по заголовкам"), "") {
                    self.act(Action::Dialog(dialogs::DialogKind::BookmarksFromText));
                }
                if kui::menu_item_enabled(ui, has, t("Attach File…", "Вложить файл…"), "") {
                    self.act(Action::Dialog(dialogs::DialogKind::Attach));
                }
            });
            ui.menu_button(t("Help", "Справка"), |ui| {
                kui::language_menu(ui);
                if kui::menu_item(ui, t("About PDF42", "О программе PDF42"), "") {
                    self.act(Action::About);
                }
            });
        });
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.img_sel = None;
        self.tool = tool;
        self.color = tool.default_color();
        self.width = match tool {
            Tool::Pen => 2.0,
            _ => 1.5,
        };
        self.placing = None;
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let has = self.doc.is_some();
        ui.horizontal(|ui| {
            if kui::tool_button(ui, ic::SIDEBAR, t("Sidebar", "Боковая панель"), true).clicked() {
                self.sidebar = if self.sidebar.is_some() { None } else { Some(SideTab::Thumbnails) };
            }
            if kui::tool_button(ui, ic::OPEN, t("Open", "Открыть"), true).clicked() {
                self.act(Action::Open);
            }
            if kui::tool_button(ui, ic::SAVE, t("Save", "Сохранить"), has).clicked() {
                self.act(Action::Save);
            }
            if kui::tool_button(ui, ic::PRINT, t("Print", "Печать"), has).clicked() {
                self.act(Action::Print);
            }
            ui.separator();
            let (cu, cr) = self.doc.as_ref().map(|d| (d.history.can_undo(), d.history.can_redo())).unwrap_or((false, false));
            if kui::tool_button(ui, ic::UNDO, t("Undo", "Отменить"), cu).clicked() {
                self.act(Action::Undo);
            }
            if kui::tool_button(ui, ic::REDO, t("Redo", "Повторить"), cr).clicked() {
                self.act(Action::Redo);
            }
            ui.separator();
            if kui::tool_button(ui, ic::ZOOM_OUT, t("Zoom Out", "Уменьшить"), has).clicked() {
                self.act(Action::ZoomOut);
            }
            ui.add_enabled(has, egui::Label::new(format!("{:.0}%", self.zoom * 100.0)));
            if kui::tool_button(ui, ic::ZOOM_IN, t("Zoom In", "Увеличить"), has).clicked() {
                self.act(Action::ZoomIn);
            }
            if kui::tool_button(ui, ic::FIT, t("Fit Width", "По ширине"), has).clicked() {
                self.act(Action::FitWidth);
            }
            ui.separator();
            let n = self.doc.as_ref().map(|d| d.page_count()).unwrap_or(0);
            if kui::tool_button(ui, ic::UP, t("Previous Page", "Предыдущая страница"), has && self.current > 0).clicked() {
                self.act(Action::GoTo(self.current.saturating_sub(1)));
            }
            let r = ui.add_enabled(has, egui::TextEdit::singleline(&mut self.page_input).desired_width(36.0).horizontal_align(egui::Align::Center));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                if let Ok(p) = self.page_input.trim().parse::<usize>() {
                    self.act(Action::GoTo(p.saturating_sub(1).min(n.saturating_sub(1))));
                }
            }
            if !r.has_focus() {
                self.page_input = if has { (self.current + 1).to_string() } else { String::new() };
            }
            ui.label(format!("/ {n}"));
            if kui::tool_button(ui, ic::DOWN, t("Next Page", "Следующая страница"), has && self.current + 1 < n).clicked() {
                self.act(Action::GoTo(self.current + 1));
            }
            ui.separator();
            if kui::tool_button(ui, "\u{E7AD}", t("Rotate Right", "Повернуть вправо"), has).clicked() {
                self.act(Action::Rotate(90));
            }
            ui.separator();
            for tool in [
                Tool::Select, Tool::Highlight, Tool::Underline, Tool::Strike, Tool::Pen, Tool::Rect, Tool::Ellipse,
                Tool::Arrow, Tool::Text, Tool::EditText, Tool::Note, Tool::Whiteout, Tool::Redact,
            ] {
                if kui::tool_toggle(ui, self.tool == tool, tool.icon(), tool.name()).clicked() {
                    self.set_tool(tool);
                }
            }
            if kui::tool_button(ui, ic::SIGNATURE, t("Signatures and Stamps", "Подписи и печати"), has).clicked() {
                self.act(Action::Signatures);
            }
            ui.separator();
            let mut c = self.color;
            if kui::color_button(ui, &mut c, t("Colour", "Цвет")) {
                self.color = c;
                if let (Some(id), Some(doc)) = (self.selected, &mut self.doc) {
                    doc.checkpoint(t("Colour", "Цвет"));
                    if let Some(m) = doc.marks.iter_mut().find(|m| m.id == id) {
                        m.color = c;
                    }
                }
            }
            ui.menu_button(format!("{:.1}", self.width), |ui| {
                for w in [0.5f32, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0] {
                    if ui.selectable_label(self.width == w, format!("{w} pt")).clicked() {
                        self.width = w;
                        if let (Some(id), Some(doc)) = (self.selected, &mut self.doc) {
                            doc.checkpoint(t("Line Width", "Толщина линии"));
                            if let Some(m) = doc.marks.iter_mut().find(|m| m.id == id) {
                                m.width = w;
                            }
                        }
                        ui.close_menu();
                    }
                }
                ui.separator();
                let mut fill_on = self.fill.is_some();
                if ui.checkbox(&mut fill_on, t("Fill shapes", "Заливка фигур")).changed() {
                    self.fill = if fill_on { Some([255, 255, 255]) } else { None };
                }
                if let Some(f) = &mut self.fill {
                    kui::palette(ui, f);
                }
                ui.horizontal(|ui| {
                    ui.label(t("Text size", "Размер текста"));
                    ui.add(egui::DragValue::new(&mut self.font_size).range(4.0..=144.0));
                });
            })
            .response
            .on_hover_text(t("Line width, fill and text size", "Толщина линии, заливка и размер текста"));
        });
    }
}

#[allow(non_snake_case)]
fn Document_is_plain(bytes: &[u8]) -> bool {
    // Files without /Encrypt are kept byte-for-byte until the first edit.
    !bytes.windows(8).any(|w| w == b"/Encrypt")
}

/// Images become A4 pages (fit) in one new PDF.
pub fn images_to_pdf(paths: &[PathBuf]) -> Result<Vec<u8>, String> {
    let mut doc = lopdf::Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    doc.objects.insert(pages_id, lopdf::dictionary! { "Type" => "Pages", "Kids" => Vec::<lopdf::Object>::new(), "Count" => 0 }.into());
    let catalog = doc.add_object(lopdf::dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    for p in paths {
        let img = load_any_image(p)?;
        let one = pdfops::image_page_pdf(&img, true);
        let other = lopdf::Document::load_mem(&one).map_err(|e| e.to_string())?;
        let n = pdfops::page_ids(&doc).len();
        pdfops::insert_document(&mut doc, other, n)?;
    }
    pdfops::save(&mut doc)
}

pub fn load_any_image(p: &std::path::Path) -> Result<image::RgbaImage, String> {
    let bytes = kit42::ooxml::read_file(p)?;
    match image::load_from_memory(&bytes) {
        Ok(i) => Ok(i.to_rgba8()),
        Err(_) => kit42::winrt::decode_image(bytes),
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let title_doc = self.doc.as_ref().map(|d| (d.name(), d.dirty()));
        kui::set_title(ctx, APP, title_doc.as_ref().map(|(n, _)| n.as_str()), title_doc.as_ref().map(|d| d.1).unwrap_or(false));
        // Files dropped on the window.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            if self.doc.is_some() {
                self.insert_files(dropped, self.current + 1);
            } else {
                for p in dropped {
                    self.act(Action::OpenPath(p));
                }
            }
        }
        if ctx.input(|i| i.key_pressed(Key::F11)) {
            let fs = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
        }
        self.handle_shortcuts(ctx);
        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu_bar(ui));
        egui::TopBottomPanel::top("toolbar")
            .frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(egui::Margin::symmetric(6, 3)))
            .show(ctx, |ui| self.toolbar(ui));
        let status = self.status_text();
        kui::status_bar(ctx, |ui| {
            ui.label(status);
        });
        if self.doc.is_some() {
            if let Some(tab) = self.sidebar {
                egui::SidePanel::left("sidebar").resizable(true).default_width(200.0).show(ctx, |ui| self.sidebar_ui(ui, tab));
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(if ctx.style().visuals.dark_mode { egui::Color32::from_gray(40) } else { egui::Color32::from_gray(225) }))
            .show(ctx, |ui| self.central(ui));
        if let Some(img) = self.picker.show(ctx) {
            self.placing = Some(Arc::new(img));
            self.tool = Tool::Select;
            self.status = t("Click on the page to place the signature or stamp.", "Щёлкните по странице, чтобы поставить подпись или печать.").into();
        }
        self.dialogs_ui(ctx);
        self.poll_job(ctx);
        let actions = std::mem::take(&mut self.actions);
        for a in actions {
            self.run(ctx, a);
        }
        // Unsaved changes on close.
        let (dirty, name) = self.doc.as_ref().map(|d| (d.dirty() && (d.history.can_undo() || !d.marks.is_empty()), d.name())).unwrap_or((false, String::new()));
        if ctx.input(|i| i.viewport().close_requested()) && dirty {
            match kui::ask_save(&name) {
                kui::SaveChoice::Save => {
                    if !self.save() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    }
                }
                kui::SaveChoice::Discard => {}
                kui::SaveChoice::Cancel => ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose),
            }
        }
    }
}

impl App {
    fn status_text(&self) -> String {
        match &self.doc {
            None => t("Open a PDF, drop files here or create one from images.", "Откройте PDF, перетащите файлы сюда или создайте PDF из изображений.").into(),
            Some(d) => {
                let (w, h) = d.sizes.get(self.current).cloned().unwrap_or((0.0, 0.0));
                let mut s = format!(
                    "{} {} / {} · {:.0} × {:.0} mm · {}: {}",
                    t("Page", "Страница"),
                    self.current + 1,
                    d.page_count(),
                    w * 25.4 / 72.0,
                    h * 25.4 / 72.0,
                    t("tool", "инструмент"),
                    self.tool.name()
                );
                if !d.marks.is_empty() {
                    s.push_str(&format!(" · {} {}", d.marks.len(), t("unsaved markups", "несохранённых пометок")));
                }
                if !self.status.is_empty() {
                    s.push_str(" · ");
                    s.push_str(&self.status);
                }
                s
            }
        }
    }

    fn confirm_close(&mut self) -> bool {
        let Some(d) = &self.doc else { return true };
        if !(d.dirty() && (d.history.can_undo() || !d.marks.is_empty())) {
            return true;
        }
        match kui::ask_save(&d.name()) {
            kui::SaveChoice::Save => self.save(),
            kui::SaveChoice::Discard => true,
            kui::SaveChoice::Cancel => false,
        }
    }

    pub fn sel_or_current(&self) -> Vec<usize> {
        if self.pages_sel.is_empty() {
            vec![self.current]
        } else {
            self.pages_sel.iter().cloned().collect()
        }
    }

    pub fn insert_files(&mut self, files: Vec<PathBuf>, at: usize) {
        let Some(doc) = &mut self.doc else { return };
        let res = doc.edit(t("Insert Pages", "Вставка страниц"), |d| {
            let mut pos = at;
            for f in &files {
                let ext = kit42::ext(f);
                let other = if ext == "pdf" {
                    pdfops::load(&kit42::ooxml::read_file(f)?, None)?
                } else {
                    let img = load_any_image(f)?;
                    lopdf::Document::load_mem(&pdfops::image_page_pdf(&img, true)).map_err(|e| e.to_string())?
                };
                pos += pdfops::insert_document(d, other, pos)?;
            }
            Ok(())
        });
        if let Err(e) = res {
            kui::error(APP, &e);
        }
    }

    fn run(&mut self, ctx: &Context, a: Action) {
        match a {
            Action::Open => {
                if let Some(paths) = kui::open_dialog(
                    t("Open", "Открыть"),
                    &[
                        (t("PDF and images", "PDF и изображения"), &["pdf", "png", "jpg", "jpeg", "tif", "tiff", "bmp", "gif", "webp", "heic"]),
                        ("PDF", &["pdf"]),
                    ],
                )
                .pick_files()
                {
                    for p in paths {
                        self.open_path(p);
                    }
                }
            }
            Action::OpenPath(p) => self.open_path(p),
            Action::Save => {
                self.save();
            }
            Action::SaveAs => {
                self.save_as();
            }
            Action::SaveFlattened => self.save_flattened(),
            Action::Print => {
                if let Some(d) = &self.doc {
                    match d.output() {
                        Ok((b, _)) => {
                            let tmp = kit42::shell::temp_file("print", "pdf");
                            if std::fs::write(&tmp, b).is_ok() {
                                kit42::shell::print(&tmp);
                            }
                        }
                        Err(e) => kui::error(APP, &e),
                    }
                }
            }
            Action::Close => {
                if self.confirm_close() {
                    self.doc = None;
                    self.mark_tex.clear();
                }
            }
            Action::Undo => {
                if let Some(d) = &mut self.doc {
                    let cur = d.snap();
                    if let Some(s) = d.history.undo(cur) {
                        d.restore(s);
                    }
                }
                self.selected = None;
            }
            Action::Redo => {
                if let Some(d) = &mut self.doc {
                    let cur = d.snap();
                    if let Some(s) = d.history.redo(cur) {
                        d.restore(s);
                    }
                }
                self.selected = None;
            }
            Action::ZoomIn => {
                self.fit = Fit::None;
                self.zoom = (self.zoom * 1.25).min(8.0);
                self.scroll_to = Some(self.current);
            }
            Action::ZoomOut => {
                self.fit = Fit::None;
                self.zoom = (self.zoom / 1.25).max(0.1);
                self.scroll_to = Some(self.current);
            }
            Action::ZoomActual => {
                self.fit = Fit::None;
                self.zoom = 1.0;
                self.scroll_to = Some(self.current);
            }
            Action::FitWidth => {
                self.fit = Fit::Width;
                self.scroll_to = Some(self.current);
            }
            Action::FitPage => {
                self.fit = Fit::Page;
                self.scroll_to = Some(self.current);
            }
            Action::GoTo(p) => {
                self.current = p;
                self.scroll_to = Some(p);
            }
            Action::Rotate(delta) => {
                let pages = self.sel_or_current();
                if let Some(d) = &mut self.doc {
                    // Marks follow the page rotation.
                    let sizes = d.sizes.clone();
                    let res = d.edit(t("Rotate", "Поворот"), |doc| pdfops::rotate(doc, &pages, delta));
                    if res.is_ok() {
                        for m in d.marks.iter_mut().filter(|m| pages.contains(&m.page)) {
                            let turns = (delta.rem_euclid(360) / 90) as usize;
                            let (mut w, mut h) = sizes[m.page];
                            for _ in 0..turns {
                                m.rotate_cw(h);
                                std::mem::swap(&mut w, &mut h);
                            }
                            let _ = w;
                        }
                    } else if let Err(e) = res {
                        kui::error(APP, &e);
                    }
                }
            }
            Action::DeletePages => {
                let pages = self.sel_or_current();
                if let Some(d) = &mut self.doc {
                    let n = d.page_count();
                    if !kui::confirm(APP, &format!("{} {}?", t("Delete pages:", "Удалить страниц:"), pages.len())) {
                        return;
                    }
                    match d.edit(t("Delete Pages", "Удаление страниц"), |doc| pdfops::delete(doc, &pages)) {
                        Ok(()) => {
                            let keep: Vec<usize> = (0..n).filter(|i| !pages.contains(i)).collect();
                            remap_marks(&mut d.marks, &keep);
                            self.pages_sel.clear();
                            self.current = self.current.min(d.page_count().saturating_sub(1));
                        }
                        Err(e) => kui::error(APP, &e),
                    }
                }
            }
            Action::InsertBlank => {
                let at = self.current + 1;
                if let Some(d) = &mut self.doc {
                    let (w, h) = d.sizes.get(self.current).cloned().unwrap_or(kit42::pdfgen::A4);
                    match d.edit(t("Insert Page", "Вставка страницы"), |doc| pdfops::insert_blank(doc, at, w, h)) {
                        Ok(()) => {
                            for m in &mut d.marks {
                                if m.page >= at {
                                    m.page += 1;
                                }
                            }
                            self.act(Action::GoTo(at));
                        }
                        Err(e) => kui::error(APP, &e),
                    }
                }
            }
            Action::InsertFile => {
                if let Some(files) = kui::open_dialog(t("Insert Pages", "Вставить страницы"), &[(t("PDF and images", "PDF и изображения"), &["pdf", "png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp", "heic"])]).pick_files() {
                    let before = self.doc.as_ref().map(|d| d.page_count()).unwrap_or(0);
                    let at = self.current + 1;
                    self.insert_files(files, at);
                    if let Some(d) = &mut self.doc {
                        let added = d.page_count() - before;
                        for m in &mut d.marks {
                            if m.page >= at {
                                m.page += added;
                            }
                        }
                    }
                }
            }
            Action::DuplicatePages => {
                let pages = self.sel_or_current();
                if let Some(d) = &mut self.doc {
                    let bytes = d.bytes.clone();
                    let last = *pages.iter().max().unwrap_or(&0);
                    let res = d.edit(t("Duplicate", "Дублирование"), |doc| {
                        let one = pdfops::extract(&bytes, &pages)?;
                        pdfops::insert_document(doc, pdfops::load(&one, None)?, last + 1)?;
                        Ok(())
                    });
                    if let Err(e) = res {
                        kui::error(APP, &e);
                    }
                }
            }
            Action::ExtractPages => {
                let pages = self.sel_or_current();
                if let Some(d) = &self.doc {
                    if let Some(p) = kui::open_dialog(t("Extract Pages", "Извлечь страницы"), &[("PDF", &["pdf"])]).set_file_name("Extract.pdf").save_file() {
                        match d.output().and_then(|(b, _)| pdfops::extract(&b, &pages)) {
                            Ok(b) => {
                                if let Err(e) = kit42::write_atomic(&p, &b) {
                                    kui::error(APP, &e.to_string());
                                }
                            }
                            Err(e) => kui::error(APP, &e),
                        }
                    }
                }
            }
            Action::MovePages(dir) => {
                let pages = self.sel_or_current();
                if let Some(d) = &mut self.doc {
                    let n = d.page_count();
                    let mut order: Vec<usize> = (0..n).collect();
                    let mut sel: Vec<usize> = pages.clone();
                    if dir < 0 {
                        sel.sort();
                        if sel[0] == 0 {
                            return;
                        }
                        for &p in &sel {
                            order.swap(p, p - 1);
                        }
                    } else {
                        sel.sort_by(|a, b| b.cmp(a));
                        if sel[0] + 1 >= n {
                            return;
                        }
                        for &p in &sel {
                            order.swap(p, p + 1);
                        }
                    }
                    self.reorder(order);
                    let moved: BTreeSet<usize> = pages.iter().map(|&p| (p as i32 + dir) as usize).collect();
                    self.current = (self.current as i32 + dir).max(0) as usize;
                    self.pages_sel = moved;
                    self.scroll_to = Some(self.current);
                }
            }
            Action::NewFromImages => {
                if let Some(files) = kui::open_dialog(t("New PDF from Images", "Новый PDF из изображений"), &[(t("Images", "Изображения"), &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "gif", "webp", "heic"])]).pick_files() {
                    match images_to_pdf(&files) {
                        Ok(b) => {
                            if self.doc.is_some() {
                                let tmp = kit42::shell::temp_file("Images", "pdf");
                                if std::fs::write(&tmp, &b).is_ok() {
                                    self.open_path(tmp);
                                }
                            } else if let Err(e) = self.open_bytes(None, b, None) {
                                kui::error(APP, &e);
                            }
                        }
                        Err(e) => kui::error(APP, &e),
                    }
                }
            }
            Action::DeleteMark if self.selected.is_none() && self.img_sel.is_some() => self.image_cmd(view::ImageCmd::Delete),
            Action::DeleteMark => {
                if let (Some(id), Some(d)) = (self.selected, &mut self.doc) {
                    d.checkpoint(t("Delete Markup", "Удаление пометки"));
                    d.marks.retain(|m| m.id != id);
                    self.selected = None;
                }
            }
            Action::ApplyRedactions => self.apply_redactions(),
            Action::FlattenNow => {
                if let Some(d) = &mut self.doc {
                    if d.marks.is_empty() {
                        return;
                    }
                    match d.output() {
                        Ok((b, _)) => {
                            d.checkpoint(t("Flatten", "Впечатывание"));
                            d.bytes = Arc::new(b);
                            d.marks.clear();
                            let _ = d.reload();
                        }
                        Err(e) => kui::error(APP, &e),
                    }
                }
            }
            Action::Ocr => self.dialog = Some(dialogs::Dialog::ocr()),
            Action::AddBookmark => {
                let cur = self.current;
                if let Some(d) = &mut self.doc {
                    let mut list = d.outline.clone();
                    let title = format!("{} {}", t("Page", "Страница"), cur + 1);
                    let pos = list.iter().position(|b| b.page > cur).unwrap_or(list.len());
                    list.insert(pos, pdfops::Bookmark { title, page: cur, level: 0 });
                    if let Err(e) = d.edit(t("Add Bookmark", "Добавление закладки"), |doc| pdfops::write_outline(doc, &list)) {
                        kui::error(APP, &e);
                    }
                    self.sidebar = Some(SideTab::Bookmarks);
                }
            }
            Action::RemoveAnnotations => {
                let pages = self.sel_or_current();
                if let Some(d) = &mut self.doc {
                    if kui::confirm(APP, t("Remove all existing annotations (comments, highlights, links) from the selected pages? Form fields stay.", "Удалить все имеющиеся аннотации (комментарии, выделения, ссылки) с выбранных страниц? Поля форм останутся.")) {
                        let r = d.edit(t("Remove Annotations", "Удаление аннотаций"), |doc| {
                            let ids = pdfops::page_ids(doc);
                            for p in &pages {
                                if let Some(&id) = ids.get(*p) {
                                    pdfops::remove_annotations(doc, id, true);
                                }
                            }
                            Ok(())
                        });
                        if let Err(e) = r {
                            kui::error(APP, &e);
                        }
                    }
                }
            }
            Action::Signatures => self.picker.open = true,
            Action::Dialog(k) => self.open_dialog(k),
            Action::About => kui::about(APP, t("PDF reader and editor", "просмотр и правка PDF")),
            Action::ExportText(f) => self.export_text(f),
            Action::AutoCrop => self.auto_crop(),
        }
        let _ = ctx;
    }

    pub fn reorder(&mut self, order: Vec<usize>) {
        if let Some(d) = &mut self.doc {
            let o2 = order.clone();
            match d.edit(t("Reorder Pages", "Порядок страниц"), |doc| pdfops::reorder(doc, &o2)) {
                Ok(()) => remap_marks(&mut d.marks, &order),
                Err(e) => kui::error(APP, &e),
            }
        }
    }

    fn save_flattened(&mut self) {
        let Some(d) = &self.doc else { return };
        let Some(p) = kui::open_dialog(t("Save Flattened Copy", "Сохранить копию без слоёв"), &[("PDF", &["pdf"])]).set_file_name(d.name()).save_file() else { return };
        // Each page becomes a 200 dpi picture with an OCR text layer: nothing hidden survives.
        let bytes = match d.output() {
            Ok((b, _)) => b,
            Err(e) => return kui::error(APP, &e),
        };
        self.spawn_job(t("Flattening pages…", "Впечатывание страниц…"), "", move |progress| {
            let (r, sizes) = PdfRenderer::open(bytes, None)?;
            let mut out = kit42::pdfgen::Doc::new();
            let font = out.font("Arial", false, false);
            for (i, (w, h)) in sizes.iter().enumerate() {
                let img = r.render(i as u32, (w / 72.0 * 200.0) as u32)?;
                let s = img.width() as f32 / w;
                let ocr = kit42::winrt::ocr(&img, None).ok();
                let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
                let mut jpg = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 85).encode_image(&rgb).map_err(|e| e.to_string())?;
                let id = out.add_image_jpeg(jpg, img.width(), img.height(), false);
                let mut page = kit42::pdfgen::Page::new(*w, *h);
                page.image(id, 0.0, 0.0, *w, *h);
                if let Some(o) = ocr {
                    o.to_pdf_layer(&mut out, &mut page, font, 1.0 / s);
                }
                out.add_page(page);
                progress.store(((i + 1) * 1000 / sizes.len()) as u32, std::sync::atomic::Ordering::Relaxed);
            }
            let b = out.finish();
            kit42::write_atomic(&p, &b).map_err(|e| e.to_string())?;
            Ok(Vec::new())
        });
    }

    fn apply_redactions(&mut self) {
        let Some(d) = &mut self.doc else { return };
        if !d.marks.iter().any(|m| matches!(m.kind, Kind::Redact)) {
            kui::info(APP, t("Mark areas with the Redact tool first.", "Сначала отметьте области инструментом «Скрыть»."));
            return;
        }
        if !kui::confirm(
            APP,
            t(
                "Apply redactions? Pages with redaction boxes become images with a recognised text layer; everything under the boxes is removed permanently.",
                "Применить скрытие? Страницы с областями скрытия станут изображениями с распознанным текстом; всё под областями будет удалено безвозвратно.",
            ),
        ) {
            return;
        }
        let redactions: Vec<Mark> = d.marks.iter().filter(|m| matches!(m.kind, Kind::Redact)).cloned().collect();
        let renderer = d.renderer.clone();
        let res = (|| {
            let mut doc = d.lopdf()?;
            let render = move |page: usize, w: u32| renderer.as_ref().ok_or("no renderer".to_string())?.render(page as u32, w);
            markup::apply(&mut doc, &redactions, &render, "")?;
            pdfops::save(&mut doc)
        })();
        match res {
            Ok(b) => {
                d.checkpoint(t("Apply Redactions", "Применение скрытия"));
                d.marks.retain(|m| !matches!(m.kind, Kind::Redact));
                d.bytes = Arc::new(b);
                let _ = d.reload();
            }
            Err(e) => kui::error(APP, &e),
        }
    }

    fn auto_crop(&mut self) {
        let pages = self.sel_or_current();
        let Some(d) = &mut self.doc else { return };
        let Some(r) = d.renderer.clone() else { return };
        let mut boxes = Vec::new();
        for &p in &pages {
            let (w, _) = d.sizes[p];
            let Ok(img) = r.render(p as u32, (w / 72.0 * 60.0) as u32) else { continue };
            let s = img.width() as f32 / w;
            let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
            for (x, y, px) in img.enumerate_pixels() {
                if px[0] < 235 || px[1] < 235 || px[2] < 235 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
            if x1 > x0 && y1 > y0 {
                let m = 6.0;
                boxes.push((p, [(x0 as f32 / s - m).max(0.0), (y0 as f32 / s - m).max(0.0), x1 as f32 / s + m, y1 as f32 / s + m]));
            }
        }
        let res = d.edit(t("Remove White Margins", "Удаление белых полей"), |doc| {
            let ids = pdfops::page_ids(doc);
            for (p, b) in &boxes {
                let g = pdfops::geom(doc, ids[*p]);
                let (wd, hd) = g.display();
                let b = [b[0], b[1], b[2].min(wd), b[3].min(hd)];
                pdfops::set_crop(doc, ids[*p], Some(g.rect_to_user(b)))?;
            }
            Ok(())
        });
        if let Err(e) = res {
            kui::error(APP, &e);
        }
        d.marks.retain(|m| !boxes.iter().any(|(p, _)| *p == m.page));
    }
}

/// `keep[i]` = old index of the page now at i. Marks on removed pages are dropped.
pub fn remap_marks(marks: &mut Vec<Mark>, keep: &[usize]) {
    marks.retain_mut(|m| match keep.iter().position(|&o| o == m.page) {
        Some(new) => {
            m.page = new;
            true
        }
        None => false,
    });
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }
    kit42::run(kit42::AppSpec { id: "pdf42", title: APP, size: [1200.0, 860.0] }, App::new)
}
