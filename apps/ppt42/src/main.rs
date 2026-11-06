#![windows_subsystem = "windows"]
//! PPT42 for Windows — presentations and slide shows (.pptx, .potx, .odp, .otp).

mod model;
mod odp;
mod pptx;
mod render;

use egui::{Color32, Context, Key, Pos2, Rect, Sense, Stroke, Vec2};
use kit42::history::History;
use kit42::icons as ic;
use kit42::richtext::{self, Align2, CharFmt, ListInfo, ListKind, Para};
use kit42::ui::{self as kui, CTRL, CTRL_SHIFT, NONE};
use kit42::{settings, t};
use model::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub const APP: &str = "PPT42";

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Normal,
    Sorter,
    Outline,
}

enum Drag {
    Move { ids: Vec<u64>, last: Pos2 },
    Resize { id: u64, handle: usize, orig: (f32, f32, f32, f32), start: Pos2 },
    Rotate { id: u64 },
    Marquee { start: Pos2 },
    Thumb { from: usize },
}

struct Show {
    index: usize,
    step: usize,
    started: f64,
    anim_started: f64,
    prev: Option<usize>,
    pen: bool,
    laser: bool,
    strokes: Vec<Vec<Pos2>>,
    screen: Option<Color32>,
    number: String,
    overview: bool,
}

