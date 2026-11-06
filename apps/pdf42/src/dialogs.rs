//! Dialogs of PDF42: watermark, page numbers, headers/footers with Bates numbering, crop,
//! page size, split, merge, properties, password, compression, export, compare, OCR.

use crate::markup::Kind;
use crate::pdfops;
use crate::{App, APP};
use egui::Context;
use kit42::pdfgen::{self, TextStyle};
use kit42::t;
use kit42::ui as kui;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    Watermark,
    PageNumbers,
    HeaderFooter,
    Crop,
    PageSize,
    Split,
    Merge,
    Properties,
    Protect,
    Compress,
    ExportImages,
    Compare,
    GoTo,
    BookmarksFromText,
    Attach,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFormat {
    Docx,
    Rtf,
    Html,
    Txt,
}

pub enum Dialog {
    Password { path: PathBuf, bytes: Vec<u8>, input: String, error: bool },
    Watermark { text: String, size: f32, color: [u8; 3], opacity: f32, angle: f32, pages: String, image: Option<PathBuf>, behind: bool },
    PageNumbers { format: usize, position: usize, start: u32, skip_first: bool, size: f32, pages: String },
    HeaderFooter { slots: [String; 6], prefix: String, suffix: String, digits: usize, bates_start: u32, size: f32, color: [u8; 3], margin: f32, pages: String },
    Crop { margins: [f32; 4], pages: String, remove: bool },
    PageSize { preset: usize, w: f32, h: f32, landscape: bool, scale: bool, pages: String },
    Split { mode: usize, n: u32, ranges: String },
    Merge { files: Vec<PathBuf> },
    Properties { info: pdfops::Info, details: Vec<(String, String)> },
    Protect { user: String, owner: String, print: bool, copy: bool, modify: bool },
    Compress { results: Option<std::sync::mpsc::Receiver<Vec<(usize, Vec<u8>)>>>, sizes: Vec<(usize, Vec<u8>)>, choice: usize, original: usize },
    ExportImages { png: bool, dpi: u32, quality: u8, pages: String },
    Compare { other: Option<PathBuf>, results: Vec<(usize, String, String)> },
    GoTo { input: String },
    Ocr { langs: Vec<(String, String)>, lang: usize, skip_text: bool },
    Note { id: u64, text: String },
    Link { id: u64, text: String },
    BookmarksFromText { levels: usize },
    Attach { files: Vec<PathBuf>, description: String },
}

const POSITIONS: [&str; 6] = ["↖", "↑", "↗", "↙", "↓", "↘"];

impl Dialog {
    pub fn password_for(path: PathBuf, bytes: Vec<u8>) -> Self {
        Dialog::Password { path, bytes, input: String::new(), error: false }
    }
    pub fn note(id: u64, text: String) -> Self {
        Dialog::Note { id, text }
    }
    pub fn link(id: u64, text: String) -> Self {
        Dialog::Link { id, text }
    }
    pub fn ocr() -> Self {
        let langs = kit42::winrt::ocr_languages();
        let lang = langs.iter().position(|(tag, _)| tag.starts_with(kit42::lang().code())).unwrap_or(0);
        Dialog::Ocr { langs, lang, skip_text: true }
    }
    pub fn crop_from_rect(_page: usize, r: [f32; 4], size: (f32, f32)) -> Self {
        let mm = |v: f32| (v * 25.4 / 72.0 * 10.0).round() / 10.0;
        Dialog::Crop { margins: [mm(r[0]), mm(r[1]), mm(size.0 - r[2]), mm(size.1 - r[3])], pages: String::new(), remove: false }
    }
}

