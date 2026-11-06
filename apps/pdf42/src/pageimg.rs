//! Images already painted on a page, edited like objects in a drawing program: move and resize
//! (an extra `cm` around the `Do`), delete, replace with another picture, and save to a file.

use crate::pdfops;
use crate::textlayer::{self, inverse, mul, PageImage, M};
use lopdf::content::Operation;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

/// Image streams are not decompressed by lopdf; inflate Flate data here.
fn inflate(st: &Stream, flate: bool) -> Option<Vec<u8>> {
    if !flate {
        return Some(st.content.clone());
    }
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(&st.content[..]).read_to_end(&mut out).ok()?;
    Some(out)
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn find(doc: &Document, page: ObjectId, op: usize) -> Result<PageImage, String> {
    textlayer::page_images(doc, page).into_iter().find(|i| i.op == Some(op)).ok_or_else(|| "The image was not found on the page.".to_string())
}

/// Replaces the `Do` at `op` with `q m cm /name Do Q`, or removes it when `with` is None.
fn rewrite_do(doc: &mut Document, page: ObjectId, op: usize, with: Option<(M, Vec<u8>)>) -> Result<(), String> {
    let mut ops = textlayer::page_ops(doc, page).ok_or("The page content could not be read.")?;
    if ops.get(op).map(|o| o.operator.as_str()) != Some("Do") {
        return Err("The page content changed; select the image again.".into());
    }
    let new: Vec<Operation> = match with {
        None => vec![],
        Some((m, name)) => vec![
            Operation::new("q", vec![]),
            Operation::new("cm", m.iter().map(|v| Object::Real(*v)).collect()),
            Operation::new("Do", vec![Object::Name(name)]),
            Operation::new("Q", vec![]),
        ],
    };
    ops.splice(op..=op, new);
    textlayer::write_content(doc, page, ops)
}

/// Moves and/or resizes the image drawn by `op` so that its box `from` becomes `to`
/// (displayed page points). Rotation and flips of the original placement are kept.
pub fn transform(doc: &mut Document, page: ObjectId, op: usize, from: [f32; 4], to: [f32; 4]) -> Result<(), String> {
    let img = find(doc, page, op)?;
    let g = pdfops::geom(doc, page);
    let (a, b) = (g.rect_to_user(from), g.rect_to_user(to));
    let sx = (b[2] - b[0]) / (a[2] - a[0]).max(1e-3);
    let sy = (b[3] - b[1]) / (a[3] - a[1]).max(1e-3);
    // Affine map of the old user-space box onto the new one.
    let aff: M = [sx, 0.0, 0.0, sy, b[0] - a[0] * sx, b[1] - a[1] * sy];
    let inv = inverse(&img.ctm).ok_or("The image has no area.")?;
    let m = mul(&mul(&img.ctm, &aff), &inv);
    rewrite_do(doc, page, op, Some((m, img.name)))
}

/// Removes the image from the page.
pub fn delete(doc: &mut Document, page: ObjectId, op: usize) -> Result<(), String> {
    find(doc, page, op)?;
    rewrite_do(doc, page, op, None)
}

/// Number of colour components of a JPEG (from its SOF marker).
fn jpeg_components(b: &[u8]) -> Option<u8> {
    let mut i = 2;
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            return b.get(i + 9).copied();
        }
        i += 2 + len;
    }
    None
}

/// Adds a picture file as an Image XObject. Returns its id and pixel size.
pub fn add_image(doc: &mut Document, bytes: &[u8]) -> Result<(ObjectId, u32, u32), String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("{}: {e}", kit42::t("The picture could not be read", "Не удалось прочитать рисунок")))?;
    let (w, h) = (img.width(), img.height());
    let rgba = img.to_rgba8();
    let alpha = rgba.pixels().any(|p| p[3] < 255);
    let comps = if bytes.starts_with(&[0xFF, 0xD8]) { jpeg_components(bytes) } else { None };
    let stream = match comps {
        // JPEG (grey or RGB) is embedded as is, without recompression.
        Some(c @ (1 | 3)) if !alpha => Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
                "ColorSpace" => if c == 1 { "DeviceGray" } else { "DeviceRGB" }, "BitsPerComponent" => 8, "Filter" => "DCTDecode",
            },
            bytes.to_vec(),
        ),
        _ => {
            let rgb: Vec<u8> = rgba.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
            let mut d = dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
                "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "FlateDecode",
            };
            if alpha {
                let a: Vec<u8> = rgba.pixels().map(|p| p[3]).collect();
                let mut sm = Stream::new(
                    dictionary! {
                        "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
                        "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8, "Filter" => "FlateDecode",
                    },
                    kit42::pdfgen::deflate(&a),
                );
                sm.allows_compression = false;
                let sid = doc.add_object(sm);
                d.set("SMask", Object::Reference(sid));
            }
            Stream::new(d, kit42::pdfgen::deflate(&rgb))
        }
    };
    let mut stream = stream;
    stream.allows_compression = false;
    Ok((doc.add_object(stream), w, h))
}

