//! PDF export of DOC42 documents with its own pagination: word wrapping across runs of
//! different fonts, alignment and justification, lists, tables, pictures, headers/footers.

use crate::model::*;
use kit42::pdfgen::{self, Doc, FontId, Page, TextStyle};
use kit42::richtext::{list_numbers, Align2, CharFmt, Para};
use std::collections::HashMap;

enum Op {
    Text { font: FontId, size: f32, color: [u8; 3], x: f32, y: f32, text: String, underline: bool, strike: bool, link: Option<String>, highlight: Option<[u8; 3]> },
    Fill { x: f32, y: f32, w: f32, h: f32, color: [u8; 3] },
    Stroke { x: f32, y: f32, w: f32, h: f32 },
    Line { x1: f32, y1: f32, x2: f32, y2: f32 },
    Image { id: pdfgen::ImageId, x: f32, y: f32, w: f32, h: f32 },
}

struct Layout<'a> {
    pdf: Doc,
    fonts: HashMap<(String, bool, bool), FontId>,
    pages: Vec<Vec<Op>>,
    y: f32,
    doc: &'a Document,
}

struct Piece {
    text: String,
    fmt: CharFmt,
    width: f32,
    space: bool,
}

impl<'a> Layout<'a> {
    fn font(&mut self, f: &CharFmt) -> FontId {
        let key = (f.font.clone(), f.bold, f.italic);
        if let Some(id) = self.fonts.get(&key) {
            return *id;
        }
        let id = self.pdf.font(&f.font, f.bold, f.italic);
        self.fonts.insert(key, id);
        id
    }

    fn top(&self) -> f32 {
        self.doc.setup.margins[1]
    }

    fn bottom(&self) -> f32 {
        self.doc.setup.height - self.doc.setup.margins[3]
    }

    fn new_page(&mut self) {
        self.pages.push(vec![]);
        self.y = self.top();
    }

    fn ensure(&mut self, h: f32) {
        if self.y + h > self.bottom() && self.y > self.top() + 0.5 {
            self.new_page();
        }
    }

    fn ops(&mut self) -> &mut Vec<Op> {
        self.pages.last_mut().unwrap()
    }

    /// Breaks a paragraph into lines of pieces within `width`.
    fn lines(&mut self, p: &Para, width: f32, first_indent: f32) -> Vec<(Vec<Piece>, f32, f32)> {
        let mut pieces: Vec<Piece> = vec![];
        for r in &p.runs {
            let f = &r.fmt;
            let size = f.size * if f.script != 0 { 0.65 } else { 1.0 };
            let font = self.font(f);
            let mut word = String::new();
            let mut push = |w: &mut String, space: bool, pieces: &mut Vec<Piece>, pdf: &mut Doc| {
                if !w.is_empty() {
                    let width = pdf.measure(font, size, w);
                    pieces.push(Piece { text: std::mem::take(w), fmt: f.clone(), width, space });
                }
            };
            for ch in r.text.chars() {
                match ch {
                    ' ' => {
                        push(&mut word, false, &mut pieces, &mut self.pdf);
                        word.push(' ');
                        push(&mut word, true, &mut pieces, &mut self.pdf);
                    }
                    '\n' => {
                        push(&mut word, false, &mut pieces, &mut self.pdf);
                        pieces.push(Piece { text: "\n".into(), fmt: f.clone(), width: 0.0, space: false });
                    }
                    '\t' => {
                        push(&mut word, false, &mut pieces, &mut self.pdf);
                        pieces.push(Piece { text: "    ".into(), fmt: f.clone(), width: size * 2.0, space: true });
                    }
                    c => word.push(c),
                }
            }
            push(&mut word, false, &mut pieces, &mut self.pdf);
        }
        let mut lines = vec![];
        let mut cur: Vec<Piece> = vec![];
        let mut w = 0.0;
        let mut avail = width - first_indent;
        let line_h = |line: &[Piece], p: &Para| {
            let size = line.iter().map(|x| x.fmt.size).fold(0.0, f32::max).max(p.runs[0].fmt.size);
            size * 1.2 * p.line_spacing.max(0.5)
        };
        for piece in pieces {
            if piece.text == "\n" {
                let h = line_h(&cur, p);
                lines.push((std::mem::take(&mut cur), h, avail));
                w = 0.0;
                avail = width;
                continue;
            }
            if w + piece.width > avail && !cur.is_empty() && !piece.space {
                while cur.last().map(|x| x.space).unwrap_or(false) {
                    cur.pop();
                }
                let h = line_h(&cur, p);
                lines.push((std::mem::take(&mut cur), h, avail));
                w = 0.0;
                avail = width;
            }
            if piece.space && cur.is_empty() && !lines.is_empty() {
                continue;
            }
            // A single word wider than the line is split by characters.
            if piece.width > avail && !piece.space {
                let font = self.font(&piece.fmt);
                let mut chunk = String::new();
                for ch in piece.text.chars() {
                    let cand = format!("{chunk}{ch}");
                    if self.pdf.measure(font, piece.fmt.size, &cand) > avail && !chunk.is_empty() {
                        let cw = self.pdf.measure(font, piece.fmt.size, &chunk);
                        cur.push(Piece { text: std::mem::take(&mut chunk), fmt: piece.fmt.clone(), width: cw, space: false });
                        let h = line_h(&cur, p);
                        lines.push((std::mem::take(&mut cur), h, avail));
                        avail = width;
                    }
                    chunk.push(ch);
                }
                let cw = self.pdf.measure(font, piece.fmt.size, &chunk);
                w = cw;
                cur.push(Piece { text: chunk, fmt: piece.fmt.clone(), width: cw, space: false });
                continue;
            }
            w += piece.width;
            cur.push(piece);
        }
        while cur.last().map(|x| x.space).unwrap_or(false) {
            cur.pop();
        }
        let h = line_h(&cur, p);
        lines.push((cur, h, avail));
        lines
    }

