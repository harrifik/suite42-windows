//! Fonts: Segoe UI for the interface, Segoe Fluent Icons for toolbar glyphs and an index of
//! installed font files so documents can use their own families (Times New Roman, Calibri…).

use egui::{FontData, FontDefinitions, FontFamily};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

pub const ICON_FAMILY: &str = "k42-icons";
pub const UI_BOLD: &str = "k42-ui-bold";

static DEFS: OnceLock<Mutex<(FontDefinitions, HashSet<String>)>> = OnceLock::new();
static INDEX: OnceLock<BTreeMap<String, FaceSet>> = OnceLock::new();

/// Font files of one family.
#[derive(Clone, Default, Debug)]
pub struct FaceSet {
    pub regular: Option<PathBuf>,
    pub bold: Option<PathBuf>,
    pub italic: Option<PathBuf>,
    pub bold_italic: Option<PathBuf>,
}

impl FaceSet {
    pub fn pick(&self, bold: bool, italic: bool) -> Option<&PathBuf> {
        let want = match (bold, italic) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        };
        want.as_ref()
            .or(if bold { self.bold.as_ref() } else { None })
            .or(if italic { self.italic.as_ref() } else { None })
            .or(self.regular.as_ref())
            .or(self.bold.as_ref())
    }
}

pub fn fonts_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from(
        std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()),
    )
    .join("Fonts")];
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft\\Windows\\Fonts"));
    }
    dirs
}

fn system_font(name: &str) -> Option<Vec<u8>> {
    fonts_dirs()
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| p.exists())
        .and_then(|p| std::fs::read(p).ok())
}

/// Index of installed families, built once on first use.
pub fn index() -> &'static BTreeMap<String, FaceSet> {
    INDEX.get_or_init(|| {
        let mut map: BTreeMap<String, FaceSet> = BTreeMap::new();
        for dir in fonts_dirs() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                let path = entry.path();
                let ext = crate::ext(&path);
                if ext != "ttf" && ext != "otf" {
                    continue;
                }
                let Ok(data) = std::fs::read(&path) else { continue };
                let Ok(face) = ttf_parser::Face::parse(&data, 0) else { continue };
                let mut family = None;
                let mut typo_family = None;
                for name in face.names() {
                    if !name.is_unicode() && name.platform_id != ttf_parser::PlatformId::Macintosh {
                        continue;
                    }
                    let english = name.language_id == 0x0409 || name.platform_id == ttf_parser::PlatformId::Macintosh;
                    if !english {
                        continue;
                    }
                    match name.name_id {
                        ttf_parser::name_id::FAMILY if family.is_none() => family = name.to_string(),
                        ttf_parser::name_id::TYPOGRAPHIC_FAMILY if typo_family.is_none() => {
                            typo_family = name.to_string()
                        }
                        _ => {}
                    }
                }
                let Some(fam) = typo_family.or(family) else { continue };
                let bold = face.is_bold() || face.weight().to_number() >= 600;
                let italic = face.is_italic() || face.is_oblique();
                let weight = face.weight().to_number();
                let set = map.entry(fam).or_default();
                let slot = match (bold, italic) {
                    (false, false) => &mut set.regular,
                    (true, false) => &mut set.bold,
                    (false, true) => &mut set.italic,
                    (true, true) => &mut set.bold_italic,
                };
                // Prefer the canonical weights (400 / 700) over light, semibold, black…
                let better = slot.is_none() || weight == 400 || weight == 700;
                if better {
                    *slot = Some(path);
                }
            }
        }
        map
    })
}

/// Installed families sorted by name, for font pickers.
pub fn family_names() -> Vec<String> {
    index()
        .iter()
        .filter(|(_, f)| f.regular.is_some())
        .map(|(n, _)| n.clone())
        .collect()
}

/// A font file for drawing or embedding text; falls back to Segoe UI / Arial.
pub fn font_file(family: &str, bold: bool, italic: bool) -> Option<PathBuf> {
    let idx = index();
    let lookup = |name: &str| idx.get(name).and_then(|s| s.pick(bold, italic)).cloned();
    lookup(family)
        .or_else(|| lookup(&substitute(family)))
        .or_else(|| lookup("Segoe UI"))
        .or_else(|| lookup("Arial"))
}

/// Close replacements for common document fonts missing on Windows.
pub fn substitute(family: &str) -> String {
    match family {
        "Aptos" | "Helvetica" | "Helvetica Neue" | "Liberation Sans" | "Arimo" | "DejaVu Sans" => "Arial",
        "Times" | "Liberation Serif" | "Tinos" | "DejaVu Serif" | "PT Serif" => "Times New Roman",
        "Courier" | "Liberation Mono" | "Menlo" | "Monaco" | "DejaVu Sans Mono" => "Courier New",
        "Carlito" => "Calibri",
        "Caladea" => "Cambria",
        "PT Sans" | "Open Sans" | "Roboto" | "San Francisco" | "SF Pro" => "Segoe UI",
        _ => "Segoe UI",
    }
    .to_string()
}