/// Runs `f` on the page's own XObject resource dictionary, creating it when needed.
fn with_xobjects<R>(doc: &mut Document, page: ObjectId, f: impl FnOnce(&mut Dictionary) -> R) -> Result<R, String> {
    let res_ref = match doc.get_dictionary(page).map_err(err)?.get(b"Resources") {
        Ok(Object::Reference(r)) => Some(*r),
        Ok(Object::Dictionary(_)) => None,
        _ => {
            // Inherited resources become the page's own copy.
            let r = textlayer::page_resources(doc, page);
            doc.get_dictionary_mut(page).map_err(err)?.set("Resources", Object::Dictionary(r));
            None
        }
    };
    let xo_ref = {
        let res = match res_ref {
            Some(r) => doc.get_dictionary(r).map_err(err)?,
            None => doc.get_dictionary(page).map_err(err)?.get(b"Resources").and_then(|o| o.as_dict()).map_err(err)?,
        };
        match res.get(b"XObject") {
            Ok(Object::Reference(x)) => Some(*x),
            _ => None,
        }
    };
    if let Some(x) = xo_ref {
        return Ok(f(doc.get_dictionary_mut(x).map_err(err)?));
    }
    let res = match res_ref {
        Some(r) => doc.get_dictionary_mut(r).map_err(err)?,
        None => doc.get_dictionary_mut(page).map_err(err)?.get_mut(b"Resources").and_then(|o| o.as_dict_mut()).map_err(err)?,
    };
    if !matches!(res.get(b"XObject"), Ok(Object::Dictionary(_))) {
        res.set("XObject", Dictionary::new());
    }
    Ok(f(res.get_mut(b"XObject").and_then(|o| o.as_dict_mut()).map_err(err)?))
}

/// Replaces the image with a picture file, fitted into the old box with its own proportions.
pub fn replace(doc: &mut Document, page: ObjectId, op: usize, bytes: &[u8]) -> Result<(), String> {
    let img = find(doc, page, op)?;
    let (id, w, h) = add_image(doc, bytes)?;
    let name = with_xobjects(doc, page, |xo| {
        let mut n = 1;
        while xo.has(format!("K42Im{n}").as_bytes()) {
            n += 1;
        }
        let name = format!("K42Im{n}").into_bytes();
        xo.set(name.clone(), Object::Reference(id));
        name
    })?;
    // Fit, centred, into the old box; the new picture is drawn upright on screen.
    let r = img.rect;
    let (ow, oh) = (r[2] - r[0], r[3] - r[1]);
    let ratio = h as f32 / w.max(1) as f32;
    let (nw, nh) = if oh / ow.max(1e-3) > ratio { (ow, ow * ratio) } else { (oh / ratio, oh) };
    let (x0, y0) = (r[0] + (ow - nw) / 2.0, r[1] + (oh - nh) / 2.0);
    let g = pdfops::geom(doc, page);
    let p00 = g.to_user(x0, y0 + nh);
    let p10 = g.to_user(x0 + nw, y0 + nh);
    let p01 = g.to_user(x0, y0);
    let want: M = [p10.0 - p00.0, p10.1 - p00.1, p01.0 - p00.0, p01.1 - p00.1, p00.0, p00.1];
    let inv = inverse(&img.ctm).ok_or("The image has no area.")?;
    rewrite_do(doc, page, op, Some((mul(&want, &inv), name)))
}

