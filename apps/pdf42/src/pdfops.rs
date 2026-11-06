//! PDF structure operations on lopdf documents: page tree, geometry, overlays stamped as
//! Form XObjects, annotations, outlines, attachments, forms, compression, merge and split.

use kit42::pdfgen;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use std::collections::BTreeMap;

pub type R<T> = Result<T, String>;

fn e<E: std::fmt::Display>(x: E) -> String {
    x.to_string()
}

pub fn load(bytes: &[u8], password: Option<&str>) -> R<Document> {
    let mut doc = Document::load_mem(bytes).map_err(e)?;
    if doc.is_encrypted() {
        doc.decrypt(password.unwrap_or("")).map_err(|_| "password".to_string())?;
        doc.trailer.remove(b"Encrypt");
    }
    Ok(doc)
}

pub fn save(doc: &mut Document) -> R<Vec<u8>> {
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(e)?;
    Ok(out)
}

fn root_pages(doc: &Document) -> R<ObjectId> {
    doc.catalog().map_err(e)?.get(b"Pages").and_then(|o| o.as_reference()).map_err(e)
}

fn inherited<'a>(doc: &'a Document, page: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut id = page;
    for _ in 0..64 {
        let d = doc.get_dictionary(id).ok()?;
        if let Ok(v) = d.get(key) {
            return Some(v);
        }
        id = d.get(b"Parent").and_then(|p| p.as_reference()).ok()?;
    }
    None
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
}

fn num(o: &Object) -> f32 {
    match o {
        Object::Integer(i) => *i as f32,
        Object::Real(r) => *r,
        _ => 0.0,
    }
}

fn rect_of(doc: &Document, o: &Object) -> Option<[f32; 4]> {
    let a = deref(doc, o).as_array().ok()?;
    if a.len() != 4 {
        return None;
    }
    let v: Vec<f32> = a.iter().map(|x| num(deref(doc, x))).collect();
    Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

/// Visible box (CropBox ∩ MediaBox) and rotation of a page.
#[derive(Clone, Copy, Debug)]
pub struct Geom {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub rot: i32,
}

impl Geom {
    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn h(&self) -> f32 {
        self.y1 - self.y0
    }
    /// Displayed size (rotation applied).
    pub fn display(&self) -> (f32, f32) {
        if self.rot % 180 == 0 {
            (self.w(), self.h())
        } else {
            (self.h(), self.w())
        }
    }
    /// Displayed point (top-left origin, points) → user space.
    pub fn to_user(&self, x: f32, y: f32) -> (f32, f32) {
        match self.rot {
            90 => (self.x0 + y, self.y0 + x),
            180 => (self.x1 - x, self.y0 + y),
            270 => (self.x1 - y, self.y1 - x),
            _ => (self.x0 + x, self.y1 - y),
        }
    }
    pub fn rect_to_user(&self, r: [f32; 4]) -> [f32; 4] {
        let (ax, ay) = self.to_user(r[0], r[1]);
        let (bx, by) = self.to_user(r[2], r[3]);
        [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
    }
    /// Matrix mapping a displayed-orientation overlay (origin bottom-left) into user space.
    pub fn overlay_matrix(&self) -> [f32; 6] {
        let (_, hd) = self.display();
        match self.rot {
            90 => [0.0, 1.0, -1.0, 0.0, self.x0 + hd, self.y0],
            180 => [-1.0, 0.0, 0.0, -1.0, self.x1, self.y0 + hd],
            270 => [0.0, -1.0, 1.0, 0.0, self.x1 - hd, self.y1],
            _ => [1.0, 0.0, 0.0, 1.0, self.x0, self.y0],
        }
    }
    pub fn rotation_matrix(&self) -> [f32; 6] {
        match self.rot {
            90 => [0.0, 1.0, -1.0, 0.0, 0.0, 0.0],
            180 => [-1.0, 0.0, 0.0, -1.0, 0.0, 0.0],
            270 => [0.0, -1.0, 1.0, 0.0, 0.0, 0.0],
            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }
}

pub fn geom(doc: &Document, page: ObjectId) -> Geom {
    let media = inherited(doc, page, b"MediaBox").and_then(|o| rect_of(doc, o)).unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let crop = inherited(doc, page, b"CropBox").and_then(|o| rect_of(doc, o)).unwrap_or(media);
    let b = [crop[0].max(media[0]), crop[1].max(media[1]), crop[2].min(media[2]), crop[3].min(media[3])];
    let b = if b[2] - b[0] < 1.0 || b[3] - b[1] < 1.0 { media } else { b };
    let rot = inherited(doc, page, b"Rotate").map(|o| num(deref(doc, o)) as i32).unwrap_or(0).rem_euclid(360);
    let rot = (rot / 90) * 90;
    Geom { x0: b[0], y0: b[1], x1: b[2], y1: b[3], rot }
}

pub fn page_ids(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().values().cloned().collect()
}

/// Makes the page tree flat (every page a direct kid of the root Pages node) with inherited
/// attributes copied onto pages, so pages can be reordered and inserted freely.
pub fn flatten_tree(doc: &mut Document) -> R<()> {
    let root = root_pages(doc)?;
    let pages = page_ids(doc);
    for &p in &pages {
        for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
            let has = doc.get_dictionary(p).map(|d| d.has(key)).unwrap_or(false);
            if !has {
                if let Some(v) = inherited(doc, p, key).cloned() {
                    doc.get_dictionary_mut(p).map_err(e)?.set(key.to_vec(), v);
                }
            }
        }
        doc.get_dictionary_mut(p).map_err(e)?.set("Parent", Object::Reference(root));
    }
    // Drop intermediate nodes.
    let mut stack: Vec<ObjectId> = doc
        .get_dictionary(root)
        .ok()
        .and_then(|d| d.get(b"Kids").ok())
        .and_then(|k| k.as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();
    while let Some(id) = stack.pop() {
        let is_node = doc.get_dictionary(id).map(|d| d.type_is(b"Pages")).unwrap_or(false);
        if is_node {
            if let Ok(kids) = doc.get_dictionary(id).and_then(|d| d.get(b"Kids")).and_then(|k| k.as_array()) {
                stack.extend(kids.iter().filter_map(|o| o.as_reference().ok()));
            }
            doc.objects.remove(&id);
        }
    }
    set_kids(doc, &pages)
}

fn set_kids(doc: &mut Document, pages: &[ObjectId]) -> R<()> {
    let root = root_pages(doc)?;
    let d = doc.get_dictionary_mut(root).map_err(e)?;
    d.set("Kids", Object::Array(pages.iter().map(|&p| Object::Reference(p)).collect()));
    d.set("Count", pages.len() as i64);
    Ok(())
}

/// Reorders pages: `order[i]` is the old index of the page that becomes page i.
pub fn reorder(doc: &mut Document, order: &[usize]) -> R<()> {
    flatten_tree(doc)?;
    let pages = page_ids(doc);
    let new: Vec<ObjectId> = order.iter().filter_map(|&i| pages.get(i).cloned()).collect();
    for p in pages.iter().filter(|p| !new.contains(p)) {
        doc.objects.remove(p);
    }
    set_kids(doc, &new)?;
    remove_dangling_outline_dests(doc);
    Ok(())
}

pub fn rotate(doc: &mut Document, idx: &[usize], delta: i32) -> R<()> {
    let pages = page_ids(doc);
    for &i in idx {
        if let Some(&p) = pages.get(i) {
            let cur = geom(doc, p).rot;
            let new = (cur + delta).rem_euclid(360);
            doc.get_dictionary_mut(p).map_err(e)?.set("Rotate", new as i64);
        }
    }
    Ok(())
}

pub fn delete(doc: &mut Document, idx: &[usize]) -> R<()> {
    let n = page_ids(doc).len();
    let keep: Vec<usize> = (0..n).filter(|i| !idx.contains(i)).collect();
    if keep.is_empty() {
        return Err(kit42::t("A document must keep at least one page.", "В документе должна остаться хотя бы одна страница.").into());
    }
    reorder(doc, &keep)?;
    doc.prune_objects();
    Ok(())
}

pub fn insert_blank(doc: &mut Document, at: usize, w: f32, h: f32) -> R<()> {
    flatten_tree(doc)?;
    let root = root_pages(doc)?;
    let content = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => root,
        "MediaBox" => vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)],
        "Resources" => dictionary! {},
        "Contents" => content,
    });
    let mut pages = page_ids(doc);
    pages.insert(at.min(pages.len()), page);
    set_kids(doc, &pages)
}

