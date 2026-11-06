//! A small PDF writer: pages with vector graphics, images (with alpha) and Unicode text in
//! embedded TrueType fonts (Type0 / Identity-H with ToUnicode, so text stays searchable).
//! Coordinates passed to `Page` methods are in points from the TOP-LEFT corner.

use flate2::{write::ZlibEncoder, Compression};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageId(pub usize);

struct FontEntry {
    data: Vec<u8>,
    units_per_em: f32,
    ascent: f32,
    descent: f32,
    used: BTreeMap<u16, (char, u16)>,
    cmap: HashMap<char, u16>,
    advances: HashMap<u16, u16>,
    name: String,
}

impl FontEntry {
    fn parse(data: Vec<u8>, name: &str) -> Option<Self> {
        let face = ttf_parser::Face::parse(&data, 0).ok()?;
        let upem = face.units_per_em() as f32;
        let ascent = face.ascender() as f32 / upem;
        let descent = face.descender() as f32 / upem;
        drop(face);
        Some(Self {
            data,
            units_per_em: upem,
            ascent,
            descent,
            used: BTreeMap::new(),
            cmap: HashMap::new(),
            advances: HashMap::new(),
            name: name.chars().filter(|c| c.is_ascii_alphanumeric()).collect(),
        })
    }

    fn glyph(&mut self, c: char) -> Option<u16> {
        if let Some(&g) = self.cmap.get(&c) {
            return if g == 0 { None } else { Some(g) };
        }
        let face = ttf_parser::Face::parse(&self.data, 0).ok()?;
        let g = face.glyph_index(c).map(|g| g.0).unwrap_or(0);
        let adv = face.glyph_hor_advance(ttf_parser::GlyphId(g)).unwrap_or(0);
        self.cmap.insert(c, g);
        self.advances.insert(g, adv);
        if g == 0 {
            None
        } else {
            Some(g)
        }
    }

    fn advance(&self, g: u16) -> f32 {
        *self.advances.get(&g).unwrap_or(&0) as f32 / self.units_per_em
    }
}

enum ImageData {
    Raw { w: u32, h: u32, rgb: Vec<u8>, alpha: Option<Vec<u8>>, gray: bool },
    Jpeg { w: u32, h: u32, bytes: Vec<u8>, gray: bool },
    Bilevel { w: u32, h: u32, packed: Vec<u8> },
}

/// A page under construction.
pub struct Page {
    pub width: f32,
    pub height: f32,
    ops: String,
    fonts: BTreeSet<usize>,
    images: BTreeSet<usize>,
    alphas: BTreeSet<u16>,
    links: Vec<([f32; 4], String)>,
}