/// Installs Segoe UI, its bold face and the icon font into egui.
pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let mut ui_fonts = Vec::new();
    if let Some(data) = system_font("segoeui.ttf") {
        defs.font_data.insert("segoe".into(), Arc::new(FontData::from_owned(data)));
        ui_fonts.push("segoe".to_string());
    }
    // Symbols and Cyrillic/CJK fallbacks.
    for (key, file) in [
        ("seguisym", "seguisym.ttf"),
        ("yahei", "msyh.ttc"),
        ("malgun", "malgun.ttf"),
        ("yugoth", "YuGothM.ttc"),
    ] {
        if let Some(data) = system_font(file) {
            defs.font_data.insert(key.into(), Arc::new(FontData::from_owned(data)));
            ui_fonts.push(key.to_string());
        }
    }
    let icon_data = system_font("SegoeIcons.ttf").or_else(|| system_font("segmdl2.ttf"));
    if let Some(data) = icon_data {
        defs.font_data.insert("icons".into(), Arc::new(FontData::from_owned(data)));
        defs.families.insert(FontFamily::Name(ICON_FAMILY.into()), vec!["icons".into()]);
        ui_fonts.push("icons".into());
    } else {
        defs.families.insert(FontFamily::Name(ICON_FAMILY.into()), vec![]);
    }
    let prop = defs.families.entry(FontFamily::Proportional).or_default();
    for (i, f) in ui_fonts.iter().enumerate() {
        prop.insert(i.min(prop.len()), f.clone());
    }
    // Keep egui's own fonts as the final fallback, after Windows fonts.
    let mut ordered: Vec<String> = ui_fonts.clone();
    for f in prop.iter() {
        if !ordered.contains(f) {
            ordered.push(f.clone());
        }
    }
    *prop = ordered;
    if let Some(data) = system_font("seguisb.ttf").or_else(|| system_font("segoeuib.ttf")) {
        defs.font_data.insert("segoe-bold".into(), Arc::new(FontData::from_owned(data)));
        let mut fam = vec!["segoe-bold".to_string()];
        fam.extend(ui_fonts.iter().filter(|f| *f != "segoe").cloned());
        defs.families.insert(FontFamily::Name(UI_BOLD.into()), fam);
    } else {
        let p = defs.families[&FontFamily::Proportional].clone();
        defs.families.insert(FontFamily::Name(UI_BOLD.into()), p);
    }
    if let Some(data) = system_font("consola.ttf") {
        defs.font_data.insert("consolas".into(), Arc::new(FontData::from_owned(data)));
        defs.families.entry(FontFamily::Monospace).or_default().insert(0, "consolas".into());
    }
    ctx.set_fonts(defs.clone());
    let _ = DEFS.set(Mutex::new((defs, HashSet::new())));
}

/// egui family for a document font face; loads the file the first time it is used.
/// Returns the family name to put in a `FontId`. The face is usable from the next frame.
/// Never call this inside a `ctx.fonts(…)` closure: it takes egui's context lock itself.
pub fn doc_family(ctx: &egui::Context, family: &str, bold: bool, italic: bool) -> FontFamily {
    let key = format!("doc:{}:{}{}", family, if bold { "b" } else { "" }, if italic { "i" } else { "" });
    let Some(lock) = DEFS.get() else { return FontFamily::Proportional };
    let mut guard = lock.lock().unwrap();
    let (defs, loaded) = &mut *guard;
    if loaded.contains(&key) {
        let fam = FontFamily::Name(key.into());
        let ready = ctx.fonts(|f| f.lock().fonts.definitions().families.contains_key(&fam));
        return if ready { fam } else { FontFamily::Proportional };
    }
    let Some(path) = font_file(family, bold, italic) else { return FontFamily::Proportional };
    let data_key = path.to_string_lossy().to_string();
    if !defs.font_data.contains_key(&data_key) {
        let Ok(bytes) = std::fs::read(&path) else { return FontFamily::Proportional };
        defs.font_data.insert(data_key.clone(), Arc::new(FontData::from_owned(bytes)));
    }
    let mut chain = vec![data_key];
    drop(path);
    chain.extend(defs.families[&FontFamily::Proportional].iter().cloned());
    defs.families.insert(FontFamily::Name(key.clone().into()), chain);
    loaded.insert(key.clone());
    ctx.set_fonts(defs.clone());
    // Until the new atlas exists this frame, draw with the UI font.
    FontFamily::Proportional
}

/// True when the family for a document face is already installed.
pub fn has_doc_family(family: &str, bold: bool, italic: bool) -> bool {
    let key = format!("doc:{}:{}{}", family, if bold { "b" } else { "" }, if italic { "i" } else { "" });
    DEFS.get().map(|l| l.lock().unwrap().1.contains(&key)).unwrap_or(false)
}

/// Loads font bytes for rasterizing or embedding.
pub fn load_bytes(family: &str, bold: bool, italic: bool) -> Option<Vec<u8>> {
    font_file(family, bold, italic).and_then(|p| std::fs::read(p).ok())
}