/// Copies all pages of `other` into `doc` at position `at`. Returns the number of pages inserted.
pub fn insert_document(doc: &mut Document, mut other: Document, at: usize) -> R<usize> {
    flatten_tree(doc)?;
    flatten_tree(&mut other)?;
    other.renumber_objects_with(doc.max_id + 1);
    let other_pages = page_ids(&other);
    let other_root = root_pages(&other)?;
    let other_catalog = other.trailer.get(b"Root").and_then(|r| r.as_reference()).ok();
    let root = root_pages(doc)?;
    for (id, obj) in std::mem::take(&mut other.objects) {
        if Some(id) == other_catalog || id == other_root {
            continue;
        }
        doc.objects.insert(id, obj);
    }
    doc.max_id = doc.max_id.max(other.max_id);
    for &p in &other_pages {
        if let Ok(d) = doc.get_dictionary_mut(p) {
            d.set("Parent", Object::Reference(root));
        }
    }
    let mut pages = page_ids(doc);
    let at = at.min(pages.len());
    for (k, p) in other_pages.iter().enumerate() {
        pages.insert(at + k, *p);
    }
    set_kids(doc, &pages)?;
    Ok(other_pages.len())
}

/// A new document containing only the given pages (0-based).
pub fn extract(bytes: &[u8], idx: &[usize]) -> R<Vec<u8>> {
    let mut doc = load(bytes, None)?;
    reorder(&mut doc, idx)?;
    doc.prune_objects();
    save(&mut doc)
}

fn remove_dangling_outline_dests(_doc: &mut Document) {}

/// Page rendered as an image becomes a one-page PDF (for inserting images as pages).
pub fn image_page_pdf(img: &image::RgbaImage, fit_a4: bool) -> Vec<u8> {
    let mut d = pdfgen::Doc::new();
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let (pw, ph) = if fit_a4 {
        if iw > ih {
            (pdfgen::A4.1, pdfgen::A4.0)
        } else {
            pdfgen::A4
        }
    } else {
        (iw * 72.0 / 150.0, ih * 72.0 / 150.0)
    };
    let s = (pw / iw).min(ph / ih);
    let (w, h) = (iw * s, ih * s);
    let id = d.add_image_rgba(img);
    let mut p = pdfgen::Page::new(pw, ph);
    p.image(id, (pw - w) / 2.0, (ph - h) / 2.0, w, h);
    d.add_page(p);
    d.finish()
}

/// Effective resources dictionary of a page as a mutable reference, created when missing.
fn page_resources_mut(doc: &mut Document, page: ObjectId) -> R<&mut Dictionary> {
    let has_own = doc.get_dictionary(page).map(|d| d.has(b"Resources")).unwrap_or(false);
    if !has_own {
        let inh = inherited(doc, page, b"Resources").cloned().unwrap_or(Object::Dictionary(Dictionary::new()));
        let inh = match inh {
            Object::Reference(r) => doc.get_object(r).cloned().unwrap_or(Object::Dictionary(Dictionary::new())),
            o => o,
        };
        doc.get_dictionary_mut(page).map_err(e)?.set("Resources", inh);
    }
    let res = doc.get_dictionary(page).map_err(e)?.get(b"Resources").map_err(e)?.clone();
    match res {
        Object::Reference(r) => doc.get_dictionary_mut(r).map_err(e),
        _ => doc
            .get_dictionary_mut(page)
            .map_err(e)?
            .get_mut(b"Resources")
            .and_then(|o| o.as_dict_mut())
            .map_err(e),
    }
}

fn sub_dict_mut<'a>(doc: &'a mut Document, page: ObjectId, key: &[u8]) -> R<&'a mut Dictionary> {
    let res = page_resources_mut(doc, page)?;
    if !res.has(key) {
        res.set(key.to_vec(), Dictionary::new());
    }
    let sub = res.get(key).map_err(e)?.clone();
    match sub {
        Object::Reference(r) => doc.get_dictionary_mut(r).map_err(e),
        _ => page_resources_mut(doc, page)?.get_mut(key).and_then(|o| o.as_dict_mut()).map_err(e),
    }
}