impl Page {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            ops: String::new(),
            fonts: BTreeSet::new(),
            images: BTreeSet::new(),
            alphas: BTreeSet::new(),
            links: Vec::new(),
        }
    }

    fn y(&self, y: f32) -> f32 {
        self.height - y
    }

    pub fn raw(&mut self, ops: &str) {
        self.ops.push_str(ops);
        self.ops.push('\n');
    }

    pub fn save(&mut self) {
        self.ops.push_str("q\n");
    }

    pub fn restore(&mut self) {
        self.ops.push_str("Q\n");
    }

    /// Sets fill and stroke opacity (0..1) until the next restore.
    pub fn alpha(&mut self, a: f32) {
        let key = (a.clamp(0.0, 1.0) * 100.0).round() as u16;
        self.alphas.insert(key);
        let _ = writeln!(self.ops, "/GA{key} gs");
    }

    pub fn fill_color(&mut self, rgb: [u8; 3]) {
        let _ = writeln!(self.ops, "{} {} {} rg", c(rgb[0]), c(rgb[1]), c(rgb[2]));
    }

    pub fn stroke_color(&mut self, rgb: [u8; 3]) {
        let _ = writeln!(self.ops, "{} {} {} RG", c(rgb[0]), c(rgb[1]), c(rgb[2]));
    }

    pub fn line_width(&mut self, w: f32) {
        let _ = writeln!(self.ops, "{} w", n(w));
    }

    pub fn dash(&mut self, on: f32, off: f32) {
        if on <= 0.0 {
            self.ops.push_str("[] 0 d\n");
        } else {
            let _ = writeln!(self.ops, "[{} {}] 0 d", n(on), n(off));
        }
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3]) {
        self.fill_color(rgb);
        let _ = writeln!(self.ops, "{} {} {} {} re f", n(x), n(self.y(y + h)), n(w), n(h));
    }

    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3], width: f32) {
        self.stroke_color(rgb);
        self.line_width(width);
        let _ = writeln!(self.ops, "{} {} {} {} re S", n(x), n(self.y(y + h)), n(w), n(h));
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, rgb: [u8; 3], width: f32) {
        self.stroke_color(rgb);
        self.line_width(width);
        let _ = writeln!(self.ops, "{} {} m {} {} l S", n(x1), n(self.y(y1)), n(x2), n(self.y(y2)));
    }

    /// Path from points in top-left coordinates. `close` closes it; fill/stroke optional.
    pub fn polygon(&mut self, pts: &[(f32, f32)], close: bool, fill: Option<[u8; 3]>, stroke: Option<([u8; 3], f32)>) {
        if pts.is_empty() {
            return;
        }
        if let Some(f) = fill {
            self.fill_color(f);
        }
        if let Some((s, w)) = stroke {
            self.stroke_color(s);
            self.line_width(w);
        }
        let _ = write!(self.ops, "{} {} m ", n(pts[0].0), n(self.y(pts[0].1)));
        for p in &pts[1..] {
            let _ = write!(self.ops, "{} {} l ", n(p.0), n(self.y(p.1)));
        }
        if close {
            self.ops.push_str("h ");
        }
        self.ops.push_str(match (fill.is_some(), stroke.is_some()) {
            (true, true) => "B\n",
            (true, false) => "f\n",
            (false, true) => "S\n",
            (false, false) => "n\n",
        });
    }

    /// Ellipse inscribed in a rectangle.
    pub fn ellipse(&mut self, x: f32, y: f32, w: f32, h: f32, fill: Option<[u8; 3]>, stroke: Option<([u8; 3], f32)>) {
        let k = 0.552_284_8;
        let (cx, cy) = (x + w / 2.0, self.y(y + h / 2.0));
        let (rx, ry) = (w / 2.0, h / 2.0);
        if let Some(f) = fill {
            self.fill_color(f);
        }
        if let Some((s, lw)) = stroke {
            self.stroke_color(s);
            self.line_width(lw);
        }
        let _ = write!(
            self.ops,
            "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h ",
            n(cx + rx), n(cy),
            n(cx + rx), n(cy + ry * k), n(cx + rx * k), n(cy + ry), n(cx), n(cy + ry),
            n(cx - rx * k), n(cy + ry), n(cx - rx), n(cy + ry * k), n(cx - rx), n(cy),
            n(cx - rx), n(cy - ry * k), n(cx - rx * k), n(cy - ry), n(cx), n(cy - ry),
            n(cx + rx * k), n(cy - ry), n(cx + rx), n(cy - ry * k), n(cx + rx), n(cy)
        );
        self.ops.push_str(match (fill.is_some(), stroke.is_some()) {
            (true, true) => "B\n",
            (true, false) => "f\n",
            (false, true) => "S\n",
            _ => "n\n",
        });
    }

    /// Draws an image into a rectangle (top-left coordinates).
    pub fn image(&mut self, img: ImageId, x: f32, y: f32, w: f32, h: f32) {
        self.images.insert(img.0);
        let _ = writeln!(self.ops, "q {} 0 0 {} {} {} cm /Im{} Do Q", n(w), n(h), n(x), n(self.y(y + h)), img.0);
    }

    /// Rotates subsequent drawing by `deg` degrees (clockwise on screen) around a point.
    pub fn rotate_about(&mut self, cx: f32, cy: f32, deg: f32) {
        let r = -deg.to_radians();
        let (s, co) = r.sin_cos();
        let py = self.y(cy);
        let _ = writeln!(
            self.ops,
            "1 0 0 1 {} {} cm {} {} {} {} 0 0 cm 1 0 0 1 {} {} cm",
            n(cx), n(py), n(co), n(s), n(-s), n(co), n(-cx), n(-py)
        );
    }

    pub fn link(&mut self, x: f32, y: f32, w: f32, h: f32, uri: &str) {
        let r = [x, self.y(y + h), x + w, self.y(y)];
        self.links.push((r, uri.to_string()));
    }
}

