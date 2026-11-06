//! Windows Runtime services built into Windows 10/11:
//! PDF page rendering (Windows.Data.Pdf), text recognition (Windows.Media.Ocr),
//! still-image decoding (WIC through BitmapDecoder) and scanners (Windows.Devices.Scanners).
//!
//! Every call runs on its own multithreaded-apartment worker thread, so blocking `.get()`
//! never runs on the UI (STA) thread.

use image::RgbaImage;
use windows::core::{Interface, HSTRING};
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, SoftwareBitmap};
use windows::Storage::Streams::{DataReader, DataWriter, IRandomAccessStream, InMemoryRandomAccessStream};

/// Keeps the process-wide multithreaded apartment alive. windows-rs caches activation
/// factories globally; if the MTA were torn down when a worker thread exits, the cached
/// factories would dangle and the next call would crash.
pub fn ensure_mta() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let _ = windows::Win32::System::Com::CoIncrementMTAUsage();
    });
}

/// Runs a closure in a fresh MTA thread and waits for its result.
pub fn in_mta<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    ensure_mta();
    // Windows Runtime components (OCR in particular) need more than Rust's default 2 MB stack.
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            f()
        })
        .map_err(|e| e.to_string())?
        .join()
    .map_err(|_| "Windows Runtime worker panicked".to_string())?
}

fn err(e: windows::core::Error) -> String {
    e.message().to_string()
}

fn stream_from_bytes(bytes: &[u8]) -> windows::core::Result<InMemoryRandomAccessStream> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.get()?;
    writer.FlushAsync()?.get()?;
    writer.DetachStream()?;
    stream.Seek(0)?;
    Ok(stream)
}

fn bytes_from_stream(stream: &IRandomAccessStream) -> windows::core::Result<Vec<u8>> {
    let size = stream.Size()? as u32;
    let input = stream.GetInputStreamAt(0)?;
    let reader = DataReader::CreateDataReader(&input)?;
    reader.LoadAsync(size)?.get()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf)?;
    Ok(buf)
}

/// Page sizes of a PDF in points (as displayed, rotation applied).
pub fn pdf_page_sizes(bytes: Vec<u8>, password: Option<String>) -> Result<Vec<(f32, f32)>, String> {
    in_mta(move || {
        let doc = load_pdf(&bytes, password.as_deref()).map_err(err)?;
        let n = doc.PageCount().map_err(err)?;
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let s = doc.GetPage(i).and_then(|p| p.Size()).map_err(err)?;
            // Size is in DIPs (96 per inch); PDF points are 72 per inch.
            out.push((s.Width * 0.75, s.Height * 0.75));
        }
        Ok(out)
    })
}

fn load_pdf(bytes: &[u8], password: Option<&str>) -> windows::core::Result<PdfDocument> {
    let stream = stream_from_bytes(bytes)?;
    let rs: IRandomAccessStream = stream.cast()?;
    match password {
        Some(p) if !p.is_empty() => PdfDocument::LoadFromStreamWithPasswordAsync(&rs, &HSTRING::from(p))?.get(),
        _ => PdfDocument::LoadFromStreamAsync(&rs)?.get(),
    }
}

/// True when the PDF needs a password to open.
pub fn pdf_needs_password(bytes: Vec<u8>) -> bool {
    in_mta(move || match load_pdf(&bytes, None) {
        Ok(_) => Ok(false),
        Err(e) => Ok(e.code().0 as u32 == 0x8007_052B || e.message().to_lowercase().contains("password")),
    })
    .unwrap_or(false)
}

/// A long-lived renderer that keeps the document parsed between page requests.
pub struct PdfRenderer {
    tx: std::sync::mpsc::Sender<RenderJob>,
}

struct RenderJob {
    page: u32,
    width: u32,
    reply: std::sync::mpsc::Sender<Result<RgbaImage, String>>,
}

impl PdfRenderer {
    /// Parses the document on a worker thread; fails if Windows cannot open it.
    pub fn open(bytes: Vec<u8>, password: Option<String>) -> Result<(Self, Vec<(f32, f32)>), String> {
        ensure_mta();
        let (tx, rx) = std::sync::mpsc::channel::<RenderJob>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new().stack_size(16 << 20).spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            let doc = match load_pdf(&bytes, password.as_deref()) {
                Ok(d) => d,
                Err(e) => {
                    let _ = ready_tx.send(Err(err(e)));
                    return;
                }
            };
            let mut sizes = Vec::new();
            let n = doc.PageCount().unwrap_or(0);
            for i in 0..n {
                let s = doc.GetPage(i).and_then(|p| p.Size()).map(|s| (s.Width * 0.75, s.Height * 0.75));
                sizes.push(s.unwrap_or((612.0, 792.0)));
            }
            let _ = ready_tx.send(Ok(sizes));
            while let Ok(job) = rx.recv() {
                let r = render_page(&doc, job.page, job.width).map_err(err);
                let _ = job.reply.send(r);
            }
        });
        let sizes = ready_rx.recv().map_err(|e| e.to_string())??;
        Ok((Self { tx }, sizes))
    }

    /// Renders a page to the given pixel width (blocking; call from a worker).
    pub fn render(&self, page: u32, width: u32) -> Result<RgbaImage, String> {
        let (reply, rx) = std::sync::mpsc::channel();
        self.tx.send(RenderJob { page, width, reply }).map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    /// Requests a page asynchronously; the result arrives on the returned receiver.
    pub fn request(&self, page: u32, width: u32) -> std::sync::mpsc::Receiver<Result<RgbaImage, String>> {
        let (reply, rx) = std::sync::mpsc::channel();
        let _ = self.tx.send(RenderJob { page, width, reply });
        rx
    }
}

