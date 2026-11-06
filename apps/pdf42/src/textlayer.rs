//! Positioned text for in-place editing: a content-stream interpreter that finds each glyph
//! with its font, size and colour, groups glyphs into lines, and can remove the glyphs of an
//! edited line from the page content. Pages without a text layer are recognised with OCR.

use crate::pdfops::{self, Geom};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};
use std::collections::HashMap;
use std::rc::Rc;

/// One line of text on a page, in displayed page points (origin top-left).
#[derive(Clone, Debug)]
pub struct Line {
    pub rect: [f32; 4],
    pub baseline: f32,
    pub text: String,
    pub size: f32,
    /// Installed family that best matches the PDF font.
    pub font: String,
    pub bold: bool,
    pub italic: bool,
    pub color: [u8; 3],
    pub bg: [u8; 3],
    /// The visible text is pixels (scan, OCR layer): it can only be covered, not removed.
    pub hidden: bool,
}

pub type M = [f32; 6];
pub const ID: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// An image painted on a page (an Image XObject drawn with `Do`).
#[derive(Clone, Debug)]
pub struct PageImage {
    /// Bounding box in displayed page points (origin top-left).
    pub rect: [f32; 4],
    /// The CTM at the `Do`: maps the image's unit square into user space.
    pub ctm: M,
    /// Index of the `Do` in the page's own content; None when drawn inside a form XObject.
    pub op: Option<usize>,
    pub name: Vec<u8>,
    pub id: Option<ObjectId>,
    /// Pixel size of the image.
    pub px: (u32, u32),
}

pub fn mul(a: &M, b: &M) -> M {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// Inverse of an affine matrix (None when singular).
pub fn inverse(m: &M) -> Option<M> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-9 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)])
}

fn num(o: &Object) -> Option<f32> {
    match o {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

fn nums(ops: &[Object]) -> Vec<f32> {
    ops.iter().filter_map(num).collect()
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(o),
        _ => o,
    }
}

fn dict<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    match deref(doc, o) {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

fn get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok().map(|o| deref(doc, o))
}

/// Displayed point (top-left origin) for a user-space point.
pub fn to_display(g: &Geom, x: f32, y: f32) -> (f32, f32) {
    match g.rot {
        90 => (y - g.y0, x - g.x0),
        180 => (g.x1 - x, y - g.y0),
        270 => (g.y1 - y, g.x1 - x),
        _ => (x - g.x0, g.y1 - y),
    }
}

// ───────────────────────────── fonts ─────────────────────────────

struct Font {
    two_byte: bool,
    widths: HashMap<u32, f32>,
    default_w: f32,
    to_uni: HashMap<u32, String>,
    simple: Vec<Option<char>>,
    family: String,
    bold: bool,
    italic: bool,
    ascent: f32,
    descent: f32,
}

const CP1252: [u16; 32] = [
    0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0, 0x017D, 0, 0, 0x2018,
    0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
];

fn glyph_char(name: &str) -> Option<char> {
    if name.chars().count() == 1 {
        return name.chars().next();
    }
    if let Some(h) = name.strip_prefix("uni").or_else(|| name.strip_prefix('u')) {
        if let Ok(v) = u32::from_str_radix(&h[..h.len().min(4)], 16) {
            return char::from_u32(v);
        }
    }
    Some(match name {
        "space" => ' ',
        "period" => '.',
        "comma" => ',',
        "colon" => ':',
        "semicolon" => ';',
        "hyphen" | "minus" => '-',
        "endash" => '–',
        "emdash" => '—',
        "quoteright" => '’',
        "quoteleft" => '‘',
        "quotedblleft" => '“',
        "quotedblright" => '”',
        "quotesingle" => '\'',
        "quotedbl" => '"',
        "parenleft" => '(',
        "parenright" => ')',
        "bracketleft" => '[',
        "bracketright" => ']',
        "slash" => '/',
        "backslash" => '\\',
        "exclam" => '!',
        "question" => '?',
        "percent" => '%',
        "ampersand" => '&',
        "at" => '@',
        "numbersign" => '#',
        "dollar" => '$',
        "asterisk" => '*',
        "plus" => '+',
        "equal" => '=',
        "less" => '<',
        "greater" => '>',
        "underscore" => '_',
        "bullet" => '•',
        "zero" => '0',
        "one" => '1',
        "two" => '2',
        "three" => '3',
        "four" => '4',
        "five" => '5',
        "six" => '6',
        "seven" => '7',
        "eight" => '8',
        "nine" => '9',
        "fi" => 'ﬁ',
        "fl" => 'ﬂ',
        _ => return None,
    })
}

fn utf16_hex(h: &[u8]) -> String {
    let units: Vec<u16> = h.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
    String::from_utf16_lossy(&units)
}

fn hex_bytes(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()).collect()
}

fn be(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |a, x| (a << 8) | *x as u32)
}