fn c(v: u8) -> String {
    n(v as f32 / 255.0)
}

fn n(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.to_string()
    }
}

/// Text style for `Doc::text`.
#[derive(Clone, Copy)]
pub struct TextStyle {
    pub font: FontId,
    pub size: f32,
    pub color: [u8; 3],
    pub underline: bool,
    pub strike: bool,
    /// Text render mode 3 — invisible (OCR layers).
    pub invisible: bool,
}

impl TextStyle {
    pub fn new(font: FontId, size: f32) -> Self {
        Self { font, size, color: [0, 0, 0], underline: false, strike: false, invisible: false }
    }
}

pub struct Doc {
    fonts: Vec<FontEntry>,
    font_keys: HashMap<String, FontId>,
    fallbacks: Vec<FontId>,
    images: Vec<ImageData>,
    pages: Vec<Page>,
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub pdfa: bool,
}

impl Default for Doc {
    fn default() -> Self {
        Self::new()
    }
}

impl Doc {
    pub fn new() -> Self {
        Self {
            fonts: Vec::new(),
            font_keys: HashMap::new(),
            fallbacks: Vec::new(),
            images: Vec::new(),
            pages: Vec::new(),
            title: String::new(),
            author: String::new(),
            subject: String::new(),
            keywords: String::new(),
            pdfa: false,
        }
    }

    /// Registers an installed font family (falls back to Segoe UI / Arial when missing).
    pub fn font(&mut self, family: &str, bold: bool, italic: bool) -> FontId {
        let key = format!("{family}|{bold}|{italic}");
        if let Some(&id) = self.font_keys.get(&key) {
            return id;
        }
        let path = crate::fonts::font_file(family, bold, italic);
        let id = path
            .as_ref()
            .and_then(|p| {
                let pkey = p.to_string_lossy().to_string();
                if let Some(&id) = self.font_keys.get(&pkey) {
                    return Some(id);
                }
                let data = std::fs::read(p).ok()?;
                let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let entry = FontEntry::parse(data, &name)?;
                self.fonts.push(entry);
                let id = FontId(self.fonts.len() - 1);
                self.font_keys.insert(pkey, id);
                Some(id)
            })
            .unwrap_or_else(|| {
                // Last resort: any font we already have, or a tiny placeholder.
                if !self.fonts.is_empty() {
                    FontId(0)
                } else {
                    panic!("no TrueType fonts installed")
                }
            });
        self.font_keys.insert(key, id);
        if self.fallbacks.is_empty() {
            self.init_fallbacks();
        }
        id
    }

    fn init_fallbacks(&mut self) {
        let mut fb = Vec::new();
        for fam in ["Segoe UI", "Arial", "Segoe UI Symbol", "Microsoft Sans Serif", "Nirmala UI", "Ebrima"] {
            if let Some(p) = crate::fonts::index().get(fam).and_then(|s| s.regular.clone()) {
                let pkey = p.to_string_lossy().to_string();
                let id = if let Some(&id) = self.font_keys.get(&pkey) {
                    id
                } else if let Some(entry) = std::fs::read(&p).ok().and_then(|d| FontEntry::parse(d, fam)) {
                    self.fonts.push(entry);
                    let id = FontId(self.fonts.len() - 1);
                    self.font_keys.insert(pkey, id);
                    id
                } else {
                    continue;
                };
                fb.push(id);
            }
        }
        self.fallbacks = fb;
    }

    pub fn ascent(&self, font: FontId) -> f32 {
        self.fonts[font.0].ascent
    }