impl App {
    pub fn open_dialog(&mut self, k: DialogKind) {
        let n = self.doc.as_ref().map(|d| d.page_count()).unwrap_or(0);
        let has = self.doc.is_some();
        if !has && !matches!(k, DialogKind::Merge) {
            return;
        }
        self.dialog = Some(match k {
            DialogKind::Watermark => Dialog::Watermark {
                text: t("CONFIDENTIAL", "КОНФИДЕНЦИАЛЬНО").into(),
                size: 60.0,
                color: [200, 0, 0],
                opacity: 0.25,
                angle: 45.0,
                pages: String::new(),
                image: None,
                behind: false,
            },
            DialogKind::PageNumbers => Dialog::PageNumbers { format: 0, position: 4, start: 1, skip_first: false, size: 10.0, pages: String::new() },
            DialogKind::HeaderFooter => Dialog::HeaderFooter {
                slots: [String::new(), String::new(), String::new(), String::new(), "<<page>> / <<pages>>".into(), String::new()],
                prefix: "DOC".into(),
                suffix: String::new(),
                digits: 6,
                bates_start: 1,
                size: 9.0,
                color: [0, 0, 0],
                margin: 10.0,
                pages: String::new(),
            },
            DialogKind::Crop => Dialog::Crop { margins: [10.0; 4], pages: format!("{}", self.current + 1), remove: false },
            DialogKind::PageSize => Dialog::PageSize { preset: 1, w: 210.0, h: 297.0, landscape: false, scale: true, pages: String::new() },
            DialogKind::Split => Dialog::Split { mode: 0, n: 1.max(n as u32 / 2), ranges: format!("1-{}, {}-{}", (n / 2).max(1), (n / 2 + 1).min(n), n) },
            DialogKind::Merge => {
                let mut files = vec![];
                if let Some(d) = &self.doc {
                    if let Some(p) = &d.path {
                        files.push(p.clone());
                    }
                }
                Dialog::Merge { files }
            }
            DialogKind::Properties => {
                let d = self.doc.as_ref().unwrap();
                let mut details = vec![];
                details.push((t("File", "Файл").to_string(), d.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default()));
                details.push((t("File size", "Размер файла").to_string(), format!("{:.1} KB", d.bytes.len() as f32 / 1024.0)));
                details.push((t("Pages", "Страниц").to_string(), d.page_count().to_string()));
                if let Some((w, h)) = d.sizes.first() {
                    details.push((t("Page size", "Размер страницы").to_string(), format!("{:.0} × {:.0} mm ({:.0} × {:.0} pt)", w * 25.4 / 72.0, h * 25.4 / 72.0, w, h)));
                }
                if let Ok(doc) = d.lopdf() {
                    details.push((t("PDF version", "Версия PDF").to_string(), doc.version.clone()));
                    let mut fonts = std::collections::BTreeSet::new();
                    for (_, pid) in doc.get_pages().into_iter().take(50) {
                        if let Ok(f) = doc.get_page_fonts(pid) {
                            for (_, fd) in f {
                                if let Ok(n) = fd.get(b"BaseFont").and_then(|o| o.as_name()) {
                                    fonts.insert(String::from_utf8_lossy(n).to_string());
                                }
                            }
                        }
                    }
                    details.push((t("Fonts", "Шрифты").to_string(), fonts.into_iter().collect::<Vec<_>>().join(", ")));
                }
                details.push((t("Creator", "Создано в").to_string(), d.info.creator.clone()));
                details.push((t("Producer", "Обработано").to_string(), d.info.producer.clone()));
                details.push((t("Created", "Создан").to_string(), d.info.created.clone()));
                details.push((t("Modified", "Изменён").to_string(), d.info.modified.clone()));
                Dialog::Properties { info: d.info.clone(), details }
            }
            DialogKind::Protect => Dialog::Protect { user: String::new(), owner: String::new(), print: true, copy: true, modify: false },
            DialogKind::Compress => {
                let d = self.doc.as_ref().unwrap();
                let bytes = d.output().map(|(b, _)| b).unwrap_or_else(|_| (*d.bytes).clone());
                let original = bytes.len();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let mut out = Vec::new();
                    for (i, (side, q)) in [(1200u32, 55u8), (1800, 70), (2600, 82)].iter().enumerate() {
                        if let Ok(mut doc) = pdfops::load(&bytes, None) {
                            pdfops::compress_images(&mut doc, *side, *q);
                            doc.compress();
                            if let Ok(b) = pdfops::save(&mut doc) {
                                out.push((i, b));
                            }
                        }
                    }
                    let _ = tx.send(out);
                });
                Dialog::Compress { results: Some(rx), sizes: vec![], choice: 1, original }
            }
            DialogKind::ExportImages => Dialog::ExportImages { png: true, dpi: 150, quality: 90, pages: String::new() },
            DialogKind::Compare => Dialog::Compare { other: None, results: vec![] },
            DialogKind::GoTo => Dialog::GoTo { input: String::new() },
            DialogKind::BookmarksFromText => Dialog::BookmarksFromText { levels: 1 },
            DialogKind::Attach => Dialog::Attach { files: vec![], description: String::new() },
        });
    }

    pub fn dialogs_ui(&mut self, ctx: &Context) {
        let Some(mut dlg) = self.dialog.take() else { return };
        let mut open = true;
        let mut close = false;
        let title = match &dlg {
            Dialog::Password { .. } => t("Password", "Пароль"),
            Dialog::Watermark { .. } => t("Watermark", "Водяной знак"),
            Dialog::PageNumbers { .. } => t("Page Numbers", "Номера страниц"),
            Dialog::HeaderFooter { .. } => t("Header & Footer", "Колонтитулы"),
            Dialog::Crop { .. } => t("Crop Pages", "Обрезка страниц"),
            Dialog::PageSize { .. } => t("Page Size", "Размер страницы"),
            Dialog::Split { .. } => t("Split Document", "Разделить документ"),
            Dialog::Merge { .. } => t("Merge Files", "Объединить файлы"),
            Dialog::Properties { .. } => t("Document Properties", "Свойства документа"),
            Dialog::Protect { .. } => t("Save Copy with Password", "Сохранить копию с паролем"),
            Dialog::Compress { .. } => t("Reduce File Size", "Уменьшить размер файла"),
            Dialog::ExportImages { .. } => t("Export Pages as Images", "Экспорт страниц в изображения"),
            Dialog::Compare { .. } => t("Compare Documents", "Сравнение документов"),
            Dialog::GoTo { .. } => t("Go to Page", "Перейти к странице"),
            Dialog::Ocr { .. } => t("Recognize Text", "Распознавание текста"),
            Dialog::Note { .. } => t("Note", "Заметка"),
            Dialog::Link { .. } => t("Link", "Ссылка"),
            Dialog::BookmarksFromText { .. } => t("Bookmarks from Page Text", "Закладки по тексту страниц"),
            Dialog::Attach { .. } => t("Attach Files", "Вложить файлы"),
        };
        let n = self.doc.as_ref().map(|d| d.page_count()).unwrap_or(0);
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(380.0);
                match &mut dlg {
                    Dialog::Password { path, bytes, input, error } => {
                        ui.label(format!("«{}» {}", kit42::file_name(path), t("is protected. Enter the password:", "защищён паролем. Введите пароль:")));
                        let r = ui.add(egui::TextEdit::singleline(input).password(true));
                        r.request_focus();
                        if *error {
                            ui.colored_label(egui::Color32::RED, t("Wrong password.", "Неверный пароль."));
                        }
                        if ui.button(t("Open", "Открыть")).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                            match self.open_bytes(Some(path.clone()), bytes.clone(), Some(input.clone())) {
                                Ok(()) => close = true,
                                Err(e) if e == "password" => *error = true,
                                Err(e) => {
                                    kui::error(APP, &e);
                                    close = true;
                                }
                            }
                        }
                    }
                    Dialog::Watermark { text, size, color, opacity, angle, pages, image, behind } => {
                        ui.horizontal(|ui| {
                            ui.label(t("Text:", "Текст:"));
                            ui.text_edit_singleline(text);
                        });
                        ui.horizontal(|ui| {
                            ui.label(t("or picture:", "или рисунок:"));
                            if ui.button(image.as_ref().map(|p| kit42::file_name(p)).unwrap_or_else(|| t("Choose…", "Выбрать…").into())).clicked() {
                                *image = kui::open_dialog(t("Picture", "Рисунок"), &[(t("Images", "Изображения"), &["png", "jpg", "jpeg", "bmp"])]).pick_file();
                            }
                            if image.is_some() && ui.small_button("✕").clicked() {
                                *image = None;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label(t("Size:", "Размер:"));
                            ui.add(egui::DragValue::new(size).range(6.0..=300.0).suffix(" pt"));
                            ui.label(t("Colour:", "Цвет:"));
                            kui::color_button(ui, color, t("Colour", "Цвет"));
                        });
                        ui.add(egui::Slider::new(opacity, 0.05..=1.0).text(t("Opacity", "Непрозрачность")));
                        ui.add(egui::Slider::new(angle, -90.0..=90.0).text(t("Rotation", "Поворот")).suffix("°"));
                        ui.checkbox(behind, t("Behind page content", "Под содержимым страницы"));
                        pages_field(ui, pages);
                        if ui.button(t("Apply", "Применить")).clicked() {
                            let (text, size, color, opacity, angle, image, behind) = (text.clone(), *size, *color, *opacity, *angle, image.clone(), *behind);
                            let img = image.and_then(|p| crate::load_any_image(&p).ok());
                            let set = pdfops::page_set(pages, n);
                            self.overlay(t("Watermark", "Водяной знак"), set, behind, &|d, page, _i| {
                                let (w, h) = (page.width, page.height);
                                page.save();
                                page.alpha(opacity);
                                page.rotate_about(w / 2.0, h / 2.0, -angle);
                                if let Some(img) = &img {
                                    let id = d.add_image_rgba(img);
                                    let iw = w * 0.6;
                                    let ih = iw * img.height() as f32 / img.width().max(1) as f32;
                                    page.image(id, (w - iw) / 2.0, (h - ih) / 2.0, iw, ih);
                                } else {
                                    let f = d.font("Arial", true, false);
                                    let tw = d.measure(f, size, &text);
                                    let mut st = TextStyle::new(f, size);
                                    st.color = color;
                                    d.text(page, st, (w - tw) / 2.0, h / 2.0 + size * 0.35, &text);
                                }
                                page.restore();
                            });
                            close = true;
                        }
                    }
                    Dialog::PageNumbers { format, position, start, skip_first, size, pages } => {
                        let formats = ["1", t("Page 1 of N", "Страница 1 из N"), "- 1 -", "1 / N"];
                        egui::ComboBox::from_label(t("Format", "Формат")).selected_text(formats[*format]).show_ui(ui, |ui| {
                            for (i, f) in formats.iter().enumerate() {
                                ui.selectable_value(format, i, *f);
                            }
                        });
                        position_picker(ui, position);
                        ui.horizontal(|ui| {
                            ui.label(t("Start at:", "Начать с:"));
                            ui.add(egui::DragValue::new(start).range(0..=99999));
                            ui.label(t("Size:", "Размер:"));
                            ui.add(egui::DragValue::new(size).range(5.0..=48.0));
                        });
                        ui.checkbox(skip_first, t("Skip the first page", "Не нумеровать первую страницу"));
                        pages_field(ui, pages);
                        if ui.button(t("Apply", "Применить")).clicked() {
                            let set: Vec<usize> = pdfops::page_set(pages, n).unwrap_or_default().into_iter().filter(|p| !(*skip_first && *p == 0)).collect();
                            let (format, position, start, size) = (*format, *position, *start, *size);
                            let total = set.len() as u32 + start - 1;
                            let order = set.clone();
                            self.overlay(t("Page Numbers", "Номера страниц"), Ok(set), false, &|d, page, i| {
                                let num = start + order.iter().position(|p| *p == i).unwrap_or(0) as u32;
                                let label = match format {
                                    1 => format!("{} {num} {} {total}", t("Page", "Страница"), t("of", "из")),
                                    2 => format!("- {num} -"),
                                    3 => format!("{num} / {total}"),
                                    _ => num.to_string(),
                                };
                                draw_at(d, page, position, &label, size, [0, 0, 0], 28.0);
                            });
                            close = true;
                        }
                    }
                    Dialog::HeaderFooter { slots, prefix, suffix, digits, bates_start, size, color, margin, pages } => {
                        ui.label(t("Codes: <<page>> <<pages>> <<date>> <<file>> <<bates>>", "Коды: <<page>> <<pages>> <<date>> <<file>> <<bates>>"));
                        egui::Grid::new("hf").num_columns(3).show(ui, |ui| {
                            for (i, s) in slots.iter_mut().enumerate() {
                                ui.add(egui::TextEdit::singleline(s).desired_width(130.0).hint_text(POSITIONS[i]));
                                if i % 3 == 2 {
                                    ui.end_row();
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label(t("Bates:", "Бейтс:"));
                            ui.add(egui::TextEdit::singleline(prefix).desired_width(60.0).hint_text(t("prefix", "префикс")));
                            ui.add(egui::DragValue::new(bates_start).range(0..=99_999_999));
                            ui.add(egui::DragValue::new(digits).range(1..=12).suffix(t(" digits", " цифр")));
                            ui.add(egui::TextEdit::singleline(suffix).desired_width(50.0).hint_text(t("suffix", "суффикс")));
                        });
                        ui.horizontal(|ui| {
                            ui.label(t("Size:", "Размер:"));
                            ui.add(egui::DragValue::new(size).range(5.0..=36.0));
                            kui::color_button(ui, color, t("Colour", "Цвет"));
                            ui.label(t("Margin, mm:", "Отступ, мм:"));
                            ui.add(egui::DragValue::new(margin).range(2.0..=60.0));
                        });
                        pages_field(ui, pages);
                        if ui.button(t("Apply", "Применить")).clicked() {
                            let set = pdfops::page_set(pages, n);
                            let file = self.doc.as_ref().map(|d| d.name()).unwrap_or_default();
                            let date = chrono::Local::now().format("%d.%m.%Y").to_string();
                            let (slots, prefix, suffix, digits, bs, size, color, margin) = (slots.clone(), prefix.clone(), suffix.clone(), *digits, *bates_start, *size, *color, *margin);
                            let order = set.clone().unwrap_or_default();
                            self.overlay(t("Header & Footer", "Колонтитулы"), set, false, &|d, page, i| {
                                let k = order.iter().position(|p| *p == i).unwrap_or(0) as u32;
                                let bates = format!("{prefix}{:0width$}{suffix}", bs + k, width = digits);
                                for (pos, s) in slots.iter().enumerate() {
                                    if s.trim().is_empty() {
                                        continue;
                                    }
                                    let label = s
                                        .replace("<<page>>", &(i + 1).to_string())
                                        .replace("<<pages>>", &n.to_string())
                                        .replace("<<date>>", &date)
                                        .replace("<<file>>", &file)
                                        .replace("<<bates>>", &bates);
                                    draw_at(d, page, pos, &label, size, color, pdfgen::mm(margin));
                                }
                            });
                            close = true;
                        }
                    }
                    Dialog::Crop { margins, pages, remove } => {
                        ui.label(t("Margins to cut off, mm:", "Срезать поля, мм:"));
                        egui::Grid::new("crop").show(ui, |ui| {
                            for (i, l) in [t("Left", "Слева"), t("Top", "Сверху"), t("Right", "Справа"), t("Bottom", "Снизу")].iter().enumerate() {
                                ui.label(*l);
                                ui.add(egui::DragValue::new(&mut margins[i]).range(0.0..=500.0).speed(0.5));
                                if i % 2 == 1 {
                                    ui.end_row();
                                }
                            }
                        });
                        ui.checkbox(remove, t("Remove crop (show whole pages)", "Убрать обрезку (показать страницы целиком)"));
                        pages_field(ui, pages);
                        if ui.button(t("Apply", "Применить")).clicked() {
                            let set = pdfops::page_set(pages, n).unwrap_or_default();
                            let (m, remove) = (*margins, *remove);
                            if let Some(d) = &mut self.doc {
                                let res = d.edit(t("Crop", "Обрезка"), |doc| {
                                    let ids = pdfops::page_ids(doc);
                                    for p in &set {
                                        let id = ids[*p];
                                        if remove {
                                            pdfops::set_crop(doc, id, None)?;
                                            continue;
                                        }
                                        let g = pdfops::geom(doc, id);
                                        let (wd, hd) = g.display();
                                        let r = [pdfgen::mm(m[0]), pdfgen::mm(m[1]), wd - pdfgen::mm(m[2]), hd - pdfgen::mm(m[3])];
                                        if r[2] - r[0] < 36.0 || r[3] - r[1] < 36.0 {
                                            return Err(t("The margins leave too little of the page.", "После обрезки от страницы почти ничего не остаётся.").into());
                                        }
                                        pdfops::set_crop(doc, id, Some(g.rect_to_user(r)))?;
                                    }
                                    Ok(())
                                });
                                if let Err(e) = res {
                                    kui::error(APP, &e);
                                } else {
                                    // Marks shift with the new visible origin.
                                    for mk in d.marks.iter_mut().filter(|mk| set.contains(&mk.page)) {
                                        if !remove {
                                            mk.translate(-pdfgen::mm(m[0]), -pdfgen::mm(m[1]));
                                        }
                                    }
                                }
                            }
                            self.crop_rect = None;
                            close = true;
                        }
                    }
                    Dialog::PageSize { preset, w, h, landscape, scale, pages } => {
                        let presets = [("A3", 297.0, 420.0), ("A4", 210.0, 297.0), ("A5", 148.0, 210.0), ("Letter", 215.9, 279.4), ("Legal", 215.9, 355.6), ("Tabloid", 279.4, 431.8), (t("Custom", "Свой"), 0.0, 0.0)];
                        egui::ComboBox::from_label(t("Paper", "Бумага")).selected_text(presets[*preset].0).show_ui(ui, |ui| {
                            for (i, p) in presets.iter().enumerate() {
                                if ui.selectable_value(preset, i, p.0).changed() && p.1 > 0.0 {
                                    *w = p.1;
                                    *h = p.2;
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.add_enabled(*preset == 6, egui::DragValue::new(w).range(10.0..=5000.0).suffix(" mm"));
                            ui.label("×");
                            ui.add_enabled(*preset == 6, egui::DragValue::new(h).range(10.0..=5000.0).suffix(" mm"));
                        });
                        ui.checkbox(landscape, t("Landscape", "Альбомная"));
                        ui.radio_value(scale, true, t("Scale content to fit", "Масштабировать содержимое"));
                        ui.radio_value(scale, false, t("Keep content size (extend or trim)", "Сохранить размер содержимого (расширить или обрезать)"));
                        pages_field(ui, pages);
                        if ui.button(t("Apply", "Применить")).clicked() {
                            let set = pdfops::page_set(pages, n).unwrap_or_default();
                            let (mut pw, mut ph) = (pdfgen::mm(*w), pdfgen::mm(*h));
                            if *landscape {
                                std::mem::swap(&mut pw, &mut ph);
                            }
                            let scale = *scale;
                            if let Some(d) = &mut self.doc {
                                let res = d.edit(t("Page Size", "Размер страницы"), |doc| {
                                    let ids = pdfops::page_ids(doc);
                                    for p in &set {
                                        pdfops::set_page_size(doc, ids[*p], pw, ph, scale)?;
                                    }
                                    Ok(())
                                });
                                if let Err(e) = res {
                                    kui::error(APP, &e);
                                }
                            }
                            close = true;
                        }
                    }
                    Dialog::Split { mode, n: every, ranges } => {
                        ui.radio_value(mode, 0, t("Every N pages", "Каждые N страниц"));
                        ui.add_enabled(*mode == 0, egui::DragValue::new(every).range(1..=10000));
                        ui.radio_value(mode, 1, t("By ranges (e.g. 1-3, 4-10)", "По диапазонам (например 1-3, 4-10)"));
                        ui.add_enabled(*mode == 1, egui::TextEdit::singleline(ranges));
                        ui.radio_value(mode, 2, t("One file per page", "Каждая страница — отдельный файл"));
                        if ui.button(t("Choose Folder and Split…", "Выбрать папку и разделить…")).clicked() {
                            let groups: Result<Vec<Vec<usize>>, String> = match *mode {
                                0 => Ok((0..n).collect::<Vec<_>>().chunks(*every as usize).map(|c| c.to_vec()).collect()),
                                1 => pdfops::parse_ranges(ranges, n),
                                _ => Ok((0..n).map(|i| vec![i]).collect()),
                            };
                            match groups {
                                Ok(groups) => {
                                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                        self.split_into(&dir, &groups);
                                        close = true;
                                    }
                                }
                                Err(e) => kui::error(APP, &e),
                            }
                        }
                    }
                    Dialog::Merge { files } => {
                        ui.label(t("PDFs and images are joined in this order:", "PDF и изображения будут объединены в этом порядке:"));
                        let mut action = None;
                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            for (i, f) in files.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{}. {}", i + 1, kit42::file_name(f)));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.small_button("✕").clicked() {
                                            action = Some((i, 0));
                                        }
                                        if ui.small_button("↓").clicked() {
                                            action = Some((i, 1));
                                        }
                                        if ui.small_button("↑").clicked() {
                                            action = Some((i, 2));
                                        }
                                    });
                                });
                            }
                        });
                        if let Some((i, a)) = action {
                            match a {
                                0 => {
                                    files.remove(i);
                                }
                                1 if i + 1 < files.len() => files.swap(i, i + 1),
                                2 if i > 0 => files.swap(i, i - 1),
                                _ => {}
                            }
                        }
                        ui.horizontal(|ui| {
                            if ui.button(t("Add Files…", "Добавить файлы…")).clicked() {
                                if let Some(more) = kui::open_dialog(t("Add Files", "Добавить файлы"), &[(t("PDF and images", "PDF и изображения"), &["pdf", "png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp", "heic"])]).pick_files() {
                                    files.extend(more);
                                }
                            }
                            if ui.add_enabled(files.len() >= 1, egui::Button::new(t("Merge…", "Объединить…"))).clicked() {
                                match merge_files(files) {
                                    Ok(b) => {
                                        if let Some(p) = kui::open_dialog(t("Save Merged PDF", "Сохранить объединённый PDF"), &[("PDF", &["pdf"])]).set_file_name("Merged.pdf").save_file() {
                                            if let Err(e) = kit42::write_atomic(&p, &b) {
                                                kui::error(APP, &e.to_string());
                                            } else {
                                                self.open_path(p);
                                                close = true;
                                            }
                                        }
                                    }
                                    Err(e) => kui::error(APP, &format!("{}\n{e}", t("Nothing was written.", "Ничего не записано."))),
                                }
                            }
                        });
                    }
                    Dialog::Properties { info, details } => {
                        egui::Grid::new("props").num_columns(2).show(ui, |ui| {
                            for (l, v) in [
                                (t("Title", "Заголовок"), &mut info.title),
                                (t("Author", "Автор"), &mut info.author),
                                (t("Subject", "Тема"), &mut info.subject),
                                (t("Keywords", "Ключевые слова"), &mut info.keywords),
                            ] {
                                ui.label(l);
                                ui.add(egui::TextEdit::singleline(v).desired_width(300.0));
                                ui.end_row();
                            }
                        });
                        ui.separator();
                        egui::Grid::new("details").num_columns(2).show(ui, |ui| {
                            for (k, v) in details.iter() {
                                ui.label(egui::RichText::new(k).weak());
                                ui.add(egui::Label::new(v).wrap());
                                ui.end_row();
                            }
                        });
                        if ui.button(t("Save Properties", "Сохранить свойства")).clicked() {
                            let info = info.clone();
                            if let Some(d) = &mut self.doc {
                                if let Err(e) = d.edit(t("Properties", "Свойства"), |doc| {
                                    pdfops::write_info(doc, &info);
                                    Ok(())
                                }) {
                                    kui::error(APP, &e);
                                }
                            }
                            close = true;
                        }
                    }
                    Dialog::Protect { user, owner, print, copy, modify } => {
                        egui::Grid::new("pw").num_columns(2).show(ui, |ui| {
                            ui.label(t("Password to open:", "Пароль на открытие:"));
                            ui.add(egui::TextEdit::singleline(user).password(true));
                            ui.end_row();
                            ui.label(t("Owner password:", "Пароль владельца:"));
                            ui.add(egui::TextEdit::singleline(owner).password(true));
                            ui.end_row();
                        });
                        ui.checkbox(print, t("Allow printing", "Разрешить печать"));
                        ui.checkbox(copy, t("Allow copying text", "Разрешить копирование текста"));
                        ui.checkbox(modify, t("Allow changes", "Разрешить изменения"));
                        ui.label(egui::RichText::new(t("RC4 128-bit, readable by every PDF viewer.", "RC4 128 бит, открывается любой программой просмотра PDF.")).small());
                        if ui.add_enabled(!user.is_empty() || !owner.is_empty(), egui::Button::new(t("Save Copy…", "Сохранить копию…"))).clicked() {
                            if let Some(d) = &self.doc {
                                if let Some(p) = kui::open_dialog(t("Save Protected Copy", "Сохранить защищённую копию"), &[("PDF", &["pdf"])]).set_file_name(d.name()).save_file() {
                                    let res = d.output().and_then(|(b, _)| {
                                        let mut doc = pdfops::load(&b, None)?;
                                        crate::crypt::encrypt(&mut doc, user, owner, &crate::crypt::Permissions { print: *print, copy: *copy, modify: *modify })?;
                                        pdfops::save(&mut doc)
                                    });
                                    match res {
                                        Ok(b) => {
                                            if let Err(e) = kit42::write_atomic(&p, &b) {
                                                kui::error(APP, &e.to_string());
                                            }
                                            close = true;
                                        }
                                        Err(e) => kui::error(APP, &e),
                                    }
                                }
                            }
                        }
                    }
                    Dialog::Compress { results, sizes, choice, original } => {
                        if let Some(rx) = results {
                            if let Ok(v) = rx.try_recv() {
                                *sizes = v;
                                *results = None;
                            } else {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(t("Estimating sizes…", "Оценка размеров…"));
                                });
                                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                            }
                        }
                        ui.label(format!("{}: {:.2} MB", t("Current size", "Текущий размер"), *original as f32 / 1048576.0));
                        let names = [t("Smallest (≈100 dpi)", "Наименьший (≈100 dpi)"), t("Balanced (≈150 dpi)", "Сбалансированный (≈150 dpi)"), t("High quality (≈220 dpi)", "Высокое качество (≈220 dpi)")];
                        for (i, name) in names.iter().enumerate() {
                            let size = sizes.iter().find(|(k, _)| *k == i).map(|(_, b)| format!(" — {:.2} MB", b.len() as f32 / 1048576.0)).unwrap_or_default();
                            ui.radio_value(choice, i, format!("{name}{size}"));
                        }
                        ui.label(egui::RichText::new(t("Pictures are downsampled and re-encoded as JPEG; text and vector graphics stay untouched.", "Изображения уменьшаются и пережимаются в JPEG; текст и векторная графика не меняются.")).small());
                        ui.horizontal(|ui| {
                            let chosen = sizes.iter().find(|(k, _)| k == choice).map(|(_, b)| b.clone());
                            if ui.add_enabled(chosen.is_some(), egui::Button::new(t("Compress Document", "Сжать документ"))).clicked() {
                                if let (Some(b), Some(d)) = (chosen.clone(), &mut self.doc) {
                                    d.checkpoint(t("Reduce File Size", "Уменьшение размера"));
                                    d.marks.clear();
                                    d.bytes = Arc::new(b);
                                    let _ = d.reload();
                                    close = true;
                                }
                            }
                            if ui.add_enabled(chosen.is_some(), egui::Button::new(t("Save Compressed Copy…", "Сохранить сжатую копию…"))).clicked() {
                                if let Some(p) = kui::open_dialog(t("Save Compressed Copy", "Сохранить сжатую копию"), &[("PDF", &["pdf"])]).save_file() {
                                    if let Err(e) = kit42::write_atomic(&p, chosen.as_ref().unwrap()) {
                                        kui::error(APP, &e.to_string());
                                    }
                                    close = true;
                                }
                            }
                        });
                    }
                    Dialog::ExportImages { png, dpi, quality, pages } => {
                        ui.horizontal(|ui| {
                            ui.radio_value(png, true, "PNG");
                            ui.radio_value(png, false, "JPEG");
                        });
                        ui.add(egui::Slider::new(dpi, 72..=300).text(t("Resolution, dpi", "Разрешение, dpi")));
                        if !*png {
                            ui.add(egui::Slider::new(quality, 30..=100).text(t("Quality", "Качество")));
                        }
                        pages_field(ui, pages);
                        if ui.button(t("Choose Folder and Export…", "Выбрать папку и экспортировать…")).clicked() {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                let set = pdfops::page_set(pages, n).unwrap_or_default();
                                self.export_images(dir, set, *png, *dpi, *quality);
                                close = true;
                            }
                        }
                    }
                    Dialog::Compare { other, results } => {
                        ui.horizontal(|ui| {
                            if ui.button(t("Choose Other PDF…", "Выбрать другой PDF…")).clicked() {
                                if let Some(p) = kui::open_dialog(t("Compare With", "Сравнить с"), &[("PDF", &["pdf"])]).pick_file() {
                                    *other = Some(p.clone());
                                    *results = self.compare_with(&p);
                                }
                            }
                            if let Some(o) = other {
                                ui.label(kit42::file_name(o));
                            }
                        });
                        if other.is_some() {
                            ui.label(format!("{}: {}", t("Differences", "Различий"), results.len()));
                        }
                        let mut goto = None;
                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            for (p, removed, added) in results.iter() {
                                ui.horizontal_wrapped(|ui| {
                                    if ui.link(format!("{} {}", t("p.", "с."), p + 1)).clicked() {
                                        goto = Some(*p);
                                    }
                                    if !removed.is_empty() {
                                        ui.colored_label(egui::Color32::from_rgb(200, 40, 40), format!("− {removed}"));
                                    }
                                    if !added.is_empty() {
                                        ui.colored_label(egui::Color32::from_rgb(20, 140, 60), format!("+ {added}"));
                                    }
                                });
                            }
                        });
                        if let Some(p) = goto {
                            self.current = p;
                            self.scroll_to = Some(p);
                        }
                    }
                    Dialog::GoTo { input } => {
                        let r = ui.add(egui::TextEdit::singleline(input).hint_text(format!("1–{n}")));
                        r.request_focus();
                        if ui.button(t("Go", "Перейти")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            if let Ok(p) = input.trim().parse::<usize>() {
                                self.act(crate::Action::GoTo(p.clamp(1, n.max(1)) - 1));
                            }
                            close = true;
                        }
                    }
                    Dialog::Ocr { langs, lang, skip_text } => {
                        if langs.is_empty() {
                            ui.label(t("No OCR languages are installed in Windows. Add a language with its optical character recognition feature in Settings › Time & language.", "В Windows не установлены языки распознавания. Добавьте язык с функцией распознавания текста в Параметры › Время и язык."));
                        } else {
                            egui::ComboBox::from_label(t("Language", "Язык")).selected_text(langs[*lang].1.clone()).show_ui(ui, |ui| {
                                for (i, (_, name)) in langs.iter().enumerate() {
                                    ui.selectable_value(lang, i, name);
                                }
                            });
                        }
                        ui.checkbox(skip_text, t("Skip pages that already have text", "Пропускать страницы, где уже есть текст"));
                        ui.label(egui::RichText::new(t("An invisible text layer is added so scans become searchable and selectable.", "Добавляется невидимый текстовый слой: сканы становятся доступны для поиска и выделения.")).small());
                        if ui.button(t("Recognize", "Распознать")).clicked() {
                            let tag = langs.get(*lang).map(|l| l.0.clone());
                            self.run_ocr(tag, *skip_text);
                            close = true;
                        }
                    }
                    Dialog::Note { id, text } => {
                        ui.add(egui::TextEdit::multiline(text).desired_rows(6).desired_width(360.0));
                        ui.horizontal(|ui| {
                            if ui.button(t("OK", "ОК")).clicked() {
                                if let Some(d) = &mut self.doc {
                                    if let Some(m) = d.marks.iter_mut().find(|m| m.id == *id) {
                                        m.kind = Kind::Note(text.clone());
                                    }
                                }
                                close = true;
                            }
                            if ui.button(t("Delete Note", "Удалить заметку")).clicked() {
                                if let Some(d) = &mut self.doc {
                                    d.marks.retain(|m| m.id != *id);
                                }
                                close = true;
                            }
                        });
                    }
                    Dialog::Link { id, text } => {
                        ui.label(t("Web address, e-mail, or #N for page N:", "Веб-адрес, e-mail или #N для страницы N:"));
                        let r = ui.add(egui::TextEdit::singleline(text).desired_width(360.0).hint_text("https://…  /  #3"));
                        r.request_focus();
                        ui.horizontal(|ui| {
                            if ui.button(t("OK", "ОК")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                if let Some(d) = &mut self.doc {
                                    if text.trim().is_empty() {
                                        d.marks.retain(|m| m.id != *id);
                                    } else if let Some(m) = d.marks.iter_mut().find(|m| m.id == *id) {
                                        m.kind = Kind::Link(text.trim().to_string());
                                    }
                                }
                                close = true;
                            }
                            if ui.button(t("Remove Link", "Удалить ссылку")).clicked() {
                                if let Some(d) = &mut self.doc {
                                    d.marks.retain(|m| m.id != *id);
                                }
                                close = true;
                            }
                        });
                    }
                    Dialog::BookmarksFromText { levels } => {
                        ui.label(t("Creates a bookmark for every page from its first line of text.", "Создаёт закладку для каждой страницы по первой строке её текста."));
                        ui.add(egui::Slider::new(levels, 1..=1).text(t("Levels", "Уровней")));
                        if ui.button(t("Create", "Создать")).clicked() {
                            if let Some(d) = &mut self.doc {
                                let texts = d.texts().clone();
                                let list: Vec<pdfops::Bookmark> = texts
                                    .iter()
                                    .enumerate()
                                    .map(|(i, tx)| pdfops::Bookmark {
                                        title: tx.lines().map(|l| l.trim()).find(|l| l.chars().count() > 2).map(|l| l.chars().take(80).collect()).unwrap_or_else(|| format!("{} {}", t("Page", "Страница"), i + 1)),
                                        page: i,
                                        level: 0,
                                    })
                                    .collect();
                                if let Err(e) = d.edit(t("Bookmarks", "Закладки"), |doc| pdfops::write_outline(doc, &list)) {
                                    kui::error(APP, &e);
                                }
                                self.sidebar = Some(crate::SideTab::Bookmarks);
                            }
                            close = true;
                        }
                    }
                    Dialog::Attach { files, description } => {
                        if ui.button(t("Choose Files…", "Выбрать файлы…")).clicked() {
                            if let Some(f) = rfd::FileDialog::new().pick_files() {
                                *files = f;
                            }
                        }
                        for f in files.iter() {
                            ui.label(kit42::file_name(f));
                        }
                        ui.horizontal(|ui| {
                            ui.label(t("Description:", "Описание:"));
                            ui.text_edit_singleline(description);
                        });
                        if ui.add_enabled(!files.is_empty(), egui::Button::new(t("Attach", "Вложить"))).clicked() {
                            if let Some(d) = &mut self.doc {
                                let mut list = d.attachments.clone();
                                for f in files.iter() {
                                    if let Ok(data) = std::fs::read(f) {
                                        let name = kit42::file_name(f);
                                        list.retain(|a| a.name != name);
                                        list.push(pdfops::Attachment { name, size: data.len(), data, description: description.clone() });
                                    }
                                }
                                if let Err(e) = d.edit(t("Attach Files", "Вложение файлов"), |doc| pdfops::write_attachments(doc, &list)) {
                                    kui::error(APP, &e);
                                }
                                self.sidebar = Some(crate::SideTab::Attachments);
                            }
                            close = true;
                        }
                    }
                }
            });
        if open && !close {
            self.dialog = Some(dlg);
        }
    }

    /// Draws an overlay on each page of a set and stamps it into the page content (with Undo).
    fn overlay(
        &mut self,
        label: &str,
        set: Result<Vec<usize>, String>,
        under: bool,
        draw: &dyn Fn(&mut pdfgen::Doc, &mut pdfgen::Page, usize),
    ) {
        let set = match set {
            Ok(s) => s,
            Err(e) => return kui::error(APP, &e),
        };
        let Some(d) = &mut self.doc else { return };
        let res = d.edit(label, |doc| {
            let ids = pdfops::page_ids(doc);
            for &i in &set {
                let Some(&pid) = ids.get(i) else { continue };
                let (w, h) = pdfops::geom(doc, pid).display();
                let mut pd = pdfgen::Doc::new();
                let mut page = pdfgen::Page::new(w, h);
                draw(&mut pd, &mut page, i);
                pd.add_page(page);
                pdfops::stamp(doc, pid, &pd.finish(), under)?;
            }
            Ok(())
        });
        if let Err(e) = res {
            kui::error(APP, &e);
        }
    }

    fn split_into(&mut self, dir: &std::path::Path, groups: &[Vec<usize>]) {
        let Some(d) = &self.doc else { return };
        let bytes = match d.output() {
            Ok((b, _)) => b,
            Err(e) => return kui::error(APP, &e),
        };
        let stem = d.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Document".into());
        let mut written = 0;
        for (i, g) in groups.iter().enumerate() {
            let name = if g.len() == 1 { format!("{stem} - {}.pdf", g[0] + 1) } else { format!("{stem} - {}-{}.pdf", g[0] + 1, g[g.len() - 1] + 1) };
            let mut path = dir.join(&name);
            let mut k = 2;
            while path.exists() {
                path = dir.join(format!("{} ({k}).pdf", name.trim_end_matches(".pdf")));
                k += 1;
            }
            match pdfops::extract(&bytes, g) {
                Ok(b) => {
                    if kit42::write_atomic(&path, &b).is_ok() {
                        written += 1;
                    }
                }
                Err(e) => {
                    kui::error(APP, &format!("{} {}: {e}", t("Part", "Часть"), i + 1));
                    return;
                }
            }
        }
        self.status = format!("{} {written}", t("Files written:", "Записано файлов:"));
        kit42::shell::open_path(dir);
    }

    fn export_images(&mut self, dir: PathBuf, set: Vec<usize>, png: bool, dpi: u32, quality: u8) {
        let Some(d) = &self.doc else { return };
        let bytes = d.output().map(|(b, _)| b).unwrap_or_else(|_| (*d.bytes).clone());
        let stem = d.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Page".into());
        self.spawn_job(t("Exporting pages…", "Экспорт страниц…"), "", move |progress| {
            let (r, sizes) = kit42::winrt::PdfRenderer::open(bytes, None)?;
            for (k, &i) in set.iter().enumerate() {
                let (w, h) = sizes[i];
                let mut px = (w / 72.0 * dpi as f32) as u32;
                let ph = h / 72.0 * dpi as f32;
                if px > 16384 || ph > 16384.0 || px as f32 * ph > 64_000_000.0 {
                    px = px.min(16384).min((64_000_000.0 / ph.max(1.0)) as u32);
                }
                let img = r.render(i as u32, px)?;
                let path = dir.join(format!("{stem} {:03}.{}", i + 1, if png { "png" } else { "jpg" }));
                if png {
                    img.save(&path).map_err(|e| e.to_string())?;
                } else {
                    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
                    let mut out = Vec::new();
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(&rgb).map_err(|e| e.to_string())?;
                    kit42::write_atomic(&path, &out).map_err(|e| e.to_string())?;
                }
                progress.store(((k + 1) * 1000 / set.len().max(1)) as u32, std::sync::atomic::Ordering::Relaxed);
            }
            kit42::shell::open_path(&dir);
            Ok(Vec::new())
        });
    }

    fn run_ocr(&mut self, lang: Option<String>, skip_text: bool) {
        let Some(d) = &mut self.doc else { return };
        let bytes = (*d.bytes).clone();
        let texts = if skip_text { d.texts().clone() } else { vec![] };
        self.spawn_job(t("Recognizing text…", "Распознавание текста…"), t("Recognize Text", "Распознавание текста"), move |progress| {
            let (r, sizes) = kit42::winrt::PdfRenderer::open(bytes.clone(), None)?;
            let mut doc = pdfops::load(&bytes, None)?;
            let ids = pdfops::page_ids(&doc);
            let mut done = 0;
            for (i, (w, h)) in sizes.iter().enumerate() {
                progress.store((i * 1000 / sizes.len().max(1)) as u32, std::sync::atomic::Ordering::Relaxed);
                if skip_text && texts.get(i).map(|s| s.trim().chars().count() > 20).unwrap_or(false) {
                    continue;
                }
                let px = (w / 72.0 * 300.0).min(7000.0) as u32;
                let img = r.render(i as u32, px)?;
                let s = img.width() as f32 / w;
                let res = kit42::winrt::ocr(&img, lang.clone())?;
                if res.lines.is_empty() {
                    continue;
                }
                let mut pd = pdfgen::Doc::new();
                let font = pd.font("Arial", false, false);
                let mut page = pdfgen::Page::new(*w, *h);
                res.to_pdf_layer(&mut pd, &mut page, font, 1.0 / s);
                pd.add_page(page);
                pdfops::stamp(&mut doc, ids[i], &pd.finish(), false)?;
                done += 1;
            }
            if done == 0 {
                return Err(t("No text was recognised (or every page already has text).", "Текст не распознан (или на всех страницах уже есть текст).").into());
            }
            pdfops::save(&mut doc)
        });
    }

    fn compare_with(&mut self, other: &std::path::Path) -> Vec<(usize, String, String)> {
        let Some(d) = &mut self.doc else { return vec![] };
        let a = d.texts().clone();
        let b = match kit42::ooxml::read_file(other).and_then(|b| pdfops::load(&b, None)) {
            Ok(doc) => pdfops::all_text(&doc),
            Err(e) => {
                kui::error(APP, &e);
                return vec![];
            }
        };
        let mut out = Vec::new();
        for p in 0..a.len().max(b.len()) {
            let wa: Vec<&str> = a.get(p).map(|s| s.split_whitespace().collect()).unwrap_or_default();
            let wb: Vec<&str> = b.get(p).map(|s| s.split_whitespace().collect()).unwrap_or_default();
            for (rem, add) in word_diff(&wa, &wb) {
                out.push((p, rem, add));
            }
        }
        out
    }

    pub fn export_text(&mut self, f: TextFormat) {
        let Some(d) = &mut self.doc else { return };
        let (ext, filter) = match f {
            TextFormat::Docx => ("docx", "Word"),
            TextFormat::Rtf => ("rtf", "RTF"),
            TextFormat::Html => ("html", "HTML"),
            TextFormat::Txt => ("txt", t("Text", "Текст")),
        };
        let stem = d.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Document".into());
        let Some(path) = kui::open_dialog(t("Export", "Экспорт"), &[(filter, &[ext])]).set_file_name(format!("{stem}.{ext}")).save_file() else { return };
        let pages = d.texts().clone();
        if pages.iter().all(|p| p.trim().is_empty()) {
            kui::info(APP, t("The document has no text layer. Run Tools › Recognize Text first.", "В документе нет текстового слоя. Сначала выполните распознавание текста."));
        }
        let bytes: Vec<u8> = match f {
            TextFormat::Txt => pages.join("\n\x0C\n").into_bytes(),
            TextFormat::Html => {
                let mut h = format!("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{}</title><style>body{{font-family:Segoe UI,Arial;max-width:800px;margin:2em auto}} section{{border-bottom:1px solid #ccc;padding:1em 0}}</style></head><body>\n", kit42::ooxml::esc(&stem));
                for (i, p) in pages.iter().enumerate() {
                    h.push_str(&format!("<section id=\"page{}\">\n", i + 1));
                    for l in p.lines() {
                        h.push_str(&format!("<p>{}</p>\n", kit42::ooxml::esc(l)));
                    }
                    h.push_str("</section>\n");
                }
                h.push_str("</body></html>\n");
                h.into_bytes()
            }
            TextFormat::Rtf => {
                let mut r = String::from("{\\rtf1\\ansi\\ansicpg1252\\deff0{\\fonttbl{\\f0 Calibri;}}\\f0\\fs22\n");
                for (i, p) in pages.iter().enumerate() {
                    if i > 0 {
                        r.push_str("\\page\n");
                    }
                    for l in p.lines() {
                        r.push_str(&rtf_escape(l));
                        r.push_str("\\par\n");
                    }
                }
                r.push('}');
                r.into_bytes()
            }
            TextFormat::Docx => match simple_docx(&pages) {
                Ok(b) => b,
                Err(e) => return kui::error(APP, &e),
            },
        };
        if let Err(e) = kit42::write_atomic(&path, &bytes) {
            kui::error(APP, &e.to_string());
        }
    }
}