/// Parses bfchar / bfrange sections of a ToUnicode CMap. Returns the map and the code width.
fn parse_cmap(data: &[u8]) -> (HashMap<u32, String>, Option<usize>) {
    let s = String::from_utf8_lossy(data);
    let mut map = HashMap::new();
    let mut width = None;
    // Tokens: <hex>, [, ], words.
    let mut toks: Vec<String> = Vec::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '<' => {
                let mut h = String::from("<");
                for d in it.by_ref() {
                    if d == '>' {
                        break;
                    }
                    h.push(d);
                }
                toks.push(h);
            }
            '[' | ']' => toks.push(c.to_string()),
            c if c.is_whitespace() => {}
            _ => {
                let mut w = c.to_string();
                while let Some(&d) = it.peek() {
                    if d.is_whitespace() || d == '<' || d == '[' || d == ']' {
                        break;
                    }
                    w.push(d);
                    it.next();
                }
                toks.push(w);
            }
        }
    }
    let mut i = 0;
    let mut mode = "";
    while i < toks.len() {
        let t = toks[i].as_str();
        match t {
            "begincodespacerange" | "beginbfchar" | "beginbfrange" => {
                mode = t;
                i += 1;
                continue;
            }
            "endcodespacerange" | "endbfchar" | "endbfrange" => {
                mode = "";
                i += 1;
                continue;
            }
            _ => {}
        }
        match mode {
            "begincodespacerange" if t.starts_with('<') => {
                width.get_or_insert(hex_bytes(t).len().max(1));
                i += 2;
            }
            "beginbfchar" if t.starts_with('<') && i + 1 < toks.len() => {
                let src = hex_bytes(t);
                width.get_or_insert(src.len().max(1));
                map.insert(be(&src), utf16_hex(&hex_bytes(&toks[i + 1])));
                i += 2;
            }
            "beginbfrange" if t.starts_with('<') && i + 2 < toks.len() => {
                let lo = hex_bytes(t);
                width.get_or_insert(lo.len().max(1));
                let (lo, hi) = (be(&lo), be(&hex_bytes(&toks[i + 1])));
                if toks[i + 2] == "[" {
                    let mut j = i + 3;
                    let mut c = lo;
                    while j < toks.len() && toks[j] != "]" {
                        map.insert(c, utf16_hex(&hex_bytes(&toks[j])));
                        c += 1;
                        j += 1;
                    }
                    i = j + 1;
                } else {
                    let dst = hex_bytes(&toks[i + 2]);
                    for (k, c) in (lo..=hi.min(lo + 0xFFFF)).enumerate() {
                        let mut d = dst.clone();
                        if let Some(last) = d.last_mut() {
                            *last = last.wrapping_add(k as u8);
                        }
                        map.insert(c, utf16_hex(&d));
                    }
                    i += 3;
                }
            }
            _ => i += 1,
        }
    }
    (map, width)
}

/// "ABCDEF+TimesNewRomanPS-BoldItalicMT" → ("Times New Roman", bold, italic).
pub fn match_family(base: &str, flags: i64) -> (String, bool, bool) {
    let name = base.split_once('+').map(|(p, r)| if p.len() <= 8 && p.chars().all(|c| c.is_ascii_alphanumeric()) { r } else { base }).unwrap_or(base);
    let low = name.to_lowercase();
    let bold = low.contains("bold") || low.contains("black") || low.contains("heavy") || low.contains("semibold") || flags & (1 << 18) != 0;
    let italic = low.contains("italic") || low.contains("oblique") || flags & (1 << 6) != 0;
    let core = name.split([',', '-']).next().unwrap_or(name);
    let core = core.trim_end_matches("PSMT").trim_end_matches("MT").trim_end_matches("PS");
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let key = norm(core);
    let installed = kit42::fonts::family_names();
    if let Some(f) = installed.iter().find(|f| norm(f) == key) {
        return (f.clone(), bold, italic);
    }
    if let Some(f) = installed.iter().filter(|f| key.starts_with(&norm(f)) && norm(f).len() >= 4).max_by_key(|f| f.len()) {
        // Windows file stems: arialbd, timesbi, calibriz, ariali…
        let (b, i) = match &key[norm(f).len()..] {
            "bd" | "b" => (true, false),
            "i" => (false, true),
            "bi" | "bdi" | "z" => (true, true),
            _ => (false, false),
        };
        return (f.clone(), bold || b, italic || i);
    }
    let fam = if key.contains("times") || key.contains("serif") && !key.contains("sans") || key.contains("roman") || key.contains("georgia") || key.contains("garamond") || flags & 2 != 0 && !key.contains("sans") {
        "Times New Roman"
    } else if key.contains("courier") || key.contains("mono") || flags & 1 != 0 {
        "Courier New"
    } else if key.contains("calibri") || key.contains("carlito") {
        "Calibri"
    } else if key.contains("cambria") {
        "Cambria"
    } else if key.contains("verdana") {
        "Verdana"
    } else if key.contains("tahoma") {
        "Tahoma"
    } else {
        "Arial"
    };
    (fam.to_string(), bold, italic)
}

