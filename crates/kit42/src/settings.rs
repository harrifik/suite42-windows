//! Settings shared by all six apps, stored as JSON in %APPDATA%\Suite42\settings.json.

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Mutex;

static STORE: Mutex<Option<Map<String, Value>>> = Mutex::new(None);

pub fn data_dir() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join("Suite42");
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn load_global() {
    let map = std::fs::read(path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Map<String, Value>>(&b).ok())
        .unwrap_or_default();
    let lang = map
        .get("language")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_else(crate::i18n::system_lang);
    crate::i18n::set_lang(lang);
    *STORE.lock().unwrap() = Some(map);
}

fn reload_if_needed() {
    let mut s = STORE.lock().unwrap();
    if s.is_none() {
        *s = Some(
            std::fs::read(path())
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default(),
        );
    }
}

pub fn get<T: DeserializeOwned>(key: &str) -> Option<T> {
    reload_if_needed();
    let s = STORE.lock().unwrap();
    s.as_ref()?.get(key).and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub fn get_or<T: DeserializeOwned>(key: &str, default: T) -> T {
    get(key).unwrap_or(default)
}

pub fn set<T: Serialize>(key: &str, value: T) {
    reload_if_needed();
    // Merge with what other apps may have written meanwhile.
    let mut disk: Map<String, Value> = std::fs::read(path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let v = serde_json::to_value(value).unwrap_or(Value::Null);
    disk.insert(key.to_string(), v);
    if let Ok(bytes) = serde_json::to_vec_pretty(&disk) {
        let _ = crate::write_atomic(&path(), &bytes);
    }
    *STORE.lock().unwrap() = Some(disk);
}

/// Recent documents of one app, newest first.
pub fn recent(app: &str) -> Vec<PathBuf> {
    get::<Vec<PathBuf>>(&format!("recent.{app}"))
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.exists())
        .collect()
}

pub fn add_recent(app: &str, path: &std::path::Path) {
    let mut list = recent(app);
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(15);
    set(&format!("recent.{app}"), list);
}

pub fn clear_recent(app: &str) {
    set::<Vec<PathBuf>>(&format!("recent.{app}"), vec![]);
}
