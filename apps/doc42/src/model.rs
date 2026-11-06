//! DOC42 document model: blocks of rich paragraphs, tables, pictures and page breaks, with
//! page setup, header/footer and document properties.

use kit42::richtext::{CharFmt, Para, ParaStyle};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub para: Para,
    pub shading: Option<[u8; 3]>,
}

impl Cell {
    pub fn new(text: &str, fmt: &CharFmt) -> Self {
        let mut p = Para::new(text, fmt.clone());
        p.space_after = 0.0;
        Self { para: p, shading: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub rows: Vec<Vec<Cell>>,
    /// Column widths in points.
    pub widths: Vec<f32>,
    pub borders: bool,
    pub header: bool,
}

impl Table {
    pub fn new(rows: usize, cols: usize, width: f32, fmt: &CharFmt) -> Self {
        Self {
            rows: (0..rows).map(|_| (0..cols).map(|_| Cell::new("", fmt)).collect()).collect(),
            widths: vec![width / cols as f32; cols],
            borders: true,
            header: false,
        }
    }

    pub fn cols(&self) -> usize {
        self.widths.len()
    }

    pub fn insert_row(&mut self, at: usize, fmt: &CharFmt) {
        let n = self.cols();
        self.rows.insert(at.min(self.rows.len()), (0..n).map(|_| Cell::new("", fmt)).collect());
    }

    pub fn insert_col(&mut self, at: usize, fmt: &CharFmt) {
        let at = at.min(self.cols());
        let total: f32 = self.widths.iter().sum();
        for r in &mut self.rows {
            r.insert(at, Cell::new("", fmt));
        }
        self.widths.insert(at, total / (self.widths.len() + 1) as f32);
        let s: f32 = self.widths.iter().sum();
        for w in &mut self.widths {
            *w *= total / s;
        }
    }

    pub fn delete_row(&mut self, r: usize) {
        if self.rows.len() > 1 && r < self.rows.len() {
            self.rows.remove(r);
        }
    }

    pub fn delete_col(&mut self, c: usize) {
        if self.cols() > 1 && c < self.cols() {
            let total: f32 = self.widths.iter().sum();
            for r in &mut self.rows {
                if c < r.len() {
                    r.remove(c);
                }
            }
            self.widths.remove(c);
            let s: f32 = self.widths.iter().sum();
            for w in &mut self.widths {
                *w *= total / s;
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Picture {
    #[serde(skip)]
    pub data: Arc<Vec<u8>>,
    pub width: f32,
    pub height: f32,
    pub align: kit42::richtext::Align2,
    pub id: u64,
    pub alt: String,
}

impl Picture {
    pub fn new(data: Vec<u8>, max_width: f32) -> Option<Self> {
        let img = image::load_from_memory(&data).ok()?;
        let (w, h) = (img.width() as f32 * 0.75, img.height() as f32 * 0.75);
        let s = (max_width / w).min(1.0);
        // Normalise unusual formats to PNG so every writer can embed them.
        let data = match kit42::ooxml::image_mime(&data).1 {
            "png" | "jpeg" => data,
            _ => {
                let mut out = Vec::new();
                img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
                out
            }
        };
        Some(Self { data: Arc::new(data), width: w * s, height: h * s, align: kit42::richtext::Align2::Center, id: rand::random(), alt: String::new() })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Block {
    Para(Para),
    Table(Table),
    Picture(Picture),
    PageBreak,
}

impl Block {
    pub fn para(&self) -> Option<&Para> {
        match self {
            Block::Para(p) => Some(p),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageSetup {
    pub width: f32,
    pub height: f32,
    /// left, top, right, bottom in points.
    pub margins: [f32; 4],
    pub header: String,
    pub footer: String,
    pub first_page_different: bool,
}

impl Default for PageSetup {
    fn default() -> Self {
        let a4 = kit42::pdfgen::A4;
        let cm = 72.0 / 2.54;
        Self { width: a4.0, height: a4.1, margins: [3.0 * cm, 2.0 * cm, 1.5 * cm, 2.0 * cm], header: String::new(), footer: String::new(), first_page_different: false }
    }
}

impl PageSetup {
    pub fn text_width(&self) -> f32 {
        self.width - self.margins[0] - self.margins[2]
    }
    pub fn text_height(&self) -> f32 {
        self.height - self.margins[1] - self.margins[3]
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Meta {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub setup: PageSetup,
    pub body: CharFmt,
    pub meta: Meta,
}

impl Default for Document {
    fn default() -> Self {
        let body = CharFmt::default();
        Self { blocks: vec![Block::Para(Para::new("", body.clone()))], setup: PageSetup::default(), body, meta: Meta::default() }
    }
}

impl Document {
    /// Ensures the document is editable: never empty, ends with a paragraph.
    pub fn normalize(&mut self) {
        if !matches!(self.blocks.last(), Some(Block::Para(_))) {
            self.blocks.push(Block::Para(Para::new("", self.body.clone())));
        }
        for b in &mut self.blocks {
            if let Block::Para(p) = b {
                p.normalize();
            }
        }
    }

    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for b in &self.blocks {
            match b {
                Block::Para(p) => {
                    out.push_str(&p.text());
                    out.push('\n');
                }
                Block::Table(t) => {
                    for r in &t.rows {
                        out.push_str(&r.iter().map(|c| c.para.text()).collect::<Vec<_>>().join("\t"));
                        out.push('\n');
                    }
                }
                Block::PageBreak => out.push('\x0C'),
                Block::Picture(_) => {}
            }
        }
        out
    }

    pub fn stats(&self) -> (usize, usize, usize, usize) {
        let text = self.plain_text();
        let words = text.split_whitespace().count();
        let chars = text.chars().filter(|c| *c != '\n').count();
        let no_spaces = text.chars().filter(|c| !c.is_whitespace()).count();
        let paras = self.blocks.iter().filter(|b| matches!(b, Block::Para(p) if !p.is_empty())).count();
        (words, chars, no_spaces, paras)
    }

    /// Headings for the navigator and the table of contents: (block index, level, text).
    pub fn headings(&self) -> Vec<(usize, u8, String)> {
        self.blocks
            .iter()
            .enumerate()
            .filter_map(|(i, b)| match b {
                Block::Para(p) => match p.style {
                    ParaStyle::Title => Some((i, 0, p.text())),
                    ParaStyle::Heading(n) => Some((i, n, p.text())),
                    _ => None,
                },
                _ => None,
            })
            .filter(|(_, _, t)| !t.trim().is_empty())
            .collect()
    }
}

/// Applies a paragraph style to a paragraph, keeping its text.
pub fn set_style(p: &mut Para, style: ParaStyle, body: &CharFmt) {
    let base = style.base_fmt(body);
    p.style = style;
    let (b, a) = style.spacing();
    p.space_before = b;
    p.space_after = a;
    if style == ParaStyle::Quote {
        p.indent_left = 36.0;
        p.indent_right = 36.0;
    } else if p.indent_left == 36.0 && p.indent_right == 36.0 {
        p.indent_left = 0.0;
        p.indent_right = 0.0;
    }
    p.apply_all(&mut |f| {
        let link = f.link.clone();
        *f = base.clone();
        f.link = link;
    });
    if style == ParaStyle::Code {
        p.shading = Some([0xF3, 0xF3, 0xF3]);
    } else if p.shading == Some([0xF3, 0xF3, 0xF3]) {
        p.shading = None;
    }
}