struct App {
    pres: Presentation,
    path: Option<PathBuf>,
    history: History<Presentation>,
    cur: usize,
    sel: Vec<u64>,
    sel_slides: std::collections::BTreeSet<usize>,
    editing: Option<u64>,
    edit_focus: Option<(usize, usize)>,
    typing: Option<CharFmt>,
    edit_para: Option<(usize, (usize, usize))>,
    drag: Option<Drag>,
    view: View,
    texes: HashMap<usize, egui::TextureHandle>,
    clipboard: Vec<Shape>,
    show: Option<Show>,
    guides: Vec<(bool, f32)>,
    picker: kit42::signature::MarkPicker,
    status: String,
    insert_geom: Option<Geom>,
    find: Option<(String, String)>,
    zoom_fit: bool,
    last_text_edit: Option<(u64, std::time::Instant)>,
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self {
            pres: Presentation::default(),
            path: None,
            history: History::new(60),
            cur: 0,
            sel: vec![],
            sel_slides: Default::default(),
            editing: None,
            edit_focus: None,
            typing: None,
            edit_para: None,
            drag: None,
            view: View::Normal,
            texes: HashMap::new(),
            clipboard: vec![],
            show: None,
            guides: vec![],
            picker: Default::default(),
            status: String::new(),
            insert_geom: None,
            find: None,
            zoom_fit: true,
            last_text_edit: None,
        };
        if let Some(p) = kit42::startup_files().into_iter().next() {
            app.open(p);
        }
        app
    }

    fn name(&self) -> String {
        self.path.as_ref().map(|p| kit42::file_name(p)).unwrap_or_else(|| t("Presentation1", "Презентация1").into())
    }

    fn checkpoint(&mut self, label: &str) {
        self.history.push(self.pres.clone(), label);
    }

    fn slide(&self) -> &Slide {
        &self.pres.slides[self.cur]
    }

    fn slide_mut(&mut self) -> &mut Slide {
        let c = self.cur;
        &mut self.pres.slides[c]
    }

    fn shape_mut(&mut self, id: u64) -> Option<&mut Shape> {
        self.slide_mut().shapes.iter_mut().find(|s| s.id == id)
    }

    pub fn load(path: &std::path::Path) -> Result<Presentation, String> {
        let bytes = kit42::ooxml::read_file(path)?;
        if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
            return Err(t("Legacy PowerPoint (.ppt) files are not supported. Convert the file to .pptx in PowerPoint or LibreOffice Impress.", "Старый формат PowerPoint (.ppt) не поддерживается. Преобразуйте файл в .pptx в PowerPoint или LibreOffice Impress.").into());
        }
        let pkg = kit42::ooxml::Package::read(&bytes)?;
        if pkg.parts.contains_key("content.xml") {
            odp::read(&bytes)
        } else {
            pptx::read(&bytes)
        }
    }

    fn open(&mut self, path: PathBuf) {
        if self.path.is_some() || self.history.is_dirty() {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).arg(&path).spawn();
            }
            return;
        }
        match Self::load(&path) {
            Ok(p) => {
                self.pres = p;
                let template = matches!(kit42::ext(&path).as_str(), "potx" | "otp");
                self.path = if template { None } else { Some(path.clone()) };
                self.cur = 0;
                self.sel.clear();
                self.history.clear();
                settings::add_recent("ppt42", &path);
                kui::remember_dir(&path);
            }
            Err(e) => kui::error(APP, &format!("{}\n{e}", t("The presentation could not be opened.", "Не удалось открыть презентацию."))),
        }
    }

    fn serialize(&self, path: &std::path::Path) -> Result<Vec<u8>, String> {
        match kit42::ext(path).as_str() {
            "odp" => odp::write(&self.pres, false),
            "otp" => odp::write(&self.pres, true),
            "potx" => pptx::write(&self.pres, true),
            "pdf" => Ok(render::pdf(&self.pres, false, false)),
            "md" => Ok(outline_md(&self.pres).into_bytes()),
            _ => pptx::write(&self.pres, false),
        }
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        self.commit_text();
        match self.serialize(&path).and_then(|b| kit42::write_atomic(&path, &b).map_err(|e| e.to_string())) {
            Ok(()) => {
                if !matches!(kit42::ext(&path).as_str(), "pdf" | "md" | "potx" | "otp") {
                    self.path = Some(path.clone());
                    self.history.mark_saved();
                    settings::add_recent("ppt42", &path);
                }
                self.status = format!("{} {}", t("Saved:", "Сохранено:"), kit42::file_name(&path));
                true
            }
            Err(e) => {
                kui::error(APP, &format!("{}\n{e}", t("Saving failed; the file was not changed.", "Не удалось сохранить; файл не изменён.")));
                false
            }
        }
    }

    fn save(&mut self) -> bool {
        match self.path.clone() {
            Some(p) => self.save_to(p),
            None => self.save_as(),
        }
    }

    fn save_as(&mut self) -> bool {
        let stem = self.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| t("Presentation", "Презентация").into());
        let Some(p) = kui::open_dialog(t("Save As", "Сохранить как"), &[("PowerPoint (.pptx)", &["pptx"]), ("OpenDocument (.odp)", &["odp"]), (t("PowerPoint template (.potx)", "Шаблон PowerPoint (.potx)"), &["potx"]), (t("OpenDocument template (.otp)", "Шаблон OpenDocument (.otp)"), &["otp"])]).set_file_name(format!("{stem}.pptx")).save_file() else {
            return false;
        };
        kui::remember_dir(&p);
        self.save_to(p)
    }

    fn export(&mut self, kind: u8) {
        let stem = self.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| t("Presentation", "Презентация").into());
        match kind {
            0 | 1 => {
                if let Some(p) = kui::open_dialog(t("Export as PDF", "Экспорт в PDF"), &[("PDF", &["pdf"])]).set_file_name(format!("{stem}.pdf")).save_file() {
                    let b = render::pdf(&self.pres, false, kind == 1);
                    if let Err(e) = kit42::write_atomic(&p, &b) {
                        kui::error(APP, &e.to_string());
                    }
                }
            }
            2 | 3 => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    for (i, s) in self.pres.slides.iter().enumerate().filter(|(_, s)| !s.hidden) {
                        let img = render::raster(&self.pres, s, 1920);
                        let p = dir.join(format!("{stem} {:02}.{}", i + 1, if kind == 2 { "png" } else { "jpg" }));
                        let r = if kind == 2 {
                            img.save(&p).map_err(|e| e.to_string())
                        } else {
                            let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
                            let mut out = Vec::new();
                            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&rgb).map_err(|e| e.to_string()).and_then(|_| kit42::write_atomic(&p, &out).map_err(|e| e.to_string()))
                        };
                        if let Err(e) = r {
                            kui::error(APP, &e);
                            break;
                        }
                    }
                    kit42::shell::open_path(&dir);
                }
            }
            _ => {
                if let Some(p) = kui::open_dialog(t("Export Outline", "Экспорт структуры"), &[("Markdown", &["md"]), ("RTF", &["rtf"])]).set_file_name(format!("{stem}.md")).save_file() {
                    let md = outline_md(&self.pres);
                    let bytes = if kit42::ext(&p) == "rtf" {
                        let mut r = String::from("{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Calibri;}}\\f0\\fs24\n");
                        for line in md.lines() {
                            for c in line.trim_start_matches("## ").chars() {
                                match c {
                                    '\\' | '{' | '}' => {
                                        r.push('\\');
                                        r.push(c);
                                    }
                                    c if (c as u32) < 128 => r.push(c),
                                    c => {
                                        let mut b = [0u16; 2];
                                        for u in c.encode_utf16(&mut b) {
                                            r.push_str(&format!("\\u{}?", *u as i16));
                                        }
                                    }
                                }
                            }
                            r.push_str("\\par\n");
                        }
                        r.push('}');
                        r.into_bytes()
                    } else {
                        md.into_bytes()
                    };
                    let _ = kit42::write_atomic(&p, &bytes);
                }
            }
        }
    }

    fn new_slide(&mut self, layout: usize) {
        self.commit_text();
        self.checkpoint(t("New Slide", "Новый слайд"));
        let s = self.pres.layout_slide(layout);
        let at = (self.cur + 1).min(self.pres.slides.len());
        self.pres.slides.insert(at, s);
        self.cur = at;
        self.sel.clear();
    }

    fn insert_shape(&mut self, mut s: Shape) {
        self.commit_text();
        self.checkpoint(t("Insert", "Вставка"));
        s.x = s.x.clamp(0.0, self.pres.width - 20.0);
        s.y = s.y.clamp(0.0, self.pres.height - 20.0);
        let id = s.id;
        self.slide_mut().shapes.push(s);
        self.sel = vec![id];
    }

    fn insert_picture_bytes(&mut self, data: Vec<u8>) {
        let Ok(img) = image::load_from_memory(&data) else {
            kui::error(APP, t("The picture could not be read.", "Не удалось прочитать рисунок."));
            return;
        };
        let (w, h) = (img.width() as f32, img.height() as f32);
        let s = (self.pres.width * 0.6 / w).min(self.pres.height * 0.6 / h).min(1.0);
        let (sw, sh) = (w * s, h * s);
        let shape = Shape::new(Content::Picture(Arc::new(data)), (self.pres.width - sw) / 2.0, (self.pres.height - sh) / 2.0, sw, sh);
        self.insert_shape(shape);
    }

    fn delete_selected(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        self.checkpoint(t("Delete", "Удаление"));
        let sel = self.sel.clone();
        self.slide_mut().shapes.retain(|s| !sel.contains(&s.id));
        self.sel.clear();
    }

    fn commit_text(&mut self) {
        if self.editing.take().is_some() {
            self.edit_focus = None;
            self.edit_para = None;
        }
    }

    fn style_text(&mut self, label: &str, f: impl Fn(&mut CharFmt)) {
        // Applies to the selection inside the edited paragraph, otherwise to whole selected shapes.
        if let (Some(id), Some((pi, (a, b)))) = (self.editing, self.edit_para) {
            if a < b {
                self.checkpoint(label);
                if let Some(sh) = self.shape_mut(id) {
                    if let Some(p) = sh.text.get_mut(pi) {
                        p.apply(a, b, &mut |x| f(x));
                    }
                }
                self.edit_focus = Some((pi, b));
                return;
            }
        }
        if self.sel.is_empty() {
            return;
        }
        self.checkpoint(label);
        let sel = self.sel.clone();
        for sh in self.slide_mut().shapes.iter_mut().filter(|s| sel.contains(&s.id)) {
            for p in &mut sh.text {
                p.apply_all(&mut |x| f(x));
            }
            if let Content::Table(rows) = &mut sh.content {
                for r in rows {
                    for c in r {
                        c.apply_all(&mut |x| f(x));
                    }
                }
            }
        }
    }

    fn para_op(&mut self, label: &str, f: impl Fn(&mut Para)) {
        if self.sel.is_empty() {
            return;
        }
        self.checkpoint(label);
        let sel = self.sel.clone();
        let only = self.editing.and(self.edit_para.map(|p| p.0));
        for sh in self.slide_mut().shapes.iter_mut().filter(|s| sel.contains(&s.id)) {
            for (i, p) in sh.text.iter_mut().enumerate() {
                if only.map(|o| o == i).unwrap_or(true) {
                    f(p);
                }
            }
        }
    }

    fn shape_op(&mut self, label: &str, f: impl Fn(&mut Shape)) {
        if self.sel.is_empty() {
            return;
        }
        self.checkpoint(label);
        let sel = self.sel.clone();
        for sh in self.slide_mut().shapes.iter_mut().filter(|s| sel.contains(&s.id)) {
            f(sh);
        }
    }

    fn arrange(&mut self, front: bool) {
        if self.sel.is_empty() {
            return;
        }
        self.checkpoint(t("Arrange", "Порядок"));
        let sel = self.sel.clone();
        let shapes = &mut self.slide_mut().shapes;
        let (mut picked, rest): (Vec<Shape>, Vec<Shape>) = shapes.drain(..).partition(|s| sel.contains(&s.id));
        if front {
            *shapes = rest;
            shapes.append(&mut picked);
        } else {
            picked.extend(rest);
            *shapes = picked;
        }
    }

    fn align(&mut self, how: u8) {
        let (w, h) = (self.pres.width, self.pres.height);
        self.shape_op(t("Align", "Выравнивание"), move |s| match how {
            0 => s.x = 0.0,
            1 => s.x = (w - s.w) / 2.0,
            2 => s.x = w - s.w,
            3 => s.y = 0.0,
            4 => s.y = (h - s.h) / 2.0,
            _ => s.y = h - s.h,
        });
    }

    fn start_show(&mut self, from: usize, ctx: &Context) {
        self.commit_text();
        let now = ctx.input(|i| i.time);
        let mut i = from.min(self.pres.slides.len() - 1);
        while i < self.pres.slides.len() - 1 && self.pres.slides[i].hidden {
            i += 1;
        }
        self.show = Some(Show { index: i, step: 0, started: now, anim_started: now, prev: None, pen: false, laser: false, strokes: vec![], screen: None, number: String::new(), overview: false });
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
    }

    fn end_show(&mut self, ctx: &Context) {
        if let Some(s) = self.show.take() {
            self.cur = s.index;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        kui::set_title(ctx, APP, Some(&self.name()), self.history.is_dirty());
        if self.show.is_some() {
            self.show_ui(ctx);
            return;
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for p in dropped {
            let ext = kit42::ext(&p);
            if ["png", "jpg", "jpeg", "gif", "bmp", "webp"].contains(&ext.as_str()) {
                if let Ok(d) = std::fs::read(&p) {
                    self.insert_picture_bytes(d);
                }
            } else {
                self.open(p);
            }
        }
        self.shortcuts(ctx);
        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu(ui));
        egui::TopBottomPanel::top("ribbon").frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(egui::Margin::symmetric(6, 3))).show(ctx, |ui| self.ribbon(ui));
        let status = format!("{} {} / {} · {}", t("Slide", "Слайд"), self.cur + 1, self.pres.slides.len(), self.status);
        kui::status_bar(ctx, |ui| {
            ui.label(status);
        });
        match self.view {
            View::Normal => {
                egui::SidePanel::left("thumbs").resizable(true).default_width(190.0).show(ctx, |ui| self.thumbnails(ui));
                egui::SidePanel::right("inspector").resizable(true).default_width(260.0).show(ctx, |ui| self.inspector(ui));
                egui::TopBottomPanel::bottom("notes").resizable(true).default_height(90.0).show(ctx, |ui| {
                    let mut notes = self.slide().notes.clone();
                    let r = ui.add(egui::TextEdit::multiline(&mut notes).desired_width(f32::INFINITY).desired_rows(3).hint_text(t("Click to add notes", "Заметки к слайду")));
                    if r.changed() {
                        if r.gained_focus() || self.history.undo_label() != Some(t("Notes", "Заметки")) {
                            self.checkpoint(t("Notes", "Заметки"));
                        }
                        self.slide_mut().notes = notes;
                    }
                });
                egui::CentralPanel::default().frame(egui::Frame::new().fill(if ctx.style().visuals.dark_mode { Color32::from_gray(40) } else { Color32::from_gray(222) })).show(ctx, |ui| self.canvas(ui));
            }
            View::Sorter => {
                egui::CentralPanel::default().show(ctx, |ui| self.sorter(ui));
            }
            View::Outline => {
                egui::CentralPanel::default().show(ctx, |ui| self.outline(ui));
            }
        }
        if let Some(img) = self.picker.show(ctx) {
            let mut data = Vec::new();
            if image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut data), image::ImageFormat::Png).is_ok() {
                self.insert_picture_bytes(data);
            }
        }
        self.find_window(ctx);
        let dirty = self.history.is_dirty();
        let name = self.name();
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
    fn shortcuts(&mut self, ctx: &Context) {
        if kui::shortcut(ctx, CTRL, Key::S) {
            self.save();
        }
        if kui::shortcut(ctx, CTRL_SHIFT, Key::S) {
            self.save_as();
        }
        if kui::shortcut(ctx, CTRL, Key::O) {
            self.open_dialog();
        }
        if kui::shortcut(ctx, NONE, Key::F5) {
            self.start_show(0, ctx);
        }
        if kui::shortcut(ctx, egui::Modifiers::SHIFT, Key::F5) {
            self.start_show(self.cur, ctx);
        }
        if kui::shortcut(ctx, CTRL, Key::M) {
            self.new_slide(1);
        }
        if kui::shortcut(ctx, CTRL, Key::Z) {
            self.commit_text();
            if let Some(p) = self.history.undo(self.pres.clone()) {
                self.pres = p;
                self.cur = self.cur.min(self.pres.slides.len() - 1);
            }
        }
        if kui::shortcut(ctx, CTRL, Key::Y) || kui::shortcut(ctx, CTRL_SHIFT, Key::Z) {
            self.commit_text();
            if let Some(p) = self.history.redo(self.pres.clone()) {
                self.pres = p;
                self.cur = self.cur.min(self.pres.slides.len() - 1);
            }
        }
        if kui::shortcut(ctx, CTRL, Key::B) {
            let v = !self.cur_fmt().bold;
            self.style_text(t("Bold", "Полужирный"), move |f| f.bold = v);
        }
        if kui::shortcut(ctx, CTRL, Key::I) {
            let v = !self.cur_fmt().italic;
            self.style_text(t("Italic", "Курсив"), move |f| f.italic = v);
        }
        if kui::shortcut(ctx, CTRL, Key::U) {
            let v = !self.cur_fmt().underline;
            self.style_text(t("Underline", "Подчёркнутый"), move |f| f.underline = v);
        }
        if kui::shortcut(ctx, CTRL, Key::F) {
            self.find = Some((String::new(), String::new()));
        }
        if self.editing.is_some() || ctx.wants_keyboard_input() {
            if kui::shortcut(ctx, NONE, Key::Escape) {
                self.commit_text();
            }
            return;
        }
        if kui::shortcut(ctx, NONE, Key::Delete) || kui::shortcut(ctx, NONE, Key::Backspace) {
            if self.sel.is_empty() && self.view != View::Outline {
                self.delete_slides();
            } else {
                self.delete_selected();
            }
        }
        if kui::shortcut(ctx, CTRL, Key::D) {
            self.duplicate_selected();
        }
        if kui::shortcut(ctx, CTRL, Key::A) {
            self.sel = self.slide().shapes.iter().map(|s| s.id).collect();
        }
        let events = ctx.input(|i| i.events.clone());
        for e in events {
            match e {
                egui::Event::Copy => {
                    let sel = self.sel.clone();
                    self.clipboard = self.slide().shapes.iter().filter(|s| sel.contains(&s.id)).cloned().collect();
                }
                egui::Event::Cut => {
                    let sel = self.sel.clone();
                    self.clipboard = self.slide().shapes.iter().filter(|s| sel.contains(&s.id)).cloned().collect();
                    self.delete_selected();
                }
                egui::Event::Paste(text) => {
                    if !self.clipboard.is_empty() {
                        self.checkpoint(t("Paste", "Вставка"));
                        let mut ids = vec![];
                        for mut s in self.clipboard.clone() {
                            s.id = rand::random();
                            s.x += 12.0;
                            s.y += 12.0;
                            ids.push(s.id);
                            self.slide_mut().shapes.push(s);
                        }
                        self.sel = ids;
                    } else if !text.trim().is_empty() {
                        let f = self.pres.fmt(20.0, false);
                        let s = Shape::text_box(80.0, 80.0, 500.0, 120.0, &text, f);
                        self.insert_shape(s);
                    } else if let Ok(mut cb) = arboard::Clipboard::new() {
                        if let Ok(img) = cb.get_image() {
                            if let Some(buf) = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned()) {
                                let mut data = Vec::new();
                                if image::DynamicImage::ImageRgba8(buf).write_to(&mut std::io::Cursor::new(&mut data), image::ImageFormat::Png).is_ok() {
                                    self.insert_picture_bytes(data);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        // Arrow keys nudge shapes or move between slides.
        let step = if ctx.input(|i| i.modifiers.shift) { 10.0 } else { 1.0 };
        let (dx, dy) = ctx.input(|i| {
            (
                (i.key_pressed(Key::ArrowRight) as i32 - i.key_pressed(Key::ArrowLeft) as i32) as f32,
                (i.key_pressed(Key::ArrowDown) as i32 - i.key_pressed(Key::ArrowUp) as i32) as f32,
            )
        });
        if dx != 0.0 || dy != 0.0 {
            if !self.sel.is_empty() {
                self.shape_op(t("Move", "Перемещение"), move |s| {
                    s.x += dx * step;
                    s.y += dy * step;
                });
            } else {
                let n = self.pres.slides.len() as i32;
                self.cur = (self.cur as i32 + dx as i32 + dy as i32).clamp(0, n - 1) as usize;
            }
        }
        if kui::shortcut(ctx, NONE, Key::PageDown) {
            self.cur = (self.cur + 1).min(self.pres.slides.len() - 1);
            self.sel.clear();
        }
        if kui::shortcut(ctx, NONE, Key::PageUp) {
            self.cur = self.cur.saturating_sub(1);
            self.sel.clear();
        }
    }

    fn duplicate_selected(&mut self) {
        if self.sel.is_empty() {
            self.checkpoint(t("Duplicate Slide", "Дублирование слайда"));
            let mut s = self.slide().clone();
            s.id = rand::random();
            for sh in &mut s.shapes {
                sh.id = rand::random();
            }
            self.pres.slides.insert(self.cur + 1, s);
            self.cur += 1;
            return;
        }
        self.checkpoint(t("Duplicate", "Дублирование"));
        let sel = self.sel.clone();
        let copies: Vec<Shape> = self.slide().shapes.iter().filter(|s| sel.contains(&s.id)).cloned().map(|mut s| {
            s.id = rand::random();
            s.x += 16.0;
            s.y += 16.0;
            s
        }).collect();
        self.sel = copies.iter().map(|s| s.id).collect();
        self.slide_mut().shapes.extend(copies);
    }

    fn delete_slides(&mut self) {
        if self.pres.slides.len() <= 1 {
            return;
        }
        self.checkpoint(t("Delete Slide", "Удаление слайда"));
        let mut idx: Vec<usize> = if self.sel_slides.is_empty() { vec![self.cur] } else { self.sel_slides.iter().cloned().collect() };
        idx.sort_by(|a, b| b.cmp(a));
        for i in idx {
            if self.pres.slides.len() > 1 {
                self.pres.slides.remove(i);
            }
        }
        self.sel_slides.clear();
        self.cur = self.cur.min(self.pres.slides.len() - 1);
    }

    fn cur_fmt(&self) -> CharFmt {
        if let Some(f) = &self.typing {
            return f.clone();
        }
        let sh = self.sel.first().and_then(|id| self.slide().shapes.iter().find(|s| s.id == *id));
        match (sh, self.edit_para) {
            (Some(s), Some((pi, (a, _)))) if self.editing == Some(s.id) => s.text.get(pi).map(|p| p.fmt_at(a + 1)).unwrap_or_default(),
            (Some(s), _) => s.text.first().map(|p| p.fmt_at(1)).unwrap_or_else(|| self.pres.fmt(18.0, false)),
            _ => self.pres.fmt(18.0, false),
        }
    }

    fn open_dialog(&mut self) {
        if let Some(p) = kui::open_dialog(t("Open", "Открыть"), &[(t("Presentations", "Презентации"), &["pptx", "potx", "pptm", "odp", "otp", "ppt"])]).pick_file() {
            self.open(p);
        }
    }

    fn menu(&mut self, ui: &mut egui::Ui) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button(t("File", "Файл"), |ui| {
                if kui::menu_item(ui, t("New", "Создать"), "Ctrl+N") {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).spawn();
                    }
                }
                ui.menu_button(t("Theme", "Тема"), |ui| {
                    for th in themes() {
                        if ui.button(th.name).clicked() {
                            self.apply_theme(th);
                            ui.close_menu();
                        }
                    }
                });
                if kui::menu_item(ui, t("Open…", "Открыть…"), "Ctrl+O") {
                    self.open_dialog();
                }
                ui.menu_button(t("Open Recent", "Последние файлы"), |ui| {
                    for p in settings::recent("ppt42") {
                        if ui.button(kit42::file_name(&p)).clicked() {
                            self.open(p);
                            ui.close_menu();
                        }
                    }
                });
                ui.separator();
                if kui::menu_item(ui, t("Save", "Сохранить"), "Ctrl+S") {
                    self.save();
                }
                if kui::menu_item(ui, t("Save As…", "Сохранить как…"), "Ctrl+Shift+S") {
                    self.save_as();
                }
                ui.menu_button(t("Export", "Экспорт"), |ui| {
                    for (k, l) in [(0u8, t("PDF…", "PDF…")), (1, t("Notes Pages PDF…", "PDF со страницами заметок…")), (2, t("Slides as PNG…", "Слайды в PNG…")), (3, t("Slides as JPEG…", "Слайды в JPEG…")), (4, t("Outline (Markdown/RTF)…", "Структура (Markdown/RTF)…"))] {
                        if ui.button(l).clicked() {
                            self.export(k);
                            ui.close_menu();
                        }
                    }
                });
                if kui::menu_item(ui, t("Print…", "Печать…"), "Ctrl+P") {
                    let tmp = kit42::shell::temp_file("print", "pdf");
                    if std::fs::write(&tmp, render::pdf(&self.pres, false, false)).is_ok() {
                        kit42::shell::print(&tmp);
                    }
                }
                ui.separator();
                if kui::menu_item(ui, t("Exit", "Выход"), "Alt+F4") {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button(t("Edit", "Правка"), |ui| {
                if kui::menu_item_enabled(ui, self.history.can_undo(), t("Undo", "Отменить"), "Ctrl+Z") {
                    if let Some(p) = self.history.undo(self.pres.clone()) {
                        self.pres = p;
                        self.cur = self.cur.min(self.pres.slides.len() - 1);
                    }
                }
                if kui::menu_item_enabled(ui, self.history.can_redo(), t("Redo", "Повторить"), "Ctrl+Y") {
                    if let Some(p) = self.history.redo(self.pres.clone()) {
                        self.pres = p;
                        self.cur = self.cur.min(self.pres.slides.len() - 1);
                    }
                }
                ui.separator();
                if kui::menu_item(ui, t("Duplicate", "Дублировать"), "Ctrl+D") {
                    self.duplicate_selected();
                }
                if kui::menu_item(ui, t("Delete", "Удалить"), "Del") {
                    self.delete_selected();
                }
                if kui::menu_item(ui, t("Select All", "Выделить всё"), "Ctrl+A") {
                    self.sel = self.slide().shapes.iter().map(|s| s.id).collect();
                }
                if kui::menu_item(ui, t("Find and Replace…", "Найти и заменить…"), "Ctrl+F") {
                    self.find = Some((String::new(), String::new()));
                }
            });
            ui.menu_button(t("View", "Вид"), |ui| {
                for (v, l) in [(View::Normal, t("Normal", "Обычный")), (View::Sorter, t("Slide Sorter", "Сортировщик слайдов")), (View::Outline, t("Outline", "Структура"))] {
                    if ui.radio(self.view == v, l).clicked() {
                        self.commit_text();
                        self.view = v;
                        ui.close_menu();
                    }
                }
            });
            ui.menu_button(t("Insert", "Вставка"), |ui| {
                ui.menu_button(t("New Slide", "Новый слайд"), |ui| {
                    for i in 0..LAYOUTS.len() {
                        if ui.button(layout_name(i)).clicked() {
                            self.new_slide(i);
                            ui.close_menu();
                        }
                    }
                });
                if kui::menu_item(ui, t("Duplicate Slide", "Дублировать слайд"), "") {
                    self.sel.clear();
                    self.duplicate_selected();
                }
                if kui::menu_item(ui, t("Text Box", "Надпись"), "") {
                    let f = self.pres.fmt(20.0, false);
                    let s = Shape::text_box(self.pres.width * 0.3, self.pres.height * 0.4, self.pres.width * 0.4, 60.0, t("Text", "Текст"), f);
                    self.insert_shape(s);
                }
                ui.menu_button(t("Shape", "Фигура"), |ui| {
                    for g in Geom::SHAPES.iter().chain([Geom::Line, Geom::Arrow].iter()) {
                        if ui.button(g.name()).clicked() {
                            self.add_geom(*g);
                            ui.close_menu();
                        }
                    }
                });
                if kui::menu_item(ui, t("Picture…", "Рисунок…"), "") {
                    self.insert_picture();
                }
                if kui::menu_item(ui, t("Table", "Таблица"), "") {
                    self.insert_table(3, 3);
                }
                if kui::menu_item(ui, t("Signature or Stamp…", "Подпись или печать…"), "") {
                    self.picker.open = true;
                }
            });
            ui.menu_button(t("Arrange", "Упорядочить"), |ui| {
                if kui::menu_item(ui, t("Bring to Front", "На передний план"), "") {
                    self.arrange(true);
                }
                if kui::menu_item(ui, t("Send to Back", "На задний план"), "") {
                    self.arrange(false);
                }
                ui.separator();
                for (k, l) in [(0u8, t("Align Left", "По левому краю")), (1, t("Align Centre", "По центру")), (2, t("Align Right", "По правому краю")), (3, t("Align Top", "По верхнему краю")), (4, t("Align Middle", "По середине")), (5, t("Align Bottom", "По нижнему краю"))] {
                    if ui.button(l).clicked() {
                        self.align(k);
                        ui.close_menu();
                    }
                }
                ui.separator();
                if kui::menu_item(ui, t("Flip Horizontal", "Отразить по горизонтали"), "") {
                    self.shape_op(t("Flip", "Отражение"), |s| s.flip_h = !s.flip_h);
                }
                if kui::menu_item(ui, t("Flip Vertical", "Отразить по вертикали"), "") {
                    self.shape_op(t("Flip", "Отражение"), |s| s.flip_v = !s.flip_v);
                }
            });
            ui.menu_button(t("Slide Show", "Показ"), |ui| {
                if kui::menu_item(ui, t("From Beginning", "С начала"), "F5") {
                    self.start_show(0, ui.ctx());
                    ui.close_menu();
                }
                if kui::menu_item(ui, t("From Current Slide", "С текущего слайда"), "Shift+F5") {
                    self.start_show(self.cur, ui.ctx());
                    ui.close_menu();
                }
                let mut hidden = self.slide().hidden;
                if ui.checkbox(&mut hidden, t("Hide Slide", "Скрыть слайд")).changed() {
                    self.checkpoint(t("Hide Slide", "Скрытие слайда"));
                    self.slide_mut().hidden = hidden;
                }
            });
            ui.menu_button(t("Help", "Справка"), |ui| {
                kui::language_menu(ui);
                if kui::menu_item(ui, t("About PPT42", "О программе PPT42"), "") {
                    kui::about(APP, t("presentations and slide shows", "презентации и показ слайдов"));
                }
            });
        });
    }

    fn apply_theme(&mut self, th: Theme) {
        self.checkpoint(t("Theme", "Тема"));
        let old = self.pres.theme.clone();
        for s in &mut self.pres.slides {
            for sh in &mut s.shapes {
                let title = sh.placeholder == Some(Placeholder::Title);
                for p in &mut sh.text {
                    p.apply_all(&mut |f| {
                        if f.color == old.text || f.color == old.title {
                            f.color = if title { th.title } else { th.text };
                        }
                        if f.font == old.major {
                            f.font = th.major.clone();
                        } else if f.font == old.minor {
                            f.font = th.minor.clone();
                        }
                    });
                }
                if let Some(f) = sh.fill {
                    if let Some(i) = old.accents.iter().position(|a| *a == f) {
                        sh.fill = Some(th.accents[i]);
                    }
                }
            }
            if matches!(s.background, Some(Background::Solid(c)) if c == old.background) {
                s.background = None;
            }
        }
        self.pres.theme = th;
    }

    fn add_geom(&mut self, g: Geom) {
        let (w, h) = if matches!(g, Geom::Line | Geom::Arrow) { (200.0, 0.0) } else { (160.0, 120.0) };
        let mut s = Shape::new(Content::Shape(g), (self.pres.width - w) / 2.0, (self.pres.height - h) / 2.0, w, h.max(1.0));
        if matches!(g, Geom::Line | Geom::Arrow) {
            s.line = Some((self.pres.theme.text, 2.5));
        } else {
            s.fill = Some(self.pres.theme.accents[0]);
            s.line = Some((darken(self.pres.theme.accents[0]), 1.0));
            let mut f = self.pres.fmt(18.0, false);
            f.color = [255, 255, 255];
            let mut p = para("", f);
            p.align = Align2::Center;
            s.text = vec![p];
            s.anchor = Anchor::Middle;
        }
        self.insert_shape(s);
    }

    fn insert_picture(&mut self) {
        if let Some(p) = kui::open_dialog(t("Insert Picture", "Вставить рисунок"), &[(t("Images", "Изображения"), &["png", "jpg", "jpeg", "gif", "bmp", "webp"])]).pick_file() {
            if let Ok(d) = std::fs::read(&p) {
                self.insert_picture_bytes(d);
            }
        }
    }

    fn insert_table(&mut self, r: usize, c: usize) {
        let f = self.pres.fmt(16.0, false);
        let rows: Vec<Vec<Para>> = (0..r).map(|ri| (0..c).map(|ci| {
            let mut p = para(&if ri == 0 { format!("{} {}", t("Column", "Столбец"), ci + 1) } else { String::new() }, f.clone());
            if ri == 0 {
                p.apply_all(&mut |x| {
                    x.bold = true;
                    x.color = [255, 255, 255];
                });
            }
            p
        }).collect()).collect();
        let w = self.pres.width * 0.7;
        let s = Shape::new(Content::Table(rows), (self.pres.width - w) / 2.0, self.pres.height * 0.3, w, 40.0 * r as f32);
        self.insert_shape(s);
    }

    fn ribbon(&mut self, ui: &mut egui::Ui) {
        let f = self.cur_fmt();
        ui.horizontal_wrapped(|ui| {
            ui.menu_button(format!("{}  {}", ic::ADD, t("New Slide", "Новый слайд")), |ui| {
                for i in 0..LAYOUTS.len() {
                    if ui.button(layout_name(i)).clicked() {
                        self.new_slide(i);
                        ui.close_menu();
                    }
                }
            });
            if kui::tool_button(ui, ic::SAVE, t("Save", "Сохранить"), true).clicked() {
                self.save();
            }
            if kui::tool_button(ui, ic::UNDO, t("Undo", "Отменить"), self.history.can_undo()).clicked() {
                if let Some(p) = self.history.undo(self.pres.clone()) {
                    self.pres = p;
                    self.cur = self.cur.min(self.pres.slides.len() - 1);
                }
            }
            if kui::tool_button(ui, ic::REDO, t("Redo", "Повторить"), self.history.can_redo()).clicked() {
                if let Some(p) = self.history.redo(self.pres.clone()) {
                    self.pres = p;
                    self.cur = self.cur.min(self.pres.slides.len() - 1);
                }
            }
            ui.separator();
            if kui::tool_button(ui, ic::TEXT, t("Text Box", "Надпись"), true).clicked() {
                let fm = self.pres.fmt(20.0, false);
                let s = Shape::text_box(self.pres.width * 0.3, self.pres.height * 0.4, self.pres.width * 0.4, 60.0, t("Text", "Текст"), fm);
                self.insert_shape(s);
            }
            ui.menu_button(egui::RichText::new(ic::SHAPES).family(egui::FontFamily::Name(kit42::fonts::ICON_FAMILY.into())).size(16.0), |ui| {
                egui::Grid::new("shapes").show(ui, |ui| {
                    for (i, g) in Geom::SHAPES.iter().chain([Geom::Line, Geom::Arrow].iter()).enumerate() {
                        if ui.button(g.name()).clicked() {
                            self.add_geom(*g);
                            ui.close_menu();
                        }
                        if i % 3 == 2 {
                            ui.end_row();
                        }
                    }
                });
            })
            .response
            .on_hover_text(t("Shapes", "Фигуры"));
            if kui::tool_button(ui, ic::PICTURE, t("Picture", "Рисунок"), true).clicked() {
                self.insert_picture();
            }
            if kui::tool_button(ui, ic::TABLE, t("Table", "Таблица"), true).clicked() {
                self.insert_table(3, 3);
            }
            if kui::tool_button(ui, ic::SIGNATURE, t("Signature or Stamp", "Подпись или печать"), true).clicked() {
                self.picker.open = true;
            }
            ui.separator();
            if self.fonts_cache().is_empty() {}
            egui::ComboBox::from_id_salt("pfont").width(130.0).selected_text(&f.font).height(400.0).show_ui(ui, |ui| {
                for name in kit42::fonts::family_names().into_iter().take(400) {
                    if ui.selectable_label(f.font == name, &name).clicked() {
                        self.style_text(t("Font", "Шрифт"), move |x| x.font = name.clone());
                    }
                }
            });
            egui::ComboBox::from_id_salt("psize").width(50.0).selected_text(format!("{:.0}", f.size)).show_ui(ui, |ui| {
                for s in [10.0f32, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 36.0, 40.0, 44.0, 54.0, 66.0, 80.0, 96.0] {
                    if ui.selectable_label((f.size - s).abs() < 0.5, format!("{s}")).clicked() {
                        self.style_text(t("Size", "Размер"), move |x| x.size = s);
                    }
                }
            });
            if kui::tool_button(ui, "\u{E8E8}", t("Increase Font Size", "Увеличить шрифт"), true).clicked() {
                self.style_text(t("Size", "Размер"), |x| x.size = (x.size * 1.15).round());
            }
            if kui::tool_button(ui, "\u{E8E7}", t("Decrease Font Size", "Уменьшить шрифт"), true).clicked() {
                self.style_text(t("Size", "Размер"), |x| x.size = (x.size / 1.15).round().max(6.0));
            }
            if kui::tool_toggle(ui, f.bold, ic::BOLD, t("Bold", "Полужирный")).clicked() {
                let v = !f.bold;
                self.style_text(t("Bold", "Полужирный"), move |x| x.bold = v);
            }
            if kui::tool_toggle(ui, f.italic, ic::ITALIC, t("Italic", "Курсив")).clicked() {
                let v = !f.italic;
                self.style_text(t("Italic", "Курсив"), move |x| x.italic = v);
            }
            if kui::tool_toggle(ui, f.underline, ic::UNDERLINE, t("Underline", "Подчёркнутый")).clicked() {
                let v = !f.underline;
                self.style_text(t("Underline", "Подчёркнутый"), move |x| x.underline = v);
            }
            let mut col = f.color;
            if kui::color_button(ui, &mut col, t("Text Colour", "Цвет текста")) {
                self.style_text(t("Colour", "Цвет"), move |x| x.color = col);
            }
            for (a, icon, tip) in [(Align2::Left, ic::ALIGN_LEFT, t("Left", "Слева")), (Align2::Center, ic::ALIGN_CENTER, t("Centre", "По центру")), (Align2::Right, ic::ALIGN_RIGHT, t("Right", "Справа"))] {
                if kui::tool_button(ui, icon, tip, true).clicked() {
                    self.para_op(t("Align", "Выравнивание"), move |p| p.align = a);
                }
            }
            if kui::tool_button(ui, ic::LIST, t("Bullets", "Маркеры"), true).clicked() {
                self.para_op(t("Bullets", "Маркеры"), |p| p.list = if p.list.is_some() { None } else { Some(ListInfo { kind: ListKind::Bullet, level: 0, checked: false }) });
            }
            if kui::tool_button(ui, "\u{E9D5}", t("Numbering", "Нумерация"), true).clicked() {
                self.para_op(t("Numbering", "Нумерация"), |p| p.list = if matches!(&p.list, Some(l) if l.kind == ListKind::Number) { None } else { Some(ListInfo { kind: ListKind::Number, level: 0, checked: false }) });
            }
            ui.separator();
            if ui.add(egui::Button::new(format!("{}  {}", ic::SLIDESHOW, t("Play", "Показ")))).on_hover_text("F5").clicked() {
                self.start_show(self.cur, ui.ctx());
            }
        });
    }

    fn fonts_cache(&self) -> Vec<String> {
        vec![String::new()]
    }

    fn thumbnails(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let n = self.pres.slides.len();
        let w = (ui.available_width() - 30.0).max(80.0);
        let h = w * self.pres.height / self.pres.width;
        let mut rects = vec![];
        let mut clicked = None;
        let mut drop = None;
        let mut menu_action: Option<(usize, u8)> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for i in 0..n {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{}", i + 1)).small());
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click_and_drag());
                    rects.push(rect);
                    if ui.is_rect_visible(rect) {
                        let p = ui.painter_at(rect);
                        let slide = self.pres.slides[i].clone();
                        render::paint_slide(&p, &ctx, rect, &self.pres, &slide, &mut self.texes, &|_| 1.0, None);
                        let selected = i == self.cur || self.sel_slides.contains(&i);
                        ui.painter().rect_stroke(rect, 2.0, if selected { Stroke::new(3.0, kui::accent(&ctx)) } else { Stroke::new(1.0, Color32::from_gray(150)) }, egui::StrokeKind::Outside);
                        if slide.hidden {
                            ui.painter().rect_filled(rect, 0.0, Color32::from_white_alpha(140));
                            ui.painter().line_segment([rect.left_top(), rect.right_bottom()], Stroke::new(1.0, Color32::GRAY));
                        }
                    }
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                    if resp.drag_started() {
                        self.drag = Some(Drag::Thumb { from: i });
                    }
                    resp.context_menu(|ui| {
                        for (code, label) in [(0u8, t("New Slide", "Новый слайд")), (1, t("Duplicate", "Дублировать")), (2, t("Delete", "Удалить")), (3, t("Hide / Show", "Скрыть / показать"))] {
                            if ui.button(label).clicked() {
                                menu_action = Some((i, code));
                                ui.close_menu();
                            }
                        }
                    });
                });
                ui.add_space(4.0);
            }
            if let (Some(Drag::Thumb { from }), Some(pos)) = (&self.drag, ctx.pointer_interact_pos()) {
                let target = rects.iter().position(|r| pos.y < r.center().y).unwrap_or(n);
                let y = if target < n { rects[target].top() - 3.0 } else { rects.last().map(|r| r.bottom() + 3.0).unwrap_or(0.0) };
                ui.painter().hline(rects[0].x_range(), y, Stroke::new(3.0, kui::accent(&ctx)));
                if ctx.input(|i| i.pointer.any_released()) {
                    drop = Some((*from, target));
                }
            }
        });
        if let Some(i) = clicked {
            self.commit_text();
            if ctx.input(|inp| inp.modifiers.command) {
                if !self.sel_slides.insert(i) {
                    self.sel_slides.remove(&i);
                }
            } else {
                self.sel_slides.clear();
            }
            self.cur = i;
            self.sel.clear();
        }
        if let Some((from, to)) = drop {
            self.drag = None;
            if to != from && to != from + 1 {
                self.checkpoint(t("Move Slide", "Перемещение слайда"));
                let s = self.pres.slides.remove(from);
                let to = if to > from { to - 1 } else { to };
                self.pres.slides.insert(to, s);
                self.cur = to;
            }
        }
        if let Some((i, code)) = menu_action {
            self.cur = i;
            match code {
                0 => self.new_slide(1),
                1 => {
                    self.sel.clear();
                    self.duplicate_selected();
                }
                2 => self.delete_slides(),
                _ => {
                    self.checkpoint(t("Hide Slide", "Скрытие слайда"));
                    self.pres.slides[i].hidden = !self.pres.slides[i].hidden;
                }
            }
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let avail = ui.max_rect().shrink(24.0);
        let k = (avail.width() / self.pres.width).min(avail.height() / self.pres.height);
        let rect = Rect::from_center_size(avail.center(), Vec2::new(self.pres.width * k, self.pres.height * k));
        let to_screen = |x: f32, y: f32| Pos2::new(rect.left() + x * k, rect.top() + y * k);
        let to_slide = |p: Pos2| ((p.x - rect.left()) / k, (p.y - rect.top()) / k);
        let painter = ui.painter().clone();
        painter.rect_filled(rect.translate(Vec2::new(3.0, 4.0)), 0.0, Color32::from_black_alpha(50));
        let slide = self.slide().clone();
        render::paint_slide(&painter, &ctx, rect, &self.pres, &slide, &mut self.texes, &|_| 1.0, self.editing);
        // Empty placeholders show their hint.
        for sh in &slide.shapes {
            if sh.placeholder.is_some() && !sh.has_text() && self.editing != Some(sh.id) {
                let r = Rect::from_min_size(to_screen(sh.x, sh.y), Vec2::new(sh.w * k, sh.h * k));
                dashed(&painter, r, Color32::from_gray(160));
                painter.text(r.center(), egui::Align2::CENTER_CENTER, &sh.name, egui::FontId::proportional((sh.text.first().map(|p| p.runs[0].fmt.size).unwrap_or(24.0) * k * 0.8).max(8.0)), Color32::from_gray(150));
            }
        }
        // Guides.
        for (vertical, v) in &self.guides {
            if *vertical {
                painter.vline(rect.left() + v * k, rect.y_range(), Stroke::new(1.0, Color32::from_rgb(230, 60, 140)));
            } else {
                painter.hline(rect.x_range(), rect.top() + v * k, Stroke::new(1.0, Color32::from_rgb(230, 60, 140)));
            }
        }
        // Text editing in place.
        if let Some(eid) = self.editing {
            if let Some(sh) = slide.shapes.iter().find(|s| s.id == eid) {
                self.edit_text_ui(ui, sh.clone(), to_screen(sh.x, sh.y), k);
            }
        }
        // Selection handles.
        let acc = kui::accent(&ctx);
        let mut handle_rects = vec![];
        for id in &self.sel {
            if let Some(sh) = slide.shapes.iter().find(|s| s.id == *id) {
                let r = Rect::from_min_size(to_screen(sh.x, sh.y), Vec2::new(sh.w * k, sh.h * k));
                let c = r.center();
                let a = sh.rot.to_radians();
                let corners: Vec<Pos2> = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].iter().map(|p| rot(*p, c, a)).collect();
                painter.add(egui::Shape::closed_line(corners.clone(), Stroke::new(1.0, acc)));
                if self.sel.len() == 1 && self.editing.is_none() {
                    let hs = [r.left_top(), r.center_top(), r.right_top(), r.right_center(), r.right_bottom(), r.center_bottom(), r.left_bottom(), r.left_center()];
                    for (hi, h) in hs.iter().enumerate() {
                        let p = rot(*h, c, a);
                        painter.circle_filled(p, 5.0, Color32::WHITE);
                        painter.circle_stroke(p, 5.0, Stroke::new(1.0, acc));
                        handle_rects.push((sh.id, hi, p));
                    }
                    let rh = rot(r.center_top() - Vec2::new(0.0, 22.0), c, a);
                    painter.line_segment([rot(r.center_top(), c, a), rh], Stroke::new(1.0, acc));
                    painter.circle_filled(rh, 6.0, acc);
                    handle_rects.push((sh.id, 99, rh));
                }
            }
        }
        if let Some(Drag::Marquee { start }) = &self.drag {
            if let Some(p) = ctx.pointer_interact_pos() {
                let r = Rect::from_two_pos(*start, p);
                painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), 30));
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, acc), egui::StrokeKind::Middle);
            }
        }
        // Interaction.
        let resp = ui.interact(ui.max_rect(), egui::Id::new("slidecanvas"), Sense::click_and_drag());
        let pointer = resp.interact_pointer_pos();
        let hit = |p: Pos2| -> Option<u64> {
            let (x, y) = to_slide(p);
            slide.shapes.iter().rev().find(|s| {
                let (cx, cy) = s.center();
                let a = -s.rot.to_radians();
                let (dx, dy) = (x - cx, y - cy);
                let (lx, ly) = (dx * a.cos() - dy * a.sin(), dx * a.sin() + dy * a.cos());
                let pad = 4.0 / k;
                lx.abs() <= s.w / 2.0 + pad && ly.abs() <= s.h.max(8.0) / 2.0 + pad
            }).map(|s| s.id)
        };
        if resp.double_clicked() {
            if let Some(p) = pointer {
                if let Some(id) = hit(p) {
                    self.begin_text_edit(id);
                }
            }
        } else if resp.clicked() {
            if let Some(p) = pointer {
                let shift = ctx.input(|i| i.modifiers.shift || i.modifiers.command);
                match hit(p) {
                    Some(id) if self.editing == Some(id) => {}
                    Some(id) => {
                        self.commit_text();
                        if shift {
                            if let Some(i) = self.sel.iter().position(|x| *x == id) {
                                self.sel.remove(i);
                            } else {
                                self.sel.push(id);
                            }
                        } else {
                            self.sel = vec![id];
                        }
                    }
                    None => {
                        self.commit_text();
                        self.sel.clear();
                    }
                }
            }
        }
        if resp.drag_started() && self.editing.is_none() {
            if let Some(p) = pointer {
                if let Some((id, hi, _)) = handle_rects.iter().find(|(_, _, hp)| hp.distance(p) < 8.0).cloned() {
                    self.checkpoint(t("Resize", "Изменение размера"));
                    let s = slide.shapes.iter().find(|s| s.id == id).unwrap();
                    self.drag = Some(if hi == 99 { Drag::Rotate { id } } else { Drag::Resize { id, handle: hi, orig: (s.x, s.y, s.w, s.h), start: p } });
                } else if let Some(id) = hit(p) {
                    if !self.sel.contains(&id) {
                        self.sel = vec![id];
                    }
                    self.checkpoint(t("Move", "Перемещение"));
                    self.drag = Some(Drag::Move { ids: self.sel.clone(), last: p });
                } else {
                    self.drag = Some(Drag::Marquee { start: p });
                }
            }
        }
        if resp.dragged() {
            if let Some(p) = pointer {
                self.guides.clear();
                match &mut self.drag {
                    Some(Drag::Move { ids, last }) => {
                        let mut d = (p - *last) / k;
                        *last = p;
                        let ids = ids.clone();
                        // Snap the first shape's edges/centre to the slide and other shapes.
                        if let Some(first) = self.slide().shapes.iter().find(|s| ids.contains(&s.id)).cloned() {
                            let others: Vec<Shape> = self.slide().shapes.iter().filter(|s| !ids.contains(&s.id)).cloned().collect();
                            let (nx, ny) = (first.x + d.x, first.y + d.y);
                            let mut xs = vec![0.0, self.pres.width / 2.0, self.pres.width];
                            let mut ys = vec![0.0, self.pres.height / 2.0, self.pres.height];
                            for o in &others {
                                xs.extend([o.x, o.x + o.w / 2.0, o.x + o.w]);
                                ys.extend([o.y, o.y + o.h / 2.0, o.y + o.h]);
                            }
                            let tol = 5.0 / k;
                            for (edge, off) in [(nx, 0.0), (nx + first.w / 2.0, first.w / 2.0), (nx + first.w, first.w)] {
                                if let Some(g) = xs.iter().find(|g| (edge - **g).abs() < tol) {
                                    d.x += g - edge;
                                    self.guides.push((true, *g));
                                    let _ = off;
                                    break;
                                }
                            }
                            for (edge, _) in [(ny, 0.0), (ny + first.h / 2.0, first.h / 2.0), (ny + first.h, first.h)] {
                                if let Some(g) = ys.iter().find(|g| (edge - **g).abs() < tol) {
                                    d.y += g - edge;
                                    self.guides.push((false, *g));
                                    break;
                                }
                            }
                        }
                        if let Some(Drag::Move { last, .. }) = &mut self.drag {
                            let _ = last;
                        }
                        for s in self.slide_mut().shapes.iter_mut().filter(|s| ids.contains(&s.id)) {
                            s.x += d.x;
                            s.y += d.y;
                        }
                    }
                    Some(Drag::Resize { id, handle, orig, start }) => {
                        let d = (p - *start) / k;
                        let (id, handle, (ox, oy, ow, oh)) = (*id, *handle, *orig);
                        let keep = ctx.input(|i| i.modifiers.shift);
                        if let Some(s) = self.shape_mut(id) {
                            let a = -s.rot.to_radians();
                            let (dx, dy) = (d.x * a.cos() - d.y * a.sin(), d.x * a.sin() + d.y * a.cos());
                            let (mut x, mut y, mut w, mut h) = (ox, oy, ow, oh);
                            if [0, 6, 7].contains(&handle) {
                                x = ox + dx;
                                w = ow - dx;
                            }
                            if [2, 3, 4].contains(&handle) {
                                w = ow + dx;
                            }
                            if [0, 1, 2].contains(&handle) {
                                y = oy + dy;
                                h = oh - dy;
                            }
                            if [4, 5, 6].contains(&handle) {
                                h = oh + dy;
                            }
                            if (keep || matches!(s.content, Content::Picture(_))) && [0, 2, 4, 6].contains(&handle) && ow > 0.0 {
                                let ratio = oh / ow;
                                let nh = w * ratio;
                                if [0, 2].contains(&handle) {
                                    y = oy + oh - nh;
                                }
                                h = nh;
                            }
                            if s.is_line() {
                                s.x = x;
                                s.y = y;
                                s.w = w.max(1.0);
                                s.h = h.max(0.0);
                            } else if w > 4.0 && h > 4.0 {
                                s.x = x;
                                s.y = y;
                                s.w = w;
                                s.h = h;
                            }
                        }
                    }
                    Some(Drag::Rotate { id }) => {
                        let id = *id;
                        let (sx, sy) = to_slide(p);
                        let snap = !ctx.input(|i| i.modifiers.shift);
                        if let Some(s) = self.shape_mut(id) {
                            let (cx, cy) = s.center();
                            let mut deg = (sy - cy).atan2(sx - cx).to_degrees() + 90.0;
                            if snap {
                                let r = (deg / 15.0).round() * 15.0;
                                if (deg - r).abs() < 3.0 {
                                    deg = r;
                                }
                            }
                            s.rot = deg.rem_euclid(360.0);
                        }
                    }
                    _ => {}
                }
            }
        }
        if resp.drag_stopped() {
            if let Some(Drag::Marquee { start }) = self.drag.take() {
                if let Some(p) = pointer {
                    let (a, b) = (to_slide(start), to_slide(p));
                    let r = Rect::from_two_pos(Pos2::new(a.0, a.1), Pos2::new(b.0, b.1));
                    self.sel = slide.shapes.iter().filter(|s| r.contains(Pos2::new(s.x, s.y)) && r.contains(Pos2::new(s.x + s.w, s.y + s.h))).map(|s| s.id).collect();
                }
            }
            self.drag = None;
            self.guides.clear();
        }
        resp.context_menu(|ui| {
            if ui.button(t("Bring to Front", "На передний план")).clicked() {
                self.arrange(true);
                ui.close_menu();
            }
            if ui.button(t("Send to Back", "На задний план")).clicked() {
                self.arrange(false);
                ui.close_menu();
            }
            if ui.button(t("Duplicate", "Дублировать")).clicked() {
                self.duplicate_selected();
                ui.close_menu();
            }
            if ui.button(t("Delete", "Удалить")).clicked() {
                self.delete_selected();
                ui.close_menu();
            }
        });
    }

    fn begin_text_edit(&mut self, id: u64) {
        let Some(sh) = self.slide().shapes.iter().find(|s| s.id == id).cloned() else { return };
        if matches!(sh.content, Content::Picture(_)) || sh.is_line() {
            return;
        }
        self.checkpoint(t("Edit Text", "Правка текста"));
        self.sel = vec![id];
        if let Content::Table(_) = sh.content {
            self.editing = Some(id);
            self.edit_focus = Some((0, 0));
            return;
        }
        if sh.text.is_empty() {
            let f = self.pres.fmt(18.0, false);
            if let Some(s) = self.shape_mut(id) {
                let mut p = para("", f);
                p.align = Align2::Center;
                s.text.push(p);
                s.anchor = Anchor::Middle;
            }
        }
        self.editing = Some(id);
        let n = self.slide().shapes.iter().find(|s| s.id == id).map(|s| s.text.last().map(|p| p.char_len()).unwrap_or(0)).unwrap_or(0);
        let last = self.slide().shapes.iter().find(|s| s.id == id).map(|s| s.text.len().saturating_sub(1)).unwrap_or(0);
        self.edit_focus = Some((last, n));
    }

    /// Edits the paragraphs of a shape (or table cells) in place.
    fn edit_text_ui(&mut self, ui: &mut egui::Ui, sh: Shape, origin: Pos2, k: f32) {
        let ctx = ui.ctx().clone();
        let r = Rect::from_min_size(origin, Vec2::new(sh.w * k, sh.h * k));
        ui.painter().rect_filled(r, 0.0, Color32::from_white_alpha(18));
        if let Content::Table(rows) = &sh.content {
            let n_r = rows.len().max(1);
            let n_c = rows.first().map(|x| x.len()).unwrap_or(1).max(1);
            let (cw, rh) = (sh.w / n_c as f32 * k, sh.h / n_r as f32 * k);
            let mut changed: Option<(usize, usize, Para)> = None;
            for (ri, row) in rows.iter().enumerate() {
                for (ci, cell) in row.iter().enumerate() {
                    let cr = Rect::from_min_size(origin + Vec2::new(ci as f32 * cw, ri as f32 * rh), Vec2::new(cw, rh)).shrink(4.0);
                    let mut p = cell.clone();
                    let id = egui::Id::new(("cell", sh.id, ri, ci));
                    let out = ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cr), |ui| richtext::edit_para(ui, id, &mut p, cr.width(), k, &mut self.typing, false)).inner;
                    if out.changed || !out.new_paras.is_empty() {
                        for q in out.new_paras {
                            let n = p.char_len();
                            let f = p.fmt_at(n);
                            p.splice(n, 0, "\n", &f);
                            p.append(q);
                        }
                        changed = Some((ri, ci, p));
                    }
                }
            }
            if let Some((ri, ci, p)) = changed {
                if let Some(s) = self.shape_mut(sh.id) {
                    if let Content::Table(rows) = &mut s.content {
                        rows[ri][ci] = p;
                    }
                }
            }
            return;
        }
        let inset = Vec2::new(render::INSET.0 * k, render::INSET.1 * k);
        let inner = r.shrink2(inset);
        let total_h: f32 = sh.text.iter().map(|p| richtext::galley(&ctx, p, inner.width() - list_pad(p) * k, k).size().y + p.space_after.min(24.0) * k).sum();
        let mut y = match sh.anchor {
            Anchor::Top => inner.top(),
            Anchor::Middle => inner.center().y - total_h / 2.0,
            Anchor::Bottom => inner.bottom() - total_h,
        };
        let focus = self.edit_focus.take();
        let mut events: Vec<(usize, richtext::EditOutput, Para)> = vec![];
        let markers = richtext::list_numbers(&sh.text);
        for (i, p0) in sh.text.iter().enumerate() {
            let mut p = p0.clone();
            let pad = list_pad(&p) * k;
            let h = richtext::galley(&ctx, &p, inner.width() - pad, k).size().y;
            let pr = Rect::from_min_size(Pos2::new(inner.left() + pad, y), Vec2::new(inner.width() - pad, h.max(4.0)));
            if let Some(m) = &markers[i] {
                ui.painter().text(Pos2::new(pr.left() - 4.0, y), egui::Align2::RIGHT_TOP, m, egui::FontId::proportional(p.runs[0].fmt.size * k), render::c32(p.runs[0].fmt.color));
            }
            let id = egui::Id::new(("ptext", sh.id, i));
            if let Some((fi, pos)) = focus {
                if fi == i {
                    richtext::request_cursor(&ctx, id, pos, pos);
                }
            }
            let out = ui.allocate_new_ui(egui::UiBuilder::new().max_rect(pr), |ui| richtext::edit_para(ui, id, &mut p, pr.width(), k, &mut self.typing, false)).inner;
            if out.focused {
                self.edit_para = out.selection.map(|s| (i, s));
            }
            y += h + p.space_after.min(24.0) * k;
            events.push((i, out, p));
        }
        let mut new_focus = None;
        let Some(s) = self.shape_mut(sh.id) else { return };
        for (i, out, p) in events.into_iter().rev() {
            if out.changed || !out.new_paras.is_empty() {
                s.text[i] = p;
                let n = out.new_paras.len();
                for (k2, q) in out.new_paras.into_iter().enumerate() {
                    s.text.insert(i + 1 + k2, q);
                }
                if n > 0 {
                    new_focus = Some((i + n, out.cursor_after_split));
                }
            }
            if out.merge_with_prev && i > 0 {
                let pos = s.text[i - 1].char_len();
                let cur = s.text.remove(i);
                s.text[i - 1].append(cur);
                new_focus = Some((i - 1, pos));
            }
            if out.merge_with_next && i + 1 < s.text.len() {
                let pos = s.text[i].char_len();
                let next = s.text.remove(i + 1);
                s.text[i].append(next);
                new_focus = Some((i, pos));
            }
            if out.move_prev && i > 0 {
                new_focus = Some((i - 1, s.text[i - 1].char_len()));
            }
            if out.move_next && i + 1 < s.text.len() {
                new_focus = Some((i + 1, 0));
            }
        }
        if new_focus.is_some() {
            self.edit_focus = new_focus;
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let sel = self.sel.first().cloned();
            match sel.and_then(|id| self.slide().shapes.iter().find(|s| s.id == id).cloned()) {
                None => {
                    ui.heading(t("Slide", "Слайд"));
                    ui.separator();
                    ui.label(t("Background", "Фон"));
                    let bg = self.pres.background_of(self.slide());
                    let (mut c1, mut c2, kind) = match bg {
                        Background::Solid(c) => (c, c, 0),
                        Background::Gradient(a, b) => (a, b, 1),
                        Background::Picture(_) => ([255, 255, 255], [255, 255, 255], 2),
                    };
                    ui.horizontal(|ui| {
                        if kui::color_button(ui, &mut c1, t("Colour", "Цвет")) {
                            self.checkpoint(t("Background", "Фон"));
                            self.slide_mut().background = Some(if kind == 1 { Background::Gradient(c1, c2) } else { Background::Solid(c1) });
                        }
                        if kind == 1 && kui::color_button(ui, &mut c2, t("Second colour", "Второй цвет")) {
                            self.checkpoint(t("Background", "Фон"));
                            self.slide_mut().background = Some(Background::Gradient(c1, c2));
                        }
                        if ui.small_button(t("Gradient", "Градиент")).clicked() {
                            self.checkpoint(t("Background", "Фон"));
                            self.slide_mut().background = Some(Background::Gradient(c1, darken(c1)));
                        }
                        if ui.small_button(t("Picture…", "Рисунок…")).clicked() {
                            if let Some(p) = kui::open_dialog(t("Background Picture", "Фоновый рисунок"), &[(t("Images", "Изображения"), &["png", "jpg", "jpeg", "bmp", "webp"])]).pick_file() {
                                if let Ok(d) = std::fs::read(p) {
                                    self.checkpoint(t("Background", "Фон"));
                                    self.slide_mut().background = Some(Background::Picture(Arc::new(d)));
                                }
                            }
                        }
                    });
                    if ui.button(t("Apply to All Slides", "Применить ко всем слайдам")).clicked() {
                        self.checkpoint(t("Background", "Фон"));
                        let bg = self.slide().background.clone();
                        for s in &mut self.pres.slides {
                            s.background = bg.clone();
                        }
                    }
                    ui.separator();
                    ui.label(t("Transition", "Переход"));
                    let tr = self.slide().transition;
                    ui.horizontal_wrapped(|ui| {
                        for (v, l) in [(Transition::None, t("None", "Нет")), (Transition::Fade, t("Fade", "Растворение")), (Transition::Push, t("Push", "Сдвиг")), (Transition::Wipe, t("Wipe", "Появление"))] {
                            if ui.selectable_label(tr == v, l).clicked() {
                                self.checkpoint(t("Transition", "Переход"));
                                self.slide_mut().transition = v;
                            }
                        }
                    });
                    let mut hidden = self.slide().hidden;
                    if ui.checkbox(&mut hidden, t("Hidden slide", "Скрытый слайд")).changed() {
                        self.checkpoint(t("Hide Slide", "Скрытие слайда"));
                        self.slide_mut().hidden = hidden;
                    }
                    ui.separator();
                    ui.label(t("Theme", "Тема"));
                    ui.horizontal_wrapped(|ui| {
                        for th in themes() {
                            let (r, resp) = ui.allocate_exact_size(Vec2::new(64.0, 40.0), Sense::click());
                            ui.painter().rect_filled(r, 3.0, render::c32(th.background));
                            for (i, a) in th.accents.iter().take(4).enumerate() {
                                ui.painter().rect_filled(Rect::from_min_size(r.left_bottom() + Vec2::new(4.0 + i as f32 * 14.0, -10.0), Vec2::new(12.0, 6.0)), 1.0, render::c32(*a));
                            }
                            ui.painter().text(r.left_top() + Vec2::new(4.0, 3.0), egui::Align2::LEFT_TOP, "Aa", egui::FontId::proportional(13.0), render::c32(th.title));
                            ui.painter().rect_stroke(r, 3.0, Stroke::new(if th == self.pres.theme { 2.0 } else { 1.0 }, if th == self.pres.theme { kui::accent(ui.ctx()) } else { Color32::GRAY }), egui::StrokeKind::Inside);
                            if resp.on_hover_text(th.name).clicked() {
                                self.apply_theme(th);
                            }
                        }
                    });
                }
                Some(sh) => {
                    ui.heading(t("Shape", "Фигура"));
                    ui.separator();
                    egui::Grid::new("geom").num_columns(4).show(ui, |ui| {
                        let mut v = [sh.x, sh.y, sh.w, sh.h, sh.rot];
                        let mut changed = false;
                        for (i, l) in ["X", "Y", t("W", "Ш"), t("H", "В")].iter().enumerate() {
                            ui.label(*l);
                            changed |= ui.add(egui::DragValue::new(&mut v[i]).speed(1.0).max_decimals(1)).changed();
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                        ui.label(t("Angle", "Угол"));
                        changed |= ui.add(egui::DragValue::new(&mut v[4]).range(0.0..=359.9).suffix("°")).changed();
                        ui.end_row();
                        if changed && v.iter().all(|x| x.is_finite()) {
                            if self.history.undo_label() != Some(t("Geometry", "Геометрия")) {
                                self.checkpoint(t("Geometry", "Геометрия"));
                            }
                            if let Some(s) = self.shape_mut(sh.id) {
                                s.x = v[0];
                                s.y = v[1];
                                s.w = v[2].max(1.0);
                                s.h = v[3].max(0.0);
                                s.rot = v[4];
                            }
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(t("Fill", "Заливка"));
                        let mut f = sh.fill.unwrap_or([255, 255, 255]);
                        if kui::color_button(ui, &mut f, t("Fill", "Заливка")) {
                            self.shape_op(t("Fill", "Заливка"), move |s| s.fill = Some(f));
                        }
                        if ui.small_button(t("None", "Нет")).clicked() {
                            self.shape_op(t("Fill", "Заливка"), |s| s.fill = None);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(t("Line", "Контур"));
                        let (mut c, mut w) = sh.line.unwrap_or(([0, 0, 0], 1.0));
                        if kui::color_button(ui, &mut c, t("Line", "Контур")) {
                            self.shape_op(t("Line", "Контур"), move |s| s.line = Some((c, s.line.map(|l| l.1).unwrap_or(1.0))));
                        }
                        if ui.add(egui::DragValue::new(&mut w).range(0.25..=40.0).speed(0.25).suffix(" pt")).changed() {
                            self.shape_op(t("Line", "Контур"), move |s| s.line = Some((s.line.map(|l| l.0).unwrap_or([0, 0, 0]), w)));
                        }
                        if ui.small_button(t("None", "Нет")).clicked() {
                            self.shape_op(t("Line", "Контур"), |s| s.line = None);
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(t("Accent:", "Акцент:"));
                        for a in self.pres.theme.accents {
                            let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                            ui.painter().rect_filled(r, 3.0, render::c32(a));
                            if resp.clicked() {
                                self.shape_op(t("Fill", "Заливка"), move |s| {
                                    s.fill = Some(a);
                                    s.line = Some((darken(a), 1.0));
                                });
                            }
                        }
                    });
                    if let Content::Shape(g) = sh.content {
                        if !sh.is_line() {
                            egui::ComboBox::from_label(t("Shape", "Фигура")).selected_text(g.name()).show_ui(ui, |ui| {
                                for ng in Geom::SHAPES {
                                    if ui.selectable_label(ng == g, ng.name()).clicked() {
                                        self.shape_op(t("Change Shape", "Смена фигуры"), move |s| s.content = Content::Shape(ng));
                                    }
                                }
                            });
                        }
                    }
                    if let Content::Table(rows) = &sh.content {
                        ui.separator();
                        ui.label(format!("{}: {} × {}", t("Table", "Таблица"), rows.len(), rows.first().map(|r| r.len()).unwrap_or(0)));
                        ui.horizontal(|ui| {
                            if ui.small_button(t("+ Row", "+ Строка")).clicked() {
                                let f = self.pres.fmt(16.0, false);
                                self.shape_op(t("Table", "Таблица"), move |s| {
                                    if let Content::Table(r) = &mut s.content {
                                        let n = r.first().map(|x| x.len()).unwrap_or(1);
                                        r.push((0..n).map(|_| para("", f.clone())).collect());
                                        s.h += s.h / (r.len() - 1) as f32;
                                    }
                                });
                            }
                            if ui.small_button(t("+ Column", "+ Столбец")).clicked() {
                                let f = self.pres.fmt(16.0, false);
                                self.shape_op(t("Table", "Таблица"), move |s| {
                                    if let Content::Table(r) = &mut s.content {
                                        for row in r.iter_mut() {
                                            row.push(para("", f.clone()));
                                        }
                                    }
                                });
                            }
                            if ui.small_button(t("− Row", "− Строка")).clicked() {
                                self.shape_op(t("Table", "Таблица"), |s| {
                                    if let Content::Table(r) = &mut s.content {
                                        if r.len() > 1 {
                                            r.pop();
                                            s.h -= s.h / (r.len() + 1) as f32;
                                        }
                                    }
                                });
                            }
                            if ui.small_button(t("− Column", "− Столбец")).clicked() {
                                self.shape_op(t("Table", "Таблица"), |s| {
                                    if let Content::Table(r) = &mut s.content {
                                        if r.first().map(|x| x.len()).unwrap_or(0) > 1 {
                                            for row in r.iter_mut() {
                                                row.pop();
                                            }
                                        }
                                    }
                                });
                            }
                        });
                    }
                    ui.separator();
                    ui.label(t("Text", "Текст"));
                    ui.horizontal(|ui| {
                        for (a, l) in [(Anchor::Top, t("Top", "Верх")), (Anchor::Middle, t("Middle", "Середина")), (Anchor::Bottom, t("Bottom", "Низ"))] {
                            if ui.selectable_label(sh.anchor == a, l).clicked() {
                                self.shape_op(t("Text Anchor", "Привязка текста"), move |s| s.anchor = a);
                            }
                        }
                    });
                    ui.separator();
                    ui.label(t("Animation", "Анимация"));
                    ui.horizontal_wrapped(|ui| {
                        if ui.selectable_label(sh.anim.is_none(), t("None", "Нет")).clicked() {
                            self.shape_op(t("Animation", "Анимация"), |s| s.anim = None);
                        }
                        for a in Anim::ALL {
                            if ui.selectable_label(sh.anim == Some(a), a.name()).clicked() {
                                self.shape_op(t("Animation", "Анимация"), move |s| s.anim = Some(a));
                            }
                        }
                    });
                    let order: Vec<u64> = self.slide().shapes.iter().filter(|s| s.anim.is_some()).map(|s| s.id).collect();
                    if let Some(pos) = order.iter().position(|x| *x == sh.id) {
                        ui.label(format!("{} {}", t("Click order:", "Порядок по щелчку:"), pos + 1));
                    }
                }
            }
        });
    }

    fn sorter(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let w = 220.0;
        let h = w * self.pres.height / self.pres.width;
        let mut clicked = None;
        let mut rects = vec![];
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for i in 0..self.pres.slides.len() {
                    ui.vertical(|ui| {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click_and_drag());
                        rects.push(rect);
                        let slide = self.pres.slides[i].clone();
                        render::paint_slide(&ui.painter_at(rect), &ctx, rect, &self.pres, &slide, &mut self.texes, &|_| 1.0, None);
                        ui.painter().rect_stroke(rect, 2.0, if i == self.cur { Stroke::new(3.0, kui::accent(&ctx)) } else { Stroke::new(1.0, Color32::GRAY) }, egui::StrokeKind::Outside);
                        ui.label(format!("{}{}", i + 1, if slide.hidden { t(" (hidden)", " (скрыт)") } else { "" }));
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        if resp.double_clicked() {
                            self.view = View::Normal;
                        }
                        if resp.drag_started() {
                            self.drag = Some(Drag::Thumb { from: i });
                        }
                    });
                }
            });
        });
        if let Some(i) = clicked {
            self.cur = i;
        }
        if let (Some(Drag::Thumb { from }), Some(pos)) = (&self.drag, ctx.pointer_interact_pos()) {
            if ctx.input(|i| i.pointer.any_released()) {
                let from = *from;
                let to = rects.iter().position(|r| r.contains(pos)).unwrap_or(from);
                self.drag = None;
                if to != from {
                    self.checkpoint(t("Move Slide", "Перемещение слайда"));
                    let s = self.pres.slides.remove(from);
                    self.pres.slides.insert(to, s);
                    self.cur = to;
                }
            }
        }
    }

    fn outline(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for i in 0..self.pres.slides.len() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{}", i + 1)).strong());
                    let title_id = self.pres.slides[i].shapes.iter().find(|s| s.placeholder == Some(Placeholder::Title)).map(|s| s.id);
                    let mut title = self.pres.slides[i].title();
                    let r = ui.add(egui::TextEdit::singleline(&mut title).font(egui::TextStyle::Heading).desired_width(600.0).hint_text(t("Title", "Заголовок")));
                    if r.changed() {
                        let f = self.pres.fmt(40.0, true);
                        let s = &mut self.pres.slides[i];
                        match title_id.and_then(|id| s.shapes.iter_mut().find(|x| x.id == id)) {
                            Some(sh) => {
                                let fmt = sh.text.first().map(|p| p.fmt_at(1)).unwrap_or(f);
                                let mut p = para(&title, fmt);
                                p.align = sh.text.first().map(|p| p.align).unwrap_or(Align2::Left);
                                sh.text = vec![p];
                            }
                            None => {
                                let mut sh = Shape::text_box(48.0, 30.0, 864.0, 90.0, &title, f);
                                sh.placeholder = Some(Placeholder::Title);
                                s.shapes.push(sh);
                            }
                        }
                    }
                });
                for sh in self.pres.slides[i].shapes.clone().iter().filter(|s| s.placeholder != Some(Placeholder::Title) && s.has_text()) {
                    ui.horizontal(|ui| {
                        ui.add_space(30.0);
                        let mut body = sh.plain_text();
                        if ui.add(egui::TextEdit::multiline(&mut body).desired_width(600.0).desired_rows(1)).changed() {
                            let fmt = sh.text.first().map(|p| p.fmt_at(1)).unwrap_or_else(|| self.pres.fmt(20.0, false));
                            let list = sh.text.first().and_then(|p| p.list.clone());
                            if let Some(s) = self.pres.slides[i].shapes.iter_mut().find(|x| x.id == sh.id) {
                                s.text = body.split('\n').map(|l| {
                                    let mut p = para(l, fmt.clone());
                                    p.list = list.clone();
                                    p
                                }).collect();
                            }
                        }
                    });
                }
                ui.separator();
            }
        });
    }

    fn find_window(&mut self, ctx: &Context) {
        let Some((mut f, mut r)) = self.find.take() else { return };
        let mut open = true;
        egui::Window::new(t("Find and Replace", "Найти и заменить")).open(&mut open).collapsible(false).show(ctx, |ui| {
            egui::Grid::new("fr").show(ui, |ui| {
                ui.label(t("Find:", "Найти:"));
                ui.text_edit_singleline(&mut f);
                ui.end_row();
                ui.label(t("Replace with:", "Заменить на:"));
                ui.text_edit_singleline(&mut r);
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button(t("Find Next", "Найти далее")).clicked() && !f.is_empty() {
                    let n = self.pres.slides.len();
                    let q = f.to_lowercase();
                    for k in 1..=n {
                        let i = (self.cur + k) % n;
                        let hit = self.pres.slides[i].shapes.iter().find(|s| s.plain_text().to_lowercase().contains(&q) || self.pres.slides[i].notes.to_lowercase().contains(&q));
                        if let Some(sh) = hit {
                            self.cur = i;
                            self.sel = vec![sh.id];
                            break;
                        }
                    }
                }
                if ui.button(t("Replace All", "Заменить все")).clicked() && !f.is_empty() {
                    self.checkpoint(t("Replace All", "Заменить все"));
                    let mut count = 0;
                    for s in &mut self.pres.slides {
                        for sh in &mut s.shapes {
                            for p in &mut sh.text {
                                for run in &mut p.runs {
                                    count += run.text.matches(&f).count();
                                    run.text = run.text.replace(&f, &r);
                                }
                            }
                            if let Content::Table(rows) = &mut sh.content {
                                for row in rows {
                                    for c in row {
                                        for run in &mut c.runs {
                                            count += run.text.matches(&f).count();
                                            run.text = run.text.replace(&f, &r);
                                        }
                                    }
                                }
                            }
                        }
                        count += s.notes.matches(&f).count();
                        s.notes = s.notes.replace(&f, &r);
                    }
                    self.status = format!("{} {count}", t("Replacements:", "Замен:"));
                }
            });
        });
        if open {
            self.find = Some((f, r));
        }
    }

    // ───────────────────────── slide show ─────────────────────────

    fn show_ui(&mut self, ctx: &Context) {
        let now = ctx.input(|i| i.time);
        let n = self.pres.slides.len();
        let mut end = false;
        {
            let Some(sh) = &mut self.show else { return };
            let anim_count = self.pres.slides[sh.index].shapes.iter().filter(|s| s.anim.is_some()).count();
            let next = |sh: &mut Show, slides: &[Slide], now: f64| {
                if sh.step < anim_count {
                    sh.step += 1;
                    sh.anim_started = now;
                    return false;
                }
                let mut i = sh.index + 1;
                while i < slides.len() && slides[i].hidden {
                    i += 1;
                }
                if i >= slides.len() {
                    return true;
                }
                sh.prev = Some(sh.index);
                sh.index = i;
                sh.step = 0;
                sh.started = now;
                sh.strokes.clear();
                false
            };
            let prev = |sh: &mut Show, now: f64| {
                if sh.step > 0 {
                    sh.step -= 1;
                    return;
                }
                if sh.index > 0 {
                    sh.prev = None;
                    sh.index -= 1;
                    sh.step = 0;
                    sh.started = now;
                    sh.strokes.clear();
                }
            };
            let events = ctx.input(|i| i.events.clone());
            for e in events {
                if let egui::Event::Key { key, pressed: true, .. } = e {
                    match key {
                        Key::Escape => end = true,
                        Key::ArrowRight | Key::ArrowDown | Key::Space | Key::PageDown | Key::N => {
                            if sh.overview {
                                sh.index = (sh.index + 1).min(n - 1);
                            } else if next(sh, &self.pres.slides, now) {
                                end = true;
                            }
                        }
                        Key::Enter => {
                            if let Ok(k) = sh.number.parse::<usize>() {
                                sh.index = k.clamp(1, n) - 1;
                                sh.step = 0;
                                sh.started = now;
                                sh.prev = None;
                            } else if sh.overview {
                                sh.overview = false;
                                sh.started = now;
                            } else if next(sh, &self.pres.slides, now) {
                                end = true;
                            }
                            sh.number.clear();
                        }
                        Key::ArrowLeft | Key::ArrowUp | Key::Backspace | Key::PageUp | Key::P if !sh.pen || key != Key::P => {
                            if sh.overview {
                                sh.index = sh.index.saturating_sub(1);
                            } else {
                                prev(sh, now);
                            }
                        }
                        Key::B => sh.screen = if sh.screen == Some(Color32::BLACK) { None } else { Some(Color32::BLACK) },
                        Key::W => sh.screen = if sh.screen == Some(Color32::WHITE) { None } else { Some(Color32::WHITE) },
                        Key::L => sh.laser = !sh.laser,
                        Key::E => sh.strokes.clear(),
                        Key::G => sh.overview = !sh.overview,
                        Key::Home => {
                            sh.index = 0;
                            sh.step = 0;
                        }
                        Key::End => {
                            sh.index = n - 1;
                            sh.step = 0;
                        }
                        k => {
                            let name = k.name();
                            if name.len() == 1 && name.chars().all(|c| c.is_ascii_digit()) {
                                sh.number.push_str(name);
                            }
                        }
                    }
                }
                if let egui::Event::Text(t) = &e {
                    if t == "p" || t == "з" || t == "P" {
                        sh.pen = !sh.pen;
                    }
                }
            }
        }
        if end {
            self.end_show(ctx);
            return;
        }
        let Some(sh) = &self.show else { return };
        let (index, step, started, anim_started, prev, overview, laser, pen, screen) = (sh.index, sh.step, sh.started, sh.anim_started, sh.prev, sh.overview, sh.laser, sh.pen, sh.screen);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ctx, |ui| {
            let full = ui.max_rect();
            let k = (full.width() / self.pres.width).min(full.height() / self.pres.height);
            let rect = Rect::from_center_size(full.center(), Vec2::new(self.pres.width * k, self.pres.height * k));
            let painter = ui.painter().clone();
            if overview {
                let cols = 4;
                let w = full.width() / cols as f32 - 20.0;
                let h = w * self.pres.height / self.pres.width;
                for (i, s) in self.pres.slides.iter().enumerate() {
                    let r = Rect::from_min_size(full.min + Vec2::new(10.0 + (i % cols) as f32 * (w + 20.0), 10.0 + (i / cols) as f32 * (h + 20.0)), Vec2::new(w, h));
                    render::paint_slide(&painter, &ctx, r, &self.pres, s, &mut self.texes, &|_| 1.0, None);
                    if i == index {
                        painter.rect_stroke(r, 0.0, Stroke::new(4.0, Color32::from_rgb(255, 200, 0)), egui::StrokeKind::Outside);
                    }
                    if ui.interact(r, egui::Id::new(("ov", i)), Sense::click()).clicked() {
                        if let Some(s) = &mut self.show {
                            s.index = i;
                            s.overview = false;
                            s.step = 0;
                        }
                    }
                }
                return;
            }
            let slide = self.pres.slides[index].clone();
            let anim_ids: Vec<u64> = slide.shapes.iter().filter(|s| s.anim.is_some()).map(|s| s.id).collect();
            let anim_t = ((now - anim_started) / 0.5).clamp(0.0, 1.0) as f32;
            let vis = |s: &Shape| -> f32 {
                match anim_ids.iter().position(|x| *x == s.id) {
                    None => 1.0,
                    Some(p) if p + 1 < step => 1.0,
                    Some(p) if p + 1 == step => match s.anim {
                        Some(Anim::Appear) => 1.0,
                        _ => anim_t,
                    },
                    _ => 0.0,
                }
            };
            let tt = ((now - started) / 0.6).clamp(0.0, 1.0) as f32;
            match (slide.transition, prev) {
                (Transition::Fade, Some(p)) if tt < 1.0 => {
                    let ps = self.pres.slides[p].clone();
                    render::paint_slide(&painter, &ctx, rect, &self.pres, &ps, &mut self.texes, &|_| 1.0, None);
                    render::paint_slide(&painter, &ctx, rect, &self.pres, &slide, &mut self.texes, &|s| vis(s) * tt, None);
                }
                (Transition::Push, Some(p)) if tt < 1.0 => {
                    let ps = self.pres.slides[p].clone();
                    let off = rect.height() * tt;
                    render::paint_slide(&painter.with_clip_rect(rect), &ctx, rect.translate(Vec2::new(0.0, -off)), &self.pres, &ps, &mut self.texes, &|_| 1.0, None);
                    render::paint_slide(&painter.with_clip_rect(rect), &ctx, rect.translate(Vec2::new(0.0, rect.height() - off)), &self.pres, &slide, &mut self.texes, &vis, None);
                }
                (Transition::Wipe, Some(p)) if tt < 1.0 => {
                    let ps = self.pres.slides[p].clone();
                    render::paint_slide(&painter, &ctx, rect, &self.pres, &ps, &mut self.texes, &|_| 1.0, None);
                    let clip = Rect::from_min_max(rect.min, Pos2::new(rect.left() + rect.width() * tt, rect.bottom()));
                    render::paint_slide(&painter.with_clip_rect(clip), &ctx, rect, &self.pres, &slide, &mut self.texes, &vis, None);
                }
                _ => render::paint_slide(&painter, &ctx, rect, &self.pres, &slide, &mut self.texes, &vis, None),
            }
            // Fly-in animations: draw moving copies.
            if tt < 1.0 || anim_t < 1.0 {
                ctx.request_repaint();
            }
            if let Some(c) = screen {
                painter.rect_filled(full, 0.0, c);
            }
            let resp = ui.interact(full, egui::Id::new("show"), Sense::click_and_drag());
            if let Some(s) = &mut self.show {
                if pen {
                    if resp.drag_started() {
                        s.strokes.push(vec![]);
                    }
                    if let (true, Some(p)) = (resp.dragged(), resp.interact_pointer_pos()) {
                        if let Some(st) = s.strokes.last_mut() {
                            st.push(p);
                        }
                    }
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                } else if resp.clicked() {
                    let anim_count = slide.shapes.iter().filter(|x| x.anim.is_some()).count();
                    if s.step < anim_count {
                        s.step += 1;
                        s.anim_started = now;
                    } else {
                        let mut i = s.index + 1;
                        while i < n && self.pres.slides[i].hidden {
                            i += 1;
                        }
                        if i >= n {
                            end = true;
                        } else {
                            s.prev = Some(s.index);
                            s.index = i;
                            s.step = 0;
                            s.started = now;
                            s.strokes.clear();
                        }
                    }
                }
                if resp.secondary_clicked() && s.index > 0 {
                    s.index -= 1;
                    s.step = 0;
                    s.prev = None;
                }
                for st in &s.strokes {
                    painter.add(egui::Shape::line(st.clone(), Stroke::new(4.0, Color32::from_rgb(230, 30, 30))));
                }
                if laser {
                    if let Some(p) = ctx.pointer_hover_pos() {
                        painter.circle_filled(p, 9.0, Color32::from_rgba_unmultiplied(255, 0, 0, 180));
                        painter.circle_filled(p, 4.0, Color32::from_rgb(255, 120, 120));
                        ctx.set_cursor_icon(egui::CursorIcon::None);
                        ctx.request_repaint();
                    }
                }
                if !s.number.is_empty() {
                    painter.text(full.left_bottom() + Vec2::new(20.0, -20.0), egui::Align2::LEFT_BOTTOM, &s.number, egui::FontId::proportional(28.0), Color32::WHITE);
                }
            }
        });
        if end {
            self.end_show(ctx);
        }
    }
}