fn load_font(doc: &Document, fd: &Dictionary) -> Font {
    let subtype = get(doc, fd, b"Subtype").and_then(|o| o.as_name().ok()).unwrap_or(b"");
    let two_byte = subtype == b"Type0";
    let base = get(doc, fd, b"BaseFont").and_then(|o| o.as_name().ok()).map(|b| String::from_utf8_lossy(b).to_string()).unwrap_or_default();
    let mut widths = HashMap::new();
    let mut default_w = if two_byte { 1000.0 } else { 500.0 };
    let mut desc = get(doc, fd, b"FontDescriptor").and_then(|o| dict(doc, o));
    if two_byte {
        if let Some(Object::Array(arr)) = get(doc, fd, b"DescendantFonts") {
            if let Some(cid) = arr.first().and_then(|o| dict(doc, o)) {
                if let Some(dw) = get(doc, cid, b"DW").and_then(num) {
                    default_w = dw;
                }
                desc = desc.or_else(|| get(doc, cid, b"FontDescriptor").and_then(|o| dict(doc, o)));
                if let Some(Object::Array(w)) = get(doc, cid, b"W") {
                    let mut i = 0;
                    while i < w.len() {
                        let Some(c0) = num(deref(doc, &w[i])) else { break };
                        match w.get(i + 1).map(|o| deref(doc, o)) {
                            Some(Object::Array(list)) => {
                                for (k, v) in list.iter().enumerate() {
                                    if let Some(v) = num(deref(doc, v)) {
                                        widths.insert(c0 as u32 + k as u32, v);
                                    }
                                }
                                i += 2;
                            }
                            Some(o) => {
                                let c1 = num(o).unwrap_or(c0);
                                let v = w.get(i + 2).and_then(|o| num(deref(doc, o))).unwrap_or(default_w);
                                for c in c0 as u32..=(c1 as u32).min(c0 as u32 + 0xFFFF) {
                                    widths.insert(c, v);
                                }
                                i += 3;
                            }
                            None => break,
                        }
                    }
                }
            }
        }
    } else {
        let first = get(doc, fd, b"FirstChar").and_then(num).unwrap_or(0.0) as u32;
        if let Some(Object::Array(w)) = get(doc, fd, b"Widths") {
            for (k, v) in w.iter().enumerate() {
                if let Some(v) = num(deref(doc, v)) {
                    widths.insert(first + k as u32, v);
                }
            }
        }
        if let Some(mw) = desc.and_then(|d| get(doc, d, b"MissingWidth")).and_then(num) {
            if mw > 0.0 {
                default_w = mw;
            }
        }
    }
    let flags = desc.and_then(|d| get(doc, d, b"Flags")).and_then(|o| o.as_i64().ok()).unwrap_or(0);
    let ascent = desc.and_then(|d| get(doc, d, b"Ascent")).and_then(num).filter(|v| *v > 100.0).map(|v| v / 1000.0).unwrap_or(0.8);
    let descent = desc.and_then(|d| get(doc, d, b"Descent")).and_then(num).filter(|v| *v < 0.0).map(|v| -v / 1000.0).unwrap_or(0.2);
    // Simple-font decoding: WinAnsi base with /Differences.
    let mut simple: Vec<Option<char>> = (0..256u32)
        .map(|c| match c {
            0x80..=0x9F => char::from_u32(CP1252[(c - 0x80) as usize] as u32).filter(|ch| *ch != '\0'),
            _ => char::from_u32(c),
        })
        .collect();
    if let Some(enc) = get(doc, fd, b"Encoding").and_then(|o| dict(doc, o)) {
        if let Some(Object::Array(diff)) = get(doc, enc, b"Differences") {
            let mut code = 0usize;
            for o in diff {
                match deref(doc, o) {
                    Object::Integer(i) => code = *i as usize,
                    Object::Name(n) => {
                        if code < 256 {
                            simple[code] = glyph_char(&String::from_utf8_lossy(n));
                        }
                        code += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    let mut to_uni = HashMap::new();
    let mut two = two_byte;
    if let Some(Object::Stream(s)) = fd.get(b"ToUnicode").ok().map(|o| deref(doc, o)) {
        let data = s.decompressed_content().unwrap_or_else(|_| s.content.clone());
        let (m, w) = parse_cmap(&data);
        to_uni = m;
        if let Some(w) = w {
            two = w >= 2;
        }
    }
    let (family, bold, italic) = match_family(&base, flags);
    Font { two_byte: two, widths, default_w, to_uni, simple, family, bold, italic, ascent, descent }
}

// ───────────────────────────── interpreter ─────────────────────────────

/// Where a glyph's bytes live in the page's own content stream.
#[derive(Clone, Copy, Debug)]
struct Src {
    op: usize,
    elem: usize,
    start: usize,
    len: usize,
    /// TJ adjustment that reproduces the glyph's advance.
    adv_n: f32,
}

#[derive(Clone, Debug)]
struct Glyph {
    x: f32,
    y: f32,
    adv: f32,
    size: f32,
    text: String,
    font: usize,
    color: [u8; 3],
    hidden: bool,
    src: Option<Src>,
}

#[derive(Clone)]
struct GState {
    ctm: M,
    fill: [u8; 3],
    font: Option<usize>,
    size: f32,
    tc: f32,
    tw: f32,
    th: f32,
    tl: f32,
    rise: f32,
    mode: i64,
}

struct Interp<'a> {
    doc: &'a Document,
    g: Geom,
    fonts: Vec<Rc<Font>>,
    font_ids: HashMap<ObjectId, usize>,
    glyphs: Vec<Glyph>,
    images: Vec<PageImage>,
}

fn cmyk(c: f32, m: f32, y: f32, k: f32) -> [u8; 3] {
    [((1.0 - c) * (1.0 - k) * 255.0) as u8, ((1.0 - m) * (1.0 - k) * 255.0) as u8, ((1.0 - y) * (1.0 - k) * 255.0) as u8]
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

impl<'a> Interp<'a> {
    fn font_for(&mut self, res: &Dictionary, key: &[u8]) -> Option<usize> {
        let fonts = get(self.doc, res, b"Font").and_then(|o| dict(self.doc, o))?;
        let raw = fonts.get(key).ok()?;
        let id = match raw {
            Object::Reference(id) => Some(*id),
            _ => None,
        };
        if let Some(i) = id.and_then(|id| self.font_ids.get(&id)) {
            return Some(*i);
        }
        let fd = dict(self.doc, raw)?;
        self.fonts.push(Rc::new(load_font(self.doc, fd)));
        let idx = self.fonts.len() - 1;
        if let Some(id) = id {
            self.font_ids.insert(id, idx);
        }
        Some(idx)
    }

    fn run(&mut self, ops: &[Operation], res: &Dictionary, gs: &mut GState, top: bool, depth: usize) {
        let mut stack: Vec<GState> = Vec::new();
        let (mut tm, mut tlm) = (ID, ID);
        for (oi, op) in ops.iter().enumerate() {
            let a = &op.operands;
            match op.operator.as_str() {
                "q" => stack.push(gs.clone()),
                "Q" => {
                    if let Some(s) = stack.pop() {
                        *gs = s;
                    }
                }
                "cm" => {
                    let v = nums(a);
                    if v.len() == 6 {
                        gs.ctm = mul(&[v[0], v[1], v[2], v[3], v[4], v[5]], &gs.ctm);
                    }
                }
                "rg" => {
                    let v = nums(a);
                    if v.len() == 3 {
                        gs.fill = [to8(v[0]), to8(v[1]), to8(v[2])];
                    }
                }
                "g" => {
                    if let Some(v) = nums(a).first() {
                        gs.fill = [to8(*v); 3];
                    }
                }
                "k" => {
                    let v = nums(a);
                    if v.len() == 4 {
                        gs.fill = cmyk(v[0], v[1], v[2], v[3]);
                    }
                }
                "sc" | "scn" => {
                    let v = nums(a);
                    gs.fill = match v.len() {
                        1 => [to8(v[0]); 3],
                        3 => [to8(v[0]), to8(v[1]), to8(v[2])],
                        4 => cmyk(v[0], v[1], v[2], v[3]),
                        _ => gs.fill,
                    };
                }
                "BT" => {
                    tm = ID;
                    tlm = ID;
                }
                "Tf" => {
                    if let (Some(Object::Name(n)), Some(s)) = (a.first(), a.get(1).and_then(num)) {
                        gs.font = self.font_for(res, n);
                        gs.size = s;
                    }
                }
                "Tc" => gs.tc = a.first().and_then(num).unwrap_or(0.0),
                "Tw" => gs.tw = a.first().and_then(num).unwrap_or(0.0),
                "Tz" => gs.th = a.first().and_then(num).unwrap_or(100.0) / 100.0,
                "TL" => gs.tl = a.first().and_then(num).unwrap_or(0.0),
                "Ts" => gs.rise = a.first().and_then(num).unwrap_or(0.0),
                "Tr" => gs.mode = a.first().and_then(|o| o.as_i64().ok()).unwrap_or(0),
                "Td" | "TD" => {
                    let v = nums(a);
                    if v.len() == 2 {
                        if op.operator == "TD" {
                            gs.tl = -v[1];
                        }
                        tlm = mul(&[1.0, 0.0, 0.0, 1.0, v[0], v[1]], &tlm);
                        tm = tlm;
                    }
                }
                "Tm" => {
                    let v = nums(a);
                    if v.len() == 6 {
                        tlm = [v[0], v[1], v[2], v[3], v[4], v[5]];
                        tm = tlm;
                    }
                }
                "T*" => {
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -gs.tl], &tlm);
                    tm = tlm;
                }
                "Tj" | "'" | "\"" | "TJ" => {
                    if op.operator != "Tj" && op.operator != "TJ" {
                        if op.operator == "\"" {
                            let v = nums(a);
                            if v.len() >= 2 {
                                gs.tw = v[0];
                                gs.tc = v[1];
                            }
                        }
                        tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -gs.tl], &tlm);
                        tm = tlm;
                    }
                    let elems: Vec<&Object> = match op.operator.as_str() {
                        "TJ" => match a.first() {
                            Some(Object::Array(v)) => v.iter().collect(),
                            _ => vec![],
                        },
                        _ => a.last().into_iter().collect(),
                    };
                    for (ei, e) in elems.iter().enumerate() {
                        match e {
                            Object::String(bytes, _) => self.show(bytes, gs, &mut tm, if top { Some((oi, ei)) } else { None }),
                            o => {
                                if let Some(n) = num(o) {
                                    let tx = -n / 1000.0 * gs.size * gs.th;
                                    tm = mul(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], &tm);
                                }
                            }
                        }
                    }
                }
                "Do" if depth < 6 => {
                    let Some(Object::Name(n)) = a.first() else { continue };
                    let Some(xo) = get(self.doc, res, b"XObject").and_then(|o| dict(self.doc, o)) else { continue };
                    let raw = xo.get(n).ok();
                    let Some(Object::Stream(s)) = raw.map(|o| deref(self.doc, o)) else { continue };
                    let subtype = s.dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok());
                    if subtype == Some(b"Image") {
                        let c = gs.ctm;
                        let pts = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(u, v): (f32, f32)| to_display(&self.g, u * c[0] + v * c[2] + c[4], u * c[1] + v * c[3] + c[5]));
                        let rect = [
                            pts.iter().map(|p| p.0).fold(f32::MAX, f32::min),
                            pts.iter().map(|p| p.1).fold(f32::MAX, f32::min),
                            pts.iter().map(|p| p.0).fold(f32::MIN, f32::max),
                            pts.iter().map(|p| p.1).fold(f32::MIN, f32::max),
                        ];
                        let dim = |k: &[u8]| s.dict.get(k).ok().and_then(|o| o.as_i64().ok()).unwrap_or(0) as u32;
                        self.images.push(PageImage {
                            rect,
                            ctm: c,
                            op: if top { Some(oi) } else { None },
                            name: n.clone(),
                            id: match raw {
                                Some(Object::Reference(id)) => Some(*id),
                                _ => None,
                            },
                            px: (dim(b"Width"), dim(b"Height")),
                        });
                        continue;
                    }
                    if subtype != Some(b"Form") {
                        continue;
                    }
                    let Ok(data) = s.decompressed_content().or_else(|_| Ok::<_, ()>(s.content.clone())) else { continue };
                    let Ok(content) = Content::decode(&data) else { continue };
                    let fres = s.dict.get(b"Resources").ok().and_then(|o| dict(self.doc, o)).cloned().unwrap_or_else(|| res.clone());
                    let mut inner = gs.clone();
                    if let Ok(Object::Array(m)) = s.dict.get(b"Matrix") {
                        let v: Vec<f32> = m.iter().filter_map(num).collect();
                        if v.len() == 6 {
                            inner.ctm = mul(&[v[0], v[1], v[2], v[3], v[4], v[5]], &gs.ctm);
                        }
                    }
                    self.run(&content.operations, &fres, &mut inner, false, depth + 1);
                }
                _ => {}
            }
        }
    }

    fn show(&mut self, bytes: &[u8], gs: &GState, tm: &mut M, src: Option<(usize, usize)>) {
        let Some(fi) = gs.font else { return };
        let font = self.fonts[fi].clone();
        let step = if font.two_byte { 2 } else { 1 };
        let mut i = 0;
        while i + step <= bytes.len() {
            let code = be(&bytes[i..i + step]);
            let w0 = font.widths.get(&code).copied().unwrap_or(font.default_w) / 1000.0;
            let tw = if step == 1 && code == 32 { gs.tw } else { 0.0 };
            let tx = (w0 * gs.size + gs.tc + tw) * gs.th;
            let trm = mul(&[gs.size * gs.th, 0.0, 0.0, gs.size, 0.0, gs.rise], &mul(tm, &gs.ctm));
            let next_tm = mul(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], tm);
            let trm2 = mul(&[gs.size * gs.th, 0.0, 0.0, gs.size, 0.0, gs.rise], &mul(&next_tm, &gs.ctm));
            let (x0, y0) = to_display(&self.g, trm[4], trm[5]);
            let (x1, y1) = to_display(&self.g, trm2[4], trm2[5]);
            let size = (trm[2] * trm[2] + trm[3] * trm[3]).sqrt();
            let text = font.to_uni.get(&code).cloned().unwrap_or_else(|| {
                if font.two_byte {
                    char::from_u32(code).map(|c| c.to_string()).unwrap_or_default()
                } else {
                    font.simple.get(code as usize).copied().flatten().map(|c| c.to_string()).unwrap_or_default()
                }
            });
            // Only horizontal, left-to-right text in the displayed orientation is editable.
            if (y1 - y0).abs() < 0.05 * size.max(1.0) && x1 >= x0 - 0.01 && size > 0.5 {
                self.glyphs.push(Glyph {
                    x: x0,
                    y: y0,
                    adv: x1 - x0,
                    size,
                    text,
                    font: fi,
                    color: gs.fill,
                    hidden: gs.mode == 3 || gs.mode == 7,
                    src: src.map(|(op, elem)| Src { op, elem, start: i, len: step, adv_n: if gs.size.abs() > 1e-6 && gs.th.abs() > 1e-6 { -tx * 1000.0 / (gs.size * gs.th) } else { 0.0 } }),
                });
            }
            *tm = next_tm;
            i += step;
        }
    }
}