/// Imports page 0 of a PDF (bytes) as a Form XObject in `doc`. Returns its id and BBox size.
pub fn import_as_form(doc: &mut Document, overlay: &[u8], matrix: [f32; 6]) -> R<ObjectId> {
    let mut ov = Document::load_mem(overlay).map_err(e)?;
    ov.renumber_objects_with(doc.max_id + 1);
    let ov_page = *ov.get_pages().values().next().ok_or("empty overlay")?;
    let content = ov.get_page_content(ov_page).map_err(e)?;
    let g = geom(&ov, ov_page);
    let res = inherited(&ov, ov_page, b"Resources").cloned().unwrap_or(Object::Dictionary(Dictionary::new()));
    let catalog = ov.trailer.get(b"Root").and_then(|r| r.as_reference()).ok();
    let ov_root = root_pages(&ov).ok();
    let contents_ids = ov.get_page_contents(ov_page);
    let info = ov.trailer.get(b"Info").and_then(|r| r.as_reference()).ok();
    for (id, obj) in std::mem::take(&mut ov.objects) {
        if Some(id) == catalog || Some(id) == ov_root || id == ov_page || contents_ids.contains(&id) || Some(id) == info {
            continue;
        }
        doc.objects.insert(id, obj);
    }
    doc.max_id = doc.max_id.max(ov.max_id);
    let form = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![Object::Real(g.x0), Object::Real(g.y0), Object::Real(g.x1), Object::Real(g.y1)],
            "Matrix" => matrix.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
            "Resources" => res,
        },
        content,
    );
    let mut form = form;
    let _ = form.compress();
    Ok(doc.add_object(form))
}

/// Wraps the existing page content in q/Q and appends drawing of a form XObject.
pub fn stamp(doc: &mut Document, page: ObjectId, overlay: &[u8], under: bool) -> R<()> {
    let g = geom(doc, page);
    let form = import_as_form(doc, overlay, g.overlay_matrix())?;
    let name = format!("K42ov{}", form.0);
    sub_dict_mut(doc, page, b"XObject")?.set(name.as_bytes().to_vec(), Object::Reference(form));
    let q = doc.add_object(Stream::new(dictionary! {}, b"q\n".to_vec()));
    let tail = doc.add_object(Stream::new(dictionary! {}, format!("Q\nq /{name} Do Q\n").into_bytes()));
    let mut contents: Vec<Object> = doc.get_page_contents(page).into_iter().map(Object::Reference).collect();
    if under {
        let pre = doc.add_object(Stream::new(dictionary! {}, format!("q /{name} Do Q\n").into_bytes()));
        contents.insert(0, Object::Reference(pre));
    } else {
        contents.insert(0, Object::Reference(q));
        contents.push(Object::Reference(tail));
    }
    doc.get_dictionary_mut(page).map_err(e)?.set("Contents", Object::Array(contents));
    Ok(())
}

/// Replaces a page's content with the page of another one-page PDF (redaction raster pages).
pub fn replace_page(doc: &mut Document, page: ObjectId, replacement: &[u8]) -> R<()> {
    let mut ov = Document::load_mem(replacement).map_err(e)?;
    ov.renumber_objects_with(doc.max_id + 1);
    let ov_page = *ov.get_pages().values().next().ok_or("empty page")?;
    let g = geom(&ov, ov_page);
    let content = ov.get_page_content(ov_page).map_err(e)?;
    let res = inherited(&ov, ov_page, b"Resources").cloned().unwrap_or(Object::Dictionary(Dictionary::new()));
    let catalog = ov.trailer.get(b"Root").and_then(|r| r.as_reference()).ok();
    let ov_root = root_pages(&ov).ok();
    let skip = ov.get_page_contents(ov_page);
    for (id, obj) in std::mem::take(&mut ov.objects) {
        if Some(id) == catalog || Some(id) == ov_root || id == ov_page || skip.contains(&id) {
            continue;
        }
        doc.objects.insert(id, obj);
    }
    doc.max_id = doc.max_id.max(ov.max_id);
    let mut st = Stream::new(dictionary! {}, content);
    let _ = st.compress();
    let cid = doc.add_object(st);
    let d = doc.get_dictionary_mut(page).map_err(e)?;
    d.set("Contents", Object::Reference(cid));
    d.set("Resources", res);
    d.set("MediaBox", vec![Object::Real(g.x0), Object::Real(g.y0), Object::Real(g.x1), Object::Real(g.y1)]);
    d.remove(b"CropBox");
    d.set("Rotate", 0);
    d.remove(b"Annots");
    Ok(())
}

pub fn set_crop(doc: &mut Document, page: ObjectId, user_box: Option<[f32; 4]>) -> R<()> {
    let d = doc.get_dictionary_mut(page).map_err(e)?;
    match user_box {
        Some(b) => d.set("CropBox", b.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>()),
        None => {
            d.remove(b"CropBox");
        }
    }
    Ok(())
}

/// Changes the paper size; `scale` fits the content, otherwise the canvas is extended/trimmed.
pub fn set_page_size(doc: &mut Document, page: ObjectId, w: f32, h: f32, scale: bool) -> R<()> {
    let g = geom(doc, page);
    let (ow, oh) = (g.w(), g.h());
    let (nw, nh) = if g.rot % 180 == 0 { (w, h) } else { (h, w) };
    if scale {
        let s = (nw / ow).min(nh / oh);
        let dx = (nw - ow * s) / 2.0 - g.x0 * s;
        let dy = (nh - oh * s) / 2.0 - g.y0 * s;
        let pre = doc.add_object(Stream::new(dictionary! {}, format!("q {s} 0 0 {s} {dx} {dy} cm\n").into_bytes()));
        let post = doc.add_object(Stream::new(dictionary! {}, b"\nQ\n".to_vec()));
        let mut contents: Vec<Object> = doc.get_page_contents(page).into_iter().map(Object::Reference).collect();
        contents.insert(0, Object::Reference(pre));
        contents.push(Object::Reference(post));
        let d = doc.get_dictionary_mut(page).map_err(e)?;
        d.set("Contents", Object::Array(contents));
        d.set("MediaBox", vec![0.into(), 0.into(), Object::Real(nw), Object::Real(nh)]);
        d.remove(b"CropBox");
        // Scale annotation rectangles too.
        if let Ok(annots) = doc.get_dictionary(page).and_then(|d| d.get(b"Annots")).cloned() {
            let list: Vec<ObjectId> = match deref(doc, &annots) {
                Object::Array(a) => a.iter().filter_map(|o| o.as_reference().ok()).collect(),
                _ => vec![],
            };
            for a in list {
                if let Ok(ad) = doc.get_dictionary_mut(a) {
                    if let Ok(Object::Array(r)) = ad.get(b"Rect").cloned() {
                        let v: Vec<Object> = r
                            .iter()
                            .enumerate()
                            .map(|(i, o)| Object::Real(num(o) * s + if i % 2 == 0 { dx } else { dy }))
                            .collect();
                        ad.set("Rect", v);
                    }
                }
            }
        }
    } else {
        let d = doc.get_dictionary_mut(page).map_err(e)?;
        let (cx, cy) = (g.x0, g.y1 - nh);
        d.set("MediaBox", vec![Object::Real(cx), Object::Real(cy), Object::Real(cx + nw), Object::Real(cy + nh)]);
        d.remove(b"CropBox");
    }
    Ok(())
}