impl Clone for PdfRenderer {
    fn clone(&self) -> Self {
        Self { tx: self.tx.clone() }
    }
}

/// Windows scales DestinationWidth by the display DPI; this factor (×1000) undoes it.
static RENDER_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1000);

fn render_page(doc: &PdfDocument, page: u32, width: u32) -> windows::core::Result<RgbaImage> {
    let p = doc.GetPage(page)?;
    let opts = PdfPageRenderOptions::new()?;
    let scale = RENDER_SCALE.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
    let asked = ((width as f32 / scale).round() as u32).clamp(16, 12000);
    opts.SetDestinationWidth(asked)?;
    let out = InMemoryRandomAccessStream::new()?;
    p.RenderWithOptionsToStreamAsync(&out, &opts)?.get()?;
    let rs: IRandomAccessStream = out.cast()?;
    let png = bytes_from_stream(&rs)?;
    let img = image::load_from_memory(&png)
        .map_err(|e| windows::core::Error::new(windows::core::HRESULT(0x8000_4005u32 as i32), e.to_string()))?
        .to_rgba8();
    let actual = img.width() as f32 / asked as f32;
    if (actual - 1.0).abs() > 0.02 && scale == 1.0 {
        RENDER_SCALE.store((actual * 1000.0).round() as u32, std::sync::atomic::Ordering::Relaxed);
    }
    if img.width() != width && width > 0 && (img.width() as i64 - width as i64).abs() > 2 {
        let h = (img.height() as f32 * width as f32 / img.width() as f32).round().max(1.0) as u32;
        return Ok(image::imageops::resize(&img, width, h, image::imageops::FilterType::Triangle));
    }
    Ok(img)
}

/// One-shot render of a page of PDF bytes.
pub fn pdf_render_page(bytes: Vec<u8>, page: u32, width: u32, password: Option<String>) -> Result<RgbaImage, String> {
    in_mta(move || {
        let doc = load_pdf(&bytes, password.as_deref()).map_err(err)?;
        render_page(&doc, page, width).map_err(err)
    })
}

/// Decodes any still image Windows can read (HEIC, AVIF, RAW, JXR… with installed codecs).
pub fn decode_image(bytes: Vec<u8>) -> Result<RgbaImage, String> {
    in_mta(move || {
        let stream = stream_from_bytes(&bytes).map_err(err)?;
        let rs: IRandomAccessStream = stream.cast().map_err(err)?;
        let decoder = BitmapDecoder::CreateAsync(&rs).map_err(err)?.get().map_err(err)?;
        let bmp = decoder.GetSoftwareBitmapAsync().map_err(err)?.get().map_err(err)?;
        software_bitmap_to_rgba(&bmp).map_err(err)
    })
}

fn software_bitmap_to_rgba(bmp: &SoftwareBitmap) -> windows::core::Result<RgbaImage> {
    let conv = SoftwareBitmap::ConvertWithAlpha(bmp, BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied)?;
    // Encode to PNG through WIC to read the pixels without unsafe buffer access.
    let out = InMemoryRandomAccessStream::new()?;
    let rs: IRandomAccessStream = out.cast()?;
    let enc = windows::Graphics::Imaging::BitmapEncoder::CreateAsync(
        windows::Graphics::Imaging::BitmapEncoder::PngEncoderId()?,
        &rs,
    )?
    .get()?;
    enc.SetSoftwareBitmap(&conv)?;
    enc.FlushAsync()?.get()?;
    let png = bytes_from_stream(&rs)?;
    let img = image::load_from_memory(&png)
        .map_err(|e| windows::core::Error::new(windows::core::HRESULT(0x8000_4005u32 as i32), e.to_string()))?
        .to_rgba8();
    Ok(img)
}

/// A recognised word with its box in image pixels.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct OcrWord {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct OcrLine {
    pub text: String,
    pub words: Vec<OcrWord>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct OcrResult {
    pub lines: Vec<OcrLine>,
    pub angle: f64,
}