    pub fn descent(&self, font: FontId) -> f32 {
        self.fonts[font.0].descent
    }

    /// Splits text into runs by which font has the glyphs.
    fn runs(&mut self, font: FontId, text: &str) -> Vec<(FontId, Vec<(u16, char)>)> {
        let mut out: Vec<(FontId, Vec<(u16, char)>)> = Vec::new();
        let fallbacks = self.fallbacks.clone();
        for ch in text.chars() {
            let mut chosen = None;
            if let Some(g) = self.fonts[font.0].glyph(ch) {
                chosen = Some((font, g));
            } else {
                for &fb in &fallbacks {
                    if let Some(g) = self.fonts[fb.0].glyph(ch) {
                        chosen = Some((fb, g));
                        break;
                    }
                }
            }
            let (f, g) = chosen.unwrap_or((font, 0));
            match out.last_mut() {
                Some((lf, v)) if *lf == f => v.push((g, ch)),
                _ => out.push((f, vec![(g, ch)])),
            }
        }
        out
    }

    /// Width of text in points.
    pub fn measure(&mut self, font: FontId, size: f32, text: &str) -> f32 {
        let runs = self.runs(font, text);
        let mut w = 0.0;
        for (f, glyphs) in runs {
            for (g, _) in glyphs {
                w += self.fonts[f.0].advance(g) * size;
            }
        }
        w
    }

    /// Draws a single line of text with its baseline at (x, baseline), top-left coordinates.
    pub fn text(&mut self, page: &mut Page, style: TextStyle, x: f32, baseline: f32, text: &str) -> f32 {
        let runs = self.runs(style.font, text);
        let mut caret = x;
        let y = page.height - baseline;
        if !style.invisible {
            page.fill_color(style.color);
        }
        for (f, glyphs) in runs {
            page.fonts.insert(f.0);
            let mut hex = String::with_capacity(glyphs.len() * 4);
            let mut w = 0.0;
            for (g, ch) in &glyphs {
                let adv = self.fonts[f.0].advance(*g);
                self.fonts[f.0].used.insert(*g, (*ch, (adv * 1000.0).round() as u16));
                let _ = write!(hex, "{:04X}", g);
                w += adv * style.size;
            }
            let mode = if style.invisible { 3 } else { 0 };
            let _ = writeln!(
                page.ops,
                "BT {} Tr /F{} {} Tf {} {} Td <{}> Tj ET",
                mode,
                f.0,
                n(style.size),
                n(caret),
                n(y),
                hex
            );
            caret += w;
        }
        let width = caret - x;
        if !style.invisible && (style.underline || style.strike) {
            let lw = (style.size / 16.0).max(0.5);
            if style.underline {
                page.line(x, baseline + style.size * 0.12, x + width, baseline + style.size * 0.12, style.color, lw);
            }
            if style.strike {
                page.line(x, baseline - style.size * 0.3, x + width, baseline - style.size * 0.3, style.color, lw);
            }
        }
        width
    }

    /// Invisible text stretched over a box (OCR layer).
    pub fn invisible_text(&mut self, page: &mut Page, font: FontId, x: f32, y: f32, w: f32, h: f32, text: &str) {
        if text.trim().is_empty() || w <= 0.0 || h <= 0.0 {
            return;
        }
        let size = h * 0.9;
        let natural = self.measure(font, size, text).max(0.01);
        let scale = (w / natural * 100.0).clamp(1.0, 1000.0);
        let _ = writeln!(page.ops, "BT {} Tz ET", n(scale));
        let mut style = TextStyle::new(font, size);
        style.invisible = true;
        self.text(page, style, x, y + h * 0.85, text);
        page.ops.push_str("BT 100 Tz ET\n");
    }