pub fn page_resources(doc: &Document, page: ObjectId) -> Dictionary {
    let mut cur = doc.get_dictionary(page).ok();
    for _ in 0..32 {
        let Some(d) = cur else { break };
        if let Some(r) = d.get(b"Resources").ok().and_then(|o| dict(doc, o)) {
            return r.clone();
        }
        cur = d.get(b"Parent").ok().and_then(|o| dict(doc, o));
    }
    Dictionary::new()
}

pub fn page_content(doc: &Document, page: ObjectId) -> Vec<u8> {
    let mut out = Vec::new();
    let Ok(pd) = doc.get_dictionary(page) else { return out };
    let ids: Vec<Object> = match pd.get(b"Contents") {
        Ok(Object::Array(a)) => a.clone(),
        Ok(o) => vec![o.clone()],
        Err(_) => vec![],
    };
    for o in ids {
        if let Object::Stream(s) = deref(doc, &o) {
            out.extend(s.decompressed_content().unwrap_or_else(|_| s.content.clone()));
            out.push(b'\n');
        }
    }
    out
}

struct Parsed {
    glyphs: Vec<Glyph>,
    fonts: Vec<Rc<Font>>,
    content: Content,
    images: Vec<PageImage>,
}

fn interpret(doc: &Document, page: ObjectId) -> Option<Parsed> {
    let content = Content::decode(&page_content(doc, page)).ok()?;
    let res = page_resources(doc, page);
    let mut it = Interp { doc, g: pdfops::geom(doc, page), fonts: vec![], font_ids: HashMap::new(), glyphs: vec![], images: vec![] };
    let mut gs = GState { ctm: ID, fill: [0, 0, 0], font: None, size: 0.0, tc: 0.0, tw: 0.0, th: 1.0, tl: 0.0, rise: 0.0, mode: 0 };
    it.run(&content.operations, &res, &mut gs, true, 0);
    Some(Parsed { glyphs: it.glyphs, fonts: it.fonts, content, images: it.images })
}