    /// Draws a paragraph at x within width; returns used height. Splits across pages when `paged`.
    fn para(&mut self, p: &Para, x0: f32, width: f32, marker: Option<String>, paged: bool) -> f32 {
        let start_y = self.y;
        self.y += p.space_before;
        let list_indent = p.list.as_ref().map(|l| 18.0 * (l.level as f32 + 1.0)).unwrap_or(0.0);
        let x = x0 + p.indent_left + list_indent;
        let w = (width - p.indent_left - p.indent_right - list_indent).max(20.0);
        let lines = self.lines(p, w, p.indent_first.max(0.0));
        let n = lines.len();
        for (li, (line, h, avail)) in lines.into_iter().enumerate() {
            if paged {
                self.ensure(h);
            }
            let asc = h / 1.2 / p.line_spacing.max(0.5) * 0.85;
            let base = self.y + asc + (h - h / p.line_spacing.max(0.5)) * 0.0;
            if let Some(sh) = p.shading {
                let y = self.y;
                self.ops().push(Op::Fill { x, y, w, h, color: sh });
            }
            if li == 0 {
                if let Some(m) = &marker {
                    let f = p.runs[0].fmt.clone();
                    let font = self.font(&f);
                    let mw = self.pdf.measure(font, f.size, m);
                    self.ops().push(Op::Text { font, size: f.size, color: f.color, x: x - mw - 6.0, y: base, text: m.clone(), underline: false, strike: false, link: None, highlight: None });
                }
            }
            let used: f32 = line.iter().map(|pc| pc.width).sum();
            let indent = if li == 0 { p.indent_first.max(0.0) } else { 0.0 };
            let spaces = line.iter().filter(|pc| pc.space).count();
            let extra = avail - used;
            let (mut cx, gap) = match p.align {
                Align2::Left => (x + indent, 0.0),
                Align2::Center => (x + indent + extra / 2.0, 0.0),
                Align2::Right => (x + indent + extra, 0.0),
                Align2::Justify if li + 1 < n && spaces > 0 && extra < avail * 0.4 => (x + indent, extra / spaces as f32),
                Align2::Justify => (x + indent, 0.0),
            };
            for pc in line {
                if !pc.space {
                    let f = &pc.fmt;
                    let size = f.size * if f.script != 0 { 0.65 } else { 1.0 };
                    let dy = match f.script {
                        1 => -f.size * 0.35,
                        -1 => f.size * 0.15,
                        _ => 0.0,
                    };
                    let font = self.font(f);
                    let color = if f.link.is_some() && f.color == [0, 0, 0] { [5, 99, 193] } else { f.color };
                    self.ops().push(Op::Text { font, size, color, x: cx, y: base + dy, text: pc.text.clone(), underline: f.underline || f.link.is_some(), strike: f.strike, link: f.link.clone(), highlight: f.highlight });
                    cx += pc.width;
                } else {
                    if let Some(hl) = pc.fmt.highlight {
                        let y = self.y;
                        self.ops().push(Op::Fill { x: cx, y, w: pc.width + gap, h, color: hl });
                    }
                    cx += pc.width + gap;
                }
            }
            self.y += h;
        }
        if p.border_bottom {
            let y = self.y + 2.0;
            self.ops().push(Op::Line { x1: x0, y1: y, x2: x0 + width, y2: y });
            self.y += 4.0;
        }
        self.y += p.space_after;
        self.y - start_y
    }