    /// Word-wraps text into lines that fit a width.
    pub fn wrap(&mut self, font: FontId, size: f32, text: &str, width: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            for word in para.split(' ') {
                let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                if self.measure(font, size, &cand) > width && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    // Break words longer than the line.
                    let mut w = word.to_string();
                    while self.measure(font, size, &w) > width && w.chars().count() > 1 {
                        let mut cut = String::new();
                        for ch in w.chars() {
                            if self.measure(font, size, &format!("{cut}{ch}")) > width && !cut.is_empty() {
                                break;
                            }
                            cut.push(ch);
                        }
                        let rest: String = w.chars().skip(cut.chars().count()).collect();
                        lines.push(cut);
                        w = rest;
                    }
                    line = w;
                } else {
                    line = cand;
                }
            }
            lines.push(line);
        }
        lines
    }

    pub fn add_image_rgba(&mut self, img: &image::RgbaImage) -> ImageId {
        let (w, h) = img.dimensions();
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        let mut alpha = Vec::with_capacity((w * h) as usize);
        let mut has_alpha = false;
        for p in img.pixels() {
            rgb.extend_from_slice(&p.0[..3]);
            alpha.push(p[3]);
            if p[3] != 255 {
                has_alpha = true;
            }
        }
        self.images.push(ImageData::Raw { w, h, rgb, alpha: has_alpha.then_some(alpha), gray: false });
        ImageId(self.images.len() - 1)
    }

    pub fn add_image_gray(&mut self, img: &image::GrayImage) -> ImageId {
        let (w, h) = img.dimensions();
        self.images.push(ImageData::Raw { w, h, rgb: img.as_raw().clone(), alpha: None, gray: true });
        ImageId(self.images.len() - 1)
    }

    /// A black/white image stored at 1 bit per pixel (scans).
    pub fn add_image_bilevel(&mut self, img: &image::GrayImage) -> ImageId {
        let (w, h) = img.dimensions();
        let row = (w as usize).div_ceil(8);
        let mut packed = vec![0u8; row * h as usize];
        for (x, y, p) in img.enumerate_pixels() {
            if p[0] >= 128 {
                packed[y as usize * row + x as usize / 8] |= 0x80 >> (x % 8);
            }
        }
        self.images.push(ImageData::Bilevel { w, h, packed });
        ImageId(self.images.len() - 1)
    }

    /// Embeds JPEG bytes as-is (no re-encoding).
    pub fn add_image_jpeg(&mut self, bytes: Vec<u8>, w: u32, h: u32, gray: bool) -> ImageId {
        self.images.push(ImageData::Jpeg { w, h, bytes, gray });
        ImageId(self.images.len() - 1)
    }

    pub fn add_page(&mut self, page: Page) {
        self.pages.push(page);
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn finish(mut self) -> Vec<u8> {
        if self.pages.is_empty() {
            self.pages.push(Page::new(595.0, 842.0));
        }
        let mut objs: Vec<Vec<u8>> = vec![Vec::new(), Vec::new()]; // 1: catalog, 2: pages
        let mut alloc = |objs: &mut Vec<Vec<u8>>| {
            objs.push(Vec::new());
            objs.len()
        };

        // Fonts
        let mut font_obj = HashMap::new();
        for (i, f) in self.fonts.iter().enumerate() {
            if f.used.is_empty() {
                continue;
            }
            let file_id = alloc(&mut objs);
            let desc_id = alloc(&mut objs);
            let cid_id = alloc(&mut objs);
            let tu_id = alloc(&mut objs);
            let type0_id = alloc(&mut objs);
            let comp = deflate(&f.data);
            objs[file_id - 1] = stream_obj(&format!("/Length1 {} /Filter /FlateDecode", f.data.len()), &comp);
            let base = format!("K42{:04}+{}", i, if f.name.is_empty() { "Font".into() } else { f.name.clone() });
            objs[desc_id - 1] = format!(
                "<< /Type /FontDescriptor /FontName /{base} /Flags 32 /FontBBox [-1000 -400 2000 1200] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight 700 /StemV 80 /FontFile2 {file_id} 0 R >>",
                (f.ascent * 1000.0) as i32,
                (f.descent * 1000.0) as i32
            )
            .into_bytes();
            let mut widths = String::new();
            for (g, (_, w)) in &f.used {
                let _ = write!(widths, "{} [{}] ", g, w);
            }
            objs[cid_id - 1] = format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{base} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {desc_id} 0 R /DW 0 /W [{widths}] /CIDToGIDMap /Identity >>"
            )
            .into_bytes();
            let mut cmap = String::from("/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /Adobe-Identity-UCS def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange\n");
            let entries: Vec<_> = f.used.iter().collect();
            for chunk in entries.chunks(100) {
                let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
                for (g, (ch, _)) in chunk {
                    let mut u = [0u16; 2];
                    let units = ch.encode_utf16(&mut u);
                    let hex: String = units.iter().map(|x| format!("{:04X}", x)).collect();
                    let _ = writeln!(cmap, "<{:04X}> <{}>", g, hex);
                }
                cmap.push_str("endbfchar\n");
            }
            cmap.push_str("endcmap CMapName currentdict /CMap defineresource pop end end");
            objs[tu_id - 1] = stream_obj("/Filter /FlateDecode", &deflate(cmap.as_bytes()));
            objs[type0_id - 1] = format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{base} /Encoding /Identity-H /DescendantFonts [{cid_id} 0 R] /ToUnicode {tu_id} 0 R >>"
            )
            .into_bytes();
            font_obj.insert(i, type0_id);
        }

        // Images
        let mut image_obj = HashMap::new();
        for (i, img) in self.images.iter().enumerate() {
            let id = alloc(&mut objs);
            match img {
                ImageData::Raw { w, h, rgb, alpha, gray } => {
                    let smask = alpha.as_ref().map(|a| {
                        let sid = alloc(&mut objs);
                        objs[sid - 1] = stream_obj(
                            &format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode"),
                            &deflate(a),
                        );
                        sid
                    });
                    let cs = if *gray { "/DeviceGray" } else { "/DeviceRGB" };
                    let sm = smask.map(|s| format!(" /SMask {s} 0 R")).unwrap_or_default();
                    objs[id - 1] = stream_obj(
                        &format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace {cs} /BitsPerComponent 8 /Filter /FlateDecode{sm}"),
                        &deflate(rgb),
                    );
                }
                ImageData::Jpeg { w, h, bytes, gray } => {
                    let cs = if *gray { "/DeviceGray" } else { "/DeviceRGB" };
                    objs[id - 1] = stream_obj(
                        &format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace {cs} /BitsPerComponent 8 /Filter /DCTDecode"),
                        bytes,
                    );
                }
                ImageData::Bilevel { w, h, packed } => {
                    objs[id - 1] = stream_obj(
                        &format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /FlateDecode"),
                        &deflate(packed),
                    );
                }
            }
            image_obj.insert(i, id);
        }

        // Pages
        let mut page_ids = Vec::new();
        let pages = std::mem::take(&mut self.pages);
        for page in &pages {
            let content_id = alloc(&mut objs);
            let page_id = alloc(&mut objs);
            objs[content_id - 1] = stream_obj("/Filter /FlateDecode", &deflate(page.ops.as_bytes()));
            let mut res = String::from("<< /ProcSet [/PDF /Text /ImageB /ImageC]");
            if !page.fonts.is_empty() {
                res.push_str(" /Font <<");
                for f in &page.fonts {
                    if let Some(o) = font_obj.get(f) {
                        let _ = write!(res, " /F{f} {o} 0 R");
                    }
                }
                res.push_str(" >>");
            }
            if !page.images.is_empty() {
                res.push_str(" /XObject <<");
                for im in &page.images {
                    let _ = write!(res, " /Im{im} {} 0 R", image_obj[im]);
                }
                res.push_str(" >>");
            }
            if !page.alphas.is_empty() {
                res.push_str(" /ExtGState <<");
                for a in &page.alphas {
                    let v = *a as f32 / 100.0;
                    let _ = write!(res, " /GA{a} << /Type /ExtGState /ca {} /CA {} >>", n(v), n(v));
                }
                res.push_str(" >>");
            }
            res.push_str(" >>");
            let mut annots = String::new();
            if !page.links.is_empty() {
                annots.push_str(" /Annots [");
                for (r, uri) in &page.links {
                    let _ = write!(
                        annots,
                        " << /Type /Annot /Subtype /Link /Rect [{} {} {} {}] /Border [0 0 0] /A << /S /URI /URI {} >> >>",
                        n(r[0]), n(r[1]), n(r[2]), n(r[3]), pdf_string(uri)
                    );
                }
                annots.push(']');
            }
            objs[page_id - 1] = format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources {res} /Contents {content_id} 0 R{annots} >>",
                n(page.width),
                n(page.height)
            )
            .into_bytes();
            page_ids.push(page_id);
        }
        let kids: String = page_ids.iter().map(|p| format!("{p} 0 R ")).collect();
        objs[1] = format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", page_ids.len()).into_bytes();

        // Metadata
        let now = chrono::Local::now();
        let date = now.format("D:%Y%m%d%H%M%S").to_string();
        let info_id = alloc(&mut objs);
        objs[info_id - 1] = format!(
            "<< /Producer {} /Creator (Suite42) /CreationDate ({date}) /ModDate ({date}){}{}{}{} >>",
            pdf_string("Suite42 for Windows"),
            opt_entry("Title", &self.title),
            opt_entry("Author", &self.author),
            opt_entry("Subject", &self.subject),
            opt_entry("Keywords", &self.keywords),
        )
        .into_bytes();
        let mut catalog = String::from("<< /Type /Catalog /Pages 2 0 R");
        if self.pdfa {
            let iso = now.format("%Y-%m-%dT%H:%M:%S%:z").to_string();
            let xmp = format!(
                r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:pdf="http://ns.adobe.com/pdf/1.3/"><pdfaid:part>2</pdfaid:part><pdfaid:conformance>B</pdfaid:conformance><dc:title><rdf:Alt><rdf:li xml:lang="x-default">{}</rdf:li></rdf:Alt></dc:title><xmp:CreateDate>{iso}</xmp:CreateDate><xmp:ModifyDate>{iso}</xmp:ModifyDate><xmp:CreatorTool>Suite42</xmp:CreatorTool><pdf:Producer>Suite42 for Windows</pdf:Producer></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#,
                crate::ooxml::esc(&self.title)
            );
            let meta_id = alloc(&mut objs);
            objs[meta_id - 1] = stream_obj("/Type /Metadata /Subtype /XML", xmp.as_bytes());
            let icc = srgb_icc();
            let icc_id = alloc(&mut objs);
            objs[icc_id - 1] = stream_obj("/N 3 /Filter /FlateDecode", &deflate(&icc));
            let _ = write!(
                catalog,
                " /Metadata {meta_id} 0 R /OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (sRGB IEC61966-2.1) /DestOutputProfile {icc_id} 0 R >>]"
            );
        }
        catalog.push_str(" >>");
        objs[0] = catalog.into_bytes();

        // Serialize
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
        let mut offsets = Vec::with_capacity(objs.len());
        for (i, body) in objs.iter().enumerate() {
            offsets.push(out.len());
            let _ = write!(out, "{} 0 obj\n", i + 1);
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
        for off in offsets {
            let _ = write!(out, "{:010} 00000 n \n", off);
        }
        let id: String = (0..16).map(|_| format!("{:02X}", rand::random::<u8>())).collect();
        let _ = write!(
            out,
            "trailer\n<< /Size {} /Root 1 0 R /Info {info_id} 0 R /ID [<{id}> <{id}>] >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        );
        out
    }
}