fn pages_field(ui: &mut egui::Ui, pages: &mut String) {
    ui.horizontal(|ui| {
        ui.label(t("Pages:", "Страницы:"));
        ui.add(egui::TextEdit::singleline(pages).desired_width(160.0).hint_text(t("all, odd, even, 1-3, 5", "все, нечётные, чётные, 1-3, 5")));
    });
}

fn position_picker(ui: &mut egui::Ui, pos: &mut usize) {
    ui.label(t("Position:", "Положение:"));
    egui::Grid::new("pos").show(ui, |ui| {
        for (i, p) in POSITIONS.iter().enumerate() {
            ui.selectable_value(pos, i, *p);
            if i == 2 {
                ui.end_row();
            }
        }
    });
}

/// Draws a label at one of six header/footer positions.
fn draw_at(d: &mut pdfgen::Doc, page: &mut pdfgen::Page, pos: usize, label: &str, size: f32, color: [u8; 3], margin: f32) {
    let f = d.font("Arial", false, false);
    let w = d.measure(f, size, label);
    let x = match pos % 3 {
        0 => margin,
        1 => (page.width - w) / 2.0,
        _ => page.width - margin - w,
    };
    let y = if pos < 3 { margin + size } else { page.height - margin };
    let mut st = TextStyle::new(f, size);
    st.color = color;
    d.text(page, st, x, y, label);
}

