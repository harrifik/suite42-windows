//! Writing scans: searchable PDF (optionally PDF/A-style), multi-page TIFF, JPEG/PNG per page,
//! and recognised text as TXT or RTF.

use crate::process;
use crate::Page;
use kit42::pdfgen;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Format {
    Pdf,
    PdfA,
    Tiff,
    Jpeg,
    Png,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Pdf | Format::PdfA => "pdf",
            Format::Tiff => "tif",
            Format::Jpeg => "jpg",
            Format::Png => "png",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Format::Pdf => "PDF",
            Format::PdfA => "PDF/A",
            Format::Tiff => "TIFF",
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
        }
    }
    pub const ALL: [Format; 5] = [Format::Pdf, Format::PdfA, Format::Tiff, Format::Jpeg, Format::Png];
}

/// Size presets: (target dpi, JPEG quality). dpi 0 = keep the scan resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Size {
    Lossless,
    High,
    Balanced,
    Smallest,
}

impl Size {
    pub const ALL: [Size; 4] = [Size::Lossless, Size::High, Size::Balanced, Size::Smallest];
    pub fn params(self) -> (f32, u8) {
        match self {
            Size::Lossless => (0.0, 100),
            Size::High => (300.0, 88),
            Size::Balanced => (200.0, 78),
            Size::Smallest => (150.0, 65),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Size::Lossless => kit42::t("Original (lossless)", "Исходное (без потерь)"),
            Size::High => kit42::t("High (300 dpi)", "Высокое (300 dpi)"),
            Size::Balanced => kit42::t("Balanced (200 dpi)", "Среднее (200 dpi)"),
            Size::Smallest => kit42::t("Smallest (150 dpi)", "Наименьшее (150 dpi)"),
        }
    }
}

