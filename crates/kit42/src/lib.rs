//! Kit42 — shared foundation of the Suite42 Windows apps: language, fonts and icons,
//! settings, native dialogs, Windows Runtime services (PDF rendering, OCR, scanners),
//! PDF generation, raster text, the signature and stamp library and the rich-text editor.

pub mod fonts;
pub mod history;
pub mod i18n;
pub mod icons;
pub mod ooxml;
pub mod pdfgen;
pub mod raster;
pub mod richtext;
pub mod settings;
pub mod shell;
pub mod signature;
pub mod ui;
pub mod winrt;

pub use i18n::{lang, set_lang, t, Lang};

use std::path::PathBuf;

/// Everything an app needs to start its window.
pub struct AppSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub size: [f32; 2],
}

/// Starts an eframe app with the Suite42 look: Segoe UI, Fluent icons, Windows 11 colours.
pub fn run<A: eframe::App + 'static>(
    spec: AppSpec,
    make: impl FnOnce(&eframe::CreationContext<'_>) -> A + 'static,
) -> eframe::Result<()> {
    settings::load_global();
    let icon = icons::app_icon(spec.id);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(spec.title)
        .with_inner_size(spec.size)
        .with_min_inner_size([520.0, 360.0])
        .with_drag_and_drop(true);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        spec.id,
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            ui::install_style(&cc.egui_ctx);
            if let Ok(a) = std::env::var("K42_APPEARANCE") {
                cc.egui_ctx.options_mut(|o| {
                    o.theme_preference = if a == "dark" { egui::ThemePreference::Dark } else { egui::ThemePreference::Light }
                });
            }
            let snap = std::env::var("K42_SNAPSHOT").ok().map(PathBuf::from);
            Ok(Box::new(Snapshot { inner: make(cc), snap, frames: 0 }))
        }),
    )
}

/// First command-line argument that names an existing file (Explorer "Open with").
pub fn startup_files() -> Vec<PathBuf> {
    std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect()
}

/// Lower-case extension of a path.
pub fn ext(path: &std::path::Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// File name for window titles.
pub fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Writes through a temporary file and renames, so a failed write never leaves a half file.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.k42tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, bytes)?;
    if path.exists() {
        std::fs::remove_file(path).ok();
    }
    std::fs::rename(&tmp, path).or_else(|_| {
        std::fs::copy(&tmp, path)?;
        std::fs::remove_file(&tmp)
    })
}

/// Attaches to the parent console so CLI commands of GUI-subsystem apps can print.
pub fn attach_console() {
    unsafe {
        let _ = windows::Win32::System::Console::AttachConsole(windows::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

/// Wraps an app for `K42_SNAPSHOT=out.png`: renders a few frames, saves a screenshot, exits.
struct Snapshot<A> {
    inner: A,
    snap: Option<PathBuf>,
    frames: u32,
}

impl<A: eframe::App> eframe::App for Snapshot<A> {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.inner.update(ctx, frame);
        let Some(path) = self.snap.clone() else { return };
        self.frames += 1;
        ctx.request_repaint();
        let wait: u32 = std::env::var("K42_SNAPSHOT_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
        if self.frames == wait {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, raw) {
                let _ = buf.save(&path);
            }
            self.snap = None;
            std::process::exit(0);
        }
        if self.frames > wait + 200 {
            std::process::exit(2);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.inner.save(storage);
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.inner.on_exit(gl);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.inner.clear_color(visuals)
    }
}