/// The page's decoded content operations (as indexed by `PageImage::op`).
pub fn page_ops(doc: &Document, page: ObjectId) -> Option<Vec<Operation>> {
    Content::decode(&page_content(doc, page)).ok().map(|c| c.operations)
}

/// Images painted on a page, in drawing order (topmost last).
pub fn page_images(doc: &Document, page: ObjectId) -> Vec<PageImage> {
    interpret(doc, page).map(|p| p.images).unwrap_or_default()
}

/// Replaces the page's content with `ops` as one compressed stream.
pub fn write_content(doc: &mut Document, page: ObjectId, ops: Vec<Operation>) -> Result<(), String> {
    let bytes = (Content { operations: ops }).encode().map_err(|e| e.to_string())?;
    let mut st = lopdf::Stream::new(lopdf::dictionary! { "Filter" => "FlateDecode" }, kit42::pdfgen::deflate(&bytes));
    st.allows_compression = false;
    let id = doc.add_object(st);
    let pd = doc.get_dictionary_mut(page).map_err(|e| e.to_string())?;
    pd.set("Contents", Object::Reference(id));
    Ok(())
}

/// Text lines of a page from its own text layer.
pub fn page_lines(doc: &Document, page: ObjectId) -> Vec<Line> {
    let Some(Parsed { glyphs, fonts, .. }) = interpret(doc, page) else { return vec![] };
    struct Acc {
        idx: Vec<usize>,
        text: String,
        x0: f32,
        x1: f32,
        y: f32,
        size: f32,
    }
    let mut lines: Vec<Acc> = Vec::new();
    for (gi, g) in glyphs.iter().enumerate() {
        let joined = lines.iter_mut().rev().take(3).find(|l| {
            (l.y - g.y).abs() < 0.3 * l.size.max(g.size) && g.x >= l.x1 - 0.6 * g.size && g.x - l.x1 < 1.5 * g.size && l.idx.last().map(|&j| glyphs[j].hidden) == Some(g.hidden)
        });
        match joined {
            Some(l) => {
                if g.x - l.x1 > 0.18 * g.size && !l.text.ends_with(' ') && !g.text.starts_with(' ') {
                    l.text.push(' ');
                }
                l.text.push_str(&g.text);
                l.x1 = l.x1.max(g.x + g.adv);
                l.idx.push(gi);
            }
            None => lines.push(Acc { idx: vec![gi], text: g.text.clone(), x0: g.x, x1: g.x + g.adv, y: g.y, size: g.size }),
        }
    }
    lines
        .into_iter()
        .filter(|l| !l.text.trim().is_empty())
        .map(|l| {
            // Trim leading/trailing spaces from the box.
            let vis: Vec<&Glyph> = l.idx.iter().map(|&i| &glyphs[i]).filter(|g| !g.text.trim().is_empty()).collect();
            let first = vis.first().copied().unwrap_or(&glyphs[l.idx[0]]);
            let x0 = vis.iter().map(|g| g.x).fold(f32::MAX, f32::min);
            let x1 = vis.iter().map(|g| g.x + g.adv).fold(f32::MIN, f32::max);
            let mut sizes: Vec<f32> = vis.iter().map(|g| g.size).collect();
            sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let size = sizes.get(sizes.len() / 2).copied().unwrap_or(l.size);
            let f = &fonts[first.font];
            Line {
                rect: [x0, l.y - f.ascent * size, x1, l.y + f.descent * size],
                baseline: l.y,
                text: l.text.trim().to_string(),
                size,
                font: f.family.clone(),
                bold: f.bold,
                italic: f.italic,
                color: first.color,
                bg: [255, 255, 255],
                hidden: first.hidden,
            }
        })
        .collect()
}