/// Downsamples a page for a size preset; returns the image and its effective dpi.
fn sized(p: &Page, size: Size) -> (image::RgbaImage, f32) {
    let (target, _) = size.params();
    if target <= 0.0 || p.dpi <= target + 1.0 {
        return ((*p.processed).clone(), p.dpi);
    }
    let s = target / p.dpi;
    let (w, h) = p.processed.dimensions();
    let img = image::imageops::resize(&*p.processed, ((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1), image::imageops::FilterType::Lanczos3);
    let bw = process::is_bilevel(&p.processed);
    let img = if bw { process::binarize(&img, target) } else { img };
    (img, target)
}

pub fn pdf(pages: &[Page], size: Size, pdfa: bool, title: &str) -> Result<Vec<u8>, String> {
    let mut d = pdfgen::Doc::new();
    d.pdfa = pdfa;
    d.title = title.to_string();
    let font = d.font("Arial", false, false);
    let (_, quality) = size.params();
    for p in pages {
        let (img, dpi) = sized(p, size);
        let (w_pt, h_pt) = (img.width() as f32 / dpi * 72.0, img.height() as f32 / dpi * 72.0);
        let mut page = pdfgen::Page::new(w_pt, h_pt);
        let id = if process::is_bilevel(&img) {
            d.add_image_bilevel(&process::to_luma(&img))
        } else if size == Size::Lossless {
            d.add_image_rgba(&img)
        } else {
            let gray = img.pixels().all(|q| q[0] == q[1] && q[1] == q[2]);
            let mut out = Vec::new();
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
            if gray {
                image::DynamicImage::ImageRgba8(img.clone()).to_luma8().write_with_encoder(enc).map_err(|e| e.to_string())?;
            } else {
                image::DynamicImage::ImageRgba8(img.clone()).to_rgb8().write_with_encoder(enc).map_err(|e| e.to_string())?;
            }
            d.add_image_jpeg(out, img.width(), img.height(), gray)
        };
        page.image(id, 0.0, 0.0, w_pt, h_pt);
        if let Some(o) = &p.ocr {
            // OCR boxes are in processed-image pixels at the page's own dpi.
            o.to_pdf_layer(&mut d, &mut page, font, 72.0 / p.dpi);
        }
        d.add_page(page);
    }
    Ok(d.finish())
}

pub fn tiff(pages: &[Page], size: Size) -> Result<Vec<u8>, String> {
    use tiff::encoder::{colortype, compression::Lzw, TiffEncoder};
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut enc = TiffEncoder::new(&mut buf).map_err(|e| e.to_string())?;
        for p in pages {
            let (img, _) = sized(p, size);
            let (w, h) = img.dimensions();
            let gray = img.pixels().all(|q| q[0] == q[1] && q[1] == q[2]);
            if gray {
                let g = process::to_luma(&img);
                enc.write_image_with_compression::<colortype::Gray8, _>(w, h, Lzw, g.as_raw()).map_err(|e| e.to_string())?;
            } else {
                let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
                enc.write_image_with_compression::<colortype::RGB8, _>(w, h, Lzw, rgb.as_raw()).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(buf.into_inner())
}

pub fn image_bytes(p: &Page, png: bool, size: Size) -> Result<Vec<u8>, String> {
    let (img, _) = sized(p, size);
    let mut out = Vec::new();
    if png {
        image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    } else {
        let (_, q) = size.params();
        let q = if size == Size::Lossless { 95 } else { q };
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q)
            .encode_image(&image::DynamicImage::ImageRgba8(img).to_rgb8())
            .map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Writes pages in a format; returns the files written.
pub fn save(pages: &[Page], fmt: Format, size: Size, path: &Path, one_per_page: bool) -> Result<Vec<PathBuf>, String> {
    if pages.is_empty() {
        return Err(kit42::t("There are no pages to save.", "Нет страниц для сохранения.").into());
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Scan".into());
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let title = stem.clone();
    let multi = matches!(fmt, Format::Pdf | Format::PdfA | Format::Tiff) && !one_per_page;
    let mut written = Vec::new();
    if multi {
        let bytes = match fmt {
            Format::Tiff => tiff(pages, size)?,
            _ => pdf(pages, size, fmt == Format::PdfA, &title)?,
        };
        let p = path.with_extension(fmt.ext());
        kit42::write_atomic(&p, &bytes).map_err(|e| e.to_string())?;
        written.push(p);
    } else {
        for (i, pg) in pages.iter().enumerate() {
            let p = if pages.len() == 1 { path.with_extension(fmt.ext()) } else { dir.join(format!("{stem} {:03}.{}", i + 1, fmt.ext())) };
            let bytes = match fmt {
                Format::Pdf | Format::PdfA => pdf(std::slice::from_ref(pg), size, fmt == Format::PdfA, &title)?,
                Format::Tiff => tiff(std::slice::from_ref(pg), size)?,
                Format::Jpeg => image_bytes(pg, false, size)?,
                Format::Png => image_bytes(pg, true, size)?,
            };
            kit42::write_atomic(&p, &bytes).map_err(|e| e.to_string())?;
            written.push(p);
        }
    }
    Ok(written)
}

/// Recognised text in reading order (top-to-bottom, then left-to-right).
pub fn page_text(o: &kit42::winrt::OcrResult) -> String {
    let mut lines: Vec<&kit42::winrt::OcrLine> = o.lines.iter().collect();
    lines.sort_by(|a, b| {
        let ay = a.words.first().map(|w| w.y).unwrap_or(0.0);
        let by = b.words.first().map(|w| w.y).unwrap_or(0.0);
        let ah = a.words.first().map(|w| w.h).unwrap_or(10.0);
        if (ay - by).abs() < ah * 0.5 {
            a.words.first().map(|w| w.x).unwrap_or(0.0).partial_cmp(&b.words.first().map(|w| w.x).unwrap_or(0.0)).unwrap()
        } else {
            ay.partial_cmp(&by).unwrap()
        }
    });
    lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
}

pub fn text_rtf(text: &str) -> String {
    let mut r = String::from("{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Calibri;}}\\f0\\fs22\n");
    for line in text.lines() {
        for c in line.chars() {
            match c {
                '\\' | '{' | '}' => {
                    r.push('\\');
                    r.push(c);
                }
                '\x0C' => r.push_str("\\page "),
                c if (c as u32) < 128 => r.push(c),
                c => {
                    let mut b = [0u16; 2];
                    for u in c.encode_utf16(&mut b) {
                        r.push_str(&format!("\\u{}?", *u as i16));
                    }
                }
            }
        }
        r.push_str("\\par\n");
    }
    r.push('}');
    r
}

/// Expands a name template: `Scan YYYY-MM-DD HH-mm`, `{n}` counter, `{title}` first text line.
pub fn expand_name(template: &str, n: usize, title: &str) -> String {
    let now = chrono::Local::now();
    let s = template
        .replace("YYYY", &now.format("%Y").to_string())
        .replace("MM", &now.format("%m").to_string())
        .replace("DD", &now.format("%d").to_string())
        .replace("HH", &now.format("%H").to_string())
        .replace("mm", &now.format("%M").to_string())
        .replace("ss", &now.format("%S").to_string())
        .replace("{n}", &n.to_string())
        .replace("{title}", title);
    s.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect::<String>().trim().to_string()
}
