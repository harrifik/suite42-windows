//! ZIP packages (OOXML / OpenDocument) and small XML helpers.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

/// All parts of a ZIP package in memory.
pub struct Package {
    pub parts: BTreeMap<String, Vec<u8>>,
}

impl Package {
    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let mut parts = BTreeMap::new();
        let mut total = 0usize;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
            if f.is_dir() {
                continue;
            }
            let name = f.name().trim_start_matches('/').to_string();
            let mut buf = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            total += buf.len();
            if total > 1_500_000_000 {
                return Err("package too large".into());
            }
            parts.insert(name, buf);
        }
        Ok(Self { parts })
    }

    pub fn text(&self, name: &str) -> Option<String> {
        self.parts.get(name).map(|b| decode_xml_bytes(b))
    }

    pub fn get(&self, name: &str) -> Option<&Vec<u8>> {
        self.parts.get(name)
    }
}

/// Decodes XML bytes (UTF-8 with optional BOM, or UTF-16).
pub fn decode_xml_bytes(b: &[u8]) -> String {
    if b.starts_with(&[0xFF, 0xFE]) {
        let (s, _, _) = encoding_rs::UTF_16LE.decode(b);
        return s.into_owned();
    }
    if b.starts_with(&[0xFE, 0xFF]) {
        let (s, _, _) = encoding_rs::UTF_16BE.decode(b);
        return s.into_owned();
    }
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    String::from_utf8_lossy(b).into_owned()
}

/// Builds a ZIP package. `mimetype` (OpenDocument) is stored first and uncompressed.
pub struct PackageWriter {
    zip: zip::ZipWriter<Cursor<Vec<u8>>>,
}

impl Default for PackageWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl PackageWriter {
    pub fn new() -> Self {
        Self { zip: zip::ZipWriter::new(Cursor::new(Vec::new())) }
    }

    pub fn stored(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        self.zip.start_file(name, opts).map_err(|e| e.to_string())?;
        self.zip.write_all(data).map_err(|e| e.to_string())
    }

    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(data.len() > 0xFFFF_0000);
        self.zip.start_file(name, opts).map_err(|e| e.to_string())?;
        self.zip.write_all(data).map_err(|e| e.to_string())
    }

    pub fn add_str(&mut self, name: &str, data: &str) -> Result<(), String> {
        self.add(name, data.as_bytes())
    }

    pub fn finish(self) -> Result<Vec<u8>, String> {
        self.zip.finish().map(|c| c.into_inner()).map_err(|e| e.to_string())
    }
}

/// Escapes text for XML content and attributes; drops characters XML 1.0 cannot carry.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// Helpers over roxmltree that ignore namespaces (OOXML files use many prefixes).
pub trait NodeExt<'a, 'i> {
    fn child(&self, local: &str) -> Option<roxmltree::Node<'a, 'i>>;
    fn children_named(&self, local: &str) -> Vec<roxmltree::Node<'a, 'i>>;
    fn descendant(&self, local: &str) -> Option<roxmltree::Node<'a, 'i>>;
    fn descendants_named(&self, local: &str) -> Vec<roxmltree::Node<'a, 'i>>;
    fn attr(&self, local: &str) -> Option<&'a str>;
    fn is(&self, local: &str) -> bool;
    fn text_all(&self) -> String;
}

impl<'a, 'i> NodeExt<'a, 'i> for roxmltree::Node<'a, 'i> {
    fn child(&self, local: &str) -> Option<roxmltree::Node<'a, 'i>> {
        self.children().find(|n| n.is_element() && n.tag_name().name() == local)
    }

    fn children_named(&self, local: &str) -> Vec<roxmltree::Node<'a, 'i>> {
        self.children().filter(|n| n.is_element() && n.tag_name().name() == local).collect()
    }

    fn descendant(&self, local: &str) -> Option<roxmltree::Node<'a, 'i>> {
        self.descendants().find(|n| n.is_element() && n.tag_name().name() == local)
    }

    fn descendants_named(&self, local: &str) -> Vec<roxmltree::Node<'a, 'i>> {
        self.descendants().filter(|n| n.is_element() && n.tag_name().name() == local).collect()
    }

    fn attr(&self, local: &str) -> Option<&'a str> {
        self.attributes().find(|a| a.name() == local).map(|a| a.value())
    }

    fn is(&self, local: &str) -> bool {
        self.is_element() && self.tag_name().name() == local
    }

    fn text_all(&self) -> String {
        self.descendants().filter(|n| n.is_text()).filter_map(|n| n.text()).collect()
    }
}

/// Parses XML with DTDs allowed (some producers include them).
pub fn parse(xml: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions { allow_dtd: true, nodes_limit: u32::MAX },
    )
    .map_err(|e| e.to_string())
}

/// Resolves a relationship target relative to the part that owns it.
pub fn resolve_target(owner_part: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut base: Vec<&str> = owner_part.split('/').collect();
    base.pop();
    for seg in target.split('/') {
        match seg {
            ".." => {
                base.pop();
            }
            "." | "" => {}
            s => base.push(s),
        }
    }
    base.join("/")
}

/// Relationships of a part: id → (type suffix, absolute target).
pub fn relationships(pkg: &Package, part: &str) -> BTreeMap<String, (String, String)> {
    let (dir, file) = match part.rfind('/') {
        Some(i) => (&part[..i], &part[i + 1..]),
        None => ("", part),
    };
    let rels_name = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
    let mut map = BTreeMap::new();
    let Some(xml) = pkg.text(&rels_name) else { return map };
    let Ok(doc) = parse(&xml) else { return map };
    for r in doc.root_element().children_named("Relationship") {
        let id = r.attr("Id").unwrap_or("").to_string();
        let ty = r.attr("Type").unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
        let target = r.attr("Target").unwrap_or("");
        let external = r.attr("TargetMode") == Some("External");
        let abs = if external { target.to_string() } else { resolve_target(part, target) };
        map.insert(id, (ty, abs));
    }
    map
}

/// MIME type for an image by its bytes.
pub fn image_mime(bytes: &[u8]) -> (&'static str, &'static str) {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        ("image/png", "png")
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        ("image/jpeg", "jpeg")
    } else if bytes.starts_with(b"GIF8") {
        ("image/gif", "gif")
    } else if bytes.starts_with(b"BM") {
        ("image/bmp", "bmp")
    } else if bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        ("image/webp", "webp")
    } else {
        ("image/png", "png")
    }
}

/// Reads all bytes from a path with a friendly error.
pub fn read_file(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut v = Vec::new();
    f.read_to_end(&mut v).map_err(|e| e.to_string())?;
    Ok(v)
}

/// Decodes a text file: BOMs, UTF-8, otherwise Windows-1251 / 1252 heuristics.
pub fn decode_text(bytes: &[u8]) -> (String, &'static encoding_rs::Encoding) {
    if let Some((enc, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        let (s, _, _) = enc.decode(&bytes[bom..]);
        return (s.into_owned(), enc);
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_string(), encoding_rs::UTF_8);
    }
    // Many bytes in 0xC0..=0xFF suggests Cyrillic text in Windows-1251.
    let high = bytes.iter().filter(|&&b| b >= 0xC0).count();
    let enc = if high * 4 > bytes.iter().filter(|&&b| b >= 0x80).count() * 3 {
        encoding_rs::WINDOWS_1251
    } else {
        encoding_rs::WINDOWS_1252
    };
    let (s, _, _) = enc.decode(bytes);
    (s.into_owned(), enc)
}