/// The image's own data as a file: JPEG as stored, other images as PNG.
pub fn extract(doc: &Document, img: &PageImage) -> Option<(Vec<u8>, &'static str)> {
    let s = doc.get_object(img.id?).ok()?.as_stream().ok()?;
    let filters: Vec<Vec<u8>> = match s.dict.get(b"Filter") {
        Ok(Object::Name(n)) => vec![n.clone()],
        Ok(Object::Array(a)) => a.iter().filter_map(|o| o.as_name().ok().map(|n| n.to_vec())).collect(),
        _ => vec![],
    };
    if filters.len() == 1 && filters[0] == b"DCTDecode" {
        return Some((s.content.clone(), "jpg"));
    }
    if filters.iter().any(|f| f != b"FlateDecode") || s.dict.has(b"DecodeParms") {
        return None;
    }
    let (w, h) = img.px;
    let bpc = s.dict.get(b"BitsPerComponent").ok().and_then(|o| o.as_i64().ok()).unwrap_or(8);
    if bpc != 8 || w == 0 || h == 0 {
        return None;
    }
    let data = inflate(s, !filters.is_empty())?;
    let resolve = |o: &Object| -> Object {
        match o {
            Object::Reference(id) => doc.get_object(*id).cloned().unwrap_or(Object::Null),
            o => o.clone(),
        }
    };
    let cs = s.dict.get(b"ColorSpace").map(resolve).unwrap_or(Object::Name(b"DeviceGray".to_vec()));
    let comps_of = |cs: &Object| -> Option<usize> {
        match cs {
            Object::Name(n) if n == b"DeviceRGB" || n == b"CalRGB" => Some(3),
            Object::Name(n) if n == b"DeviceGray" || n == b"CalGray" => Some(1),
            Object::Array(a) if a.first().and_then(|o| o.as_name().ok()) == Some(b"ICCBased") => {
                let st = resolve(a.get(1)?);
                st.as_stream().ok()?.dict.get(b"N").ok()?.as_i64().ok().map(|n| n as usize).filter(|n| *n == 1 || *n == 3)
            }
            _ => None,
        }
    };
    let n = (w * h) as usize;
    let mut rgba = image::RgbaImage::new(w, h);
    if let Some(c) = comps_of(&cs) {
        if data.len() < n * c {
            return None;
        }
        for (i, p) in rgba.pixels_mut().enumerate() {
            *p = if c == 3 { image::Rgba([data[i * 3], data[i * 3 + 1], data[i * 3 + 2], 255]) } else { image::Rgba([data[i], data[i], data[i], 255]) };
        }
    } else if let Object::Array(a) = &cs {
        // [/Indexed base hival lookup] with an RGB or grey base.
        if a.first().and_then(|o| o.as_name().ok()) != Some(b"Indexed") || data.len() < n {
            return None;
        }
        let base = comps_of(&resolve(a.get(1)?))?;
        let lookup = match resolve(a.get(3)?) {
            Object::String(b, _) => b,
            Object::Stream(st) => st.decompressed_content().unwrap_or_else(|_| st.content.clone()),
            _ => return None,
        };
        for (i, p) in rgba.pixels_mut().enumerate() {
            let k = data[i] as usize * base;
            let px = |j: usize| lookup.get(k + j).copied().unwrap_or(0);
            *p = if base == 3 { image::Rgba([px(0), px(1), px(2), 255]) } else { image::Rgba([px(0), px(0), px(0), 255]) };
        }
    } else {
        return None;
    }
    // Soft mask → alpha.
    if let Ok(Object::Reference(mid)) = s.dict.get(b"SMask") {
        if let Ok(ms) = doc.get_object(*mid).and_then(|o| o.as_stream()) {
            let mw = ms.dict.get(b"Width").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0) as u32;
            let mh = ms.dict.get(b"Height").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0) as u32;
            if (mw, mh) == (w, h) {
                let flate = matches!(ms.dict.get(b"Filter"), Ok(Object::Name(n)) if n == b"FlateDecode");
                if let Some(a) = inflate(ms, flate) {
                    if a.len() >= n {
                        for (i, p) in rgba.pixels_mut().enumerate() {
                            p[3] = a[i];
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(rgba).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some((out, "png"))
}
