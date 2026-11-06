//! Rich paragraphs (runs of formatted text) and an in-place paragraph editor built on egui's
//! TextEdit. DOC42 lays these paragraphs out on pages; PPT42 puts them in text boxes.

use egui::{text::LayoutJob, Align, Color32, FontId, Stroke, TextFormat};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharFmt {
    pub font: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: [u8; 3],
    pub highlight: Option<[u8; 3]>,
    /// -1 subscript, 1 superscript.
    pub script: i8,
    pub link: Option<String>,
}

impl Default for CharFmt {
    fn default() -> Self {
        Self {
            font: "Calibri".into(),
            size: 11.0,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            color: [0, 0, 0],
            highlight: None,
            script: 0,
            link: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    pub fmt: CharFmt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Align2 {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListKind {
    Bullet,
    Number,
    LowerLetter,
    UpperLetter,
    LowerRoman,
    UpperRoman,
    Check,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListInfo {
    pub kind: ListKind,
    pub level: u8,
    pub checked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ParaStyle {
    #[default]
    Body,
    Title,
    Subtitle,
    Heading(u8),
    Quote,
    Code,
    Caption,
}

impl ParaStyle {
    pub const ALL: [ParaStyle; 12] = [
        ParaStyle::Body,
        ParaStyle::Title,
        ParaStyle::Subtitle,
        ParaStyle::Heading(1),
        ParaStyle::Heading(2),
        ParaStyle::Heading(3),
        ParaStyle::Heading(4),
        ParaStyle::Heading(5),
        ParaStyle::Heading(6),
        ParaStyle::Quote,
        ParaStyle::Code,
        ParaStyle::Caption,
    ];

    pub fn name(self) -> String {
        use crate::t;
        match self {
            ParaStyle::Body => t("Body", "Обычный").into(),
            ParaStyle::Title => t("Title", "Заголовок документа").into(),
            ParaStyle::Subtitle => t("Subtitle", "Подзаголовок").into(),
            ParaStyle::Heading(n) => format!("{} {n}", t("Heading", "Заголовок")),
            ParaStyle::Quote => t("Quote", "Цитата").into(),
            ParaStyle::Code => t("Code", "Код").into(),
            ParaStyle::Caption => t("Caption", "Подпись").into(),
        }
    }

    /// Word style id used in DOCX.
    pub fn docx_id(self) -> String {
        match self {
            ParaStyle::Body => "Normal".into(),
            ParaStyle::Title => "Title".into(),
            ParaStyle::Subtitle => "Subtitle".into(),
            ParaStyle::Heading(n) => format!("Heading{n}"),
            ParaStyle::Quote => "Quote".into(),
            ParaStyle::Code => "Code".into(),
            ParaStyle::Caption => "Caption".into(),
        }
    }

    pub fn from_docx_id(id: &str) -> Self {
        let l = id.to_ascii_lowercase().replace([' ', '-'], "");
        if let Some(n) = l.strip_prefix("heading").and_then(|n| n.parse::<u8>().ok()) {
            return ParaStyle::Heading(n.clamp(1, 6));
        }
        if let Some(n) = l.strip_prefix("заголовок").and_then(|n| n.parse::<u8>().ok()) {
            return ParaStyle::Heading(n.clamp(1, 6));
        }
        match l.as_str() {
            "title" | "название" => ParaStyle::Title,
            "subtitle" => ParaStyle::Subtitle,
            "quote" | "intensequote" | "blocktext" => ParaStyle::Quote,
            "code" | "htmlpreformatted" | "sourcecode" | "preformattedtext" => ParaStyle::Code,
            "caption" => ParaStyle::Caption,
            _ => ParaStyle::Body,
        }
    }

    /// The character format a paragraph in this style starts with.
    pub fn base_fmt(self, body: &CharFmt) -> CharFmt {
        let mut f = body.clone();
        match self {
            ParaStyle::Body => {}
            ParaStyle::Title => {
                f.size = 26.0;
                f.font = "Calibri Light".into();
                f.color = [0x1F, 0x37, 0x63];
            }
            ParaStyle::Subtitle => {
                f.size = 15.0;
                f.color = [0x59, 0x59, 0x59];
            }
            ParaStyle::Heading(n) => {
                f.bold = n >= 3;
                f.size = match n {
                    1 => 16.0,
                    2 => 13.0,
                    3 => 12.0,
                    _ => 11.0,
                };
                f.font = if n <= 2 { "Calibri Light".into() } else { body.font.clone() };
                f.color = [0x2F, 0x54, 0x96];
                f.italic = n >= 4 && n % 2 == 0;
            }
            ParaStyle::Quote => {
                f.italic = true;
                f.color = [0x40, 0x40, 0x40];
            }
            ParaStyle::Code => {
                f.font = "Consolas".into();
                f.size = 10.0;
            }
            ParaStyle::Caption => {
                f.italic = true;
                f.size = 9.0;
                f.color = [0x44, 0x54, 0x6A];
            }
        }
        f
    }

    pub fn spacing(self) -> (f32, f32) {
        match self {
            ParaStyle::Title => (0.0, 6.0),
            ParaStyle::Heading(1) => (12.0, 4.0),
            ParaStyle::Heading(_) => (8.0, 3.0),
            ParaStyle::Quote => (6.0, 8.0),
            _ => (0.0, 8.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Para {
    pub runs: Vec<Run>,
    pub style: ParaStyle,
    pub align: Align2,
    pub indent_left: f32,
    pub indent_right: f32,
    pub indent_first: f32,
    pub space_before: f32,
    pub space_after: f32,
    pub line_spacing: f32,
    pub list: Option<ListInfo>,
    pub page_break_before: bool,
    pub shading: Option<[u8; 3]>,
    pub border_bottom: bool,
}

impl Default for Para {
    fn default() -> Self {
        Self::new("", CharFmt::default())
    }
}

impl Para {
    pub fn new(text: &str, fmt: CharFmt) -> Self {
        Self {
            runs: vec![Run { text: text.into(), fmt }],
            style: ParaStyle::Body,
            align: Align2::Left,
            indent_left: 0.0,
            indent_right: 0.0,
            indent_first: 0.0,
            space_before: 0.0,
            space_after: 8.0,
            line_spacing: 1.08,
            list: None,
            page_break_before: false,
            shading: None,
            border_bottom: false,
        }
    }

    pub fn with_style(text: &str, style: ParaStyle, body: &CharFmt) -> Self {
        let mut p = Self::new(text, style.base_fmt(body));
        p.style = style;
        let (b, a) = style.spacing();
        p.space_before = b;
        p.space_after = a;
        if style == ParaStyle::Quote {
            p.indent_left = 36.0;
            p.indent_right = 36.0;
        }
        p
    }

    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    pub fn char_len(&self) -> usize {
        self.runs.iter().map(|r| r.text.chars().count()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(|r| r.text.is_empty())
    }

    /// Format of the character before `pos` (what typing at `pos` continues).
    pub fn fmt_at(&self, pos: usize) -> CharFmt {
        let mut acc = 0;
        let mut last = &self.runs[0].fmt;
        for r in &self.runs {
            let n = r.text.chars().count();
            if pos <= acc + n && n > 0 && pos > acc {
                return r.fmt.clone();
            }
            if n > 0 || acc == 0 {
                last = &r.fmt;
            }
            if pos <= acc && n > 0 {
                return r.fmt.clone();
            }
            acc += n;
        }
        last.clone()
    }

    /// Splits runs so that `pos` falls on a run boundary; returns the run index starting at pos.
    fn split_at(&mut self, pos: usize) -> usize {
        let mut acc = 0;
        for i in 0..self.runs.len() {
            let n = self.runs[i].text.chars().count();
            if pos == acc {
                return i;
            }
            if pos < acc + n {
                let byte = self.runs[i].text.char_indices().nth(pos - acc).map(|(b, _)| b).unwrap_or(self.runs[i].text.len());
                let tail = self.runs[i].text.split_off(byte);
                let fmt = self.runs[i].fmt.clone();
                self.runs.insert(i + 1, Run { text: tail, fmt });
                return i + 1;
            }
            acc += n;
        }
        self.runs.len()
    }

    /// Replaces `len` chars at `pos` with `insert` in format `fmt`.
    pub fn splice(&mut self, pos: usize, len: usize, insert: &str, fmt: &CharFmt) {
        let start = self.split_at(pos);
        let end = self.split_at(pos + len);
        let removed: Vec<Run> = self.runs.drain(start..end).collect();
        if !insert.is_empty() {
            self.runs.insert(start, Run { text: insert.into(), fmt: fmt.clone() });
        } else if self.runs.is_empty() {
            let f = removed.first().map(|r| r.fmt.clone()).unwrap_or_else(|| fmt.clone());
            self.runs.push(Run { text: String::new(), fmt: f });
        }
        self.normalize();
    }

    /// Applies a change to the format of a char range (whole paragraph when start==end==0 && all).
    pub fn apply(&mut self, start: usize, end: usize, f: &mut dyn FnMut(&mut CharFmt)) {
        if self.is_empty() {
            for r in &mut self.runs {
                f(&mut r.fmt);
            }
            return;
        }
        let a = self.split_at(start);
        let b = self.split_at(end);
        for r in &mut self.runs[a..b] {
            f(&mut r.fmt);
        }
        self.normalize();
    }

    pub fn apply_all(&mut self, f: &mut dyn FnMut(&mut CharFmt)) {
        let n = self.char_len();
        if n == 0 {
            for r in &mut self.runs {
                f(&mut r.fmt);
            }
        } else {
            self.apply(0, n, f);
        }
    }

    pub fn normalize(&mut self) {
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for r in self.runs.drain(..) {
            if let Some(last) = out.last_mut() {
                if last.fmt == r.fmt {
                    last.text.push_str(&r.text);
                    continue;
                }
                if last.text.is_empty() {
                    *last = r;
                    continue;
                }
            }
            if r.text.is_empty() && !out.is_empty() {
                continue;
            }
            out.push(r);
        }
        if out.is_empty() {
            out.push(Run { text: String::new(), fmt: CharFmt::default() });
        }
        self.runs = out;
    }

    /// Splits off the text after `pos` into a new paragraph with the same paragraph format.
    pub fn split_off(&mut self, pos: usize) -> Para {
        let i = self.split_at(pos);
        let mut tail_runs: Vec<Run> = self.runs.drain(i..).collect();
        if tail_runs.is_empty() {
            let f = self.fmt_at(pos);
            tail_runs.push(Run { text: String::new(), fmt: f });
        }
        if self.runs.is_empty() {
            self.runs.push(Run { text: String::new(), fmt: tail_runs[0].fmt.clone() });
        }
        let mut next = self.clone();
        next.runs = tail_runs;
        next.page_break_before = false;
        // A heading followed by Enter continues as body text.
        if matches!(self.style, ParaStyle::Heading(_) | ParaStyle::Title | ParaStyle::Subtitle) && next.is_empty() {
            let body = CharFmt { font: next.runs[0].fmt.font.clone(), ..CharFmt::default() };
            next = Para::with_style("", ParaStyle::Body, &body);
        }
        self.normalize();
        next.normalize();
        next
    }

    pub fn append(&mut self, other: Para) {
        if self.is_empty() {
            self.runs = other.runs;
        } else {
            self.runs.extend(other.runs);
        }
        self.normalize();
    }

    /// Plain text with list markers removed.
    pub fn plain(&self) -> String {
        self.text()
    }
}

/// Finds the edit between two strings as (start char, removed chars, inserted text).
pub fn diff(old: &str, new: &str) -> (usize, usize, String) {
    let o: Vec<char> = old.chars().collect();
    let n: Vec<char> = new.chars().collect();
    let mut p = 0;
    while p < o.len() && p < n.len() && o[p] == n[p] {
        p += 1;
    }
    let mut s = 0;
    while s < o.len() - p && s < n.len() - p && o[o.len() - 1 - s] == n[n.len() - 1 - s] {
        s += 1;
    }
    (p, o.len() - p - s, n[p..n.len() - s].iter().collect())
}

pub fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Builds the egui layout job for a paragraph at a zoom (points → screen pixels).
pub fn layout_job(ctx: &egui::Context, para: &Para, wrap: f32, zoom: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    job.halign = Align::LEFT;
    job.justify = para.align == Align2::Justify;
    let mut first = true;
    for r in &para.runs {
        let f = &r.fmt;
        let size = f.size * zoom * if f.script != 0 { 0.65 } else { 1.0 };
        let family = crate::fonts::doc_family(ctx, &f.font, f.bold, f.italic);
        let has_italic_face = crate::fonts::index()
            .get(&f.font)
            .or_else(|| crate::fonts::index().get(&crate::fonts::substitute(&f.font)))
            .map(|s| s.italic.is_some() || s.bold_italic.is_some())
            .unwrap_or(false);
        let color = if f.link.is_some() && f.color == [0, 0, 0] { Color32::from_rgb(0x05, 0x63, 0xC1) } else { c32(f.color) };
        let fmt = TextFormat {
            font_id: FontId::new(size.max(1.0), family),
            color,
            background: f.highlight.map(c32).unwrap_or(Color32::TRANSPARENT),
            italics: f.italic && !has_italic_face,
            underline: if f.underline || f.link.is_some() { Stroke::new((size / 14.0).max(1.0), color) } else { Stroke::NONE },
            strikethrough: if f.strike { Stroke::new((size / 14.0).max(1.0), color) } else { Stroke::NONE },
            valign: match f.script {
                1 => Align::TOP,
                -1 => Align::BOTTOM,
                _ => Align::BOTTOM,
            },
            line_height: Some(f.size * zoom * 1.2 * para.line_spacing.max(0.5)),
            ..Default::default()
        };
        let leading = if first { (para.indent_first * zoom).max(0.0) } else { 0.0 };
        first = false;
        job.append(&r.text, leading, fmt);
    }
    if job.sections.is_empty() || job.text.is_empty() {
        // Keep the line height of an empty paragraph.
        let f = &para.runs[0].fmt;
        let family = crate::fonts::doc_family(ctx, &f.font, f.bold, f.italic);
        job.append(
            "",
            (para.indent_first * zoom).max(0.0),
            TextFormat {
                font_id: FontId::new((f.size * zoom).max(1.0), family),
                color: c32(f.color),
                line_height: Some(f.size * zoom * 1.2 * para.line_spacing.max(0.5)),
                ..Default::default()
            },
        );
    }
    job
}

/// Laid-out galley for measuring a paragraph without editing it.
pub fn galley(ctx: &egui::Context, para: &Para, wrap: f32, zoom: f32) -> Arc<egui::Galley> {
    let job = layout_job(ctx, para, wrap, zoom);
    ctx.fonts(|f| f.layout_job(job))
}

pub fn halign(a: Align2) -> Align {
    match a {
        Align2::Left | Align2::Justify => Align::LEFT,
        Align2::Center => Align::Center,
        Align2::Right => Align::RIGHT,
    }
}

/// What happened in a paragraph editor this frame.
#[derive(Debug)]
pub struct EditOutput {
    pub changed: bool,
    pub focused: bool,
    /// Selected char range (start, end), start <= end.
    pub selection: Option<(usize, usize)>,
    /// Paragraphs created by Enter or pasting several lines (to insert after this one).
    pub new_paras: Vec<Para>,
    /// Cursor position in the last new paragraph after a split.
    pub cursor_after_split: usize,
    pub merge_with_prev: bool,
    pub merge_with_next: bool,
    pub move_prev: bool,
    pub move_next: bool,
    pub rect: egui::Rect,
    pub cursor_rect: Option<egui::Rect>,
}

impl Default for EditOutput {
    fn default() -> Self {
        Self {
            changed: false,
            focused: false,
            selection: None,
            new_paras: Vec::new(),
            cursor_after_split: 0,
            merge_with_prev: false,
            merge_with_next: false,
            move_prev: false,
            move_next: false,
            rect: egui::Rect::NOTHING,
            cursor_rect: None,
        }
    }
}

/// Request to place the caret in a paragraph editor next frame.
pub fn request_cursor(ctx: &egui::Context, id: egui::Id, start: usize, end: usize) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
        egui::text::CCursor::new(start),
        egui::text::CCursor::new(end),
    )));
    egui::TextEdit::store_state(ctx, id, state);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// Edits one paragraph in place. `typing` is the format for newly typed text (set by toolbar
/// toggles with an empty selection); when None the format of the preceding character is used.
pub fn edit_para(
    ui: &mut egui::Ui,
    id: egui::Id,
    para: &mut Para,
    width: f32,
    zoom: f32,
    typing: &mut Option<CharFmt>,
    read_only: bool,
) -> EditOutput {
    let ctx = ui.ctx().clone();
    let mut out = EditOutput::default();
    let has_focus = ctx.memory(|m| m.has_focus(id));
    let state = egui::TextEdit::load_state(&ctx, id);
    let sel = state.as_ref().and_then(|s| s.cursor.char_range()).map(|r| {
        let (a, b) = (r.primary.index, r.secondary.index);
        (a.min(b), a.max(b))
    });
    let len = para.char_len();
    // Keyboard navigation across paragraph boundaries (before TextEdit handles keys).
    if has_focus && !read_only {
        let galley_key = id.with("galley");
        let last_galley: Option<Arc<egui::Galley>> = ctx.data(|d| d.get_temp(galley_key));
        let (bs, del, up, down, left, right, shift) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Backspace),
                i.key_pressed(egui::Key::Delete),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.modifiers.shift,
            )
        });
        if let Some((a, b)) = sel {
            if a == b && !shift {
                if bs && a == 0 {
                    out.merge_with_prev = true;
                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace));
                }
                if del && a == len {
                    out.merge_with_next = true;
                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
                }
                if left && a == 0 {
                    out.move_prev = true;
                }
                if right && a == len {
                    out.move_next = true;
                }
                if let Some(g) = &last_galley {
                    let cur = g.from_ccursor(egui::text::CCursor::new(a));
                    if up && cur.rcursor.row == 0 {
                        out.move_prev = true;
                        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
                    }
                    if down && cur.rcursor.row + 1 >= g.rows.len() {
                        out.move_next = true;
                        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
                    }
                }
            }
        }
    }
    let old_text = para.text();
    let mut text = old_text.clone();
    let typing_fmt = typing.clone();
    let snapshot = para.clone();
    let mut layouter = |ui: &egui::Ui, s: &str, wrap: f32| {
        let mut p = snapshot.clone();
        if s != old_text {
            let (pos, removed, ins) = diff(&old_text, s);
            let f = typing_fmt.clone().unwrap_or_else(|| p.fmt_at(pos));
            p.splice(pos, removed, &ins, &f);
        }
        let job = layout_job(ui.ctx(), &p, wrap, zoom);
        ui.fonts(|f| f.layout_job(job))
    };
    let te = egui::TextEdit::multiline(&mut text)
        .id(id)
        .frame(false)
        .margin(egui::Margin::ZERO)
        .desired_width(width)
        .desired_rows(1)
        .background_color(Color32::TRANSPARENT)
        .horizontal_align(halign(para.align))
        .interactive(!read_only)
        .lock_focus(true)
        .layouter(&mut layouter);
    let output = te.show(ui);
    out.rect = output.response.rect;
    out.focused = output.response.has_focus();
    ctx.data_mut(|d| d.insert_temp(id.with("galley"), output.galley.clone()));
    if let Some(cr) = output.cursor_range {
        let (a, b) = (cr.primary.ccursor.index, cr.secondary.ccursor.index);
        out.selection = Some((a.min(b), a.max(b)));
        if out.focused {
            let r = output.galley.pos_from_cursor(&cr.primary);
            out.cursor_rect = Some(r.translate(output.galley_pos.to_vec2()));
        }
    }
    if text != old_text {
        out.changed = true;
        let (pos, removed, ins) = diff(&old_text, &text);
        let f = typing.take().unwrap_or_else(|| para.fmt_at(pos));
        para.splice(pos, removed, &ins, &f);
        // Enter or a multi-line paste: split into paragraphs.
        if text.contains('\n') {
            let full = para.text();
            let mut cursor_in_last = 0;
            let mut breaks: Vec<usize> = full.chars().enumerate().filter(|(_, c)| *c == '\n').map(|(i, _)| i).collect();
            breaks.reverse();
            let mut tails = Vec::new();
            let cursor_abs = out.selection.map(|s| s.1).unwrap_or(full.chars().count());
            for b in breaks {
                let mut tail = para.split_off(b);
                // Remove the newline that is now at the start of the tail.
                tail.splice(0, 1, "", &tail.fmt_at(0));
                if cursor_abs > b {
                    cursor_in_last = cursor_abs - b - 1;
                }
                tails.push(tail);
            }
            tails.reverse();
            // The cursor goes to the paragraph after the last break before it.
            out.cursor_after_split = cursor_in_last.min(tails.last().map(|t| t.char_len()).unwrap_or(0));
            out.new_paras = tails;
        }
    }
    out
}

/// List marker text for the n-th item (1-based) of a list kind.
pub fn list_marker(kind: ListKind, level: u8, n: usize, checked: bool) -> String {
    match kind {
        ListKind::Bullet => ["•", "◦", "▪"][(level as usize) % 3].into(),
        ListKind::Check => if checked { "☑" } else { "☐" }.into(),
        ListKind::Number => format!("{n}."),
        ListKind::LowerLetter => format!("{}.", letters(n, false)),
        ListKind::UpperLetter => format!("{}.", letters(n, true)),
        ListKind::LowerRoman => format!("{}.", roman(n).to_lowercase()),
        ListKind::UpperRoman => format!("{}.", roman(n)),
    }
}

pub fn letters(mut n: usize, upper: bool) -> String {
    let mut s = Vec::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.push((if upper { b'A' } else { b'a' } + r as u8) as char);
        n = (n - 1) / 26;
    }
    s.iter().rev().collect()
}

pub fn roman(mut n: usize) -> String {
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    let table = [
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"),
        (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"),
    ];
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// Numbers list items in a sequence of paragraphs (restarts when a list is interrupted).
pub fn list_numbers(paras: &[Para]) -> Vec<Option<String>> {
    let mut counters = [0usize; 10];
    let mut kinds: [Option<ListKind>; 10] = [None; 10];
    let mut out = Vec::with_capacity(paras.len());
    for p in paras {
        match &p.list {
            Some(l) => {
                let lv = (l.level as usize).min(9);
                if kinds[lv] != Some(l.kind) {
                    counters[lv] = 0;
                    kinds[lv] = Some(l.kind);
                }
                counters[lv] += 1;
                for c in counters.iter_mut().skip(lv + 1) {
                    *c = 0;
                }
                out.push(Some(list_marker(l.kind, l.level, counters[lv], l.checked)));
            }
            None => {
                counters = [0; 10];
                kinds = [None; 10];
                out.push(None);
            }
        }
    }
    out
}