    fn table(&mut self, t: &Table) {
        let x0 = self.doc.setup.margins[0];
        let total: f32 = t.widths.iter().sum();
        let scale = (self.doc.setup.text_width() / total).min(1.0);
        let widths: Vec<f32> = t.widths.iter().map(|w| w * scale).collect();
        let pad = 4.0;
        for (ri, row) in t.rows.iter().enumerate() {
            // Measure the row on a scratch page.
            let save_y = self.y;
            self.pages.push(vec![]);
            let mut h: f32 = 0.0;
            for (ci, c) in row.iter().enumerate() {
                self.y = 0.0;
                let used = self.para(&c.para, 0.0, widths[ci] - 2.0 * pad, None, false);
                h = h.max(used);
            }
            self.pages.pop();
            self.y = save_y;
            let row_h = h + 2.0 * pad;
            self.ensure(row_h);
            let y = self.y;
            let mut x = x0;
            for (ci, c) in row.iter().enumerate() {
                if let Some(sh) = c.shading {
                    self.ops().push(Op::Fill { x, y, w: widths[ci], h: row_h, color: sh });
                } else if ri == 0 && t.header {
                    self.ops().push(Op::Fill { x, y, w: widths[ci], h: row_h, color: [235, 235, 235] });
                }
                self.y = y + pad;
                self.para(&c.para, x + pad, widths[ci] - 2.0 * pad, None, false);
                if t.borders {
                    self.ops().push(Op::Stroke { x, y, w: widths[ci], h: row_h });
                }
                x += widths[ci];
            }
            self.y = y + row_h;
        }
        self.y += 8.0;
    }
}