// ───────────────────────── metadata ─────────────────────────

pub fn decode_pdf_string(o: &Object) -> String {
    match o {
        Object::String(b, _) => {
            if b.starts_with(&[0xFE, 0xFF]) {
                let u: Vec<u16> = b[2..].chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
                String::from_utf16_lossy(&u)
            } else if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
                String::from_utf8_lossy(&b[3..]).into_owned()
            } else {
                // PDFDocEncoding ≈ Latin-1 for printable characters.
                b.iter().map(|&c| c as char).collect()
            }
        }
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        _ => String::new(),
    }
}

pub fn text_string(s: &str) -> Object {
    if s.is_ascii() {
        Object::string_literal(s)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(b, lopdf::StringFormat::Hexadecimal)
    }
}

#[derive(Clone, Default, Debug)]
pub struct Info {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub creator: String,
    pub producer: String,
    pub created: String,
    pub modified: String,
}

fn info_dict(doc: &Document) -> Option<&Dictionary> {
    let o = doc.trailer.get(b"Info").ok()?;
    match o {
        Object::Reference(r) => doc.get_dictionary(*r).ok(),
        Object::Dictionary(d) => Some(d),
        _ => None,
    }
}

pub fn read_info(doc: &Document) -> Info {
    let Some(d) = info_dict(doc) else { return Info::default() };
    let g = |k: &[u8]| d.get(k).map(|o| decode_pdf_string(deref(doc, o))).unwrap_or_default();
    Info {
        title: g(b"Title"),
        author: g(b"Author"),
        subject: g(b"Subject"),
        keywords: g(b"Keywords"),
        creator: g(b"Creator"),
        producer: g(b"Producer"),
        created: pretty_date(&g(b"CreationDate")),
        modified: pretty_date(&g(b"ModDate")),
    }
}

fn pretty_date(s: &str) -> String {
    let s = s.trim_start_matches("D:");
    if s.len() >= 14 {
        format!("{}-{}-{} {}:{}:{}", &s[0..4], &s[4..6], &s[6..8], &s[8..10], &s[10..12], &s[12..14])
    } else {
        s.to_string()
    }
}

pub fn write_info(doc: &mut Document, info: &Info) {
    let mut d = info_dict(doc).cloned().unwrap_or_default();
    for (k, v) in [
        ("Title", &info.title),
        ("Author", &info.author),
        ("Subject", &info.subject),
        ("Keywords", &info.keywords),
    ] {
        if v.is_empty() {
            d.remove(k.as_bytes());
        } else {
            d.set(k, text_string(v));
        }
    }
    d.set("Producer", text_string("PDF42 for Windows (Suite42)"));
    d.set("ModDate", Object::string_literal(chrono::Local::now().format("D:%Y%m%d%H%M%S").to_string()));
    let id = doc.add_object(d);
    doc.trailer.set("Info", Object::Reference(id));
}

// ───────────────────────── outlines ─────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub title: String,
    pub page: usize,
    pub level: usize,
}

pub fn read_outline(doc: &Document) -> Vec<Bookmark> {
    let pages: Vec<ObjectId> = page_ids(doc);
    let mut out = Vec::new();
    let Ok(cat) = doc.catalog() else { return out };
    let Ok(first) = cat
        .get(b"Outlines")
        .and_then(|o| doc.dereference(o))
        .and_then(|(_, o)| o.as_dict())
        .and_then(|d| d.get(b"First"))
        .and_then(|f| f.as_reference())
    else {
        return out;
    };
    let named = named_dests(doc);
    fn walk(doc: &Document, id: ObjectId, level: usize, pages: &[ObjectId], named: &BTreeMap<String, Object>, out: &mut Vec<Bookmark>, guard: &mut usize) {
        let mut cur = Some(id);
        while let Some(i) = cur {
            *guard += 1;
            if *guard > 20000 {
                return;
            }
            let Ok(d) = doc.get_dictionary(i) else { return };
            let title = d.get(b"Title").map(|t| decode_pdf_string(deref(doc, t))).unwrap_or_default();
            let mut dest = d.get(b"Dest").ok().cloned();
            if dest.is_none() {
                if let Ok(a) = d.get(b"A").map(|a| deref(doc, a)).and_then(|a| a.as_dict()) {
                    dest = a.get(b"D").ok().cloned();
                }
            }
            let page = dest.map(|dd| dest_page(doc, &dd, pages, named)).unwrap_or(0);
            out.push(Bookmark { title, page, level });
            if let Ok(f) = d.get(b"First").and_then(|f| f.as_reference()) {
                walk(doc, f, level + 1, pages, named, out, guard);
            }
            cur = d.get(b"Next").and_then(|n| n.as_reference()).ok();
        }
    }
    let mut guard = 0;
    walk(doc, first, 0, &pages, &named, &mut out, &mut guard);
    out
}

fn named_dests(doc: &Document) -> BTreeMap<String, Object> {
    let mut map = BTreeMap::new();
    let Ok(cat) = doc.catalog() else { return map };
    if let Ok(names) = cat.get(b"Names").map(|o| deref(doc, o)).and_then(|o| o.as_dict()) {
        if let Ok(tree) = names.get(b"Dests").map(|o| deref(doc, o)) {
            name_tree(doc, tree, &mut map, 0);
        }
    }
    map
}

fn name_tree(doc: &Document, node: &Object, out: &mut BTreeMap<String, Object>, depth: usize) {
    if depth > 32 {
        return;
    }
    let Ok(d) = deref(doc, node).as_dict() else { return };
    if let Ok(arr) = d.get(b"Names").map(|o| deref(doc, o)).and_then(|o| o.as_array()) {
        for pair in arr.chunks(2) {
            if pair.len() == 2 {
                out.insert(decode_pdf_string(deref(doc, &pair[0])), pair[1].clone());
            }
        }
    }
    if let Ok(kids) = d.get(b"Kids").map(|o| deref(doc, o)).and_then(|o| o.as_array()) {
        for k in kids {
            name_tree(doc, k, out, depth + 1);
        }
    }
}

