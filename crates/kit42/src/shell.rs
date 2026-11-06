//! Windows shell integration: open, print, reveal in Explorer, sister apps.

use std::path::{Path, PathBuf};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

fn shell_execute(verb: &str, target: &str, params: Option<&str>) -> bool {
    let verb = HSTRING::from(verb);
    let target = HSTRING::from(target);
    let params_h = params.map(HSTRING::from);
    let r = unsafe {
        ShellExecuteW(
            windows::Win32::Foundation::HWND::default(),
            &verb,
            &target,
            params_h.as_ref().map(|p| PCWSTR(p.as_ptr())).unwrap_or(PCWSTR::null()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    r.0 as isize > 32
}

/// Opens a file or URL with its default application.
pub fn open(target: &str) -> bool {
    shell_execute("open", target, None)
}

pub fn open_path(path: &Path) -> bool {
    open(&path.to_string_lossy())
}

/// Sends a file to the default printer through its registered print verb,
/// falling back to opening it so the user can print from the viewer.
pub fn print(path: &Path) -> bool {
    shell_execute("print", &path.to_string_lossy(), None) || open_path(path)
}

/// Shows the file selected in File Explorer.
pub fn reveal(path: &Path) {
    let _ = std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}

/// Path of a sister app's executable next to the running one.
pub fn sister_app(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join(format!("{name}.exe"));
    p.exists().then_some(p)
}

/// Opens a file in a sister app (e.g. a saved scan in PDF42), otherwise in the default app.
pub fn open_in(app: &str, file: &Path) {
    if let Some(exe) = sister_app(app) {
        if std::process::Command::new(exe).arg(file).spawn().is_ok() {
            return;
        }
    }
    open_path(file);
}

/// A unique path in the temp folder.
pub fn temp_file(stem: &str, ext: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("Suite42");
    std::fs::create_dir_all(&dir).ok();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%3f");
    dir.join(format!("{stem}-{stamp}.{ext}"))
}

/// The user's Documents folder.
pub fn documents_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .map(|p| PathBuf::from(p).join("Documents"))
        .unwrap_or_else(|_| std::env::temp_dir())
}