pub fn export(doc: &Document) -> Vec<u8> {
    let mut l = Layout { pdf: Doc::new(), fonts: HashMap::new(), pages: vec![vec![]], y: doc.setup.margins[1], doc };
    l.pdf.title = doc.meta.title.clone();
    l.pdf.author = doc.meta.author.clone();
    l.pdf.subject = doc.meta.subject.clone();
    l.pdf.keywords = doc.meta.keywords.clone();
    let paras: Vec<Para> = doc.blocks.iter().map(|b| b.para().cloned().unwrap_or_default()).collect();
    let markers = list_numbers(&paras);
    let x0 = doc.setup.margins[0];
    let tw = doc.setup.text_width();
    for (i, b) in doc.blocks.iter().enumerate() {
        match b {
            Block::Para(p) => {
                if p.page_break_before && l.y > l.top() {
                    l.new_page();
                }
                l.para(p, x0, tw, markers[i].clone(), true);
            }
            Block::Table(t) => l.table(t),
            Block::Picture(pic) => {
                let (w, h) = (pic.width.min(tw), pic.height * pic.width.min(tw) / pic.width.max(1.0));
                let h = h.min(l.bottom() - l.top());
                let w = w.min(h * pic.width / pic.height.max(1.0));
                l.ensure(h);
                if let Ok(img) = image::load_from_memory(&pic.data) {
                    let id = if kit42::ooxml::image_mime(&pic.data).1 == "jpeg" {
                        let gray = img.color().channel_count() == 1;
                        l.pdf.add_image_jpeg((*pic.data).clone(), img.width(), img.height(), gray)
                    } else {
                        l.pdf.add_image_rgba(&img.to_rgba8())
                    };
                    let x = match pic.align {
                        Align2::Center => x0 + (tw - w) / 2.0,
                        Align2::Right => x0 + tw - w,
                        _ => x0,
                    };
                    let y = l.y;
                    l.ops().push(Op::Image { id, x, y, w, h });
                }
                l.y += h + 6.0;
            }
            Block::PageBreak => l.new_page(),
        }
    }
    // Emit pages with header/footer now that the total is known.
    let total = l.pages.len();
    let pages = std::mem::take(&mut l.pages);
    let hf_font = l.font(&CharFmt { size: 9.0, ..doc.body.clone() });
    for (pi, ops) in pages.into_iter().enumerate() {
        let mut page = Page::new(doc.setup.width, doc.setup.height);
        let skip_hf = doc.setup.first_page_different && pi == 0;
        for (text, y) in [(&doc.setup.header, doc.setup.margins[1] * 0.5), (&doc.setup.footer, doc.setup.height - doc.setup.margins[3] * 0.45)] {
            if text.trim().is_empty() || skip_hf {
                continue;
            }
            let s = text.replace("{page}", &(pi + 1).to_string()).replace("{pages}", &total.to_string());
            let w = l.pdf.measure(hf_font, 9.0, &s);
            let mut st = TextStyle::new(hf_font, 9.0);
            st.color = [90, 90, 90];
            l.pdf.text(&mut page, st, (doc.setup.width - w) / 2.0, y, &s);
        }
        for op in ops {
            match op {
                Op::Text { font, size, color, x, y, text, underline, strike, link, highlight } => {
                    if let Some(hl) = highlight {
                        let w = l.pdf.measure(font, size, &text);
                        page.fill_rect(x, y - size * 0.85, w, size * 1.15, hl);
                    }
                    let mut st = TextStyle::new(font, size);
                    st.color = color;
                    st.underline = underline;
                    st.strike = strike;
                    let w = l.pdf.text(&mut page, st, x, y, &text);
                    if let Some(u) = link {
                        page.link(x, y - size, w, size * 1.2, &u);
                    }
                }
                Op::Fill { x, y, w, h, color } => page.fill_rect(x, y, w, h, color),
                Op::Stroke { x, y, w, h } => page.stroke_rect(x, y, w, h, [128, 128, 128], 0.5),
                Op::Line { x1, y1, x2, y2 } => page.line(x1, y1, x2, y2, [160, 160, 160], 0.75),
                Op::Image { id, x, y, w, h } => page.image(id, x, y, w, h),
            }
        }
        l.pdf.add_page(page);
    }
    l.pdf.finish()
}

/// Page index of every block (for the table of contents), using the PDF layout.
pub fn block_pages(doc: &Document) -> Vec<usize> {
    let mut l = Layout { pdf: Doc::new(), fonts: HashMap::new(), pages: vec![vec![]], y: doc.setup.margins[1], doc };
    let x0 = doc.setup.margins[0];
    let tw = doc.setup.text_width();
    let mut out = vec![];
    for b in &doc.blocks {
        match b {
            Block::Para(p) => {
                if p.page_break_before && l.y > l.top() {
                    l.new_page();
                }
                let before = l.pages.len();
                l.para(p, x0, tw, None, true);
                // A heading moved to a new page by its first line counts on that page.
                out.push(if l.pages.len() > before { before } else { l.pages.len() - 1 });
            }
            Block::Table(t) => {
                out.push(l.pages.len() - 1);
                l.table(t);
            }
            Block::Picture(pic) => {
                l.ensure(pic.height.min(l.bottom() - l.top()));
                out.push(l.pages.len() - 1);
                l.y += pic.height + 6.0;
            }
            Block::PageBreak => {
                out.push(l.pages.len() - 1);
                l.new_page();
            }
        }
    }
    out
}