fn list_pad(p: &Para) -> f32 {
    p.list.as_ref().map(|l| 22.0 + l.level as f32 * 22.0).unwrap_or(0.0) + p.indent_left
}

fn rot(p: Pos2, c: Pos2, a: f32) -> Pos2 {
    let (s, co) = a.sin_cos();
    let d = p - c;
    c + Vec2::new(d.x * co - d.y * s, d.x * s + d.y * co)
}

fn darken(c: [u8; 3]) -> [u8; 3] {
    c.map(|x| (x as f32 * 0.75) as u8)
}

fn dashed(p: &egui::Painter, r: Rect, c: Color32) {
    let st = Stroke::new(1.0, c);
    for (a, b) in [(r.left_top(), r.right_top()), (r.right_top(), r.right_bottom()), (r.right_bottom(), r.left_bottom()), (r.left_bottom(), r.left_top())] {
        let len = a.distance(b);
        let n = (len / 8.0) as i32;
        for i in (0..n).step_by(2) {
            let t0 = i as f32 / n as f32;
            let t1 = ((i + 1) as f32 / n as f32).min(1.0);
            p.line_segment([a + (b - a) * t0, a + (b - a) * t1], st);
        }
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a.starts_with("--")).unwrap_or(false) {
        kit42::attach_console();
        kit42::settings::load_global();
        let res: Result<String, String> = (|| match args[0].as_str() {
            "--demo" => {
                let out = args.get(1).ok_or("usage: --demo out.pptx")?;
                let mut p = Presentation::default();
                p.slides.clear();
                let mut s0 = p.layout_slide(0);
                s0.shapes[0].text = vec![{ let mut x = para("Suite42 для Windows", p.fmt(54.0, true)); x.align = Align2::Center; x }];
                s0.shapes[1].text = vec![{ let mut x = para("Шесть приложений на Rust", p.fmt(24.0, false)); x.align = Align2::Center; x }];
                p.slides.push(s0);
                let mut s1 = p.layout_slide(1);
                s1.shapes[0].text = vec![para("Что внутри", p.fmt(40.0, true))];
                s1.shapes[1].text = ["PDF42 — PDF", "DOC42 — документы", "XLS42 — таблицы", "IMG42 — изображения", "PPT42 — презентации", "SCN42 — сканирование"].iter().map(|t| {
                    let mut x = para(t, p.fmt(24.0, false));
                    x.list = Some(ListInfo { kind: ListKind::Bullet, level: 0, checked: false });
                    x
                }).collect();
                s1.transition = Transition::Fade;
                s1.notes = "Заметки докладчика".into();
                p.slides.push(s1);
                let mut s2 = p.layout_slide(4);
                s2.shapes[0].text = vec![para("Фигуры и таблицы", p.fmt(40.0, true))];
                for (i, g) in [Geom::RoundRect, Geom::Ellipse, Geom::RightArrow, Geom::Star5].iter().enumerate() {
                    let mut sh = Shape::new(Content::Shape(*g), 60.0 + i as f32 * 220.0, 170.0, 180.0, 120.0);
                    sh.fill = Some(p.theme.accents[i]);
                    sh.anim = Some(Anim::Fade);
                    let mut f = p.fmt(18.0, false);
                    f.color = [255, 255, 255];
                    let mut x = para(g.name(), f);
                    x.align = Align2::Center;
                    sh.text = vec![x];
                    sh.anchor = Anchor::Middle;
                    if i == 2 {
                        sh.rot = 15.0;
                    }
                    s2.shapes.push(sh);
                }
                let f = p.fmt(16.0, false);
                let rows = vec![
                    vec![para("Приложение", CharFmt { bold: true, color: [255, 255, 255], ..f.clone() }), para("Форматы", CharFmt { bold: true, color: [255, 255, 255], ..f.clone() })],
                    vec![para("DOC42", f.clone()), para("DOCX, ODT, RTF, MD", f.clone())],
                    vec![para("XLS42", f.clone()), para("XLSX, XLS, ODS, CSV", f.clone())],
                ];
                s2.shapes.push(Shape::new(Content::Table(rows), 180.0, 340.0, 600.0, 150.0));
                p.slides.push(s2);
                let path = PathBuf::from(out);
                let b = match kit42::ext(&path).as_str() {
                    "odp" => odp::write(&p, false)?,
                    _ => pptx::write(&p, false)?,
                };
                kit42::write_atomic(&path, &b).map_err(|e| e.to_string())?;
                Ok(format!("demo -> {out}"))
            }
            "--info" => {
                let p = App::load(std::path::Path::new(args.get(1).ok_or("usage")?))?;
                let mut s = format!("{} slides {}x{}\n", p.slides.len(), p.width, p.height);
                for (i, sl) in p.slides.iter().enumerate() {
                    s.push_str(&format!("{}: {} shapes, title \"{}\", notes {} chars\n", i + 1, sl.shapes.len(), sl.title(), sl.notes.len()));
                }
                Ok(s)
            }
            "--export" | "--convert" => {
                let (i, o) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?);
                let p = App::load(std::path::Path::new(i))?;
                let out = PathBuf::from(o);
                if out.is_dir() || o.ends_with('\\') || o.ends_with('/') {
                    std::fs::create_dir_all(&out).ok();
                    for (k, s) in p.slides.iter().enumerate() {
                        render::raster(&p, s, 1600).save(out.join(format!("slide{:02}.png", k + 1))).map_err(|e| e.to_string())?;
                    }
                    return Ok(format!("{} images", p.slides.len()));
                }
                let b = match kit42::ext(&out).as_str() {
                    "pdf" => render::pdf(&p, args.iter().any(|a| a == "--hidden"), args.iter().any(|a| a == "--notes")),
                    "odp" => odp::write(&p, false)?,
                    "otp" => odp::write(&p, true)?,
                    "potx" => pptx::write(&p, true)?,
                    "md" => outline_md(&p).into_bytes(),
                    "png" => {
                        let img = render::raster(&p, &p.slides[0], 1600);
                        let mut v = Vec::new();
                        image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png).map_err(|e| e.to_string())?;
                        v
                    }
                    _ => pptx::write(&p, false)?,
                };
                kit42::write_atomic(&out, &b).map_err(|e| e.to_string())?;
                Ok(format!("{} slides -> {o}", p.slides.len()))
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
    kit42::run(kit42::AppSpec { id: "ppt42", title: APP, size: [1320.0, 860.0] }, App::new)
}
