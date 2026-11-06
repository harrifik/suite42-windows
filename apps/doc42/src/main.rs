#![windows_subsystem = "windows"]
//! DOC42 for Windows — rich documents and page layout (.docx, .odt, .rtf, .md, .html, .txt, PDF export).

mod docx;
mod editor;
mod formats;
mod model;
mod pdfout;

use editor::{all_locs, get_para, get_para_mut, Loc};
use egui::{Context, Key};
use kit42::history::History;
use kit42::icons as ic;
use kit42::richtext::{Align2, CharFmt, ListInfo, ListKind, Para, ParaStyle};
use kit42::ui::{self as kui, CTRL, CTRL_ALT, CTRL_SHIFT, NONE};
use kit42::{settings, t};
use model::*;
use std::collections::HashMap;
use std::path::PathBuf;

pub const APP: &str = "DOC42";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fmt {
    Docx,
    Odt,
    Rtf,
    Markdown,
    Html,
    Txt,
}

impl Fmt {
    fn from_path(p: &std::path::Path) -> Fmt {
        match kit42::ext(p).as_str() {
            "odt" | "ott" => Fmt::Odt,
            "rtf" => Fmt::Rtf,
            "md" | "markdown" | "mdown" => Fmt::Markdown,
            "html" | "htm" | "xhtml" => Fmt::Html,
            "txt" | "text" | "log" | "csv" | "ini" => Fmt::Txt,
            _ => Fmt::Docx,
        }
    }
}

enum Dialog {
    PageSetup(PageSetup),
    Paragraph { indent: [f32; 3], spacing: [f32; 2], line: f32 },
    Table { rows: usize, cols: usize },
    Link { url: String },
    Properties(Meta),
    Stats,
    Special,
    Find,
}