impl OcrLine {
    /// Bounding box of the line (x, y, w, h).
    pub fn bbox(&self) -> (f32, f32, f32, f32) {
        let x0 = self.words.iter().map(|w| w.x).fold(f32::MAX, f32::min);
        let y0 = self.words.iter().map(|w| w.y).fold(f32::MAX, f32::min);
        let x1 = self.words.iter().map(|w| w.x + w.w).fold(f32::MIN, f32::max);
        let y1 = self.words.iter().map(|w| w.y + w.h).fold(f32::MIN, f32::max);
        if self.words.is_empty() {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            (x0, y0, x1 - x0, y1 - y0)
        }
    }
}

impl OcrResult {
    /// Adds an invisible text layer to a PDF page. `k` maps image pixels to points.
    pub fn to_pdf_layer(&self, doc: &mut crate::pdfgen::Doc, page: &mut crate::pdfgen::Page, font: crate::pdfgen::FontId, k: f32) {
        for l in &self.lines {
            let (x, y, w, h) = l.bbox();
            // Lines are written whole (searchable as phrases); text is the words joined.
            let text = l.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
            doc.invisible_text(page, font, x * k, y * k, w * k, h * k, &text);
        }
    }

    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

/// OCR languages installed in Windows (Settings › Time & language › Language).
pub fn ocr_languages() -> Vec<(String, String)> {
    in_mta(|| {
        let langs = windows::Media::Ocr::OcrEngine::AvailableRecognizerLanguages().map_err(err)?;
        let mut v = Vec::new();
        for l in langs {
            v.push((
                l.LanguageTag().map(|s| s.to_string()).unwrap_or_default(),
                l.NativeName().map(|s| s.to_string()).unwrap_or_default(),
            ));
        }
        Ok(v)
    })
    .unwrap_or_default()
}

/// Recognises text with the Windows OCR engine. `lang` is a BCP-47 tag or None for the
/// user's profile languages. Large images are scaled to the engine's limit and boxes scaled back.
fn dbg(s: &str) {
    if std::env::var("K42_DEBUG").is_ok() {
        eprintln!("[winrt] {s}");
    }
}

pub fn ocr(img: &RgbaImage, lang: Option<String>) -> Result<OcrResult, String> {
    dbg("ocr: max dimension");
    let max = in_mta(|| windows::Media::Ocr::OcrEngine::MaxImageDimension().map_err(err)).unwrap_or(10000);
    let (w, h) = img.dimensions();
    let scale = if w.max(h) > max { max as f32 / w.max(h) as f32 } else { 1.0 };
    let src = if scale < 1.0 {
        image::imageops::resize(
            img,
            (w as f32 * scale) as u32,
            (h as f32 * scale) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img.clone()
    };
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(src)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    dbg("ocr: png encoded");
    in_mta(move || {
        use windows::Media::Ocr::OcrEngine;
        dbg("ocr: creating engine");
        let engine = match lang.as_deref() {
            Some(tag) => {
                let l = windows::Globalization::Language::CreateLanguage(&HSTRING::from(tag)).map_err(err)?;
                if OcrEngine::IsLanguageSupported(&l).unwrap_or(false) {
                    OcrEngine::TryCreateFromLanguage(&l).map_err(err)?
                } else {
                    OcrEngine::TryCreateFromUserProfileLanguages().map_err(err)?
                }
            }
            None => OcrEngine::TryCreateFromUserProfileLanguages().map_err(err)?,
        };
        dbg("ocr: engine ok, decoding");
        let stream = stream_from_bytes(&png).map_err(err)?;
        let rs: IRandomAccessStream = stream.cast().map_err(err)?;
        let decoder = BitmapDecoder::CreateAsync(&rs).map_err(err)?.get().map_err(err)?;
        dbg("ocr: decoder ok");
        let bmp = decoder.GetSoftwareBitmapAsync().map_err(err)?.get().map_err(err)?;
        dbg("ocr: bitmap ok");
        let bmp = SoftwareBitmap::ConvertWithAlpha(&bmp, BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied)
            .map_err(err)?;
        dbg("ocr: recognizing");
        let res = engine.RecognizeAsync(&bmp).map_err(err)?.get().map_err(err)?;
        dbg("ocr: recognized");
        let mut out = OcrResult::default();
        if let Ok(a) = res.TextAngle() {
            out.angle = a.Value().unwrap_or(0.0);
        }
        let inv = 1.0 / scale;
        for line in res.Lines().map_err(err)? {
            let mut l = OcrLine { text: line.Text().map(|s| s.to_string()).unwrap_or_default(), words: vec![] };
            for word in line.Words().map_err(err)? {
                let r = word.BoundingRect().map_err(err)?;
                l.words.push(OcrWord {
                    text: word.Text().map(|s| s.to_string()).unwrap_or_default(),
                    x: r.X * inv,
                    y: r.Y * inv,
                    w: r.Width * inv,
                    h: r.Height * inv,
                });
            }
            out.lines.push(l);
        }
        Ok(out)
    })
}

/// A scanner reported by Windows (WIA / eSCL devices with a WinRT driver).
#[derive(Clone, Debug, PartialEq)]
pub struct ScannerInfo {
    pub id: String,
    pub name: String,
}

pub fn list_scanners() -> Result<Vec<ScannerInfo>, String> {
    in_mta(|| {
        use windows::Devices::Enumeration::DeviceInformation;
        use windows::Devices::Scanners::ImageScanner;
        let sel = ImageScanner::GetDeviceSelector().map_err(err)?;
        let devs = DeviceInformation::FindAllAsyncAqsFilter(&sel).map_err(err)?.get().map_err(err)?;
        let mut v = Vec::new();
        for d in devs {
            v.push(ScannerInfo {
                id: d.Id().map(|s| s.to_string()).unwrap_or_default(),
                name: d.Name().map(|s| s.to_string()).unwrap_or_default(),
            });
        }
        Ok(v)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScanSource {
    Auto,
    Flatbed,
    Feeder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScanColor {
    Color,
    Gray,
    BlackWhite,
}

#[derive(Clone, Debug)]
pub struct ScanCaps {
    pub flatbed: bool,
    pub feeder: bool,
    pub duplex: bool,
}

pub fn scanner_caps(id: String) -> Result<ScanCaps, String> {
    in_mta(move || {
        use windows::Devices::Scanners::{ImageScanner, ImageScannerScanSource};
        let s = ImageScanner::FromIdAsync(&HSTRING::from(id)).map_err(err)?.get().map_err(err)?;
        let flatbed = s.IsScanSourceSupported(ImageScannerScanSource::Flatbed).unwrap_or(false);
        let feeder = s.IsScanSourceSupported(ImageScannerScanSource::Feeder).unwrap_or(false);
        let duplex = feeder && s.FeederConfiguration().and_then(|f| f.CanScanDuplex()).unwrap_or(false);
        Ok(ScanCaps { flatbed, feeder, duplex })
    })
}

/// Scans into a folder and returns the files the driver wrote (one per page).
pub fn scan(
    id: String,
    source: ScanSource,
    color: ScanColor,
    dpi: u32,
    duplex: bool,
    folder: std::path::PathBuf,
) -> Result<Vec<std::path::PathBuf>, String> {
    in_mta(move || {
        use windows::Devices::Scanners::*;
        let s = ImageScanner::FromIdAsync(&HSTRING::from(id)).map_err(err)?.get().map_err(err)?;
        let mode = match color {
            ScanColor::Color => ImageScannerColorMode::Color,
            ScanColor::Gray => ImageScannerColorMode::Grayscale,
            ScanColor::BlackWhite => ImageScannerColorMode::Monochrome,
        };
        let res = ImageScannerResolution { DpiX: dpi as f32, DpiY: dpi as f32 };
        let src = match source {
            ScanSource::Auto => ImageScannerScanSource::Default,
            ScanSource::Flatbed => ImageScannerScanSource::Flatbed,
            ScanSource::Feeder => ImageScannerScanSource::Feeder,
        };
        match source {
            ScanSource::Flatbed => {
                if let Ok(c) = s.FlatbedConfiguration() {
                    let _ = c.SetColorMode(mode);
                    let _ = c.SetDesiredResolution(res);
                }
            }
            ScanSource::Feeder => {
                if let Ok(c) = s.FeederConfiguration() {
                    let _ = c.SetColorMode(mode);
                    let _ = c.SetDesiredResolution(res);
                    let _ = c.SetDuplex(duplex);
                    let _ = c.SetMaxNumberOfPages(0);
                }
            }
            ScanSource::Auto => {}
        }
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let sf = windows::Storage::StorageFolder::GetFolderFromPathAsync(&HSTRING::from(folder.as_os_str()))
            .map_err(err)?
            .get()
            .map_err(err)?;
        let result = s.ScanFilesToFolderAsync(src, &sf).map_err(err)?.get().map_err(err)?;
        let mut files = Vec::new();
        for f in result.ScannedFiles().map_err(err)? {
            if let Ok(p) = f.Path() {
                files.push(std::path::PathBuf::from(p.to_string()));
            }
        }
        Ok(files)
    })
}

/// Opens the classic Windows Fax and Scan / WIA acquisition as a fallback when no WinRT scanner exists.
pub fn open_windows_scan() {
    let _ = std::process::Command::new("wfs.exe").spawn().or_else(|_| {
        std::process::Command::new("explorer.exe").arg("ms-windows-store://pdp/?productid=9WZDNCRFJ3PV").spawn()
    });
}
