//! Interface language. Strings live next to the code as `t("English", "Русский")` pairs,
//! so a missing translation is impossible by construction.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Ru];

    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ru => "Русский",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::Ru,
        _ => Lang::En,
    }
}

pub fn set_lang(lang: Lang) {
    LANG.store(
        match lang {
            Lang::En => 0,
            Lang::Ru => 1,
        },
        Ordering::Relaxed,
    );
}

/// Picks the string for the current interface language.
#[inline]
pub fn t(en: &'static str, ru: &'static str) -> &'static str {
    match lang() {
        Lang::En => en,
        Lang::Ru => ru,
    }
}

/// The Windows user interface language, used until the user picks one.
pub fn system_lang() -> Lang {
    let mut buf = [0u16; 85];
    let n = unsafe { windows::Win32::Globalization::GetUserDefaultLocaleName(&mut buf) };
    let name = String::from_utf16_lossy(&buf[..(n.max(1) as usize - 1)]);
    let base = name.split('-').next().unwrap_or("").to_ascii_lowercase();
    match base.as_str() {
        "ru" | "uk" | "be" | "kk" => Lang::Ru,
        _ => Lang::En,
    }
}