pub struct App {
    pub doc: Document,
    pub path: Option<PathBuf>,
    pub fmt: Fmt,
    pub encoding: &'static encoding_rs::Encoding,
    pub history: History<Document>,
    pub last_edit: Option<(Loc, std::time::Instant)>,
    pub zoom: f32,
    pub focus: Option<Loc>,
    pub sel: Option<(usize, usize)>,
    pub pending_focus: Option<(Loc, usize, usize)>,
    pub scroll_to_block: Option<usize>,
    pub scroll_target_hint: Option<usize>,
    pub typing: Option<CharFmt>,
    pub select_all: bool,
    pub picker: kit42::signature::MarkPicker,
    dialog: Option<Dialog>,
    pub pic_tex: HashMap<u64, egui::TextureHandle>,
    pub selected_picture: Option<usize>,
    pub resizing: bool,
    pub page_count: usize,
    pub cur_page: usize,
    pub status: String,
    show_nav: bool,
    find: String,
    replace: String,
    match_case: bool,
    whole_word: bool,
    find_pos: Option<(Loc, usize)>,
    fonts: Vec<String>,
    font_filter: String,
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self {
            doc: Document::default(),
            path: None,
            fmt: Fmt::Docx,
            encoding: encoding_rs::UTF_8,
            history: History::new(200),
            last_edit: None,
            zoom: settings::get_or("doc42.zoom", 1.0),
            focus: None,
            sel: None,
            pending_focus: Some((Loc::Para(0), 0, 0)),
            scroll_to_block: None,
            scroll_target_hint: None,
            typing: None,
            select_all: false,
            picker: Default::default(),
            dialog: None,
            pic_tex: HashMap::new(),
            selected_picture: None,
            resizing: false,
            page_count: 1,
            cur_page: 0,
            status: String::new(),
            show_nav: false,
            find: String::new(),
            replace: String::new(),
            match_case: false,
            whole_word: false,
            find_pos: None,
            fonts: vec![],
            font_filter: String::new(),
        };
        if let Some(p) = kit42::startup_files().into_iter().next() {
            app.open(p);
        }
        app
    }

    fn name(&self) -> String {
        self.path.as_ref().map(|p| kit42::file_name(p)).unwrap_or_else(|| t("Untitled", "Без имени").into())
    }

    fn dirty(&self) -> bool {
        self.history.is_dirty()
    }

    pub fn load(path: &std::path::Path) -> Result<(Document, Fmt, &'static encoding_rs::Encoding), String> {
        let bytes = kit42::ooxml::read_file(path)?;
        let ext = kit42::ext(path);
        let base = path.parent();
        if ext == "doc" || bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
            let text = doc97_text(&bytes).ok_or_else(|| t("This legacy Word (.doc) file could not be read. Save it as .docx in Word or LibreOffice and open it again.", "Не удалось прочитать старый формат Word (.doc). Сохраните его как .docx в Word или LibreOffice и откройте снова.").to_string())?;
            return Ok((formats::read_text(&text, &CharFmt::default()), Fmt::Docx, encoding_rs::UTF_8));
        }
        if bytes.starts_with(b"PK") {
            if let Ok(pkg) = kit42::ooxml::Package::read(&bytes) {
                if pkg.parts.contains_key("content.xml") {
                    return Ok((formats::read_odt(&bytes)?, Fmt::Odt, encoding_rs::UTF_8));
                }
            }
            return Ok((docx::read(&bytes)?, Fmt::Docx, encoding_rs::UTF_8));
        }
        if bytes.starts_with(b"{\\rtf") {
            return Ok((formats::read_rtf(&bytes)?, Fmt::Rtf, encoding_rs::UTF_8));
        }
        let (text, enc) = kit42::ooxml::decode_text(&bytes);
        let fmt = Fmt::from_path(path);
        let doc = match fmt {
            Fmt::Markdown => formats::read_markdown(&text, base),
            Fmt::Html => formats::read_html(&text, base),
            _ if text.trim_start().to_lowercase().starts_with("<!doctype html") || text.trim_start().to_lowercase().starts_with("<html") => formats::read_html(&text, base),
            _ => formats::read_text(&text, &CharFmt { font: "Consolas".into(), size: 10.5, ..CharFmt::default() }),
        };
        Ok((doc, if fmt == Fmt::Docx { Fmt::Txt } else { fmt }, enc))
    }

    fn open(&mut self, path: PathBuf) {
        if self.dirty() || self.path.is_some() || !self.doc.plain_text().trim().is_empty() {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).arg(&path).spawn();
            }
            return;
        }
        match Self::load(&path) {
            Ok((doc, fmt, enc)) => {
                self.doc = doc;
                self.fmt = fmt;
                self.encoding = enc;
                self.path = Some(path.clone());
                self.history.clear();
                self.pic_tex.clear();
                self.pending_focus = Some((Loc::Para(0), 0, 0));
                if self.doc.blocks.first().map(|b| !matches!(b, Block::Para(_))).unwrap_or(false) {
                    self.doc.blocks.insert(0, Block::Para(Para::new("", self.doc.body.clone())));
                }
                settings::add_recent("doc42", &path);
                kui::remember_dir(&path);
            }
            Err(e) => kui::error(APP, &format!("{}\n{e}", t("The document could not be opened.", "Не удалось открыть документ."))),
        }
    }

    fn serialize(&self, path: &std::path::Path) -> Result<Vec<u8>, String> {
        Ok(match kit42::ext(path).as_str() {
            "odt" => formats::write_odt(&self.doc)?,
            "rtf" => formats::write_rtf(&self.doc),
            "md" | "markdown" => formats::write_markdown(&self.doc, Some(path)).into_bytes(),
            "html" | "htm" => formats::write_html(&self.doc, true).into_bytes(),
            "txt" => {
                let text = self.doc.plain_text();
                let (bytes, _, bad) = self.encoding.encode(&text);
                if bad || self.encoding == encoding_rs::UTF_8 {
                    text.into_bytes()
                } else {
                    bytes.into_owned()
                }
            }
            "pdf" => pdfout::export(&self.doc),
            _ => docx::write(&self.doc)?,
        })
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        match self.serialize(&path).and_then(|b| kit42::write_atomic(&path, &b).map_err(|e| e.to_string())) {
            Ok(()) => {
                if kit42::ext(&path) != "pdf" {
                    self.fmt = Fmt::from_path(&path);
                    self.path = Some(path.clone());
                    self.history.mark_saved();
                    settings::add_recent("doc42", &path);
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
            Some(p) if kit42::ext(&p) != "doc" => self.save_to(p),
            _ => self.save_as(),
        }
    }

    fn save_as(&mut self) -> bool {
        let stem = self.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| t("Document", "Документ").into());
        let Some(p) = kui::open_dialog(t("Save As", "Сохранить как"), &[
            ("Word (.docx)", &["docx"]),
            ("OpenDocument (.odt)", &["odt"]),
            ("Rich Text (.rtf)", &["rtf"]),
            ("Markdown (.md)", &["md"]),
            ("HTML", &["html"]),
            (t("Plain text", "Текст"), &["txt"]),
        ])
        .set_file_name(format!("{stem}.docx"))
        .save_file() else {
            return false;
        };
        kui::remember_dir(&p);
        self.save_to(p)
    }

    fn export_pdf(&mut self) {
        let stem = self.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| t("Document", "Документ").into());
        if let Some(p) = kui::open_dialog(t("Export as PDF", "Экспорт в PDF"), &[("PDF", &["pdf"])]).set_file_name(format!("{stem}.pdf")).save_file() {
            let b = pdfout::export(&self.doc);
            match kit42::write_atomic(&p, &b) {
                Ok(()) => self.status = format!("{} {}", t("Exported:", "Экспортировано:"), kit42::file_name(&p)),
                Err(e) => kui::error(APP, &e.to_string()),
            }
        }
    }

    // ───────────── formatting ─────────────

    fn current_fmt(&self) -> CharFmt {
        if let Some(f) = &self.typing {
            return f.clone();
        }
        match self.focus.and_then(|l| get_para(&self.doc, l)) {
            Some(p) => {
                let pos = self.sel.map(|(a, b)| if a < b { a + 1 } else { a }).unwrap_or(0);
                p.fmt_at(pos)
            }
            None => self.doc.body.clone(),
        }
    }

    fn current_para(&self) -> Option<&Para> {
        self.focus.and_then(|l| get_para(&self.doc, l))
    }

    fn refocus(&mut self) {
        if let Some(l) = self.focus {
            let (a, b) = self.sel.unwrap_or((0, 0));
            self.pending_focus = Some((l, a, b));
        }
    }

    fn apply_char(&mut self, label: &str, f: impl Fn(&mut CharFmt)) {
        if self.select_all {
            self.checkpoint(label);
            for l in all_locs(&self.doc) {
                if let Some(p) = get_para_mut(&mut self.doc, l) {
                    p.apply_all(&mut |x| f(x));
                }
            }
        } else if let Some(l) = self.focus {
            match self.sel {
                Some((a, b)) if a < b => {
                    self.checkpoint(label);
                    if let Some(p) = get_para_mut(&mut self.doc, l) {
                        p.apply(a, b, &mut |x| f(x));
                    }
                }
                _ => {
                    let mut fmt = self.current_fmt();
                    f(&mut fmt);
                    // An empty paragraph takes the format directly.
                    let empty = get_para(&self.doc, l).map(|p| p.is_empty()).unwrap_or(false);
                    if empty {
                        self.checkpoint(label);
                        if let Some(p) = get_para_mut(&mut self.doc, l) {
                            p.apply_all(&mut |x| f(x));
                        }
                    } else {
                        self.typing = Some(fmt);
                    }
                }
            }
        }
        self.refocus();
    }

    fn apply_para(&mut self, label: &str, f: impl Fn(&mut Para, &CharFmt)) {
        let locs: Vec<Loc> = if self.select_all { all_locs(&self.doc) } else { self.focus.into_iter().collect() };
        if locs.is_empty() {
            return;
        }
        self.checkpoint(label);
        let body = self.doc.body.clone();
        for l in locs {
            if let Some(p) = get_para_mut(&mut self.doc, l) {
                f(p, &body);
            }
        }
        self.refocus();
    }

    fn toggle_list(&mut self, kind: ListKind) {
        let on = self.current_para().and_then(|p| p.list.as_ref()).map(|l| l.kind == kind).unwrap_or(false);
        self.apply_para(t("List", "Список"), |p, _| p.list = if on { None } else { Some(ListInfo { kind, level: p.list.as_ref().map(|l| l.level).unwrap_or(0), checked: false }) });
    }

    fn insert_block(&mut self, b: Block) {
        self.checkpoint(t("Insert", "Вставка"));
        let at = match self.focus {
            Some(Loc::Para(i)) => {
                // Split the paragraph at the cursor so the block lands there.
                let pos = self.sel.map(|s| s.1).unwrap_or(0);
                if let Some(Block::Para(p)) = self.doc.blocks.get_mut(i) {
                    if pos > 0 && pos < p.char_len() {
                        let tail = p.split_off(pos);
                        self.doc.blocks.insert(i + 1, Block::Para(tail));
                        i + 1
                    } else if pos == 0 && !p.is_empty() {
                        i
                    } else {
                        i + 1
                    }
                } else {
                    i + 1
                }
            }
            Some(Loc::Cell(b, ..)) => b + 1,
            None => self.doc.blocks.len(),
        };
        self.doc.blocks.insert(at, b);
        if !matches!(self.doc.blocks.get(at + 1), Some(Block::Para(_))) {
            self.doc.blocks.insert(at + 1, Block::Para(Para::new("", self.doc.body.clone())));
        }
        self.pending_focus = Some((Loc::Para(at + 1), 0, 0));
        self.doc.normalize();
    }

    fn insert_text(&mut self, s: &str) {
        let Some(l) = self.focus else { return };
        self.checkpoint(t("Insert", "Вставка"));
        let (a, b) = self.sel.unwrap_or((0, 0));
        if let Some(p) = get_para_mut(&mut self.doc, l) {
            let f = p.fmt_at(a);
            p.splice(a, b - a, s, &f);
        }
        let pos = a + s.chars().count();
        self.sel = Some((pos, pos));
        self.pending_focus = Some((l, pos, pos));
    }

    fn insert_toc(&mut self) {
        let pages = pdfout::block_pages(&self.doc);
        let heads = self.doc.headings();
        if heads.is_empty() {
            kui::info(APP, t("There are no headings. Apply Heading 1–3 styles first.", "Нет заголовков. Сначала примените стили «Заголовок 1–3»."));
            return;
        }
        self.checkpoint(t("Table of Contents", "Оглавление"));
        let at = self.focus.map(|l| l.block()).unwrap_or(0);
        let mut paras = vec![Block::Para(Para::with_style(t("Contents", "Содержание"), ParaStyle::Heading(1), &self.doc.body))];
        let tw = self.doc.setup.text_width();
        for (b, lvl, text) in heads.iter().filter(|h| h.1 <= 3) {
            let page = pages.get(*b).map(|p| p + 1).unwrap_or(1) + if *b >= at { 0 } else { 0 };
            let mut p = Para::new("", self.doc.body.clone());
            p.indent_left = (*lvl as f32).max(1.0) * 14.0 - 14.0;
            p.space_after = 2.0;
            // Dot leaders approximated with the text width.
            let fill = ((tw - p.indent_left) / (self.doc.body.size * 0.28)) as usize;
            let dots = fill.saturating_sub(text.chars().count() + 4).min(160);
            let mut f = self.doc.body.clone();
            f.link = Some(format!("#h{b}"));
            p.runs = vec![
                kit42::richtext::Run { text: text.clone(), fmt: f },
                kit42::richtext::Run { text: format!(" {} {page}", ".".repeat(dots / 2)), fmt: self.doc.body.clone() },
            ];
            paras.push(Block::Para(p));
        }
        paras.push(Block::PageBreak);
        for (k, b) in paras.into_iter().enumerate() {
            self.doc.blocks.insert(at + k, b);
        }
        self.doc.normalize();
    }

    // ───────────── find & replace ─────────────

    fn matches_in(&self, text: &str) -> Vec<(usize, usize)> {
        if self.find.is_empty() {
            return vec![];
        }
        let (hay, needle) = if self.match_case { (text.to_string(), self.find.clone()) } else { (text.to_lowercase(), self.find.to_lowercase()) };
        let chars: Vec<char> = hay.chars().collect();
        let nd: Vec<char> = needle.chars().collect();
        let mut out = vec![];
        let mut i = 0;
        while i + nd.len() <= chars.len() {
            if chars[i..i + nd.len()] == nd[..] {
                let ok = !self.whole_word
                    || ((i == 0 || !chars[i - 1].is_alphanumeric()) && (i + nd.len() == chars.len() || !chars[i + nd.len()].is_alphanumeric()));
                if ok {
                    out.push((i, i + nd.len()));
                    i += nd.len();
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    fn find_next(&mut self) {
        let locs = all_locs(&self.doc);
        if locs.is_empty() || self.find.is_empty() {
            return;
        }
        let (start_loc, start_pos) = self.find_pos.or(self.focus.map(|l| (l, self.sel.map(|s| s.1).unwrap_or(0)))).unwrap_or((locs[0], 0));
        let start_idx = locs.iter().position(|l| *l == start_loc).unwrap_or(0);
        for k in 0..=locs.len() {
            let l = locs[(start_idx + k) % locs.len()];
            let text = get_para(&self.doc, l).map(|p| p.text()).unwrap_or_default();
            let from = if k == 0 { start_pos } else { 0 };
            if let Some((a, b)) = self.matches_in(&text).into_iter().find(|(a, _)| *a >= from) {
                self.pending_focus = Some((l, a, b));
                self.focus = Some(l);
                self.sel = Some((a, b));
                self.find_pos = Some((l, b));
                self.scroll_to_block = Some(l.block());
                return;
            }
        }
        self.status = t("Not found.", "Не найдено.").into();
    }

    fn replace_one(&mut self) {
        if let (Some(l), Some((a, b))) = (self.focus, self.sel) {
            let text = get_para(&self.doc, l).map(|p| p.text()).unwrap_or_default();
            if self.matches_in(&text).contains(&(a, b)) {
                self.checkpoint(t("Replace", "Замена"));
                let r = self.replace.clone();
                if let Some(p) = get_para_mut(&mut self.doc, l) {
                    let f = p.fmt_at(a + 1);
                    p.splice(a, b - a, &r, &f);
                }
                self.find_pos = Some((l, a + r.chars().count()));
            }
        }
        self.find_next();
    }

    fn replace_all(&mut self) {
        if self.find.is_empty() {
            return;
        }
        let before = self.doc.clone();
        let mut n = 0;
        let r = self.replace.clone();
        for l in all_locs(&self.doc) {
            let text = get_para(&self.doc, l).map(|p| p.text()).unwrap_or_default();
            let ms = self.matches_in(&text);
            if let Some(p) = get_para_mut(&mut self.doc, l) {
                for (a, b) in ms.into_iter().rev() {
                    let f = p.fmt_at(a + 1);
                    p.splice(a, b - a, &r, &f);
                    n += 1;
                }
            }
        }
        if n > 0 {
            self.history.push(before, t("Replace All", "Заменить все"));
        }
        self.status = format!("{} {n}", t("Replacements:", "Замен:"));
    }
}

/// Text of a Word 97–2003 binary document (piece table in the WordDocument stream).
fn doc97_text(bytes: &[u8]) -> Option<String> {
    let cfb = cfb_streams(bytes)?;
    let wd = cfb.get("WordDocument")?;
    let rd16 = |b: &[u8], o: usize| -> Option<u16> { Some(u16::from_le_bytes([*b.get(o)?, *b.get(o + 1)?])) };
    let rd32 = |b: &[u8], o: usize| -> Option<u32> { Some(u32::from_le_bytes([*b.get(o)?, *b.get(o + 1)?, *b.get(o + 2)?, *b.get(o + 3)?])) };
    let flags = rd16(wd, 0x0A)?;
    let table = cfb.get(if flags & 0x0200 != 0 { "1Table" } else { "0Table" })?;
    let fc_clx = rd32(wd, 0x01A2)? as usize;
    let lcb_clx = rd32(wd, 0x01A6)? as usize;
    let clx = table.get(fc_clx..fc_clx + lcb_clx)?;
    let mut i = 0;
    while i < clx.len() && clx[i] == 0x01 {
        let cb = rd16(clx, i + 1)? as usize;
        i += 3 + cb;
    }
    if clx.get(i)? != &0x02 {
        return None;
    }
    let lcb = rd32(clx, i + 1)? as usize;
    let plc = clx.get(i + 5..i + 5 + lcb)?;
    let n = (lcb - 4) / 12;
    let mut out = String::new();
    for k in 0..n {
        let cp0 = rd32(plc, k * 4)? as usize;
        let cp1 = rd32(plc, (k + 1) * 4)? as usize;
        let pcd = (n + 1) * 4 + k * 8;
        let fc = rd32(plc, pcd + 2)?;
        let compressed = fc & 0x4000_0000 != 0;
        let len = cp1.saturating_sub(cp0);
        if compressed {
            let off = ((fc & 0x3FFF_FFFF) / 2) as usize;
            let bytes = wd.get(off..off + len)?;
            let (s, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
            out.push_str(&s);
        } else {
            let off = fc as usize;
            let raw = wd.get(off..off + len * 2)?;
            let u: Vec<u16> = raw.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            out.push_str(&String::from_utf16_lossy(&u));
        }
    }
    // Word control characters: 13 paragraph, 11 line break, 12 page break, 7 cell, fields 19/20/21.
    let mut clean = String::with_capacity(out.len());
    let mut field = 0;
    for c in out.chars() {
        match c as u32 {
            19 => field += 1,
            20 => field = 0,
            21 => field = 0,
            _ if field > 0 => {}
            13 | 11 => clean.push('\n'),
            12 => clean.push('\x0C'),
            7 => clean.push('\t'),
            1 | 8 => {}
            _ => clean.push(c),
        }
    }
    Some(clean)
}

/// Minimal compound file (OLE2) reader returning streams by name.
fn cfb_streams(b: &[u8]) -> Option<HashMap<String, Vec<u8>>> {
    if !b.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return None;
    }
    let rd16 = |o: usize| -> Option<u16> { Some(u16::from_le_bytes([*b.get(o)?, *b.get(o + 1)?])) };
    let rd32 = |o: usize| -> Option<u32> { Some(u32::from_le_bytes([*b.get(o)?, *b.get(o + 1)?, *b.get(o + 2)?, *b.get(o + 3)?])) };
    let ss = 1usize << rd16(0x1E)?;
    let mss = 1usize << rd16(0x20)?;
    let sector = |s: u32| 512.max(ss) + s as usize * ss - if ss == 512 { 0 } else { ss - 512 };
    let sector = |s: u32| if ss == 512 { 512 + s as usize * 512 } else { sector(s) };
    // FAT from the DIFAT in the header (first 109 entries; larger files use DIFAT sectors).
    let mut fat_sectors = vec![];
    for k in 0..109 {
        let s = rd32(0x4C + k * 4)?;
        if s < 0xFFFF_FFFA {
            fat_sectors.push(s);
        }
    }
    let mut difat = rd32(0x44)?;
    let mut guard = 0;
    while difat < 0xFFFF_FFFA && guard < 1000 {
        guard += 1;
        let base = sector(difat);
        for k in 0..(ss / 4 - 1) {
            let s = rd32(base + k * 4)?;
            if s < 0xFFFF_FFFA {
                fat_sectors.push(s);
            }
        }
        difat = rd32(base + ss - 4)?;
    }
    let mut fat = vec![];
    for s in fat_sectors {
        let base = sector(s);
        for k in 0..ss / 4 {
            fat.push(rd32(base + k * 4)?);
        }
    }
    let chain = |start: u32| -> Vec<u8> {
        let mut v = vec![];
        let mut s = start;
        let mut g = 0;
        while s < 0xFFFF_FFFA && (s as usize) < fat.len() && g < 1_000_000 {
            g += 1;
            let base = sector(s);
            if let Some(chunk) = b.get(base..base + ss) {
                v.extend_from_slice(chunk);
            }
            s = fat[s as usize];
        }
        v
    };
    let dir = chain(rd32(0x30)?);
    let minifat = chain(rd32(0x3C)?);
    let minifat: Vec<u32> = minifat.chunks(4).filter(|c| c.len() == 4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let cutoff = rd32(0x38)? as usize;
    let mut root_stream = vec![];
    let mut out = HashMap::new();
    for (i, e) in dir.chunks(128).enumerate() {
        if e.len() < 128 {
            break;
        }
        let name_len = u16::from_le_bytes([e[64], e[65]]) as usize;
        let u: Vec<u16> = e[..name_len.saturating_sub(2).min(64)].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let name = String::from_utf16_lossy(&u);
        let ty = e[66];
        let start = u32::from_le_bytes([e[116], e[117], e[118], e[119]]);
        let size = u32::from_le_bytes([e[120], e[121], e[122], e[123]]) as usize;
        if i == 0 {
            root_stream = chain(start);
            continue;
        }
        if ty != 2 {
            continue;
        }
        let data = if size < cutoff {
            let mut v = vec![];
            let mut s = start;
            let mut g = 0;
            while s < 0xFFFF_FFFA && (s as usize) < minifat.len() && g < 1_000_000 {
                g += 1;
                let off = s as usize * mss;
                if let Some(chunk) = root_stream.get(off..off + mss) {
                    v.extend_from_slice(chunk);
                }
                s = minifat[s as usize];
            }
            v
        } else {
            chain(start)
        };
        out.insert(name, data.into_iter().take(size).collect());
    }
    Some(out)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        kui::set_title(ctx, APP, Some(&self.name()), self.dirty());
        if let Some(p) = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).next()) {
            let ext = kit42::ext(&p);
            if ["png", "jpg", "jpeg", "gif", "bmp", "webp"].contains(&ext.as_str()) {
                if let Some(pic) = std::fs::read(&p).ok().and_then(|d| Picture::new(d, self.doc.setup.text_width())) {
                    self.insert_block(Block::Picture(pic));
                }
            } else {
                self.open(p);
            }
        }
        self.shortcuts(ctx);
        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu(ui));
        egui::TopBottomPanel::top("ribbon").frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(egui::Margin::symmetric(6, 4))).show(ctx, |ui| self.ribbon(ui));
        if matches!(self.dialog, Some(Dialog::Find)) {
            egui::TopBottomPanel::top("find").show(ctx, |ui| self.find_bar(ui));
        }
        let (words, ..) = self.doc.stats();
        let mut zoom = self.zoom;
        kui::status_bar(ctx, |ui| {
            ui.label(format!("{} {} / {} · {} {words} · {}", t("Page", "Страница"), self.cur_page + 1, self.page_count, t("words:", "слов:"), self.status));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut zoom, 0.3..=3.0).show_value(false));
                ui.label(format!("{:.0}%", zoom * 100.0));
            });
        });
        if zoom != self.zoom {
            self.zoom = zoom;
            settings::set("doc42.zoom", zoom);
        }
        if self.show_nav {
            egui::SidePanel::left("nav").default_width(220.0).resizable(true).show(ctx, |ui| self.navigator(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(if ctx.style().visuals.dark_mode { egui::Color32::from_gray(45) } else { egui::Color32::from_gray(228) }))
            .show(ctx, |ui| self.editor(ui));
        if let Some(img) = self.picker.show(ctx) {
            let mut data = Vec::new();
            if image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut data), image::ImageFormat::Png).is_ok() {
                if let Some(mut pic) = Picture::new(data, 180.0) {
                    pic.align = Align2::Left;
                    self.insert_block(Block::Picture(pic));
                }
            }
        }
        self.dialogs(ctx);
        let name = self.name();
        let dirty = self.dirty();
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
        if kui::shortcut(ctx, CTRL, Key::Z) {
            self.undo();
        }
        if kui::shortcut(ctx, CTRL, Key::Y) || kui::shortcut(ctx, CTRL_SHIFT, Key::Z) {
            self.redo();
        }
        if kui::shortcut(ctx, CTRL, Key::S) {
            self.save();
        }
        if kui::shortcut(ctx, CTRL_SHIFT, Key::S) {
            self.save_as();
        }
        if kui::shortcut(ctx, CTRL, Key::O) {
            self.open_dialog();
        }
        if kui::shortcut(ctx, CTRL, Key::N) {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).spawn();
            }
        }
        if kui::shortcut(ctx, CTRL, Key::P) {
            self.print();
        }
        if kui::shortcut(ctx, CTRL, Key::F) || kui::shortcut(ctx, CTRL, Key::H) {
            self.dialog = Some(Dialog::Find);
        }
        if kui::shortcut(ctx, CTRL, Key::B) {
            let v = !self.current_fmt().bold;
            self.apply_char(t("Bold", "Полужирный"), move |f| f.bold = v);
        }
        if kui::shortcut(ctx, CTRL, Key::I) {
            let v = !self.current_fmt().italic;
            self.apply_char(t("Italic", "Курсив"), move |f| f.italic = v);
        }
        if kui::shortcut(ctx, CTRL, Key::U) {
            let v = !self.current_fmt().underline;
            self.apply_char(t("Underline", "Подчёркнутый"), move |f| f.underline = v);
        }
        if kui::shortcut(ctx, CTRL, Key::E) {
            self.apply_para(t("Align", "Выравнивание"), |p, _| p.align = Align2::Center);
        }
        if kui::shortcut(ctx, CTRL, Key::L) {
            self.apply_para(t("Align", "Выравнивание"), |p, _| p.align = Align2::Left);
        }
        if kui::shortcut(ctx, CTRL, Key::R) {
            self.apply_para(t("Align", "Выравнивание"), |p, _| p.align = Align2::Right);
        }
        if kui::shortcut(ctx, CTRL, Key::J) {
            self.apply_para(t("Align", "Выравнивание"), |p, _| p.align = Align2::Justify);
        }
        for (k, st) in [(Key::Num0, ParaStyle::Body), (Key::Num1, ParaStyle::Heading(1)), (Key::Num2, ParaStyle::Heading(2)), (Key::Num3, ParaStyle::Heading(3))] {
            if kui::shortcut(ctx, CTRL_ALT, k) {
                self.apply_para(t("Style", "Стиль"), move |p, body| set_style(p, st, body));
            }
        }
        if kui::shortcut(ctx, CTRL, Key::Enter) {
            self.insert_block(Block::PageBreak);
        }
        if kui::shortcut(ctx, CTRL, Key::K) {
            self.dialog = Some(Dialog::Link { url: self.current_fmt().link.unwrap_or_default() });
        }
        if kui::shortcut(ctx, CTRL, Key::A) && self.focus.is_some() {
            // TextEdit selects the paragraph; formatting commands then apply to the whole document.
            self.select_all = true;
            if let Some(l) = self.focus {
                let n = get_para(&self.doc, l).map(|p| p.char_len()).unwrap_or(0);
                self.pending_focus = Some((l, 0, n));
            }
        }
        if kui::shortcut(ctx, CTRL, Key::F5) || kui::shortcut(ctx, NONE, Key::F5) {
            self.show_nav = !self.show_nav;
        }
        // Tab / Shift+Tab at the start of a list item change its level.
        if let (Some(l), Some((0, 0))) = (self.focus, self.sel) {
            if get_para(&self.doc, l).map(|p| p.list.is_some()).unwrap_or(false) {
                let shift = ctx.input(|i| i.modifiers.shift);
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, Key::Tab) || i.consume_key(egui::Modifiers::NONE, Key::Tab)) {
                    self.apply_para(t("List Level", "Уровень списка"), move |p, _| {
                        if let Some(li) = &mut p.list {
                            li.level = if shift { li.level.saturating_sub(1) } else { (li.level + 1).min(8) };
                        }
                    });
                }
            }
        }
        if let Some(b) = self.selected_picture {
            if !ctx.wants_keyboard_input() && (kui::shortcut(ctx, NONE, Key::Delete) || kui::shortcut(ctx, NONE, Key::Backspace)) {
                self.checkpoint(t("Delete Picture", "Удаление рисунка"));
                if b < self.doc.blocks.len() {
                    self.doc.blocks.remove(b);
                }
                self.selected_picture = None;
                self.doc.normalize();
            }
        }
    }

    fn undo(&mut self) {
        if let Some(d) = self.history.undo(self.doc.clone()) {
            self.doc = d;
            self.last_edit = None;
            self.after_restore();
        }
    }

    fn redo(&mut self) {
        if let Some(d) = self.history.redo(self.doc.clone()) {
            self.doc = d;
            self.last_edit = None;
            self.after_restore();
        }
    }

    fn after_restore(&mut self) {
        self.doc.normalize();
        if let Some(l) = self.focus {
            if get_para(&self.doc, l).is_none() {
                self.focus = Some(Loc::Para(0));
            }
        }
        let l = self.focus.unwrap_or(Loc::Para(0));
        let n = get_para(&self.doc, l).map(|p| p.char_len()).unwrap_or(0);
        let pos = self.sel.map(|s| s.1.min(n)).unwrap_or(0);
        self.pending_focus = Some((l, pos, pos));
    }

    fn open_dialog(&mut self) {
        if let Some(p) = kui::open_dialog(t("Open", "Открыть"), &[
            (t("Documents", "Документы"), &["docx", "doc", "odt", "rtf", "md", "markdown", "html", "htm", "txt"]),
            ("Word", &["docx", "doc"]),
            ("OpenDocument", &["odt"]),
            ("RTF", &["rtf"]),
            ("Markdown", &["md", "markdown"]),
            ("HTML", &["html", "htm"]),
            (t("Text", "Текст"), &["txt"]),
        ])
        .pick_file()
        {
            self.open(p);
        }
    }

    fn print(&mut self) {
        let tmp = kit42::shell::temp_file("print", "pdf");
        if std::fs::write(&tmp, pdfout::export(&self.doc)).is_ok() {
            kit42::shell::print(&tmp);
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
                if kui::menu_item(ui, t("Open…", "Открыть…"), "Ctrl+O") {
                    self.open_dialog();
                }
                ui.menu_button(t("Open Recent", "Последние файлы"), |ui| {
                    for p in settings::recent("doc42") {
                        if ui.button(kit42::file_name(&p)).on_hover_text(p.display().to_string()).clicked() {
                            self.open(p);
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button(t("Clear Menu", "Очистить список")).clicked() {
                        settings::clear_recent("doc42");
                        ui.close_menu();
                    }
                });
                ui.separator();
                if kui::menu_item(ui, t("Save", "Сохранить"), "Ctrl+S") {
                    self.save();
                }
                if kui::menu_item(ui, t("Save As…", "Сохранить как…"), "Ctrl+Shift+S") {
                    self.save_as();
                }
                if kui::menu_item(ui, t("Export as PDF…", "Экспорт в PDF…"), "") {
                    self.export_pdf();
                }
                ui.separator();
                if kui::menu_item(ui, t("Page Setup…", "Параметры страницы…"), "") {
                    self.dialog = Some(Dialog::PageSetup(self.doc.setup.clone()));
                }
                if kui::menu_item(ui, t("Properties…", "Свойства…"), "") {
                    self.dialog = Some(Dialog::Properties(self.doc.meta.clone()));
                }
                if kui::menu_item(ui, t("Print…", "Печать…"), "Ctrl+P") {
                    self.print();
                }
                ui.separator();
                if kui::menu_item(ui, t("Exit", "Выход"), "Alt+F4") {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button(t("Edit", "Правка"), |ui| {
                if kui::menu_item_enabled(ui, self.history.can_undo(), t("Undo", "Отменить"), "Ctrl+Z") {
                    self.undo();
                }
                if kui::menu_item_enabled(ui, self.history.can_redo(), t("Redo", "Повторить"), "Ctrl+Y") {
                    self.redo();
                }
                ui.separator();
                if kui::menu_item(ui, t("Find and Replace…", "Найти и заменить…"), "Ctrl+F") {
                    self.dialog = Some(Dialog::Find);
                }
                if kui::menu_item(ui, t("Select All (format whole document)", "Выделить всё (формат всего документа)"), "Ctrl+A") {
                    self.select_all = true;
                }
            });
            ui.menu_button(t("View", "Вид"), |ui| {
                if kui::menu_item(ui, t("Navigator", "Навигатор"), "F5") {
                    self.show_nav = !self.show_nav;
                }
                for z in [0.5f32, 0.75, 1.0, 1.25, 1.5, 2.0] {
                    if ui.radio((self.zoom - z).abs() < 0.01, format!("{:.0}%", z * 100.0)).clicked() {
                        self.zoom = z;
                        ui.close_menu();
                    }
                }
                if kui::menu_item(ui, t("Full Screen", "Во весь экран"), "F11") {
                    let fs = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
                }
            });
            ui.menu_button(t("Insert", "Вставка"), |ui| {
                if kui::menu_item(ui, t("Table…", "Таблица…"), "") {
                    self.dialog = Some(Dialog::Table { rows: 3, cols: 3 });
                }
                if kui::menu_item(ui, t("Picture…", "Рисунок…"), "") {
                    self.insert_picture();
                }
                if kui::menu_item(ui, t("Page Break", "Разрыв страницы"), "Ctrl+Enter") {
                    self.insert_block(Block::PageBreak);
                }
                if kui::menu_item(ui, t("Hyperlink…", "Гиперссылка…"), "Ctrl+K") {
                    self.dialog = Some(Dialog::Link { url: self.current_fmt().link.unwrap_or_default() });
                }
                if kui::menu_item(ui, t("Table of Contents", "Оглавление"), "") {
                    self.insert_toc();
                }
                if kui::menu_item(ui, t("Special Character…", "Специальный символ…"), "") {
                    self.dialog = Some(Dialog::Special);
                }
                ui.menu_button(t("Field", "Поле"), |ui| {
                    let date = chrono::Local::now().format("%d.%m.%Y").to_string();
                    let time = chrono::Local::now().format("%H:%M").to_string();
                    let title = self.doc.meta.title.clone();
                    let author = self.doc.meta.author.clone();
                    let file = self.name();
                    for (label, v) in [(t("Date", "Дата"), date), (t("Time", "Время"), time), (t("Title", "Заголовок"), title), (t("Author", "Автор"), author), (t("File Name", "Имя файла"), file)] {
                        if ui.button(label).clicked() {
                            self.insert_text(&v);
                            ui.close_menu();
                        }
                    }
                });
                if kui::menu_item(ui, t("Signature or Stamp…", "Подпись или печать…"), "Ctrl+Shift+K") {
                    self.picker.open = true;
                }
            });
            ui.menu_button(t("Format", "Формат"), |ui| {
                ui.menu_button(t("Style", "Стиль"), |ui| {
                    for st in ParaStyle::ALL {
                        if ui.button(st.name()).clicked() {
                            self.apply_para(t("Style", "Стиль"), move |p, body| set_style(p, st, body));
                            ui.close_menu();
                        }
                    }
                });
                if kui::menu_item(ui, t("Paragraph…", "Абзац…"), "") {
                    if let Some(p) = self.current_para() {
                        self.dialog = Some(Dialog::Paragraph { indent: [p.indent_left, p.indent_right, p.indent_first], spacing: [p.space_before, p.space_after], line: p.line_spacing });
                    }
                }
                if kui::menu_item(ui, t("Clear Formatting", "Очистить формат"), "") {
                    self.apply_para(t("Clear Formatting", "Очистка формата"), |p, body| {
                        p.apply_all(&mut |f| {
                            let link = f.link.clone();
                            *f = p_style_fmt(body);
                            f.link = link;
                        });
                        set_style(p, ParaStyle::Body, body);
                    });
                }
                ui.separator();
                for (k, l) in [(ListKind::Bullet, t("Bullets", "Маркеры")), (ListKind::Number, t("Numbering", "Нумерация")), (ListKind::LowerLetter, "a, b, c"), (ListKind::UpperRoman, "I, II, III"), (ListKind::Check, t("Checklist", "Флажки"))] {
                    if ui.button(l).clicked() {
                        self.toggle_list(k);
                        ui.close_menu();
                    }
                }
                ui.separator();
                if kui::menu_item(ui, t("Superscript", "Надстрочный"), "") {
                    let v = if self.current_fmt().script == 1 { 0 } else { 1 };
                    self.apply_char(t("Superscript", "Надстрочный"), move |f| f.script = v);
                }
                if kui::menu_item(ui, t("Subscript", "Подстрочный"), "") {
                    let v = if self.current_fmt().script == -1 { 0 } else { -1 };
                    self.apply_char(t("Subscript", "Подстрочный"), move |f| f.script = v);
                }
                if kui::menu_item(ui, t("Bottom Border", "Нижняя граница"), "") {
                    self.apply_para(t("Border", "Граница"), |p, _| p.border_bottom = !p.border_bottom);
                }
            });
            ui.menu_button(t("Table", "Таблица"), |ui| {
                let in_table = matches!(self.focus, Some(Loc::Cell(..)));
                if kui::menu_item(ui, t("Insert Table…", "Вставить таблицу…"), "") {
                    self.dialog = Some(Dialog::Table { rows: 3, cols: 3 });
                }
                ui.separator();
                for (label, op) in [
                    (t("Insert Row Above", "Вставить строку выше"), 0),
                    (t("Insert Row Below", "Вставить строку ниже"), 1),
                    (t("Insert Column Left", "Вставить столбец слева"), 2),
                    (t("Insert Column Right", "Вставить столбец справа"), 3),
                    (t("Delete Row", "Удалить строку"), 4),
                    (t("Delete Column", "Удалить столбец"), 5),
                    (t("Delete Table", "Удалить таблицу"), 6),
                    (t("Header Row", "Строка заголовка"), 7),
                    (t("Borders On/Off", "Границы вкл./выкл."), 8),
                ] {
                    if kui::menu_item_enabled(ui, in_table, label, "") {
                        self.table_op(op);
                    }
                }
                ui.menu_button(t("Cell Shading", "Заливка ячейки"), |ui| {
                    let mut c = [255u8, 255, 255];
                    if kui::palette(ui, &mut c) {
                        if let Some(Loc::Cell(b, r, col)) = self.focus {
                            self.checkpoint(t("Shading", "Заливка"));
                            if let Some(Block::Table(tb)) = self.doc.blocks.get_mut(b) {
                                tb.rows[r][col].shading = if c == [255, 255, 255] { None } else { Some(c) };
                            }
                        }
                        ui.close_menu();
                    }
                });
            });
            ui.menu_button(t("Tools", "Сервис"), |ui| {
                if kui::menu_item(ui, t("Word Count…", "Статистика…"), "") {
                    self.dialog = Some(Dialog::Stats);
                }
                if kui::menu_item(ui, t("Update Table of Contents", "Обновить оглавление"), "") {
                    // Remove an existing generated contents block (heading "Contents" + link lines) and rebuild.
                    let title = t("Contents", "Содержание");
                    if let Some(start) = self.doc.blocks.iter().position(|b| matches!(b, Block::Para(p) if p.text() == title)) {
                        self.checkpoint(t("Update Contents", "Обновление оглавления"));
                        let mut end = start + 1;
                        while matches!(self.doc.blocks.get(end), Some(Block::Para(p)) if p.runs.first().map(|r| r.fmt.link.as_deref().unwrap_or("").starts_with("#h")).unwrap_or(false)) {
                            end += 1;
                        }
                        if matches!(self.doc.blocks.get(end), Some(Block::PageBreak)) {
                            end += 1;
                        }
                        self.doc.blocks.drain(start..end);
                        self.focus = Some(Loc::Para(start.min(self.doc.blocks.len().saturating_sub(1))));
                    }
                    self.insert_toc();
                }
            });
            ui.menu_button(t("Help", "Справка"), |ui| {
                kui::language_menu(ui);
                if kui::menu_item(ui, t("About DOC42", "О программе DOC42"), "") {
                    kui::about(APP, t("documents and page layout", "документы и вёрстка страниц"));
                }
            });
        });
    }

    fn table_op(&mut self, op: u8) {
        let Some(Loc::Cell(b, r, c)) = self.focus else { return };
        self.checkpoint(t("Table", "Таблица"));
        let body = self.doc.body.clone();
        let mut remove = false;
        if let Some(Block::Table(tb)) = self.doc.blocks.get_mut(b) {
            match op {
                0 => tb.insert_row(r, &body),
                1 => tb.insert_row(r + 1, &body),
                2 => tb.insert_col(c, &body),
                3 => tb.insert_col(c + 1, &body),
                4 => {
                    if tb.rows.len() == 1 {
                        remove = true;
                    } else {
                        tb.delete_row(r);
                    }
                }
                5 => {
                    if tb.cols() == 1 {
                        remove = true;
                    } else {
                        tb.delete_col(c);
                    }
                }
                6 => remove = true,
                7 => tb.header = !tb.header,
                _ => tb.borders = !tb.borders,
            }
        }
        if remove {
            self.doc.blocks.remove(b);
            self.doc.normalize();
            self.focus = Some(Loc::Para(b.min(self.doc.blocks.len() - 1)));
            self.pending_focus = self.focus.map(|l| (l, 0, 0));
        } else {
            let (rows, cols) = match self.doc.blocks.get(b) {
                Some(Block::Table(t)) => (t.rows.len(), t.cols()),
                _ => (1, 1),
            };
            let l = Loc::Cell(b, r.min(rows - 1), c.min(cols - 1));
            self.focus = Some(l);
            self.pending_focus = Some((l, 0, 0));
        }
    }

    fn insert_picture(&mut self) {
        if let Some(p) = kui::open_dialog(t("Insert Picture", "Вставить рисунок"), &[(t("Images", "Изображения"), &["png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff"])]).pick_file() {
            match std::fs::read(&p).ok().and_then(|d| Picture::new(d, self.doc.setup.text_width())) {
                Some(pic) => self.insert_block(Block::Picture(pic)),
                None => kui::error(APP, t("The picture could not be read.", "Не удалось прочитать рисунок.")),
            }
        }
    }

    fn ribbon(&mut self, ui: &mut egui::Ui) {
        let f = self.current_fmt();
        let para = self.current_para().cloned();
        ui.horizontal_wrapped(|ui| {
            if kui::tool_button(ui, ic::OPEN, t("Open", "Открыть"), true).clicked() {
                self.open_dialog();
            }
            if kui::tool_button(ui, ic::SAVE, t("Save", "Сохранить"), true).clicked() {
                self.save();
            }
            if kui::tool_button(ui, ic::PRINT, t("Print", "Печать"), true).clicked() {
                self.print();
            }
            if kui::tool_button(ui, "\u{EA90}", t("Export as PDF", "Экспорт в PDF"), true).clicked() {
                self.export_pdf();
            }
            ui.separator();
            if kui::tool_button(ui, ic::UNDO, t("Undo", "Отменить"), self.history.can_undo()).clicked() {
                self.undo();
            }
            if kui::tool_button(ui, ic::REDO, t("Redo", "Повторить"), self.history.can_redo()).clicked() {
                self.redo();
            }
            ui.separator();
            let style = para.as_ref().map(|p| p.style).unwrap_or(ParaStyle::Body);
            egui::ComboBox::from_id_salt("style").width(130.0).selected_text(style.name()).show_ui(ui, |ui| {
                for st in ParaStyle::ALL {
                    let base = st.base_fmt(&self.doc.body);
                    let txt = egui::RichText::new(st.name()).size((base.size * 1.1).clamp(11.0, 22.0)).color(kit42::richtext::c32(base.color));
                    let txt = if base.bold { txt.strong() } else { txt };
                    if ui.selectable_label(style == st, txt).clicked() {
                        self.apply_para(t("Style", "Стиль"), move |p, body| set_style(p, st, body));
                    }
                }
            });
            if self.fonts.is_empty() {
                self.fonts = kit42::fonts::family_names();
            }
            egui::ComboBox::from_id_salt("font").width(140.0).selected_text(&f.font).height(400.0).show_ui(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.font_filter).hint_text(t("Search", "Поиск")));
                let filter = self.font_filter.to_lowercase();
                let fonts = self.fonts.clone();
                for name in fonts.iter().filter(|n| filter.is_empty() || n.to_lowercase().contains(&filter)).take(300) {
                    if ui.selectable_label(*name == f.font, name).clicked() {
                        let n = name.clone();
                        self.apply_char(t("Font", "Шрифт"), move |x| x.font = n.clone());
                    }
                }
            });
            egui::ComboBox::from_id_salt("size").width(52.0).selected_text(format!("{}", f.size)).show_ui(ui, |ui| {
                for s in [8.0f32, 9.0, 10.0, 10.5, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0, 28.0, 32.0, 36.0, 48.0, 72.0] {
                    if ui.selectable_label(f.size == s, format!("{s}")).clicked() {
                        self.apply_char(t("Size", "Размер"), move |x| x.size = s);
                    }
                }
            });
            if kui::tool_toggle(ui, f.bold, ic::BOLD, t("Bold (Ctrl+B)", "Полужирный (Ctrl+B)")).clicked() {
                let v = !f.bold;
                self.apply_char(t("Bold", "Полужирный"), move |x| x.bold = v);
            }
            if kui::tool_toggle(ui, f.italic, ic::ITALIC, t("Italic (Ctrl+I)", "Курсив (Ctrl+I)")).clicked() {
                let v = !f.italic;
                self.apply_char(t("Italic", "Курсив"), move |x| x.italic = v);
            }
            if kui::tool_toggle(ui, f.underline, ic::UNDERLINE, t("Underline (Ctrl+U)", "Подчёркнутый (Ctrl+U)")).clicked() {
                let v = !f.underline;
                self.apply_char(t("Underline", "Подчёркнутый"), move |x| x.underline = v);
            }
            if kui::tool_toggle(ui, f.strike, "\u{EDE0}", t("Strikethrough", "Зачёркнутый")).clicked() {
                let v = !f.strike;
                self.apply_char(t("Strikethrough", "Зачёркнутый"), move |x| x.strike = v);
            }
            let mut color = f.color;
            if kui::color_button(ui, &mut color, t("Text Colour", "Цвет текста")) {
                self.apply_char(t("Colour", "Цвет"), move |x| x.color = color);
            }
            let mut hl = f.highlight.unwrap_or([255, 255, 0]);
            if kui::color_button(ui, &mut hl, t("Highlight", "Выделение цветом")) {
                let v = if hl == [255, 255, 255] { None } else { Some(hl) };
                self.apply_char(t("Highlight", "Выделение"), move |x| x.highlight = v);
            }
            ui.separator();
            let align = para.as_ref().map(|p| p.align).unwrap_or(Align2::Left);
            for (a, icon, tip) in [(Align2::Left, ic::ALIGN_LEFT, t("Left", "По левому краю")), (Align2::Center, ic::ALIGN_CENTER, t("Centre", "По центру")), (Align2::Right, ic::ALIGN_RIGHT, t("Right", "По правому краю")), (Align2::Justify, "\u{E8E4}", t("Justify", "По ширине"))] {
                let icon = if a == Align2::Justify { "\u{E700}" } else { icon };
                if kui::tool_toggle(ui, align == a, icon, tip).clicked() {
                    self.apply_para(t("Align", "Выравнивание"), move |p, _| p.align = a);
                }
            }
            ui.separator();
            let lk = para.as_ref().and_then(|p| p.list.as_ref().map(|l| l.kind));
            if kui::tool_toggle(ui, lk == Some(ListKind::Bullet), ic::LIST, t("Bullets", "Маркеры")).clicked() {
                self.toggle_list(ListKind::Bullet);
            }
            if kui::tool_toggle(ui, lk == Some(ListKind::Number), "\u{E9D5}", t("Numbering", "Нумерация")).clicked() {
                self.toggle_list(ListKind::Number);
            }
            if kui::tool_toggle(ui, lk == Some(ListKind::Check), "\u{E73A}", t("Checklist", "Флажки")).clicked() {
                self.toggle_list(ListKind::Check);
            }
            if kui::tool_button(ui, "\u{E8A0}", t("Decrease Indent", "Уменьшить отступ"), true).clicked() {
                self.apply_para(t("Indent", "Отступ"), |p, _| match &mut p.list {
                    Some(l) => l.level = l.level.saturating_sub(1),
                    None => p.indent_left = (p.indent_left - 36.0).max(0.0),
                });
            }
            if kui::tool_button(ui, "\u{E8A1}", t("Increase Indent", "Увеличить отступ"), true).clicked() {
                self.apply_para(t("Indent", "Отступ"), |p, _| match &mut p.list {
                    Some(l) => l.level = (l.level + 1).min(8),
                    None => p.indent_left += 36.0,
                });
            }
            ui.menu_button(egui::RichText::new("\u{E9E9}").family(egui::FontFamily::Name(kit42::fonts::ICON_FAMILY.into())), |ui| {
                ui.label(t("Line spacing", "Междустрочный интервал"));
                for v in [1.0f32, 1.15, 1.5, 2.0, 2.5, 3.0] {
                    if ui.selectable_label(para.as_ref().map(|p| (p.line_spacing - v).abs() < 0.01).unwrap_or(false), format!("{v}")).clicked() {
                        self.apply_para(t("Line Spacing", "Интервал"), move |p, _| p.line_spacing = v);
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.button(t("Add space before", "Добавить интервал перед")).clicked() {
                    self.apply_para(t("Spacing", "Интервал"), |p, _| p.space_before += 6.0);
                    ui.close_menu();
                }
                if ui.button(t("Remove space after", "Убрать интервал после")).clicked() {
                    self.apply_para(t("Spacing", "Интервал"), |p, _| p.space_after = 0.0);
                    ui.close_menu();
                }
            })
            .response
            .on_hover_text(t("Line and paragraph spacing", "Интервалы"));
            ui.separator();
            if kui::tool_button(ui, ic::TABLE, t("Insert Table", "Вставить таблицу"), true).clicked() {
                self.dialog = Some(Dialog::Table { rows: 3, cols: 3 });
            }
            if kui::tool_button(ui, ic::PICTURE, t("Insert Picture", "Вставить рисунок"), true).clicked() {
                self.insert_picture();
            }
            if kui::tool_button(ui, ic::LINK, t("Hyperlink", "Гиперссылка"), true).clicked() {
                self.dialog = Some(Dialog::Link { url: f.link.clone().unwrap_or_default() });
            }
            if kui::tool_button(ui, ic::SIGNATURE, t("Signature or Stamp", "Подпись или печать"), true).clicked() {
                self.picker.open = true;
            }
            if kui::tool_button(ui, ic::SEARCH, t("Find and Replace", "Найти и заменить"), true).clicked() {
                self.dialog = Some(Dialog::Find);
            }
            if kui::tool_toggle(ui, self.show_nav, ic::SIDEBAR, t("Navigator", "Навигатор")).clicked() {
                self.show_nav = !self.show_nav;
            }
            if self.select_all {
                ui.label(egui::RichText::new(t("Whole document", "Весь документ")).color(kui::accent(ui.ctx())));
            }
        });
    }

    fn find_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(t("Find", "Найти"));
            let r = ui.add(egui::TextEdit::singleline(&mut self.find).desired_width(200.0));
            if r.changed() {
                self.find_pos = None;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.find_next();
            }
            ui.label(t("Replace", "Заменить на"));
            ui.add(egui::TextEdit::singleline(&mut self.replace).desired_width(200.0));
            ui.checkbox(&mut self.match_case, t("Match case", "С учётом регистра"));
            ui.checkbox(&mut self.whole_word, t("Whole words", "Слово целиком"));
            if ui.button(t("Find Next", "Найти далее")).clicked() {
                self.find_next();
            }
            if ui.button(t("Replace", "Заменить")).clicked() {
                self.replace_one();
            }
            if ui.button(t("Replace All", "Заменить все")).clicked() {
                self.replace_all();
            }
            if kui::tool_button(ui, ic::CLOSE, t("Close", "Закрыть"), true).clicked() {
                self.dialog = None;
            }
        });
    }

    fn navigator(&mut self, ui: &mut egui::Ui) {
        ui.heading(t("Navigator", "Навигатор"));
        ui.separator();
        let heads = self.doc.headings();
        if heads.is_empty() {
            ui.label(egui::RichText::new(t("Headings appear here.", "Здесь появятся заголовки.")).weak());
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (b, lvl, text) in heads {
                ui.horizontal(|ui| {
                    ui.add_space(lvl as f32 * 12.0);
                    if ui.selectable_label(self.focus.map(|f| f.block()) == Some(b), text.chars().take(60).collect::<String>()).clicked() {
                        self.scroll_to_block = Some(b);
                        self.pending_focus = Some((Loc::Para(b), 0, 0));
                    }
                });
            }
            let tables = self.doc.blocks.iter().enumerate().filter(|(_, b)| matches!(b, Block::Table(_))).count();
            let pics = self.doc.blocks.iter().filter(|b| matches!(b, Block::Picture(_))).count();
            ui.separator();
            ui.label(format!("{}: {tables} · {}: {pics}", t("Tables", "Таблиц"), t("Pictures", "Рисунков")));
        });
    }

    fn dialogs(&mut self, ctx: &Context) {
        let Some(mut d) = self.dialog.take() else { return };
        if matches!(d, Dialog::Find) {
            self.dialog = Some(d);
            return;
        }
        let mut open = true;
        let mut close = false;
        let title = match &d {
            Dialog::PageSetup(_) => t("Page Setup", "Параметры страницы"),
            Dialog::Paragraph { .. } => t("Paragraph", "Абзац"),
            Dialog::Table { .. } => t("Insert Table", "Вставка таблицы"),
            Dialog::Link { .. } => t("Hyperlink", "Гиперссылка"),
            Dialog::Properties(_) => t("Document Properties", "Свойства документа"),
            Dialog::Stats => t("Statistics", "Статистика"),
            Dialog::Special => t("Special Characters", "Специальные символы"),
            Dialog::Find => "",
        };
        egui::Window::new(title).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.set_min_width(340.0);
            match &mut d {
                Dialog::PageSetup(s) => {
                    let cm = 72.0 / 2.54;
                    let papers = [("A4", kit42::pdfgen::A4), ("A3", kit42::pdfgen::A3), ("A5", kit42::pdfgen::A5), ("Letter", kit42::pdfgen::LETTER), ("Legal", kit42::pdfgen::LEGAL)];
                    let land = s.width > s.height;
                    ui.horizontal(|ui| {
                        ui.label(t("Paper:", "Бумага:"));
                        for (n, (w, h)) in papers {
                            let cur = ((s.width.min(s.height) - w).abs() < 1.0) && ((s.width.max(s.height) - h).abs() < 1.0);
                            if ui.selectable_label(cur, n).clicked() {
                                if land {
                                    s.width = h;
                                    s.height = w;
                                } else {
                                    s.width = w;
                                    s.height = h;
                                }
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.radio(!land, t("Portrait", "Книжная")).clicked() && land {
                            std::mem::swap(&mut s.width, &mut s.height);
                        }
                        if ui.radio(land, t("Landscape", "Альбомная")).clicked() && !land {
                            std::mem::swap(&mut s.width, &mut s.height);
                        }
                    });
                    ui.label(t("Margins, cm:", "Поля, см:"));
                    egui::Grid::new("m").show(ui, |ui| {
                        for (i, l) in [t("Left", "Левое"), t("Top", "Верхнее"), t("Right", "Правое"), t("Bottom", "Нижнее")].iter().enumerate() {
                            ui.label(*l);
                            let mut v = s.margins[i] / cm;
                            if ui.add(egui::DragValue::new(&mut v).range(0.0..=10.0).speed(0.05).max_decimals(2)).changed() {
                                s.margins[i] = v * cm;
                            }
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
                    ui.label(t("Header ({page}, {pages}):", "Верхний колонтитул ({page}, {pages}):"));
                    ui.text_edit_singleline(&mut s.header);
                    ui.label(t("Footer:", "Нижний колонтитул:"));
                    ui.text_edit_singleline(&mut s.footer);
                    ui.checkbox(&mut s.first_page_different, t("Different first page", "Особый колонтитул первой страницы"));
                    if ui.button(t("OK", "ОК")).clicked() {
                        if s.text_width() < 36.0 || s.text_height() < 36.0 {
                            kui::error(APP, t("The margins leave too little space for text.", "Поля оставляют слишком мало места для текста."));
                        } else {
                            self.checkpoint(t("Page Setup", "Параметры страницы"));
                            self.doc.setup = s.clone();
                            close = true;
                        }
                    }
                }
                Dialog::Paragraph { indent, spacing, line } => {
                    let cm = 72.0 / 2.54;
                    egui::Grid::new("pg").show(ui, |ui| {
                        for (i, l) in [t("Left indent, cm", "Отступ слева, см"), t("Right indent, cm", "Отступ справа, см"), t("First line, cm", "Первая строка, см")].iter().enumerate() {
                            ui.label(*l);
                            let mut v = indent[i] / cm;
                            if ui.add(egui::DragValue::new(&mut v).range(-5.0..=15.0).speed(0.05).max_decimals(2)).changed() {
                                indent[i] = v * cm;
                            }
                            ui.end_row();
                        }
                        ui.label(t("Space before, pt", "Интервал перед, пт"));
                        ui.add(egui::DragValue::new(&mut spacing[0]).range(0.0..=200.0));
                        ui.end_row();
                        ui.label(t("Space after, pt", "Интервал после, пт"));
                        ui.add(egui::DragValue::new(&mut spacing[1]).range(0.0..=200.0));
                        ui.end_row();
                        ui.label(t("Line spacing", "Междустрочный"));
                        ui.add(egui::DragValue::new(line).range(0.5..=5.0).speed(0.05));
                        ui.end_row();
                    });
                    if ui.button(t("OK", "ОК")).clicked() {
                        let (i, s, l) = (*indent, *spacing, *line);
                        self.apply_para(t("Paragraph", "Абзац"), move |p, _| {
                            p.indent_left = i[0];
                            p.indent_right = i[1];
                            p.indent_first = i[2];
                            p.space_before = s[0];
                            p.space_after = s[1];
                            p.line_spacing = l;
                        });
                        close = true;
                    }
                }
                Dialog::Table { rows, cols } => {
                    ui.horizontal(|ui| {
                        ui.label(t("Rows", "Строк"));
                        ui.add(egui::DragValue::new(rows).range(1..=500));
                        ui.label(t("Columns", "Столбцов"));
                        ui.add(egui::DragValue::new(cols).range(1..=30));
                    });
                    if ui.button(t("Insert", "Вставить")).clicked() {
                        let tb = Table::new(*rows, *cols, self.doc.setup.text_width(), &self.doc.body);
                        self.insert_block(Block::Table(tb));
                        let b = self.pending_focus.map(|p| p.0.block()).unwrap_or(1).saturating_sub(1);
                        self.pending_focus = Some((Loc::Cell(b, 0, 0), 0, 0));
                        close = true;
                    }
                }
                Dialog::Link { url } => {
                    ui.label(t("Address (https://…, mailto:… or #heading):", "Адрес (https://…, mailto:… или #заголовок):"));
                    ui.add(egui::TextEdit::singleline(url).desired_width(360.0));
                    ui.horizontal(|ui| {
                        if ui.button(t("OK", "ОК")).clicked() {
                            let u = url.trim().to_string();
                            let link = if u.is_empty() { None } else if !u.contains(':') && !u.starts_with('#') { Some(format!("https://{u}")) } else { Some(u) };
                            self.apply_char(t("Hyperlink", "Гиперссылка"), move |f| f.link = link.clone());
                            close = true;
                        }
                        if ui.button(t("Remove Link", "Удалить ссылку")).clicked() {
                            self.apply_char(t("Hyperlink", "Гиперссылка"), |f| f.link = None);
                            close = true;
                        }
                        if !url.is_empty() && ui.button(t("Open", "Открыть")).clicked() {
                            kit42::shell::open(url);
                        }
                    });
                }
                Dialog::Properties(m) => {
                    egui::Grid::new("meta").show(ui, |ui| {
                        for (l, v) in [(t("Title", "Название"), &mut m.title), (t("Author", "Автор"), &mut m.author), (t("Subject", "Тема"), &mut m.subject), (t("Keywords", "Ключевые слова"), &mut m.keywords)] {
                            ui.label(l);
                            ui.add(egui::TextEdit::singleline(v).desired_width(280.0));
                            ui.end_row();
                        }
                    });
                    if ui.button(t("OK", "ОК")).clicked() {
                        self.checkpoint(t("Properties", "Свойства"));
                        self.doc.meta = m.clone();
                        close = true;
                    }
                }
                Dialog::Stats => {
                    let (w, c, ns, p) = self.doc.stats();
                    egui::Grid::new("st").show(ui, |ui| {
                        for (l, v) in [
                            (t("Pages", "Страниц"), self.page_count),
                            (t("Words", "Слов"), w),
                            (t("Characters (with spaces)", "Знаков (с пробелами)"), c),
                            (t("Characters (no spaces)", "Знаков (без пробелов)"), ns),
                            (t("Paragraphs", "Абзацев"), p),
                        ] {
                            ui.label(l);
                            ui.label(v.to_string());
                            ui.end_row();
                        }
                    });
                }
                Dialog::Special => {
                    let chars = "©®™§¶†‡•…–—«»„“”‘’‹›°±×÷≈≠≤≥∞√∑∏∫∂∆πΩµαβγδλσφ€£¥₽₴¢←↑→↓↔⇒⇔✓✗★☆♠♣♥♦½¼¾¹²³";
                    egui::Grid::new("sp").show(ui, |ui| {
                        for (i, ch) in chars.chars().enumerate() {
                            if ui.add_sized([30.0, 30.0], egui::Button::new(egui::RichText::new(ch.to_string()).size(18.0))).clicked() {
                                self.insert_text(&ch.to_string());
                            }
                            if i % 12 == 11 {
                                ui.end_row();
                            }
                        }
                    });
                }
                Dialog::Find => {}
            }
        });
        if open && !close {
            self.dialog = Some(d);
        }
    }
}

fn p_style_fmt(body: &CharFmt) -> CharFmt {
    body.clone()
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a == "--convert").unwrap_or(false) {
        kit42::attach_console();
        kit42::settings::load_global();
        let (Some(i), Some(o)) = (args.get(1), args.get(2)) else {
            eprintln!("usage: --convert input output.(docx|odt|rtf|md|html|txt|pdf)");
            std::process::exit(1);
        };
        let res = App::load(std::path::Path::new(i)).and_then(|(doc, fmt, enc)| {
            let app_doc = doc;
            let mut tmp = App {
                doc: app_doc,
                path: None,
                fmt,
                encoding: enc,
                history: History::new(1),
                last_edit: None,
                zoom: 1.0,
                focus: None,
                sel: None,
                pending_focus: None,
                scroll_to_block: None,
                scroll_target_hint: None,
                typing: None,
                select_all: false,
                picker: Default::default(),
                dialog: None,
                pic_tex: HashMap::new(),
                selected_picture: None,
                resizing: false,
                page_count: 1,
                cur_page: 0,
                status: String::new(),
                show_nav: false,
                find: String::new(),
                replace: String::new(),
                match_case: false,
                whole_word: false,
                find_pos: None,
                fonts: vec![],
                font_filter: String::new(),
            };
            let out = std::path::PathBuf::from(o);
            let b = tmp.serialize(&out)?;
            tmp.path = Some(out.clone());
            kit42::write_atomic(&out, &b).map_err(|e| e.to_string())?;
            let (w, ..) = tmp.doc.stats();
            Ok(format!("{} blocks, {w} words -> {o}", tmp.doc.blocks.len()))
        });
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
    kit42::run(kit42::AppSpec { id: "doc42", title: APP, size: [1180.0, 900.0] }, App::new)
}