/// Lines recognised by OCR in a page raster; `k` converts pixels to points.
pub fn ocr_lines(res: &kit42::winrt::OcrResult, k: f32) -> Vec<Line> {
    let boxes: Vec<_> = res.lines.iter().filter(|l| !l.words.is_empty()).collect();
    // Word boxes span cap/ascender top to descender bottom: about 0.95 em.
    let mut sizes: Vec<f32> = boxes.iter().map(|l| l.bbox().3 * k / 0.95).collect();
    // A line without ascenders or descenders ("чаю.") looks smaller than its neighbours.
    let raw = sizes.clone();
    for i in 0..sizes.len() {
        let near = [i.checked_sub(1), Some(i + 1)].into_iter().flatten().filter_map(|j| raw.get(j)).cloned().fold(f32::MAX, f32::min);
        if near < f32::MAX && raw[i] < 0.8 * near && raw[i] > 0.5 * near {
            sizes[i] = near;
        }
    }
    boxes
        .iter()
        .zip(sizes)
        .map(|(l, size)| {
            let (x, y, w, h) = l.bbox();
            let text = l.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
            let size = size.max(4.0);
            Line {
                rect: [x * k, y * k, (x + w) * k, (y + h) * k],
                baseline: y * k + 0.74 * size,
                text,
                size,
                font: "Arial".into(),
                bold: false,
                italic: false,
                color: [0, 0, 0],
                bg: [255, 255, 255],
                hidden: true,
            }
        })
        .collect()
}