pub fn merge_files(files: &[PathBuf]) -> Result<Vec<u8>, String> {
    let mut out: Option<lopdf::Document> = None;
    for f in files {
        let ext = kit42::ext(f);
        let part = if ext == "pdf" {
            pdfops::load(&kit42::ooxml::read_file(f)?, None).map_err(|e| format!("{}: {e}", kit42::file_name(f)))?
        } else {
            let img = crate::load_any_image(f).map_err(|e| format!("{}: {e}", kit42::file_name(f)))?;
            lopdf::Document::load_mem(&pdfops::image_page_pdf(&img, true)).map_err(|e| e.to_string())?
        };
        match &mut out {
            None => {
                let mut p = part;
                pdfops::flatten_tree(&mut p)?;
                out = Some(p);
            }
            Some(doc) => {
                let n = pdfops::page_ids(doc).len();
                pdfops::insert_document(doc, part, n)?;
            }
        }
    }
    let mut doc = out.ok_or("no files")?;
    doc.prune_objects();
    pdfops::save(&mut doc)
}

/// Word-level differences as (removed, added) chunks.
fn word_diff(a: &[&str], b: &[&str]) -> Vec<(String, String)> {
    let (n, m) = (a.len().min(3000), b.len().min(3000));
    let mut dp = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    let (mut rem, mut add) = (Vec::new(), Vec::new());
    let flush = |rem: &mut Vec<&str>, add: &mut Vec<&str>, out: &mut Vec<(String, String)>| {
        if !rem.is_empty() || !add.is_empty() {
            out.push((rem.join(" "), add.join(" ")));
            rem.clear();
            add.clear();
        }
    };
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            flush(&mut rem, &mut add, &mut out);
            i += 1;
            j += 1;
        } else if j < m && (i >= n || dp[i][j + 1] >= dp[i + 1][j]) {
            add.push(b[j]);
            j += 1;
        } else {
            rem.push(a[i]);
            i += 1;
        }
    }
    flush(&mut rem, &mut add, &mut out);
    out
}

pub fn rtf_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\t' => out.push_str("\\tab "),
            c if (c as u32) < 128 => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    out
}

/// A plain Word document: one paragraph per line, page breaks between pages.
fn simple_docx(pages: &[String]) -> Result<Vec<u8>, String> {
    use kit42::ooxml::esc;
    let mut body = String::new();
    for (i, p) in pages.iter().enumerate() {
        for (k, l) in p.lines().enumerate() {
            let br = if i > 0 && k == 0 { "<w:r><w:br w:type=\"page\"/></w:r>" } else { "" };
            body.push_str(&format!("<w:p>{br}<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>", esc(l)));
        }
    }
    let doc = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1134" w:right="850" w:bottom="1134" w:left="1701" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#);
    let mut w = kit42::ooxml::PackageWriter::new();
    w.add_str("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#)?;
    w.add_str("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#)?;
    w.add_str("word/document.xml", &doc)?;
    w.finish()
}