fn dest_page(doc: &Document, dest: &Object, pages: &[ObjectId], named: &BTreeMap<String, Object>) -> usize {
    let d = deref(doc, dest);
    match d {
        Object::Array(a) => a
            .first()
            .and_then(|o| o.as_reference().ok())
            .and_then(|r| pages.iter().position(|p| *p == r))
            .or_else(|| a.first().and_then(|o| o.as_i64().ok()).map(|i| i as usize))
            .unwrap_or(0),
        Object::String(..) | Object::Name(_) => {
            let key = decode_pdf_string(d);
            named
                .get(&key)
                .map(|o| {
                    let o = deref(doc, o);
                    match o {
                        Object::Dictionary(dd) => dd.get(b"D").map(|x| dest_page(doc, x, pages, &BTreeMap::new())).unwrap_or(0),
                        _ => dest_page(doc, o, pages, &BTreeMap::new()),
                    }
                })
                .unwrap_or(0)
        }
        Object::Dictionary(dd) => dd.get(b"D").map(|x| dest_page(doc, x, pages, named)).unwrap_or(0),
        _ => 0,
    }
}

/// Writes a new outline tree from a flat list with levels.
pub fn write_outline(doc: &mut Document, items: &[Bookmark]) -> R<()> {
    let pages = page_ids(doc);
    let root_id = doc.new_object_id();
    let ids: Vec<ObjectId> = items.iter().map(|_| doc.new_object_id()).collect();
    let n = items.len();
    let parent_of = |i: usize| -> Option<usize> { (0..i).rev().find(|&j| items[j].level < items[i].level) };
    let mut dicts: Vec<Dictionary> = Vec::with_capacity(n);
    for (i, b) in items.iter().enumerate() {
        let parent = parent_of(i).map(|p| ids[p]).unwrap_or(root_id);
        let mut d = Dictionary::new();
        d.set("Title", text_string(&b.title));
        d.set("Parent", Object::Reference(parent));
        let page = pages.get(b.page.min(pages.len().saturating_sub(1))).cloned();
        if let Some(p) = page {
            d.set("Dest", vec![Object::Reference(p), Object::Name(b"Fit".to_vec())]);
        }
        let prev = (0..i).rev().take_while(|&j| items[j].level >= b.level).find(|&j| items[j].level == b.level);
        let next = (i + 1..n).take_while(|&j| items[j].level >= b.level).find(|&j| items[j].level == b.level);
        if let Some(p) = prev {
            d.set("Prev", Object::Reference(ids[p]));
        }
        if let Some(nx) = next {
            d.set("Next", Object::Reference(ids[nx]));
        }
        let children: Vec<usize> = (i + 1..n).take_while(|&j| items[j].level > b.level).filter(|&j| items[j].level == b.level + 1).collect();
        if let (Some(f), Some(l)) = (children.first(), children.last()) {
            d.set("First", Object::Reference(ids[*f]));
            d.set("Last", Object::Reference(ids[*l]));
            d.set("Count", children.len() as i64);
        }
        dicts.push(d);
    }
    for (i, d) in dicts.into_iter().enumerate() {
        doc.objects.insert(ids[i], Object::Dictionary(d));
    }
    let top: Vec<usize> = (0..n).filter(|&i| items[i].level == 0).collect();
    let mut root = dictionary! { "Type" => "Outlines", "Count" => top.len() as i64 };
    if let (Some(f), Some(l)) = (top.first(), top.last()) {
        root.set("First", Object::Reference(ids[*f]));
        root.set("Last", Object::Reference(ids[*l]));
    }
    doc.objects.insert(root_id, Object::Dictionary(root));
    let cat = doc.catalog_mut().map_err(e)?;
    if n == 0 {
        cat.remove(b"Outlines");
    } else {
        cat.set("Outlines", Object::Reference(root_id));
        cat.set("PageMode", Object::Name(b"UseOutlines".to_vec()));
    }
    Ok(())
}

// ───────────────────────── attachments ─────────────────────────

#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    pub size: usize,
    pub data: Vec<u8>,
    pub description: String,
}

pub fn read_attachments(doc: &Document) -> Vec<Attachment> {
    let mut map = BTreeMap::new();
    if let Ok(cat) = doc.catalog() {
        if let Ok(names) = cat.get(b"Names").map(|o| deref(doc, o)).and_then(|o| o.as_dict()) {
            if let Ok(tree) = names.get(b"EmbeddedFiles").map(|o| deref(doc, o)) {
                name_tree(doc, tree, &mut map, 0);
            }
        }
    }
    let mut out = Vec::new();
    for (key, spec) in map {
        let Ok(fs) = deref(doc, &spec).as_dict() else { continue };
        let name = fs
            .get(b"UF")
            .or_else(|_| fs.get(b"F"))
            .map(|o| decode_pdf_string(deref(doc, o)))
            .unwrap_or(key);
        let description = fs.get(b"Desc").map(|o| decode_pdf_string(deref(doc, o))).unwrap_or_default();
        let data = fs
            .get(b"EF")
            .map(|o| deref(doc, o))
            .and_then(|o| o.as_dict())
            .and_then(|ef| ef.get(b"UF").or_else(|_| ef.get(b"F")))
            .map(|o| deref(doc, o))
            .and_then(|o| o.as_stream())
            .map(|s| s.decompressed_content().unwrap_or_else(|_| s.content.clone()))
            .unwrap_or_default();
        out.push(Attachment { name, size: data.len(), data, description });
    }
    out
}

/// Rewrites the EmbeddedFiles name tree with the given attachments.
pub fn write_attachments(doc: &mut Document, list: &[Attachment]) -> R<()> {
    let mut arr = Vec::new();
    let mut sorted = list.to_vec();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for a in &sorted {
        let mut st = Stream::new(dictionary! { "Type" => "EmbeddedFile", "Params" => dictionary! { "Size" => a.data.len() as i64 } }, a.data.clone());
        let _ = st.compress();
        let ef = doc.add_object(st);
        let spec = doc.add_object(dictionary! {
            "Type" => "Filespec",
            "F" => text_string(&a.name),
            "UF" => text_string(&a.name),
            "Desc" => text_string(&a.description),
            "EF" => dictionary! { "F" => ef, "UF" => ef },
        });
        arr.push(text_string(&a.name));
        arr.push(Object::Reference(spec));
    }
    let names_obj = doc.catalog().ok().and_then(|c| c.get(b"Names").ok().cloned());
    let mut names = match names_obj {
        Some(Object::Reference(r)) => doc.get_dictionary(r).cloned().unwrap_or_default(),
        Some(Object::Dictionary(d)) => d,
        _ => Dictionary::new(),
    };
    if arr.is_empty() {
        names.remove(b"EmbeddedFiles");
    } else {
        names.set("EmbeddedFiles", dictionary! { "Names" => arr });
    }
    let id = doc.add_object(names);
    doc.catalog_mut().map_err(e)?.set("Names", Object::Reference(id));
    Ok(())
}