/// Samples the background (border ring) and text colour (most contrasting pixels) of each line.
pub fn sample_colors(lines: &mut [Line], img: &image::RgbaImage, px_per_pt: f32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let px = |x: i32, y: i32| {
        let p = img.get_pixel(x.clamp(0, w - 1) as u32, y.clamp(0, h - 1) as u32);
        [p[0], p[1], p[2]]
    };
    for l in lines {
        let x0 = (l.rect[0] * px_per_pt) as i32;
        let y0 = (l.rect[1] * px_per_pt) as i32;
        let x1 = (l.rect[2] * px_per_pt).ceil() as i32;
        let y1 = (l.rect[3] * px_per_pt).ceil() as i32;
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let pad = (2.0 * px_per_pt) as i32 + 1;
        let mut ring = Vec::new();
        for x in (x0 - pad..=x1 + pad).step_by(2) {
            ring.push(px(x, y0 - pad));
            ring.push(px(x, y1 + pad));
        }
        for y in (y0 - pad..=y1 + pad).step_by(2) {
            ring.push(px(x0 - pad, y));
            ring.push(px(x1 + pad, y));
        }
        let med = |v: &mut Vec<[u8; 3]>, c: usize| {
            v.sort_by_key(|p| p[c]);
            v[v.len() / 2][c]
        };
        let bg = [med(&mut ring, 0), med(&mut ring, 1), med(&mut ring, 2)];
        l.bg = bg;
        if l.hidden {
            let mut inside: Vec<([u8; 3], i32)> = Vec::new();
            for y in (y0..y1).step_by(1) {
                for x in (x0..x1).step_by(1) {
                    let p = px(x, y);
                    let d = (0..3).map(|c| (p[c] as i32 - bg[c] as i32).abs()).sum::<i32>();
                    inside.push((p, d));
                }
            }
            inside.sort_by_key(|(_, d)| -d);
            let n = (inside.len() / 12).max(1);
            if inside[0].1 > 60 {
                let mut acc = [0u32; 3];
                for (p, _) in &inside[..n] {
                    for c in 0..3 {
                        acc[c] += p[c] as u32;
                    }
                }
                l.color = acc.map(|v| (v / n as u32) as u8);
            }
        }
    }
}