fn opt_entry(key: &str, v: &str) -> String {
    if v.is_empty() {
        String::new()
    } else {
        format!(" /{key} {}", pdf_string(v))
    }
}

/// A PDF text string: ASCII literal, otherwise UTF-16BE hex with BOM.
pub fn pdf_string(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) {
        let esc = s.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
        format!("({esc})")
    } else {
        let mut hex = String::from("<FEFF");
        for u in s.encode_utf16() {
            let _ = write!(hex, "{:04X}", u);
        }
        hex.push('>');
        hex
    }
}

fn stream_obj(dict_entries: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("<< {dict_entries} /Length {} >>\nstream\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream");
    v
}

pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// Minimal sRGB ICC profile (v2) for the PDF/A output intent.
fn srgb_icc() -> Vec<u8> {
    // Profile header + tag table with rXYZ/gXYZ/bXYZ/wtpt/rTRC/gTRC/bTRC/desc/cprt.
    let mut tags: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    let xyz = |x: f64, y: f64, z: f64| {
        let mut v = b"XYZ \0\0\0\0".to_vec();
        for c in [x, y, z] {
            v.extend_from_slice(&((c * 65536.0).round() as i32).to_be_bytes());
        }
        v
    };
    let mut curv = b"curv\0\0\0\0".to_vec();
    curv.extend_from_slice(&1u32.to_be_bytes());
    curv.extend_from_slice(&((2.2 * 256.0) as u16).to_be_bytes());
    curv.extend_from_slice(&[0, 0]);
    let mut desc = b"desc\0\0\0\0".to_vec();
    let name = b"sRGB IEC61966-2.1\0";
    desc.extend_from_slice(&(name.len() as u32).to_be_bytes());
    desc.extend_from_slice(name);
    desc.extend_from_slice(&[0u8; 4 + 4 + 2 + 1 + 67]);
    let mut cprt = b"text\0\0\0\0".to_vec();
    cprt.extend_from_slice(b"Public domain\0");
    tags.push((*b"desc", desc));
    tags.push((*b"wtpt", xyz(0.9505, 1.0, 1.0890)));
    tags.push((*b"rXYZ", xyz(0.4361, 0.2225, 0.0139)));
    tags.push((*b"gXYZ", xyz(0.3851, 0.7169, 0.0971)));
    tags.push((*b"bXYZ", xyz(0.1431, 0.0606, 0.7141)));
    tags.push((*b"rTRC", curv.clone()));
    tags.push((*b"gTRC", curv.clone()));
    tags.push((*b"bTRC", curv));
    tags.push((*b"cprt", cprt));
    let header_len = 128 + 4 + tags.len() * 12;
    let mut data = Vec::new();
    let mut table = Vec::new();
    let mut offset = header_len;
    for (sig, body) in &tags {
        let mut b = body.clone();
        while b.len() % 4 != 0 {
            b.push(0);
        }
        table.extend_from_slice(sig);
        table.extend_from_slice(&(offset as u32).to_be_bytes());
        table.extend_from_slice(&(body.len() as u32).to_be_bytes());
        offset += b.len();
        data.extend_from_slice(&b);
    }
    let total = header_len + data.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(b"none");
    out.extend_from_slice(&[2, 0x10, 0, 0]);
    out.extend_from_slice(b"mntrRGB XYZ ");
    out.extend_from_slice(&[0u8; 12]);
    out.extend_from_slice(b"acsp");
    out.extend_from_slice(b"MSFT");
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&[0u8; 8]);
    out.extend_from_slice(&[0u8; 4]);
    for c in [0.9642f64, 1.0, 0.8249] {
        out.extend_from_slice(&((c * 65536.0).round() as i32).to_be_bytes());
    }
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&[0u8; 16]);
    out.extend_from_slice(&[0u8; 28]);
    debug_assert_eq!(out.len(), 128);
    out.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    out.extend_from_slice(&table);
    out.extend_from_slice(&data);
    out
}

/// Paper sizes in points.
pub const A4: (f32, f32) = (595.28, 841.89);
pub const A3: (f32, f32) = (841.89, 1190.55);
pub const A5: (f32, f32) = (419.53, 595.28);
pub const LETTER: (f32, f32) = (612.0, 792.0);
pub const LEGAL: (f32, f32) = (612.0, 1008.0);
pub const TABLOID: (f32, f32) = (792.0, 1224.0);

pub fn mm(v: f32) -> f32 {
    v * 72.0 / 25.4
}