// ───────────────────────── forms ─────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    Text,
    Check,
    Radio,
    Choice(Vec<String>),
    Button,
    Signature,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub id: ObjectId,
    pub name: String,
    pub kind: FieldKind,
    pub value: String,
    pub on_state: String,
    pub widgets: Vec<(usize, [f32; 4])>,
}

pub fn read_fields(doc: &Document) -> Vec<Field> {
    let mut out = Vec::new();
    let pages = page_ids(doc);
    let Ok(cat) = doc.catalog() else { return out };
    let Ok(form) = cat.get(b"AcroForm").map(|o| deref(doc, o)).and_then(|o| o.as_dict()) else { return out };
    let Ok(fields) = form.get(b"Fields").map(|o| deref(doc, o)).and_then(|o| o.as_array()) else { return out };
    fn walk(doc: &Document, id: ObjectId, prefix: &str, inherited_ft: Option<Vec<u8>>, pages: &[ObjectId], out: &mut Vec<Field>, depth: usize) {
        if depth > 32 {
            return;
        }
        let Ok(d) = doc.get_dictionary(id) else { return };
        let part = d.get(b"T").map(|t| decode_pdf_string(deref(doc, t))).unwrap_or_default();
        let name = if prefix.is_empty() { part.clone() } else if part.is_empty() { prefix.to_string() } else { format!("{prefix}.{part}") };
        let ft = d.get(b"FT").ok().and_then(|o| o.as_name().ok()).map(|n| n.to_vec()).or(inherited_ft);
        let kids: Vec<ObjectId> = d
            .get(b"Kids")
            .map(|o| deref(doc, o))
            .and_then(|o| o.as_array())
            .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
            .unwrap_or_default();
        let kid_fields: Vec<ObjectId> = kids.iter().cloned().filter(|k| doc.get_dictionary(*k).map(|kd| kd.has(b"T")).unwrap_or(false)).collect();
        if !kid_fields.is_empty() {
            for k in kid_fields {
                walk(doc, k, &name, ft.clone(), pages, out, depth + 1);
            }
            return;
        }
        let flags = d.get(b"Ff").and_then(|o| o.as_i64()).unwrap_or(0);
        let mut on_state = String::new();
        let widgets: Vec<ObjectId> = if kids.is_empty() { vec![id] } else { kids };
        let mut wrects = Vec::new();
        for w in &widgets {
            if let Ok(wd) = doc.get_dictionary(*w) {
                if let Ok(n) = wd.get(b"AP").map(|o| deref(doc, o)).and_then(|o| o.as_dict()).and_then(|ap| ap.get(b"N")).map(|o| deref(doc, o)).and_then(|o| o.as_dict()) {
                    for (k, _) in n.iter() {
                        if k != b"Off" {
                            on_state = String::from_utf8_lossy(k).into_owned();
                        }
                    }
                }
                let page = wd.get(b"P").and_then(|p| p.as_reference()).ok().and_then(|p| pages.iter().position(|x| *x == p)).unwrap_or(usize::MAX);
                if let Some(r) = wd.get(b"Rect").ok().and_then(|r| rect_of(doc, r)) {
                    wrects.push((page, r));
                }
            }
        }
        let kind = match ft.as_deref() {
            Some(b"Tx") => FieldKind::Text,
            Some(b"Btn") if flags & (1 << 16) != 0 => FieldKind::Button,
            Some(b"Btn") if flags & (1 << 15) != 0 => FieldKind::Radio,
            Some(b"Btn") => FieldKind::Check,
            Some(b"Ch") => {
                let opts = d
                    .get(b"Opt")
                    .map(|o| deref(doc, o))
                    .and_then(|o| o.as_array())
                    .map(|a| {
                        a.iter()
                            .map(|o| match deref(doc, o) {
                                Object::Array(p) => p.get(1).or(p.first()).map(|x| decode_pdf_string(deref(doc, x))).unwrap_or_default(),
                                x => decode_pdf_string(x),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                FieldKind::Choice(opts)
            }
            Some(b"Sig") => FieldKind::Signature,
            _ => FieldKind::Text,
        };
        let value = d.get(b"V").map(|v| decode_pdf_string(deref(doc, v))).unwrap_or_default();
        out.push(Field { id, name, kind, value, on_state, widgets: wrects });
    }
    for f in fields {
        if let Ok(id) = f.as_reference() {
            walk(doc, id, "", None, &pages, &mut out, 0);
        }
    }
    out
}

/// Writes field values; text appearances are generated with an embedded font so they show
/// correctly everywhere (including Cyrillic), checkboxes switch /AS.
pub fn write_field_values(doc: &mut Document, fields: &[Field]) -> R<()> {
    for f in fields {
        let widgets: Vec<ObjectId> = {
            let d = doc.get_dictionary(f.id).map_err(e)?;
            let kids: Vec<ObjectId> = d
                .get(b"Kids")
                .ok()
                .and_then(|o| o.as_array().ok())
                .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
                .unwrap_or_default();
            if kids.is_empty() {
                vec![f.id]
            } else {
                kids
            }
        };
        match &f.kind {
            FieldKind::Check | FieldKind::Radio => {
                let on = !f.value.is_empty() && f.value != "Off";
                let state = if on { if f.on_state.is_empty() { "Yes".to_string() } else { f.on_state.clone() } } else { "Off".to_string() };
                doc.get_dictionary_mut(f.id).map_err(e)?.set("V", Object::Name(state.as_bytes().to_vec()));
                for w in widgets {
                    if let Ok(wd) = doc.get_dictionary_mut(w) {
                        wd.set("AS", Object::Name(state.as_bytes().to_vec()));
                    }
                }
            }
            FieldKind::Text | FieldKind::Choice(_) => {
                doc.get_dictionary_mut(f.id).map_err(e)?.set("V", text_string(&f.value));
                for w in widgets {
                    let rect = doc.get_dictionary(w).ok().and_then(|wd| wd.get(b"Rect").ok().and_then(|r| rect_of(doc, r)));
                    let Some(r) = rect else { continue };
                    let (rw, rh) = (r[2] - r[0], r[3] - r[1]);
                    if rw <= 0.0 || rh <= 0.0 {
                        continue;
                    }
                    let mut pd = pdfgen::Doc::new();
                    let font = pd.font("Arial", false, false);
                    let mut page = pdfgen::Page::new(rw, rh);
                    let size = (rh * 0.62).clamp(6.0, 12.0);
                    let multiline = rh > size * 2.6;
                    if multiline {
                        let lines = pd.wrap(font, size, &f.value, rw - 4.0);
                        for (i, l) in lines.iter().enumerate() {
                            pd.text(&mut page, pdfgen::TextStyle::new(font, size), 2.0, 2.0 + size * (i as f32 + 1.0), l);
                        }
                    } else {
                        pd.text(&mut page, pdfgen::TextStyle::new(font, size), 2.0, rh / 2.0 + size * 0.35, &f.value);
                    }
                    pd.add_page(page);
                    let bytes = pd.finish();
                    let form = import_as_form(doc, &bytes, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0])?;
                    if let Ok(Object::Stream(st)) = doc.get_object_mut(form) {
                        st.dict.set("BBox", vec![0.into(), 0.into(), Object::Real(rw), Object::Real(rh)]);
                    }
                    if let Ok(wd) = doc.get_dictionary_mut(w) {
                        wd.set("AP", dictionary! { "N" => form });
                    }
                }
            }
            _ => {}
        }
    }
    if let Ok(cat) = doc.catalog() {
        if let Ok(af) = cat.get(b"AcroForm").cloned() {
            match af {
                Object::Reference(r) => {
                    if let Ok(d) = doc.get_dictionary_mut(r) {
                        d.set("NeedAppearances", false);
                    }
                }
                Object::Dictionary(mut d) => {
                    d.set("NeedAppearances", false);
                    doc.catalog_mut().map_err(e)?.set("AcroForm", d);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Adds a new form field widget on a page (user-space rect).
pub fn add_field(doc: &mut Document, page: ObjectId, name: &str, kind: &FieldKind, rect: [f32; 4]) -> R<()> {
    let r: Vec<Object> = rect.iter().map(|v| Object::Real(*v)).collect();
    let mut d = dictionary! {
        "Type" => "Annot",
        "Subtype" => "Widget",
        "T" => text_string(name),
        "Rect" => r,
        "P" => page,
        "F" => 4,
        "MK" => dictionary! { "BC" => vec![Object::Real(0.4), Object::Real(0.4), Object::Real(0.4)], "BG" => vec![Object::Real(1.0), Object::Real(1.0), Object::Real(1.0)] },
    };
    match kind {
        FieldKind::Check => {
            let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
            let on = doc.add_object(Stream::new(
                dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)] },
                format!("q 0.4 G 1 w 0.5 0.5 {} {} re S 0 g 1.5 w {} {} m {} {} l {} {} l S Q", w - 1.0, h - 1.0, w * 0.2, h * 0.5, w * 0.42, h * 0.25, w * 0.8, h * 0.78).into_bytes(),
            ));
            let off = doc.add_object(Stream::new(
                dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)] },
                format!("q 0.4 G 1 w 0.5 0.5 {} {} re S Q", w - 1.0, h - 1.0).into_bytes(),
            ));
            d.set("FT", Object::Name(b"Btn".to_vec()));
            d.set("V", Object::Name(b"Off".to_vec()));
            d.set("AS", Object::Name(b"Off".to_vec()));
            d.set("AP", dictionary! { "N" => dictionary! { "Yes" => on, "Off" => off } });
        }
        FieldKind::Choice(opts) => {
            d.set("FT", Object::Name(b"Ch".to_vec()));
            d.set("Ff", 1 << 17);
            d.set("Opt", opts.iter().map(|o| text_string(o)).collect::<Vec<_>>());
            d.set("DA", Object::string_literal("/Helv 10 Tf 0 g"));
        }
        _ => {
            d.set("FT", Object::Name(b"Tx".to_vec()));
            d.set("DA", Object::string_literal("/Helv 10 Tf 0 g"));
        }
    }
    let wid = doc.add_object(d);
    add_annot_ref(doc, page, wid)?;
    // Register in AcroForm.
    let helv = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
    let af_obj = doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok().cloned());
    let af_id = match af_obj {
        Some(Object::Reference(r)) => r,
        Some(Object::Dictionary(dd)) => doc.add_object(dd),
        _ => doc.add_object(dictionary! { "Fields" => Vec::<Object>::new(), "DA" => Object::string_literal("/Helv 10 Tf 0 g"), "DR" => dictionary! { "Font" => dictionary! { "Helv" => helv } } }),
    };
    doc.catalog_mut().map_err(e)?.set("AcroForm", Object::Reference(af_id));
    let af = doc.get_dictionary_mut(af_id).map_err(e)?;
    if !af.has(b"DR") {
        af.set("DR", dictionary! { "Font" => dictionary! { "Helv" => helv } });
    }
    match af.get_mut(b"Fields") {
        Ok(Object::Array(a)) => a.push(Object::Reference(wid)),
        _ => af.set("Fields", vec![Object::Reference(wid)]),
    }
    Ok(())
}

pub fn add_annot_ref(doc: &mut Document, page: ObjectId, annot: ObjectId) -> R<()> {
    let existing = doc.get_dictionary(page).map_err(e)?.get(b"Annots").ok().cloned();
    match existing {
        Some(Object::Reference(r)) => {
            if let Ok(Object::Array(a)) = doc.get_object_mut(r) {
                a.push(Object::Reference(annot));
            }
        }
        Some(Object::Array(mut a)) => {
            a.push(Object::Reference(annot));
            doc.get_dictionary_mut(page).map_err(e)?.set("Annots", a);
        }
        _ => doc.get_dictionary_mut(page).map_err(e)?.set("Annots", vec![Object::Reference(annot)]),
    }
    Ok(())
}

/// Number of existing annotations per page (for the status bar and "remove annotations").
pub fn remove_annotations(doc: &mut Document, page: ObjectId, keep_widgets: bool) {
    let annots = doc.get_dictionary(page).ok().and_then(|d| d.get(b"Annots").ok().cloned());
    let list: Vec<Object> = match annots.as_ref().map(|a| deref(doc, a)) {
        Some(Object::Array(a)) => a.clone(),
        _ => return,
    };
    let kept: Vec<Object> = list
        .into_iter()
        .filter(|o| {
            keep_widgets
                && o.as_reference()
                    .ok()
                    .and_then(|r| doc.get_dictionary(r).ok())
                    .map(|d| d.get(b"Subtype").and_then(|s| s.as_name()).map(|n| n == b"Widget").unwrap_or(false))
                    .unwrap_or(false)
        })
        .collect();
    if let Ok(d) = doc.get_dictionary_mut(page) {
        if kept.is_empty() {
            d.remove(b"Annots");
        } else {
            d.set("Annots", kept);
        }
    }
}

// ───────────────────────── text ─────────────────────────

/// Page text in reading order (top to bottom), including text inside form XObjects such as edits.
pub fn page_text(doc: &Document, page_number: u32) -> String {
    let Some(&pid) = doc.get_pages().get(&page_number) else { return String::new() };
    let mut lines = crate::textlayer::page_lines(doc, pid);
    if lines.is_empty() {
        return doc.extract_text(&[page_number]).unwrap_or_default();
    }
    lines.sort_by(|a, b| ((a.baseline / 2.0).round(), a.rect[0]).partial_cmp(&((b.baseline / 2.0).round(), b.rect[0])).unwrap_or(std::cmp::Ordering::Equal));
    lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
}

pub fn all_text(doc: &Document) -> Vec<String> {
    let n = doc.get_pages().len() as u32;
    (1..=n).map(|p| page_text(doc, p)).collect()
}

// ───────────────────────── compression ─────────────────────────

/// Re-encodes pictures as JPEG with the longest side capped. Returns (images changed, bytes saved).
pub fn compress_images(doc: &mut Document, max_side: u32, quality: u8) -> (usize, i64) {
    let ids: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, o)| {
            let st = o.as_stream().ok()?;
            let is_img = st.dict.get(b"Subtype").and_then(|s| s.as_name()).map(|n| n == b"Image").unwrap_or(false);
            (is_img && !st.dict.has(b"ImageMask") && !st.dict.has(b"Mask")).then_some(*id)
        })
        .collect();
    let mut changed = 0;
    let mut saved: i64 = 0;
    for id in ids {
        let Ok(Object::Stream(st)) = doc.get_object(id).cloned() else { continue };
        let filters = st.filters().unwrap_or_default();
        let bpc = st.dict.get(b"BitsPerComponent").and_then(|o| o.as_i64()).unwrap_or(8);
        let w = st.dict.get(b"Width").and_then(|o| o.as_i64()).unwrap_or(0) as u32;
        let h = st.dict.get(b"Height").and_then(|o| o.as_i64()).unwrap_or(0) as u32;
        if w == 0 || h == 0 || bpc != 8 {
            continue;
        }
        let cs = st.dict.get(b"ColorSpace").ok().map(|o| deref(doc, o).clone());
        let cs_name = cs.as_ref().and_then(|c| c.as_name().ok()).map(|n| n.to_vec());
        let img: Option<image::DynamicImage> = if filters.iter().any(|f| f == "DCTDecode") && filters.len() == 1 {
            image::load_from_memory_with_format(&st.content, image::ImageFormat::Jpeg).ok()
        } else if filters.iter().all(|f| f == "FlateDecode") && !filters.is_empty() {
            let raw = st.decompressed_content().ok();
            match (raw, cs_name.as_deref()) {
                (Some(raw), Some(b"DeviceRGB")) if raw.len() >= (w * h * 3) as usize && !st.dict.has(b"DecodeParms") => {
                    image::RgbImage::from_raw(w, h, raw[..(w * h * 3) as usize].to_vec()).map(image::DynamicImage::ImageRgb8)
                }
                (Some(raw), Some(b"DeviceGray")) if raw.len() >= (w * h) as usize && !st.dict.has(b"DecodeParms") => {
                    image::GrayImage::from_raw(w, h, raw[..(w * h) as usize].to_vec()).map(image::DynamicImage::ImageLuma8)
                }
                _ => None,
            }
        } else {
            None
        };
        let Some(img) = img else { continue };
        let gray = img.color().channel_count() == 1;
        let img = if w.max(h) > max_side {
            img.resize(max_side, max_side, image::imageops::FilterType::Lanczos3)
        } else {
            img
        };
        let mut out = Vec::new();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
        let ok = if gray { img.to_luma8().write_with_encoder(enc).is_ok() } else { img.to_rgb8().write_with_encoder(enc).is_ok() };
        if !ok || out.len() >= st.content.len() {
            continue;
        }
        saved += st.content.len() as i64 - out.len() as i64;
        let mut dict = st.dict.clone();
        dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        dict.remove(b"DecodeParms");
        dict.set("Width", img.width() as i64);
        dict.set("Height", img.height() as i64);
        dict.set("BitsPerComponent", 8);
        dict.set("ColorSpace", Object::Name(if gray { b"DeviceGray".to_vec() } else { b"DeviceRGB".to_vec() }));
        dict.remove(b"Decode");
        let mut ns = Stream::new(dict, out);
        ns.allows_compression = false;
        doc.objects.insert(id, Object::Stream(ns));
        changed += 1;
    }
    (changed, saved)
}