/// Removes the glyphs whose centre lies in each rectangle (displayed points) from the page's own
/// content stream. Returns, per rectangle, whether the text there is gone (glyphs were found and
/// all of them came from the page content); otherwise the caller covers the area instead.
pub fn remove_text(doc: &mut Document, page: ObjectId, rects: &[[f32; 4]]) -> Vec<bool> {
    let Some(Parsed { glyphs, content, .. }) = interpret(doc, page) else { return vec![false; rects.len()] };
    let mut ok = vec![true; rects.len()];
    let mut found = vec![false; rects.len()];
    // op index → removals (elem, start, len, adv_n)
    let mut cut: HashMap<usize, Vec<(usize, usize, usize, f32)>> = HashMap::new();
    for g in &glyphs {
        let (cx, cy) = (g.x + g.adv / 2.0, g.y - g.size * 0.3);
        for (ri, r) in rects.iter().enumerate() {
            if cx >= r[0] - 0.5 && cx <= r[2] + 0.5 && cy >= r[1] - 0.5 && cy <= r[3] + 0.5 {
                found[ri] = true;
                match g.src {
                    Some(s) => cut.entry(s.op).or_default().push((s.elem, s.start, s.len, s.adv_n)),
                    None => ok[ri] = false,
                }
            }
        }
    }
    for i in 0..rects.len() {
        ok[i] &= found[i];
    }
    if cut.is_empty() {
        return ok;
    }
    let mut out: Vec<Operation> = Vec::with_capacity(content.operations.len() + cut.len() * 2);
    for (oi, op) in content.operations.into_iter().enumerate() {
        let Some(list) = cut.get(&oi) else {
            out.push(op);
            continue;
        };
        let elems: Vec<Object> = match op.operator.as_str() {
            "TJ" => match op.operands.first() {
                Some(Object::Array(v)) => v.clone(),
                _ => vec![],
            },
            _ => op.operands.last().cloned().into_iter().collect(),
        };
        let mut arr: Vec<Object> = Vec::new();
        let push_num = |arr: &mut Vec<Object>, n: f32| {
            if let Some(Object::Real(prev)) = arr.last_mut() {
                *prev += n;
            } else {
                arr.push(Object::Real(n));
            }
        };
        for (ei, e) in elems.into_iter().enumerate() {
            match e {
                Object::String(bytes, fmt) => {
                    let mut gone: Vec<(usize, usize, f32)> = list.iter().filter(|c| c.0 == ei).map(|c| (c.1, c.2, c.3)).collect();
                    if gone.is_empty() {
                        arr.push(Object::String(bytes, fmt));
                        continue;
                    }
                    gone.sort_by_key(|g| g.0);
                    let mut pos = 0;
                    for (start, len, adv) in gone {
                        if start > pos {
                            arr.push(Object::String(bytes[pos..start].to_vec(), fmt));
                        }
                        push_num(&mut arr, adv);
                        pos = start + len;
                    }
                    if pos < bytes.len() {
                        arr.push(Object::String(bytes[pos..].to_vec(), fmt));
                    }
                }
                other => {
                    if let Some(n) = num(&other) {
                        push_num(&mut arr, n);
                    } else {
                        arr.push(other);
                    }
                }
            }
        }
        match op.operator.as_str() {
            "'" => out.push(Operation::new("T*", vec![])),
            "\"" => {
                out.push(Operation::new("Tw", vec![op.operands[0].clone()]));
                out.push(Operation::new("Tc", vec![op.operands[1].clone()]));
                out.push(Operation::new("T*", vec![]));
            }
            _ => {}
        }
        if arr.is_empty() {
            arr.push(Object::String(vec![], StringFormat::Literal));
        }
        out.push(Operation::new("TJ", vec![Object::Array(arr)]));
    }
    match write_content(doc, page, out) {
        Ok(()) => ok,
        Err(_) => vec![false; rects.len()],
    }
}