/// Splits page ranges like "1-3, 5, 7-" (1-based) into 0-based index lists.
pub fn parse_ranges(s: &str, n: usize) -> R<Vec<Vec<usize>>> {
    let mut groups = Vec::new();
    for part in s.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (part, part),
        };
        let a: usize = if a.is_empty() { 1 } else { a.parse().map_err(|_| format!("{}: {part}", kit42::t("Invalid range", "Неверный диапазон")))? };
        let b: usize = if b.is_empty() { n } else { b.parse().map_err(|_| format!("{}: {part}", kit42::t("Invalid range", "Неверный диапазон")))? };
        if a == 0 || b == 0 || a > n || b > n || a > b {
            return Err(format!("{}: {part}", kit42::t("Range outside the document", "Диапазон вне документа")));
        }
        groups.push((a - 1..b).collect());
    }
    if groups.is_empty() {
        return Err(kit42::t("No pages given", "Страницы не указаны").into());
    }
    Ok(groups)
}

/// Page set from "all", "odd", "even" or ranges.
pub fn page_set(s: &str, n: usize) -> R<Vec<usize>> {
    let t = s.trim().to_lowercase();
    if t.is_empty() || t == "all" || t == "все" {
        return Ok((0..n).collect());
    }
    if t == "odd" || t == "нечётные" || t == "нечетные" {
        return Ok((0..n).step_by(2).collect());
    }
    if t == "even" || t == "чётные" || t == "четные" {
        return Ok((1..n).step_by(2).collect());
    }
    Ok(parse_ranges(s, n)?.into_iter().flatten().collect())
}
