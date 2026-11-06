#![windows_subsystem = "windows"]
//! XLS42 for Windows — spreadsheets with formulas and charts (.xlsx, .xls, .ods, .csv).

mod charts;
mod export;
mod formula;
mod functions;
mod grid;
mod io;
mod model;
mod numfmt;

use egui::{Context, Key};
use kit42::history::History;
use kit42::icons as ic;
use kit42::ui::{self as kui, CTRL, CTRL_SHIFT, NONE};
use kit42::{settings, t};
use model::*;
use std::path::PathBuf;
use std::sync::Arc;

pub const APP: &str = "XLS42";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sel {
    pub active: (u32, u32),
    pub other: (u32, u32),
}

impl Sel {
    pub fn rect(&self) -> (u32, u32, u32, u32) {
        (self.active.0.min(self.other.0), self.active.1.min(self.other.1), self.active.0.max(self.other.0), self.active.1.max(self.other.1))
    }
    pub fn single(&self) -> bool {
        self.active == self.other
    }
}

pub struct Clip {
    pub r0: u32,
    pub c0: u32,
    pub cells: Vec<Vec<Option<Cell>>>,
    pub text: String,
    pub cut: Option<(usize, u32, u32, u32, u32)>,
}

pub enum Dialog {
    Format { tab: usize, style: Style, custom: String },
    Find { find: String, replace: String, formulas: bool, case: bool, whole: bool },
    Chart { kind: ChartKind, title: String, legend: bool, edit: Option<u64> },
    Rename(String),
    Functions(String),
    Export { all: bool, selection: bool, landscape: usize, fit: bool, grid: bool },
    Filter { col: u32, values: Vec<(String, bool)> },
    Sort { col: u32, desc: bool, header: bool },
    GoTo(String),
}

pub struct App {
    pub wb: Workbook,
    pub path: Option<PathBuf>,
    pub history: History<Workbook>,
    pub sheet: usize,
    pub sel: Sel,
    pub editing: Option<String>,
    pub edit_cell: Option<(usize, u32, u32)>,
    pub edit_focus_cell: bool,
    pub zoom: f32,
    pub scroll_to: Option<(u32, u32)>,
    pub clip: Option<Clip>,
    pub drag: Option<grid::Drag>,
    pub selected_chart: Option<u64>,
    pub dialog: Option<Dialog>,
    pub status: String,
    pub csv_delim: char,
    pub encoding: &'static encoding_rs::Encoding,
    pub picker: kit42::signature::MarkPicker,
    pub request_editor_focus: bool,
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self {
            wb: Workbook::default(),
            path: None,
            history: History::new(40),
            sheet: 0,
            sel: Sel { active: (0, 0), other: (0, 0) },
            editing: None,
            edit_cell: None,
            edit_focus_cell: true,
            zoom: settings::get_or("xls42.zoom", 1.0),
            scroll_to: None,
            clip: None,
            drag: None,
            selected_chart: None,
            dialog: None,
            status: String::new(),
            csv_delim: ',',
            encoding: encoding_rs::UTF_8,
            picker: Default::default(),
            request_editor_focus: false,
        };
        if let Some(p) = kit42::startup_files().into_iter().next() {
            app.open(p);
        }
        app
    }

    pub fn sh(&self) -> &Sheet {
        &self.wb.sheets[self.sheet]
    }

    pub fn sh_mut(&mut self) -> &mut Sheet {
        &mut self.wb.sheets[self.sheet]
    }

    fn name(&self) -> String {
        self.path.as_ref().map(|p| kit42::file_name(p)).unwrap_or_else(|| t("Book1", "Книга1").into())
    }

    pub fn checkpoint(&mut self, label: &str) {
        self.history.push(self.wb.clone(), label);
    }

    pub fn recalc(&mut self) {
        formula::recalc(&mut self.wb);
    }

    pub fn load(path: &std::path::Path) -> Result<(Workbook, char, &'static encoding_rs::Encoding), String> {
        let bytes = kit42::ooxml::read_file(path)?;
        let ext = kit42::ext(path);
        if bytes.starts_with(b"PK") {
            let pkg = kit42::ooxml::Package::read(&bytes)?;
            if pkg.parts.contains_key("content.xml") {
                return Ok((io::read_ods(&bytes)?, ',', encoding_rs::UTF_8));
            }
            return Ok((io::read_xlsx(&bytes)?, ',', encoding_rs::UTF_8));
        }
        if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
            return Ok((io::read_xls(&bytes)?, ',', encoding_rs::UTF_8));
        }
        let (mut wb, d, enc) = io::read_csv(&bytes, &kit42::file_name(path));
        if ext == "tsv" {
            return Ok((wb, '\t', enc));
        }
        formula::recalc(&mut wb);
        Ok((wb, d, enc))
    }

    fn open(&mut self, path: PathBuf) {
        let blank = self.path.is_none() && !self.history.is_dirty() && self.wb.sheets.len() == 1 && self.wb.sheets[0].cells.is_empty();
        if !blank {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).arg(&path).spawn();
            }
            return;
        }
        match Self::load(&path) {
            Ok((wb, d, enc)) => {
                self.wb = wb;
                self.csv_delim = d;
                self.encoding = enc;
                self.path = Some(path.clone());
                self.sheet = 0;
                self.sel = Sel { active: (0, 0), other: (0, 0) };
                self.history.clear();
                settings::add_recent("xls42", &path);
                kui::remember_dir(&path);
            }
            Err(e) => kui::error(APP, &format!("{}\n{e}", t("The workbook could not be opened.", "Не удалось открыть книгу."))),
        }
    }

    fn serialize(&self, path: &std::path::Path) -> Result<Vec<u8>, String> {
        match kit42::ext(path).as_str() {
            "ods" => io::write_ods(&self.wb),
            "csv" | "txt" => Ok(io::write_csv(&self.wb, self.sheet, if self.csv_delim == '\t' { ';' } else { self.csv_delim }).into_bytes()),
            "tsv" => Ok(io::write_csv(&self.wb, self.sheet, '\t').into_bytes()),
            "pdf" => Ok(export::export(&self.wb, self.sheet, &export::Options { all_sheets: true, selection: None, landscape: None, fit_width: true, gridlines: true })),
            _ => io::write_xlsx(&self.wb),
        }
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        self.commit_edit();
        match self.serialize(&path).and_then(|b| kit42::write_atomic(&path, &b).map_err(|e| e.to_string())) {
            Ok(()) => {
                self.path = Some(path.clone());
                self.history.mark_saved();
                settings::add_recent("xls42", &path);
                self.status = format!("{} {}", t("Saved:", "Сохранено:"), kit42::file_name(&path));
                if matches!(kit42::ext(&path).as_str(), "csv" | "tsv" | "txt") && self.wb.sheets.len() > 1 {
                    self.status.push_str(t(" (CSV keeps only the active sheet)", " (CSV хранит только активный лист)"));
                }
                true
            }
            Err(e) => {
                kui::error(APP, &format!("{}\n{e}", t("Saving failed; the file was not changed.", "Не удалось сохранить; файл не изменён.")));
                false
            }
        }
    }

    fn save(&mut self) -> bool {
        match self.path.clone() {
            Some(p) if kit42::ext(&p) != "xls" => self.save_to(p),
            _ => self.save_as(),
        }
    }

    fn save_as(&mut self) -> bool {
        let stem = self.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| t("Book1", "Книга1").into());
        let Some(p) = kui::open_dialog(t("Save As", "Сохранить как"), &[("Excel (.xlsx)", &["xlsx"]), ("OpenDocument (.ods)", &["ods"]), ("CSV", &["csv"]), ("TSV", &["tsv"])]).set_file_name(format!("{stem}.xlsx")).save_file() else {
            return false;
        };
        kui::remember_dir(&p);
        self.save_to(p)
    }

    // ───────────── cell editing ─────────────

    /// Sets what a cell contains (input text) without undo/recalc.
    pub fn put(&mut self, s: usize, r: u32, c: u32, input: &str) {
        let sh = &mut self.wb.sheets[s];
        let style = sh.cells.get(&(r, c)).map(|x| x.style).unwrap_or(0);
        if input.is_empty() {
            if style == 0 {
                sh.cells.remove(&(r, c));
            } else if let Some(cell) = sh.cells.get_mut(&(r, c)) {
                cell.input.clear();
                cell.value = Value::Empty;
                cell.expr = None;
            }
            return;
        }
        let (value, auto_fmt, expr) = if input.starts_with('=') && input.len() > 1 {
            let e = formula::parse(input).ok().map(Arc::new);
            (if e.is_some() { Value::Empty } else { Value::Err("#NAME?".into()) }, None, e)
        } else {
            let (v, f) = parse_input(input);
            (v, f, None)
        };
        let mut style_id = style;
        if let Some(f) = auto_fmt {
            if self.wb.style(style).numfmt == "General" {
                let mut st = self.wb.style(style).clone();
                st.numfmt = f.into();
                style_id = self.wb.style_id(&st);
            }
        }
        self.wb.sheets[s].cells.insert((r, c), Cell { input: input.to_string(), value, style: style_id, expr });
    }

    pub fn start_edit(&mut self, initial: Option<String>) {
        let (r, c) = self.sel.active;
        let current = self.sh().cells.get(&(r, c)).map(|x| x.input.clone()).unwrap_or_default();
        self.editing = Some(initial.unwrap_or(current));
        self.edit_cell = Some((self.sheet, r, c));
        self.edit_focus_cell = true;
        self.request_editor_focus = true;
    }

    pub fn commit_edit(&mut self) -> bool {
        let (Some(text), Some((s, r, c))) = (self.editing.clone(), self.edit_cell) else { return true };
        if let Some(cell) = self.wb.sheets[s].cells.get(&(r, c)) {
            if cell.input == text {
                self.editing = None;
                self.edit_cell = None;
                return true;
            }
        } else if text.is_empty() {
            self.editing = None;
            self.edit_cell = None;
            return true;
        }
        if text.starts_with('=') && text.len() > 1 {
            if let Err(e) = formula::parse(&text) {
                // Close unbalanced parentheses like Excel does.
                let open = text.matches('(').count().saturating_sub(text.matches(')').count());
                let fixed = format!("{text}{}", ")".repeat(open));
                if open == 0 || formula::parse(&fixed).is_err() {
                    kui::error(APP, &format!("{}\n{e}", t("There is a problem with this formula.", "В формуле ошибка.")));
                    return false;
                }
                self.editing = Some(fixed);
                return self.commit_edit();
            }
        }
        // Writes into a legacy array or a merged covered cell are rejected.
        if let Some((mr, mc, _, _)) = self.wb.sheets[s].merge_at(r, c) {
            if (mr, mc) != (r, c) {
                self.editing = None;
                self.edit_cell = None;
                return true;
            }
        }
        self.checkpoint(t("Typing", "Ввод"));
        self.put(s, r, c, &text);
        self.recalc();
        self.editing = None;
        self.edit_cell = None;
        true
    }

    pub fn cancel_edit(&mut self) {
        self.editing = None;
        self.edit_cell = None;
    }

    pub fn move_sel(&mut self, dr: i64, dc: i64, extend: bool) {
        let (r, c) = if extend { self.sel.other } else { self.sel.active };
        let sh = self.sh();
        let mut nr = (r as i64 + dr).clamp(0, MAX_ROWS as i64 - 1) as u32;
        let mut nc = (c as i64 + dc).clamp(0, MAX_COLS as i64 - 1) as u32;
        // Skip hidden rows/columns.
        while sh.hidden_rows.contains(&nr) && dr != 0 && nr > 0 && nr < MAX_ROWS - 1 {
            nr = (nr as i64 + dr.signum()) as u32;
        }
        while sh.hidden_cols.contains(&nc) && dc != 0 && nc > 0 && nc < MAX_COLS - 1 {
            nc = (nc as i64 + dc.signum()) as u32;
        }
        if extend {
            self.sel.other = (nr, nc);
        } else {
            // Moving out of a merged area jumps past it.
            if let Some((r0, c0, r1, c1)) = sh.merge_at(r, c) {
                if dr > 0 && nr <= r1 {
                    nr = r1 + 1;
                }
                if dc > 0 && nc <= c1 {
                    nc = c1 + 1;
                }
                let _ = (r0, c0);
            }
            self.sel = Sel { active: (nr, nc), other: (nr, nc) };
        }
        self.scroll_to = Some(if extend { self.sel.other } else { self.sel.active });
    }

    /// Ctrl+arrow: jump to the edge of the data region.
    pub fn jump(&mut self, dr: i64, dc: i64, extend: bool) {
        let (mut r, mut c) = if extend { self.sel.other } else { self.sel.active };
        let filled = |sh: &Sheet, r: u32, c: u32| !sh.value(r, c).is_empty();
        let sh = self.sh();
        let start_filled = filled(sh, r, c);
        let next = |r: u32, c: u32| -> Option<(u32, u32)> {
            let nr = r as i64 + dr;
            let nc = c as i64 + dc;
            (nr >= 0 && nc >= 0 && nr < MAX_ROWS as i64 && nc < MAX_COLS as i64).then_some((nr as u32, nc as u32))
        };
        let limit = sh.used().map(|(ur, uc)| (ur + 1, uc + 1)).unwrap_or((0, 0));
        match next(r, c) {
            Some((nr, nc)) if start_filled && filled(sh, nr, nc) => {
                while let Some((a, b)) = next(r, c) {
                    if !filled(sh, a, b) {
                        break;
                    }
                    r = a;
                    c = b;
                }
            }
            _ => {
                while let Some((a, b)) = next(r, c) {
                    r = a;
                    c = b;
                    if filled(sh, a, b) || (dr > 0 && a > limit.0) || (dc > 0 && b > limit.1) {
                        break;
                    }
                }
            }
        }
        if extend {
            self.sel.other = (r, c);
        } else {
            self.sel = Sel { active: (r, c), other: (r, c) };
        }
        self.scroll_to = Some((r, c));
    }

    pub fn clear_selection(&mut self, contents: bool, formats: bool) {
        let (r0, c0, r1, c1) = self.sel.rect();
        self.checkpoint(t("Clear", "Очистка"));
        let keys: Vec<(u32, u32)> = self.sh().cells.range((r0, 0)..=(r1, MAX_COLS)).filter(|(k, _)| k.1 >= c0 && k.1 <= c1).map(|(k, _)| *k).collect();
        for k in keys {
            if contents && formats {
                self.sh_mut().cells.remove(&k);
            } else if contents {
                self.put(self.sheet, k.0, k.1, "");
            } else if let Some(cell) = self.sh_mut().cells.get_mut(&k) {
                cell.style = 0;
            }
        }
        self.recalc();
    }

    /// Applies a style change to every cell in the selection.
    pub fn style_sel(&mut self, label: &str, f: impl Fn(&mut Style)) {
        let (r0, c0, r1, c1) = self.sel.rect();
        if (r1 - r0 + 1) as u64 * (c1 - c0 + 1) as u64 > 100_000 {
            kui::error(APP, t("Formatting is limited to 100,000 cells per operation.", "Форматирование ограничено 100 000 ячеек за одну операцию."));
            return;
        }
        self.checkpoint(label);
        let mut cache: std::collections::HashMap<u32, u32> = Default::default();
        for r in r0..=r1 {
            for c in c0..=c1 {
                let old = self.sh().cells.get(&(r, c)).map(|x| x.style).unwrap_or(0);
                let new = *cache.entry(old).or_insert_with(|| {
                    let mut st = self.wb.style(old).clone();
                    f(&mut st);
                    self.wb.style_id(&st)
                });
                let sheet = self.sheet;
                let cell = self.wb.sheets[sheet].cells.entry((r, c)).or_insert_with(|| Cell { input: String::new(), value: Value::Empty, style: 0, expr: None });
                cell.style = new;
            }
        }
        // Rows grow to fit larger fonts (like Excel's automatic row height).
        if r1 - r0 < 5000 {
            for r in r0..=r1 {
                let need = self.sh().cells.range((r, 0)..=(r, MAX_COLS)).map(|(_, c)| self.wb.style(c.style).size() * 96.0 / 72.0 * 1.45).fold(DEF_ROW_H, f32::max);
                let cur = self.sh().row_h.get(&r).cloned().unwrap_or(DEF_ROW_H);
                if need > cur + 0.5 {
                    self.sh_mut().row_h.insert(r, need.ceil());
                }
            }
        }
    }

    pub fn active_style(&self) -> Style {
        let (r, c) = self.sel.active;
        self.sh().cells.get(&(r, c)).map(|x| self.wb.style(x.style).clone()).unwrap_or_default()
    }

    // ───────────── clipboard ─────────────

    pub fn copy(&mut self, ctx: &Context, cut: bool) {
        let (r0, c0, r1, c1) = self.sel.rect();
        if (r1 - r0 + 1) as u64 * (c1 - c0 + 1) as u64 > 100_000 {
            kui::error(APP, t("Copy is limited to 100,000 cells.", "Копирование ограничено 100 000 ячеек."));
            return;
        }
        let sh = self.sh();
        let mut cells = vec![];
        let mut text = String::new();
        for r in r0..=r1 {
            let mut row = vec![];
            let mut line = vec![];
            for c in c0..=c1 {
                let cell = sh.cells.get(&(r, c)).cloned();
                let v = sh.value(r, c);
                let fmt = cell.as_ref().map(|x| self.wb.style(x.style).numfmt.clone()).unwrap_or_default();
                let t = numfmt::format(&v, &fmt).0;
                line.push(if t.contains(['\t', '\n', '"']) { format!("\"{}\"", t.replace('"', "\"\"")) } else { t });
                row.push(cell);
            }
            cells.push(row);
            text.push_str(&line.join("\t"));
            text.push_str("\r\n");
        }
        ctx.copy_text(text.clone());
        self.clip = Some(Clip { r0, c0, cells, text, cut: cut.then_some((self.sheet, r0, c0, r1, c1)) });
        self.status = if cut { t("Cut — choose where to paste.", "Вырезано — выберите место вставки.") } else { t("Copied.", "Скопировано.") }.into();
    }

    pub fn paste(&mut self, text: &str, values_only: bool, transpose: bool) {
        let (r, c) = self.sel.active;
        let internal = self.clip.as_ref().filter(|cl| cl.text.trim_end() == text.trim_end() || text.is_empty());
        if let Some(cl) = internal {
            let rows = cl.cells.len();
            let cols = cl.cells.first().map(|x| x.len()).unwrap_or(0);
            let (sr, sc) = (cl.r0, cl.c0);
            let cells = cl.cells.clone();
            let cut = cl.cut;
            self.checkpoint(t("Paste", "Вставка"));
            if let Some((s, r0, c0, r1, c1)) = cut {
                for rr in r0..=r1 {
                    for cc in c0..=c1 {
                        self.wb.sheets[s].cells.remove(&(rr, cc));
                    }
                }
            }
            for i in 0..rows {
                for j in 0..cols {
                    let (tr, tc) = if transpose { (r + j as u32, c + i as u32) } else { (r + i as u32, c + j as u32) };
                    let sheet = self.sheet;
                    match &cells[i][j] {
                        Some(cell) => {
                            let input = if values_only {
                                match &cell.value {
                                    Value::Num(n) => fmt_general(*n),
                                    v => v.as_text(),
                                }
                            } else if cut.is_some() {
                                cell.input.clone()
                            } else {
                                formula::shift_formula(&cell.input, tr as i64 - (sr + i as u32) as i64, tc as i64 - (sc + j as u32) as i64)
                            };
                            self.put(sheet, tr, tc, &input);
                            if !values_only {
                                if let Some(x) = self.wb.sheets[sheet].cells.get_mut(&(tr, tc)) {
                                    x.style = cell.style;
                                }
                            }
                        }
                        None => {
                            if !values_only {
                                self.wb.sheets[sheet].cells.remove(&(tr, tc));
                            }
                        }
                    }
                }
            }
            if cut.is_some() {
                self.clip = None;
            }
            self.sel.other = if transpose { (r + cols as u32 - 1, c + rows as u32 - 1) } else { (r + rows as u32 - 1, c + cols as u32 - 1) };
        } else {
            // External text: tab-separated, quoted fields allowed.
            let rows = io::parse_csv(text, '\t');
            if rows.is_empty() {
                return;
            }
            self.checkpoint(t("Paste", "Вставка"));
            for (i, row) in rows.iter().enumerate() {
                for (j, v) in row.iter().enumerate() {
                    let sheet = self.sheet;
                    self.put(sheet, r + i as u32, c + j as u32, v.trim_end_matches('\r'));
                }
            }
            let w = rows.iter().map(|r| r.len()).max().unwrap_or(1);
            self.sel.other = (r + rows.len() as u32 - 1, c + w as u32 - 1);
        }
        self.recalc();
    }

    // ───────────── fill ─────────────

    /// Fills from the source block to the target rectangle (fill handle / Ctrl+D / Ctrl+R).
    pub fn fill(&mut self, src: (u32, u32, u32, u32), dst: (u32, u32, u32, u32)) {
        let (r0, c0, r1, c1) = src;
        let down = dst.2 > r1 || dst.0 < r0;
        let along: Vec<u32> = if down { (c0..=c1).collect() } else { (r0..=r1).collect() };
        self.checkpoint(t("Fill", "Заполнение"));
        for line in along {
            let srcs: Vec<(u32, u32)> = if down { (r0..=r1).map(|r| (r, line)).collect() } else { (c0..=c1).map(|c| (line, c)).collect() };
            let inputs: Vec<Option<Cell>> = srcs.iter().map(|k| self.sh().cells.get(k).cloned()).collect();
            let nums: Vec<Option<f64>> = inputs.iter().map(|c| c.as_ref().filter(|x| !x.is_formula()).and_then(|x| if let Value::Num(n) = x.value { Some(n) } else { None })).collect();
            let all_num = nums.iter().all(|n| n.is_some()) && !nums.is_empty();
            let step = if all_num && nums.len() >= 2 { (nums[nums.len() - 1].unwrap() - nums[0].unwrap()) / (nums.len() - 1) as f64 } else { 0.0 };
            let targets: Vec<(u32, u32)> = if down {
                if dst.2 > r1 { (r1 + 1..=dst.2).map(|r| (r, line)).collect() } else { (dst.0..r0).rev().map(|r| (r, line)).collect() }
            } else if dst.3 > c1 {
                (c1 + 1..=dst.3).map(|c| (line, c)).collect()
            } else {
                (dst.1..c0).rev().map(|c| (line, c)).collect()
            };
            let forward = if down { dst.2 > r1 } else { dst.3 > c1 };
            for (k, (tr, tc)) in targets.into_iter().enumerate() {
                let idx = if forward { k % inputs.len() } else { inputs.len() - 1 - (k % inputs.len()) };
                let Some(src_cell) = inputs[idx].clone() else {
                    self.sh_mut().cells.remove(&(tr, tc));
                    continue;
                };
                let (sr, sc) = srcs[idx];
                let input = if src_cell.is_formula() {
                    formula::shift_formula(&src_cell.input, tr as i64 - sr as i64, tc as i64 - sc as i64)
                } else if all_num && step != 0.0 {
                    let base = if forward { nums[nums.len() - 1].unwrap() } else { nums[0].unwrap() };
                    let v = base + step * (k as f64 + 1.0) * if forward { 1.0 } else { -1.0 };
                    fmt_general(v)
                } else if let Some((prefix, n)) = split_trailing_number(&src_cell.input) {
                    // "Item 1" → "Item 2"
                    format!("{prefix}{}", n + (k as i64 + 1) * if forward { 1 } else { -1 })
                } else {
                    src_cell.input.clone()
                };
                let sheet = self.sheet;
                self.put(sheet, tr, tc, &input);
                if let Some(x) = self.wb.sheets[sheet].cells.get_mut(&(tr, tc)) {
                    x.style = src_cell.style;
                }
            }
        }
        self.recalc();
    }

    // ───────────── rows / columns ─────────────

    pub fn insert_delete(&mut self, rows: bool, insert: bool) {
        let (r0, c0, r1, c1) = self.sel.rect();
        let (at, count) = if rows { (r0, (r1 - r0 + 1) as i64) } else { (c0, (c1 - c0 + 1) as i64) };
        let count = if insert { count } else { -count };
        self.checkpoint(if rows { t("Rows", "Строки") } else { t("Columns", "Столбцы") });
        let on = self.sh().name.clone();
        // Adjust formulas everywhere.
        for sh in &mut self.wb.sheets {
            let own = sh.name.clone();
            for cell in sh.cells.values_mut() {
                if cell.is_formula() {
                    let nf = formula::adjust_formula(&cell.input, &own, &on, rows, at, count);
                    if nf != cell.input {
                        cell.input = nf;
                        cell.expr = None;
                    }
                }
            }
        }
        let sh = &mut self.wb.sheets[self.sheet];
        let shift = |p: u32| -> Option<u32> {
            if count > 0 {
                Some(if p >= at { p + count as u32 } else { p })
            } else {
                let del_end = at + (-count) as u32;
                if p < at {
                    Some(p)
                } else if p >= del_end {
                    Some(p - (-count) as u32)
                } else {
                    None
                }
            }
        };
        let old = std::mem::take(&mut sh.cells);
        for ((r, c), cell) in old {
            let k = if rows { shift(r).map(|nr| (nr, c)) } else { shift(c).map(|nc| (r, nc)) };
            if let Some(k) = k {
                sh.cells.insert(k, cell);
            }
        }
        if rows {
            sh.row_h = std::mem::take(&mut sh.row_h).into_iter().filter_map(|(r, h)| shift(r).map(|n| (n, h))).collect();
            sh.hidden_rows = std::mem::take(&mut sh.hidden_rows).into_iter().filter_map(shift).collect();
        } else {
            sh.col_w = std::mem::take(&mut sh.col_w).into_iter().filter_map(|(c, w)| shift(c).map(|n| (n, w))).collect();
            sh.hidden_cols = std::mem::take(&mut sh.hidden_cols).into_iter().filter_map(shift).collect();
        }
        sh.merges = sh
            .merges
            .iter()
            .filter_map(|&(a, b, c, d)| if rows { Some((shift(a)?, b, shift(c)?, d)) } else { Some((a, shift(b)?, c, shift(d)?)) })
            .collect();
        for ch in &mut sh.charts {
            if rows {
                if let (Some(a), Some(b)) = (shift(ch.range.0), shift(ch.range.2)) {
                    ch.range.0 = a;
                    ch.range.2 = b;
                }
                ch.anchor.0 = shift(ch.anchor.0).unwrap_or(at);
            } else {
                if let (Some(a), Some(b)) = (shift(ch.range.1), shift(ch.range.3)) {
                    ch.range.1 = a;
                    ch.range.3 = b;
                }
                ch.anchor.1 = shift(ch.anchor.1).unwrap_or(at);
            }
        }
        self.recalc();
    }

    pub fn sort(&mut self, col: u32, desc: bool, header: bool) {
        let (mut r0, mut c0, mut r1, mut c1) = self.sel.rect();
        if self.sel.single() {
            // Current region.
            let (ur, uc) = self.sh().used().unwrap_or((0, 0));
            r0 = 0;
            c0 = 0;
            r1 = ur;
            c1 = uc;
        }
        let start = if header { r0 + 1 } else { r0 };
        if start > r1 {
            return;
        }
        self.checkpoint(t("Sort", "Сортировка"));
        let sh = &self.wb.sheets[self.sheet];
        let mut rows: Vec<(u32, Vec<Option<Cell>>, Value)> = (start..=r1)
            .map(|r| ((r), (c0..=c1).map(|c| sh.cells.get(&(r, c)).cloned()).collect(), sh.value(r, col)))
            .collect();
        rows.sort_by(|a, b| {
            // Empty cells always last.
            match (a.2.is_empty(), b.2.is_empty()) {
                (true, false) => return std::cmp::Ordering::Greater,
                (false, true) => return std::cmp::Ordering::Less,
                _ => {}
            }
            let o = natural_cmp(&a.2, &b.2);
            if desc {
                o.reverse()
            } else {
                o
            }
        });
        let sheet = self.sheet;
        for (i, (old_r, cells, _)) in rows.into_iter().enumerate() {
            let r = start + i as u32;
            for (j, cell) in cells.into_iter().enumerate() {
                let c = c0 + j as u32;
                match cell {
                    Some(mut cell) => {
                        if cell.is_formula() {
                            cell.input = formula::shift_formula(&cell.input, r as i64 - old_r as i64, 0);
                            cell.expr = None;
                        }
                        self.wb.sheets[sheet].cells.insert((r, c), cell);
                    }
                    None => {
                        self.wb.sheets[sheet].cells.remove(&(r, c));
                    }
                }
            }
        }
        self.recalc();
    }

    pub fn autosum(&mut self) {
        let (r, c) = self.sel.active;
        let sh = self.sh();
        // Sum the numbers above, else to the left.
        let mut top = r;
        while top > 0 && matches!(sh.value(top - 1, c), Value::Num(_)) {
            top -= 1;
        }
        let f = if top < r {
            format!("=SUM({}:{})", addr(top, c), addr(r - 1, c))
        } else {
            let mut left = c;
            while left > 0 && matches!(sh.value(r, left - 1), Value::Num(_)) {
                left -= 1;
            }
            if left < c {
                format!("=SUM({}:{})", addr(r, left), addr(r, c - 1))
            } else {
                "=SUM()".into()
            }
        };
        self.start_edit(Some(f));
    }

    fn sel_stats(&self) -> String {
        let (r0, c0, r1, c1) = self.sel.rect();
        if self.sel.single() {
            return String::new();
        }
        let sh = self.sh();
        let mut sum = 0.0;
        let mut n = 0;
        let mut cnt = 0;
        for (&(r, c), _) in sh.cells.range((r0, 0)..=(r1, MAX_COLS)) {
            if c < c0 || c > c1 {
                continue;
            }
            let v = sh.value(r, c);
            if !v.is_empty() {
                cnt += 1;
            }
            if let Value::Num(x) = v {
                sum += x;
                n += 1;
            }
        }
        if n == 0 {
            return format!("{} {cnt}", t("Count:", "Количество:"));
        }
        format!("{} {} · {} {} · {} {cnt}", t("Sum:", "Сумма:"), fmt_general(sum), t("Average:", "Среднее:"), fmt_general(sum / n as f64), t("Count:", "Количество:"))
    }

    pub fn find_next(&mut self, find: &str, formulas: bool, case: bool, whole: bool) -> bool {
        if find.is_empty() {
            return false;
        }
        let needle = if case { find.to_string() } else { find.to_lowercase() };
        let sh = self.sh();
        let keys: Vec<(u32, u32)> = sh.cells.keys().cloned().collect();
        let start = keys.iter().position(|k| *k > self.sel.active).unwrap_or(0);
        for i in 0..keys.len() {
            let k = keys[(start + i) % keys.len()];
            let cell = &sh.cells[&k];
            let hay = if formulas { cell.input.clone() } else { cell.value.as_text() };
            let hay = if case { hay } else { hay.to_lowercase() };
            if (whole && hay == needle) || (!whole && hay.contains(&needle)) {
                self.sel = Sel { active: k, other: k };
                self.scroll_to = Some(k);
                return true;
            }
        }
        self.status = t("Not found.", "Не найдено.").into();
        false
    }

    pub fn replace_all(&mut self, find: &str, rep: &str, case: bool, whole: bool) -> usize {
        if find.is_empty() {
            return 0;
        }
        self.checkpoint(t("Replace All", "Заменить все"));
        let mut n = 0;
        let sheet = self.sheet;
        let keys: Vec<(u32, u32)> = self.wb.sheets[sheet].cells.keys().cloned().collect();
        for k in keys {
            let input = self.wb.sheets[sheet].cells[&k].input.clone();
            let new = if whole {
                if (case && input == find) || (!case && input.to_lowercase() == find.to_lowercase()) {
                    rep.to_string()
                } else {
                    continue;
                }
            } else if case {
                if !input.contains(find) {
                    continue;
                }
                input.replace(find, rep)
            } else {
                let lower = input.to_lowercase();
                if !lower.contains(&find.to_lowercase()) {
                    continue;
                }
                let mut out = String::new();
                let mut rest = input.as_str();
                let fl = find.to_lowercase();
                while let Some(i) = rest.to_lowercase().find(&fl) {
                    out.push_str(&rest[..i]);
                    out.push_str(rep);
                    rest = &rest[i + find.len()..];
                }
                out.push_str(rest);
                out
            };
            self.put(sheet, k.0, k.1, &new);
            n += 1;
        }
        self.recalc();
        n
    }
}

pub fn split_trailing_number(s: &str) -> Option<(String, i64)> {
    if s.starts_with('=') {
        return None;
    }
    let digits: String = s.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    if digits.is_empty() || digits.len() == s.len() {
        return None;
    }
    Some((s[..s.len() - digits.len()].to_string(), digits.parse().ok()?))
}

/// Natural order: "Item 2" < "Item 10".
pub fn natural_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    if let (Value::Str(x), Value::Str(y)) = (a, b) {
        if let (Some((px, nx)), Some((py, ny))) = (split_trailing_number(x), split_trailing_number(y)) {
            if px.to_lowercase() == py.to_lowercase() {
                return nx.cmp(&ny);
            }
        }
    }
    formula::compare(a, b)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        kui::set_title(ctx, APP, Some(&self.name()), self.history.is_dirty());
        if let Some(p) = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).next()) {
            self.open(p);
        }
        self.keyboard(ctx);
        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu(ui));
        egui::TopBottomPanel::top("ribbon").frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(egui::Margin::symmetric(6, 3))).show(ctx, |ui| self.ribbon(ui));
        egui::TopBottomPanel::top("fbar").frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(egui::Margin::symmetric(6, 3))).show(ctx, |ui| self.formula_bar(ui));
        let stats = self.sel_stats();
        let mut zoom = self.zoom;
        kui::status_bar(ctx, |ui| {
            ui.label(if self.editing.is_some() { t("Edit", "Правка") } else { t("Ready", "Готово") });
            if !self.status.is_empty() {
                ui.label(format!("· {}", self.status));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut zoom, 0.4..=3.0).show_value(false));
                ui.label(format!("{:.0}%", zoom * 100.0));
                ui.separator();
                ui.label(stats);
            });
        });
        if zoom != self.zoom {
            self.zoom = zoom;
            settings::set("xls42.zoom", zoom);
        }
        egui::TopBottomPanel::bottom("tabs").exact_height(30.0).show(ctx, |ui| self.sheet_tabs(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(ctx.style().visuals.extreme_bg_color)).show(ctx, |ui| self.grid(ui));
        self.dialogs(ctx);
        if let Some(img) = self.picker.show(ctx) {
            self.status = format!("{} {}×{}", t("Signatures are inserted in DOC42/PDF42; image size", "Подписи вставляются в DOC42/PDF42; размер"), img.width(), img.height());
        }
        let dirty = self.history.is_dirty();
        let name = self.name();
        if ctx.input(|i| i.viewport().close_requested()) && dirty {
            match kui::ask_save(&name) {
                kui::SaveChoice::Save => {
                    if !self.save() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    }
                }
                kui::SaveChoice::Discard => {}
                kui::SaveChoice::Cancel => ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose),
            }
        }
    }
}

impl App {
    fn keyboard(&mut self, ctx: &Context) {
        if self.dialog.is_some() && !matches!(self.dialog, Some(Dialog::Find { .. })) {
            return;
        }
        if kui::shortcut(ctx, CTRL, Key::S) {
            self.save();
        }
        if kui::shortcut(ctx, CTRL_SHIFT, Key::S) {
            self.save_as();
        }
        if kui::shortcut(ctx, CTRL, Key::O) {
            self.open_dialog();
        }
        if kui::shortcut(ctx, CTRL, Key::N) {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).spawn();
            }
        }
        if kui::shortcut(ctx, CTRL, Key::P) {
            self.print();
        }
        if kui::shortcut(ctx, CTRL, Key::F) || kui::shortcut(ctx, CTRL, Key::H) {
            self.dialog = Some(Dialog::Find { find: String::new(), replace: String::new(), formulas: false, case: false, whole: false });
        }
        if self.editing.is_some() {
            // Keys while editing: Enter/Tab commit, Esc cancels (handled by the editor widgets).
            return;
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        if kui::shortcut(ctx, CTRL, Key::Z) {
            if let Some(w) = self.history.undo(self.wb.clone()) {
                self.wb = w;
                self.sheet = self.sheet.min(self.wb.sheets.len() - 1);
            }
        }
        if kui::shortcut(ctx, CTRL, Key::Y) || kui::shortcut(ctx, CTRL_SHIFT, Key::Z) {
            if let Some(w) = self.history.redo(self.wb.clone()) {
                self.wb = w;
                self.sheet = self.sheet.min(self.wb.sheets.len() - 1);
            }
        }
        if kui::shortcut(ctx, CTRL, Key::B) {
            let v = !self.active_style().bold;
            self.style_sel(t("Bold", "Полужирный"), |s| s.bold = v);
        }
        if kui::shortcut(ctx, CTRL, Key::I) {
            let v = !self.active_style().italic;
            self.style_sel(t("Italic", "Курсив"), |s| s.italic = v);
        }
        if kui::shortcut(ctx, CTRL, Key::U) {
            let v = !self.active_style().underline;
            self.style_sel(t("Underline", "Подчёркивание"), |s| s.underline = v);
        }
        if kui::shortcut(ctx, CTRL, Key::A) {
            let (ur, uc) = self.sh().used().unwrap_or((0, 0));
            self.sel = Sel { active: (0, 0), other: (ur, uc) };
        }
        if kui::shortcut(ctx, CTRL, Key::D) {
            let (r0, c0, r1, c1) = self.sel.rect();
            if r1 > r0 {
                self.fill((r0, c0, r0, c1), (r0, c0, r1, c1));
            }
        }
        if kui::shortcut(ctx, CTRL, Key::R) {
            let (r0, c0, r1, c1) = self.sel.rect();
            if c1 > c0 {
                self.fill((r0, c0, r1, c0), (r0, c0, r1, c1));
            }
        }
        if kui::shortcut(ctx, CTRL, Key::G) {
            self.dialog = Some(Dialog::GoTo(String::new()));
        }
        if kui::shortcut(ctx, NONE, Key::F2) {
            self.start_edit(None);
        }
        if kui::shortcut(ctx, NONE, Key::Delete) || kui::shortcut(ctx, NONE, Key::Backspace) {
            if let Some(id) = self.selected_chart {
                self.checkpoint(t("Delete Chart", "Удаление диаграммы"));
                self.sh_mut().charts.retain(|c| c.id != id);
                self.selected_chart = None;
            } else {
                self.clear_selection(true, false);
            }
        }
        let shift = ctx.input(|i| i.modifiers.shift);
        let ctrl = ctx.input(|i| i.modifiers.command);
        for (k, dr, dc) in [(Key::ArrowDown, 1, 0), (Key::ArrowUp, -1, 0), (Key::ArrowRight, 0, 1), (Key::ArrowLeft, 0, -1)] {
            if ctx.input(|i| i.key_pressed(k)) {
                if ctrl {
                    self.jump(dr, dc, shift);
                } else {
                    self.move_sel(dr, dc, shift);
                }
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Enter)) {
            self.move_sel(if shift { -1 } else { 1 }, 0, false);
        }
        if ctx.input(|i| i.key_pressed(Key::Tab)) {
            self.move_sel(0, if shift { -1 } else { 1 }, false);
        }
        if ctx.input(|i| i.key_pressed(Key::PageDown)) {
            self.move_sel(30, 0, shift);
        }
        if ctx.input(|i| i.key_pressed(Key::PageUp)) {
            self.move_sel(-30, 0, shift);
        }
        if ctx.input(|i| i.key_pressed(Key::Home)) {
            let r = self.sel.active.0;
            self.sel = Sel { active: (if ctrl { 0 } else { r }, 0), other: (if ctrl { 0 } else { r }, 0) };
            self.scroll_to = Some(self.sel.active);
        }
        if ctx.input(|i| i.key_pressed(Key::End)) && ctrl {
            let (ur, uc) = self.sh().used().unwrap_or((0, 0));
            self.sel = Sel { active: (ur, uc), other: (ur, uc) };
            self.scroll_to = Some((ur, uc));
        }
        // Clipboard and typing.
        let events = ctx.input(|i| i.events.clone());
        for e in events {
            match e {
                egui::Event::Copy => self.copy(ctx, false),
                egui::Event::Cut => self.copy(ctx, true),
                egui::Event::Paste(text) => self.paste(&text, false, false),
                egui::Event::Text(s) if !ctrl => {
                    if s == "=" || s == "+" {
                        self.start_edit(Some("=".into()));
                    } else {
                        self.start_edit(Some(s));
                    }
                    break;
                }
                _ => {}
            }
        }
    }

    fn open_dialog(&mut self) {
        if let Some(p) = kui::open_dialog(t("Open", "Открыть"), &[
            (t("Spreadsheets", "Таблицы"), &["xlsx", "xlsm", "xls", "ods", "csv", "tsv", "txt"]),
            ("Excel", &["xlsx", "xlsm", "xls"]),
            ("OpenDocument", &["ods"]),
            ("CSV / TSV", &["csv", "tsv", "txt"]),
        ])
        .pick_file()
        {
            self.open(p);
        }
    }

    fn print(&mut self) {
        self.commit_edit();
        let b = export::export(&self.wb, self.sheet, &export::Options { all_sheets: false, selection: None, landscape: None, fit_width: true, gridlines: true });
        let tmp = kit42::shell::temp_file("print", "pdf");
        if std::fs::write(&tmp, b).is_ok() {
            kit42::shell::print(&tmp);
        }
    }

    fn menu(&mut self, ui: &mut egui::Ui) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button(t("File", "Файл"), |ui| {
                if kui::menu_item(ui, t("New", "Создать"), "Ctrl+N") {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).spawn();
                    }
                }
                if kui::menu_item(ui, t("Open…", "Открыть…"), "Ctrl+O") {
                    self.open_dialog();
                }
                ui.menu_button(t("Open Recent", "Последние файлы"), |ui| {
                    for p in settings::recent("xls42") {
                        if ui.button(kit42::file_name(&p)).clicked() {
                            self.open(p);
                            ui.close_menu();
                        }
                    }
                });
                if kui::menu_item(ui, t("Import CSV as Sheet…", "Импорт CSV на новый лист…"), "") {
                    if let Some(p) = kui::open_dialog(t("Import", "Импорт"), &[("CSV / TSV", &["csv", "tsv", "txt"])]).pick_file() {
                        if let Ok(b) = std::fs::read(&p) {
                            let (mut w, ..) = io::read_csv(&b, &kit42::file_name(&p));
                            self.checkpoint(t("Import", "Импорт"));
                            let mut sh = w.sheets.remove(0);
                            // Restyle into this workbook's style table.
                            for cell in sh.cells.values_mut() {
                                let st = w.styles[cell.style as usize].clone();
                                cell.style = self.wb.style_id(&st);
                            }
                            self.wb.sheets.push(sh);
                            self.sheet = self.wb.sheets.len() - 1;
                            self.recalc();
                        }
                    }
                }
                ui.separator();
                if kui::menu_item(ui, t("Save", "Сохранить"), "Ctrl+S") {
                    self.save();
                }
                if kui::menu_item(ui, t("Save As…", "Сохранить как…"), "Ctrl+Shift+S") {
                    self.save_as();
                }
                if kui::menu_item(ui, t("Export as PDF…", "Экспорт в PDF…"), "") {
                    self.dialog = Some(Dialog::Export { all: false, selection: false, landscape: 0, fit: true, grid: true });
                }
                if kui::menu_item(ui, t("Print…", "Печать…"), "Ctrl+P") {
                    self.print();
                }
                ui.separator();
                if kui::menu_item(ui, t("Exit", "Выход"), "Alt+F4") {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button(t("Edit", "Правка"), |ui| {
                if kui::menu_item_enabled(ui, self.history.can_undo(), t("Undo", "Отменить"), "Ctrl+Z") {
                    if let Some(w) = self.history.undo(self.wb.clone()) {
                        self.wb = w;
                        self.sheet = self.sheet.min(self.wb.sheets.len() - 1);
                    }
                }
                if kui::menu_item_enabled(ui, self.history.can_redo(), t("Redo", "Повторить"), "Ctrl+Y") {
                    if let Some(w) = self.history.redo(self.wb.clone()) {
                        self.wb = w;
                        self.sheet = self.sheet.min(self.wb.sheets.len() - 1);
                    }
                }
                ui.separator();
                if kui::menu_item(ui, t("Cut", "Вырезать"), "Ctrl+X") {
                    self.copy(ui.ctx(), true);
                }
                if kui::menu_item(ui, t("Copy", "Копировать"), "Ctrl+C") {
                    self.copy(ui.ctx(), false);
                }
                if kui::menu_item(ui, t("Paste", "Вставить"), "Ctrl+V") {
                    let text = self.clip.as_ref().map(|c| c.text.clone()).unwrap_or_default();
                    self.paste(&text, false, false);
                }
                if kui::menu_item_enabled(ui, self.clip.is_some(), t("Paste Values", "Вставить значения"), "") {
                    let text = self.clip.as_ref().map(|c| c.text.clone()).unwrap_or_default();
                    self.paste(&text, true, false);
                }
                if kui::menu_item_enabled(ui, self.clip.is_some(), t("Paste Transposed", "Вставить с транспонированием"), "") {
                    let text = self.clip.as_ref().map(|c| c.text.clone()).unwrap_or_default();
                    self.paste(&text, false, true);
                }
                ui.separator();
                if kui::menu_item(ui, t("Fill Down", "Заполнить вниз"), "Ctrl+D") {
                    let (r0, c0, r1, c1) = self.sel.rect();
                    if r1 > r0 {
                        self.fill((r0, c0, r0, c1), (r0, c0, r1, c1));
                    }
                }
                if kui::menu_item(ui, t("Fill Right", "Заполнить вправо"), "Ctrl+R") {
                    let (r0, c0, r1, c1) = self.sel.rect();
                    if c1 > c0 {
                        self.fill((r0, c0, r1, c0), (r0, c0, r1, c1));
                    }
                }
                if kui::menu_item(ui, t("Clear Contents", "Очистить содержимое"), "Del") {
                    self.clear_selection(true, false);
                }
                if kui::menu_item(ui, t("Clear Formats", "Очистить форматы"), "") {
                    self.clear_selection(false, true);
                }
                if kui::menu_item(ui, t("Clear All", "Очистить всё"), "") {
                    self.clear_selection(true, true);
                }
                ui.separator();
                if kui::menu_item(ui, t("Find and Replace…", "Найти и заменить…"), "Ctrl+F") {
                    self.dialog = Some(Dialog::Find { find: String::new(), replace: String::new(), formulas: false, case: false, whole: false });
                }
                if kui::menu_item(ui, t("Go To…", "Перейти…"), "Ctrl+G") {
                    self.dialog = Some(Dialog::GoTo(String::new()));
                }
            });
            ui.menu_button(t("View", "Вид"), |ui| {
                let (r, c) = self.sel.active;
                if kui::menu_item(ui, t("Freeze Panes", "Закрепить области"), "") {
                    self.checkpoint(t("Freeze", "Закрепление"));
                    self.sh_mut().freeze = (r, c);
                }
                if kui::menu_item(ui, t("Freeze Top Row", "Закрепить верхнюю строку"), "") {
                    self.checkpoint(t("Freeze", "Закрепление"));
                    self.sh_mut().freeze = (1, 0);
                }
                if kui::menu_item(ui, t("Freeze First Column", "Закрепить первый столбец"), "") {
                    self.checkpoint(t("Freeze", "Закрепление"));
                    self.sh_mut().freeze = (0, 1);
                }
                if kui::menu_item(ui, t("Unfreeze Panes", "Снять закрепление"), "") {
                    self.checkpoint(t("Freeze", "Закрепление"));
                    self.sh_mut().freeze = (0, 0);
                }
                ui.separator();
                let mut g = self.sh().gridlines;
                if ui.checkbox(&mut g, t("Gridlines", "Сетка")).changed() {
                    self.sh_mut().gridlines = g;
                }
                if kui::menu_item(ui, t("Full Screen", "Во весь экран"), "F11") {
                    let fs = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
                }
            });
            ui.menu_button(t("Insert", "Вставка"), |ui| {
                if kui::menu_item(ui, t("Rows", "Строки"), "") {
                    self.insert_delete(true, true);
                }
                if kui::menu_item(ui, t("Columns", "Столбцы"), "") {
                    self.insert_delete(false, true);
                }
                if kui::menu_item(ui, t("Chart…", "Диаграмма…"), "") {
                    self.dialog = Some(Dialog::Chart { kind: ChartKind::Column, title: String::new(), legend: true, edit: None });
                }
                if kui::menu_item(ui, t("Function…", "Функция…"), "Shift+F3") {
                    self.dialog = Some(Dialog::Functions(String::new()));
                }
                if kui::menu_item(ui, t("Sheet", "Лист"), "") {
                    self.add_sheet();
                }
            });
            ui.menu_button(t("Format", "Формат"), |ui| {
                if kui::menu_item(ui, t("Cells…", "Ячейки…"), "Ctrl+1") {
                    let st = self.active_style();
                    self.dialog = Some(Dialog::Format { tab: 0, custom: st.numfmt.clone(), style: st });
                }
                if kui::menu_item(ui, t("Merge and Centre", "Объединить и поместить в центре"), "") {
                    self.merge();
                }
                if kui::menu_item(ui, t("Unmerge", "Разъединить"), "") {
                    let (r0, c0, r1, c1) = self.sel.rect();
                    self.checkpoint(t("Unmerge", "Разъединение"));
                    self.sh_mut().merges.retain(|m| !(m.0 >= r0 && m.2 <= r1 && m.1 >= c0 && m.3 <= c1) && !(m.0 <= r0 && m.2 >= r0 && m.1 <= c0 && m.3 >= c0));
                }
                ui.separator();
                if kui::menu_item(ui, t("Autofit Column Width", "Автоподбор ширины"), "") {
                    let (_, c0, _, c1) = self.sel.rect();
                    self.checkpoint(t("Column Width", "Ширина столбца"));
                    for c in c0..=c1 {
                        self.autofit(ui.ctx(), c);
                    }
                }
                if kui::menu_item(ui, t("Hide Rows", "Скрыть строки"), "") {
                    let (r0, _, r1, _) = self.sel.rect();
                    self.checkpoint(t("Hide", "Скрытие"));
                    for r in r0..=r1 {
                        self.sh_mut().hidden_rows.insert(r);
                    }
                }
                if kui::menu_item(ui, t("Hide Columns", "Скрыть столбцы"), "") {
                    let (_, c0, _, c1) = self.sel.rect();
                    self.checkpoint(t("Hide", "Скрытие"));
                    for c in c0..=c1 {
                        self.sh_mut().hidden_cols.insert(c);
                    }
                }
                if kui::menu_item(ui, t("Unhide All", "Показать все"), "") {
                    self.checkpoint(t("Unhide", "Показ"));
                    self.sh_mut().hidden_rows.clear();
                    self.sh_mut().hidden_cols.clear();
                }
            });
            ui.menu_button(t("Data", "Данные"), |ui| {
                let col = self.sel.active.1;
                if kui::menu_item(ui, t("Sort Ascending", "Сортировка по возрастанию"), "") {
                    let header = self.guess_header();
                    self.sort(col, false, header);
                }
                if kui::menu_item(ui, t("Sort Descending", "Сортировка по убыванию"), "") {
                    let header = self.guess_header();
                    self.sort(col, true, header);
                }
                if kui::menu_item(ui, t("Sort…", "Сортировка…"), "") {
                    self.dialog = Some(Dialog::Sort { col, desc: false, header: self.guess_header() });
                }
                ui.separator();
                if kui::menu_item(ui, t("AutoFilter", "Автофильтр"), "") {
                    self.toggle_filter();
                }
                if kui::menu_item(ui, t("Reset Filter", "Сбросить фильтр"), "") {
                    self.checkpoint(t("Filter", "Фильтр"));
                    self.sh_mut().hidden_rows.clear();
                }
                if kui::menu_item(ui, t("Remove Duplicates", "Удалить дубликаты"), "") {
                    self.remove_duplicates();
                }
                ui.separator();
                if kui::menu_item(ui, t("Delete Rows", "Удалить строки"), "") {
                    self.insert_delete(true, false);
                }
                if kui::menu_item(ui, t("Delete Columns", "Удалить столбцы"), "") {
                    self.insert_delete(false, false);
                }
                if kui::menu_item(ui, t("Recalculate", "Пересчитать"), "F9") {
                    self.recalc();
                }
            });
            ui.menu_button(t("Help", "Справка"), |ui| {
                kui::language_menu(ui);
                if kui::menu_item(ui, t("Supported Functions…", "Поддерживаемые функции…"), "") {
                    self.dialog = Some(Dialog::Functions(String::new()));
                }
                if kui::menu_item(ui, t("About XLS42", "О программе XLS42"), "") {
                    kui::about(APP, t("spreadsheets, formulas and charts", "таблицы, формулы и диаграммы"));
                }
            });
        });
    }

    fn guess_header(&self) -> bool {
        let sh = self.sh();
        let (r0, c0, _, c1) = if self.sel.single() { (0, 0, 0, sh.used().map(|u| u.1).unwrap_or(0)) } else { self.sel.rect() };
        (c0..=c1).any(|c| matches!(sh.value(r0, c), Value::Str(_))) && (c0..=c1).any(|c| matches!(sh.value(r0 + 1, c), Value::Num(_)))
    }

    pub fn merge(&mut self) {
        let (r0, c0, r1, c1) = self.sel.rect();
        if r0 == r1 && c0 == c1 {
            return;
        }
        let others = (r0..=r1).flat_map(|r| (c0..=c1).map(move |c| (r, c))).filter(|&(r, c)| (r, c) != (r0, c0) && !self.sh().value(r, c).is_empty()).count();
        if others > 0 && !kui::confirm(APP, t("Merging keeps only the upper-left value. Continue?", "При объединении сохранится только значение левой верхней ячейки. Продолжить?")) {
            return;
        }
        self.checkpoint(t("Merge", "Объединение"));
        let sheet = self.sheet;
        for r in r0..=r1 {
            for c in c0..=c1 {
                if (r, c) != (r0, c0) {
                    self.put(sheet, r, c, "");
                }
            }
        }
        self.sh_mut().merges.retain(|m| m.2 < r0 || m.0 > r1 || m.3 < c0 || m.1 > c1);
        self.sh_mut().merges.push((r0, c0, r1, c1));
        self.style_sel(t("Merge", "Объединение"), |s| s.halign = HAlign::Center);
        self.recalc();
    }

    pub fn toggle_filter(&mut self) {
        self.checkpoint(t("Filter", "Фильтр"));
        if self.sh().filter.is_some() {
            self.sh_mut().filter = None;
            self.sh_mut().hidden_rows.clear();
            return;
        }
        let (r0, c0, r1, c1) = if self.sel.single() {
            let (ur, uc) = self.sh().used().unwrap_or((0, 0));
            (0, 0, ur, uc)
        } else {
            self.sel.rect()
        };
        self.sh_mut().filter = Some((r0, c0, r1, c1));
    }

    fn remove_duplicates(&mut self) {
        let (r0, c0, r1, c1) = if self.sel.single() {
            let (ur, uc) = self.sh().used().unwrap_or((0, 0));
            (0, 0, ur, uc)
        } else {
            self.sel.rect()
        };
        let header = self.guess_header();
        let start = if header { r0 + 1 } else { r0 };
        let sh = self.sh();
        let mut seen = std::collections::HashSet::new();
        let mut keep = vec![];
        for r in start..=r1 {
            let key: Vec<String> = (c0..=c1).map(|c| sh.value(r, c).as_text().to_lowercase()).collect();
            if seen.insert(key) {
                keep.push(r);
            }
        }
        let removed = (r1 - start + 1) as usize - keep.len();
        if removed == 0 {
            self.status = t("No duplicates found.", "Дубликаты не найдены.").into();
            return;
        }
        self.checkpoint(t("Remove Duplicates", "Удаление дубликатов"));
        let sheet = self.sheet;
        let rows: Vec<Vec<Option<Cell>>> = keep.iter().map(|r| (c0..=c1).map(|c| self.wb.sheets[sheet].cells.get(&(*r, c)).cloned()).collect()).collect();
        for r in start..=r1 {
            for c in c0..=c1 {
                self.wb.sheets[sheet].cells.remove(&(r, c));
            }
        }
        for (i, row) in rows.into_iter().enumerate() {
            for (j, cell) in row.into_iter().enumerate() {
                if let Some(cell) = cell {
                    self.wb.sheets[sheet].cells.insert((start + i as u32, c0 + j as u32), cell);
                }
            }
        }
        self.recalc();
        self.status = format!("{} {removed}", t("Duplicates removed:", "Удалено дубликатов:"));
    }

    pub fn autofit(&mut self, ctx: &Context, c: u32) {
        let sh = self.sh();
        let mut w: f32 = 30.0;
        for (&(_, cc), cell) in sh.cells.iter().filter(|(k, _)| k.1 == c) {
            let st = self.wb.style(cell.style);
            let text = numfmt::format(&cell.value, &st.numfmt).0;
            let gw = ctx.fonts(|f| f.layout_no_wrap(text, egui::FontId::proportional(st.size() * 96.0 / 72.0), egui::Color32::BLACK).size().x);
            w = w.max(gw + 10.0);
            let _ = cc;
        }
        self.sh_mut().col_w.insert(c, w.min(600.0));
    }

    pub fn add_sheet(&mut self) {
        self.checkpoint(t("Insert Sheet", "Вставка листа"));
        let mut n = self.wb.sheets.len() + 1;
        let base = t("Sheet", "Лист");
        while self.wb.sheet_index(&format!("{base}{n}")).is_some() {
            n += 1;
        }
        self.wb.sheets.push(Sheet::new(&format!("{base}{n}")));
        self.sheet = self.wb.sheets.len() - 1;
        self.sel = Sel { active: (0, 0), other: (0, 0) };
    }

    fn ribbon(&mut self, ui: &mut egui::Ui) {
        let st = self.active_style();
        ui.horizontal_wrapped(|ui| {
            if kui::tool_button(ui, ic::OPEN, t("Open", "Открыть"), true).clicked() {
                self.open_dialog();
            }
            if kui::tool_button(ui, ic::SAVE, t("Save", "Сохранить"), true).clicked() {
                self.save();
            }
            if kui::tool_button(ui, ic::PRINT, t("Print", "Печать"), true).clicked() {
                self.print();
            }
            ui.separator();
            if kui::tool_button(ui, ic::UNDO, t("Undo", "Отменить"), self.history.can_undo()).clicked() {
                if let Some(w) = self.history.undo(self.wb.clone()) {
                    self.wb = w;
                    self.sheet = self.sheet.min(self.wb.sheets.len() - 1);
                }
            }
            if kui::tool_button(ui, ic::REDO, t("Redo", "Повторить"), self.history.can_redo()).clicked() {
                if let Some(w) = self.history.redo(self.wb.clone()) {
                    self.wb = w;
                    self.sheet = self.sheet.min(self.wb.sheets.len() - 1);
                }
            }
            ui.separator();
            egui::ComboBox::from_id_salt("fsize").width(48.0).selected_text(format!("{}", st.size())).show_ui(ui, |ui| {
                for s in [8.0f32, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 36.0] {
                    if ui.selectable_label(st.size() == s, format!("{s}")).clicked() {
                        self.style_sel(t("Size", "Размер"), move |x| x.size10 = (s * 10.0) as u32);
                    }
                }
            });
            if kui::tool_toggle(ui, st.bold, ic::BOLD, t("Bold", "Полужирный")).clicked() {
                let v = !st.bold;
                self.style_sel(t("Bold", "Полужирный"), move |x| x.bold = v);
            }
            if kui::tool_toggle(ui, st.italic, ic::ITALIC, t("Italic", "Курсив")).clicked() {
                let v = !st.italic;
                self.style_sel(t("Italic", "Курсив"), move |x| x.italic = v);
            }
            if kui::tool_toggle(ui, st.underline, ic::UNDERLINE, t("Underline", "Подчёркивание")).clicked() {
                let v = !st.underline;
                self.style_sel(t("Underline", "Подчёркивание"), move |x| x.underline = v);
            }
            let mut col = st.color;
            if kui::color_button(ui, &mut col, t("Font Colour", "Цвет шрифта")) {
                self.style_sel(t("Colour", "Цвет"), move |x| x.color = col);
            }
            let mut fill = st.fill.unwrap_or([255, 255, 0]);
            if kui::color_button(ui, &mut fill, t("Fill Colour", "Цвет заливки")) {
                let v = if fill == [255, 255, 255] { None } else { Some(fill) };
                self.style_sel(t("Fill", "Заливка"), move |x| x.fill = v);
            }
            ui.separator();
            for (a, icon, tip) in [(HAlign::Left, ic::ALIGN_LEFT, t("Align Left", "По левому краю")), (HAlign::Center, ic::ALIGN_CENTER, t("Centre", "По центру")), (HAlign::Right, ic::ALIGN_RIGHT, t("Align Right", "По правому краю"))] {
                if kui::tool_toggle(ui, st.halign == a, icon, tip).clicked() {
                    let v = if st.halign == a { HAlign::General } else { a };
                    self.style_sel(t("Align", "Выравнивание"), move |x| x.halign = v);
                }
            }
            if kui::tool_toggle(ui, st.wrap, "\u{E751}", t("Wrap Text", "Перенос текста")).clicked() {
                let v = !st.wrap;
                self.style_sel(t("Wrap", "Перенос"), move |x| x.wrap = v);
            }
            if kui::tool_button(ui, "\u{E8C4}", t("Merge and Centre", "Объединить и поместить в центре"), true).clicked() {
                self.merge();
            }
            ui.menu_button(egui::RichText::new("\u{E81E}").family(egui::FontFamily::Name(kit42::fonts::ICON_FAMILY.into())), |ui| {
                for (label, b) in [
                    (t("All Borders", "Все границы"), 0u8),
                    (t("Outside Borders", "Внешние границы"), 1),
                    (t("Bottom Border", "Нижняя граница"), 2),
                    (t("No Border", "Без границ"), 3),
                ] {
                    if ui.button(label).clicked() {
                        self.borders(b);
                        ui.close_menu();
                    }
                }
            })
            .response
            .on_hover_text(t("Borders", "Границы"));
            ui.separator();
            egui::ComboBox::from_id_salt("numfmt")
                .width(110.0)
                .selected_text(numfmt::presets().iter().find(|p| p.1 == st.numfmt).map(|p| p.0).unwrap_or(&st.numfmt).to_string())
                .show_ui(ui, |ui| {
                    for (name, code) in numfmt::presets() {
                        if ui.selectable_label(st.numfmt == code, name).clicked() {
                            self.style_sel(t("Number Format", "Формат числа"), move |x| x.numfmt = code.into());
                        }
                    }
                });
            if kui::tool_button(ui, "\u{E948}", t("Increase Decimal", "Увеличить разрядность"), true).clicked() {
                self.decimals(1);
            }
            if kui::tool_button(ui, "\u{E949}", t("Decrease Decimal", "Уменьшить разрядность"), true).clicked() {
                self.decimals(-1);
            }
            ui.separator();
            if ui.add(egui::Button::new(egui::RichText::new("Σ").size(17.0)).frame(false)).on_hover_text(t("AutoSum", "Автосумма")).clicked() {
                self.autosum();
            }
            if kui::tool_button(ui, "\u{E74A}", t("Sort Ascending", "По возрастанию"), true).clicked() {
                let h = self.guess_header();
                self.sort(self.sel.active.1, false, h);
            }
            if kui::tool_button(ui, "\u{E74B}", t("Sort Descending", "По убыванию"), true).clicked() {
                let h = self.guess_header();
                self.sort(self.sel.active.1, true, h);
            }
            if kui::tool_toggle(ui, self.sh().filter.is_some(), ic::FILTER, t("AutoFilter", "Автофильтр")).clicked() {
                self.toggle_filter();
            }
            if kui::tool_button(ui, ic::CHART, t("Insert Chart", "Вставить диаграмму"), true).clicked() {
                self.dialog = Some(Dialog::Chart { kind: ChartKind::Column, title: String::new(), legend: true, edit: None });
            }
            if kui::tool_button(ui, ic::SEARCH, t("Find and Replace", "Найти и заменить"), true).clicked() {
                self.dialog = Some(Dialog::Find { find: String::new(), replace: String::new(), formulas: false, case: false, whole: false });
            }
        });
    }

    fn borders(&mut self, kind: u8) {
        let (r0, c0, r1, c1) = self.sel.rect();
        self.checkpoint(t("Borders", "Границы"));
        let sheet = self.sheet;
        for r in r0..=r1 {
            for c in c0..=c1 {
                let old = self.wb.sheets[sheet].cells.get(&(r, c)).map(|x| x.style).unwrap_or(0);
                let mut st = self.wb.style(old).clone();
                match kind {
                    0 => st.border = [true; 4],
                    1 => {
                        st.border[0] |= c == c0;
                        st.border[2] |= c == c1;
                        st.border[1] |= r == r0;
                        st.border[3] |= r == r1;
                    }
                    2 => st.border[3] |= r == r1,
                    _ => st.border = [false; 4],
                }
                let id = self.wb.style_id(&st);
                self.wb.sheets[sheet].cells.entry((r, c)).or_insert_with(|| Cell { input: String::new(), value: Value::Empty, style: 0, expr: None }).style = id;
            }
        }
    }

    fn decimals(&mut self, delta: i32) {
        let st = self.active_style();
        let code = if st.numfmt == "General" { "0".to_string() } else { st.numfmt.clone() };
        let new = if delta > 0 {
            if let Some(i) = code.find('.') {
                format!("{}0{}", &code[..i + 1], &code[i + 1..])
            } else if let Some(i) = code.rfind('0') {
                format!("{}.0{}", &code[..i + 1], &code[i + 1..])
            } else {
                code
            }
        } else if let Some(i) = code.find('.') {
            let after = &code[i + 1..];
            if after.starts_with("0") && after[1..].starts_with(|c: char| c == '0') {
                format!("{}{}", &code[..i + 1], &after[1..])
            } else {
                format!("{}{}", &code[..i], &after[1.min(after.len())..])
            }
        } else {
            code
        };
        self.style_sel(t("Decimals", "Разрядность"), move |x| x.numfmt = new.clone());
    }

    fn formula_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (r0, c0, r1, c1) = self.sel.rect();
            let name = if self.sel.single() { addr(r0, c0) } else { format!("{}:{}", addr(r0, c0), addr(r1, c1)) };
            let mut name_box = name.clone();
            let r = ui.add(egui::TextEdit::singleline(&mut name_box).desired_width(90.0).id(egui::Id::new("namebox")));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.goto(&name_box);
            }
            if ui.add(egui::Button::new(egui::RichText::new("fx").italics()).frame(false)).on_hover_text(t("Insert Function", "Вставить функцию")).clicked() {
                self.dialog = Some(Dialog::Functions(String::new()));
            }
            let (ar, ac) = self.sel.active;
            let mut text = match &self.editing {
                Some(t) => t.clone(),
                None => self.sh().cells.get(&(ar, ac)).map(|c| c.input.clone()).unwrap_or_default(),
            };
            let resp = ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY).id(egui::Id::new("fbar")).font(egui::TextStyle::Monospace));
            if resp.gained_focus() && self.editing.is_none() {
                self.editing = Some(text.clone());
                self.edit_cell = Some((self.sheet, ar, ac));
                self.edit_focus_cell = false;
            }
            if resp.changed() {
                self.editing = Some(text);
            }
            if resp.lost_focus() && self.editing.is_some() && !self.edit_focus_cell {
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.cancel_edit();
                } else if self.commit_edit() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    self.move_sel(1, 0, false);
                }
            }
        });
    }

    pub fn goto(&mut self, target: &str) {
        let target = target.trim();
        let (sheet, cell) = match target.rsplit_once('!') {
            Some((s, c)) => (self.wb.sheet_index(s.trim_matches('\'')), c),
            None => (Some(self.sheet), target),
        };
        let Some(s) = sheet else { return };
        let parts: Vec<&str> = cell.split(':').collect();
        if let Some((r0, c0)) = parse_addr(parts[0]) {
            let other = parts.get(1).and_then(|p| parse_addr(p)).unwrap_or((r0, c0));
            self.sheet = s;
            self.sel = Sel { active: (r0, c0), other };
            self.scroll_to = Some((r0, c0));
        } else if let Some(f) = self.wb.names.get(&cell.to_uppercase()).cloned() {
            self.goto(f.trim_start_matches('='));
        }
    }

    fn sheet_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            if kui::tool_button(ui, ic::ADD, t("New Sheet", "Новый лист"), true).clicked() {
                self.add_sheet();
            }
            let mut action: Option<(usize, u8)> = None;
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for i in 0..self.wb.sheets.len() {
                        let sh = &self.wb.sheets[i];
                        let mut txt = egui::RichText::new(&sh.name);
                        if i == self.sheet {
                            txt = txt.strong();
                        }
                        let b = egui::Button::new(txt).selected(i == self.sheet).stroke(match sh.tab_color {
                            Some(c) => egui::Stroke::new(2.0, kui::c32(c)),
                            None => egui::Stroke::NONE,
                        });
                        let r = ui.add(b);
                        if r.clicked() {
                            self.commit_edit();
                            self.sheet = i;
                            self.sel = Sel { active: (0, 0), other: (0, 0) };
                            self.selected_chart = None;
                        }
                        if r.double_clicked() {
                            self.sheet = i;
                            self.dialog = Some(Dialog::Rename(self.wb.sheets[i].name.clone()));
                        }
                        r.context_menu(|ui| {
                            for (label, code) in [
                                (t("Rename…", "Переименовать…"), 0u8),
                                (t("Duplicate", "Дублировать"), 1),
                                (t("Move Left", "Переместить влево"), 2),
                                (t("Move Right", "Переместить вправо"), 3),
                                (t("Delete", "Удалить"), 4),
                            ] {
                                if ui.button(label).clicked() {
                                    action = Some((i, code));
                                    ui.close_menu();
                                }
                            }
                            ui.menu_button(t("Tab Colour", "Цвет ярлычка"), |ui| {
                                let mut c = self.wb.sheets[i].tab_color.unwrap_or([0, 112, 192]);
                                if kui::palette(ui, &mut c) {
                                    self.wb.sheets[i].tab_color = if c == [255, 255, 255] { None } else { Some(c) };
                                    ui.close_menu();
                                }
                            });
                        });
                    }
                });
            });
            if let Some((i, code)) = action {
                self.sheet_action(i, code);
            }
        });
    }

    fn sheet_action(&mut self, i: usize, code: u8) {
        match code {
            0 => {
                self.sheet = i;
                self.dialog = Some(Dialog::Rename(self.wb.sheets[i].name.clone()));
            }
            1 => {
                self.checkpoint(t("Duplicate Sheet", "Дублирование листа"));
                let mut copy = self.wb.sheets[i].clone();
                let mut n = 2;
                while self.wb.sheet_index(&format!("{} ({n})", copy.name)).is_some() {
                    n += 1;
                }
                copy.name = format!("{} ({n})", copy.name);
                self.wb.sheets.insert(i + 1, copy);
                self.sheet = i + 1;
            }
            2 if i > 0 => {
                self.checkpoint(t("Move Sheet", "Перемещение листа"));
                self.wb.sheets.swap(i, i - 1);
                self.sheet = i - 1;
            }
            3 if i + 1 < self.wb.sheets.len() => {
                self.checkpoint(t("Move Sheet", "Перемещение листа"));
                self.wb.sheets.swap(i, i + 1);
                self.sheet = i + 1;
            }
            4 => {
                if self.wb.sheets.len() == 1 {
                    kui::error(APP, t("A workbook must contain at least one sheet.", "В книге должен быть хотя бы один лист."));
                    return;
                }
                if kui::confirm(APP, &format!("{} «{}»?", t("Delete sheet", "Удалить лист"), self.wb.sheets[i].name)) {
                    self.checkpoint(t("Delete Sheet", "Удаление листа"));
                    self.wb.sheets.remove(i);
                    self.sheet = self.sheet.min(self.wb.sheets.len() - 1);
                    self.recalc();
                }
            }
            _ => {}
        }
    }

    fn dialogs(&mut self, ctx: &Context) {
        let Some(mut d) = self.dialog.take() else { return };
        let mut open = true;
        let mut close = false;
        let title = match &d {
            Dialog::Format { .. } => t("Format Cells", "Формат ячеек"),
            Dialog::Find { .. } => t("Find and Replace", "Найти и заменить"),
            Dialog::Chart { .. } => t("Chart", "Диаграмма"),
            Dialog::Rename(_) => t("Rename Sheet", "Переименовать лист"),
            Dialog::Functions(_) => t("Functions", "Функции"),
            Dialog::Export { .. } => t("Export as PDF", "Экспорт в PDF"),
            Dialog::Filter { .. } => t("Filter", "Фильтр"),
            Dialog::Sort { .. } => t("Sort", "Сортировка"),
            Dialog::GoTo(_) => t("Go To", "Перейти"),
        };
        egui::Window::new(title).open(&mut open).collapsible(false).resizable(false).default_pos(ctx.screen_rect().center() - egui::vec2(180.0, 160.0)).show(ctx, |ui| {
            ui.set_min_width(340.0);
            match &mut d {
                Dialog::Format { tab, style, custom } => {
                    ui.horizontal(|ui| {
                        for (i, l) in [t("Number", "Число"), t("Font", "Шрифт"), t("Alignment", "Выравнивание"), t("Border & Fill", "Граница и заливка")].iter().enumerate() {
                            ui.selectable_value(tab, i, *l);
                        }
                    });
                    ui.separator();
                    match tab {
                        0 => {
                            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                                for (name, code) in numfmt::presets() {
                                    if ui.selectable_label(*custom == code, name).clicked() {
                                        *custom = code.into();
                                    }
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label(t("Code:", "Код:"));
                                ui.text_edit_singleline(custom);
                            });
                            let (ar, ac) = self.sel.active;
                            let v = self.sh().value(ar, ac);
                            let sample = if v.is_empty() { Value::Num(-1234.5678) } else { v };
                            ui.label(format!("{} {}", t("Preview:", "Образец:"), numfmt::format(&sample, custom).0));
                        }
                        1 => {
                            ui.horizontal(|ui| {
                                ui.label(t("Font:", "Шрифт:"));
                                ui.text_edit_singleline(&mut style.font);
                            });
                            let mut sz = style.size();
                            ui.add(egui::Slider::new(&mut sz, 6.0..=72.0).text(t("Size", "Размер")));
                            style.size10 = (sz * 10.0) as u32;
                            ui.checkbox(&mut style.bold, t("Bold", "Полужирный"));
                            ui.checkbox(&mut style.italic, t("Italic", "Курсив"));
                            ui.checkbox(&mut style.underline, t("Underline", "Подчёркнутый"));
                            ui.checkbox(&mut style.strike, t("Strikethrough", "Зачёркнутый"));
                            ui.horizontal(|ui| {
                                ui.label(t("Colour:", "Цвет:"));
                                kui::color_button(ui, &mut style.color, t("Colour", "Цвет"));
                            });
                        }
                        2 => {
                            ui.label(t("Horizontal:", "По горизонтали:"));
                            ui.horizontal(|ui| {
                                for (a, l) in [(HAlign::General, t("General", "Обычное")), (HAlign::Left, t("Left", "Слева")), (HAlign::Center, t("Centre", "По центру")), (HAlign::Right, t("Right", "Справа"))] {
                                    ui.selectable_value(&mut style.halign, a, l);
                                }
                            });
                            ui.label(t("Vertical:", "По вертикали:"));
                            ui.horizontal(|ui| {
                                for (a, l) in [(VAlign::Top, t("Top", "Сверху")), (VAlign::Center, t("Centre", "По центру")), (VAlign::Bottom, t("Bottom", "Снизу"))] {
                                    ui.selectable_value(&mut style.valign, a, l);
                                }
                            });
                            ui.checkbox(&mut style.wrap, t("Wrap text", "Переносить текст"));
                            ui.checkbox(&mut style.locked, t("Locked", "Защищаемая ячейка"));
                        }
                        _ => {
                            ui.label(t("Borders:", "Границы:"));
                            ui.horizontal(|ui| {
                                for (i, l) in [t("Left", "Слева"), t("Top", "Сверху"), t("Right", "Справа"), t("Bottom", "Снизу")].iter().enumerate() {
                                    ui.checkbox(&mut style.border[i], *l);
                                }
                            });
                            let mut on = style.fill.is_some();
                            ui.checkbox(&mut on, t("Fill", "Заливка"));
                            if on {
                                let mut c = style.fill.unwrap_or([255, 242, 204]);
                                kui::palette(ui, &mut c);
                                style.fill = Some(c);
                            } else {
                                style.fill = None;
                            }
                        }
                    }
                    ui.separator();
                    if ui.button(t("OK", "ОК")).clicked() {
                        let mut st = style.clone();
                        st.numfmt = custom.clone();
                        let base = self.active_style();
                        // Apply only the properties that differ from the active cell, so mixed selections keep the rest.
                        self.style_sel(t("Format Cells", "Формат ячеек"), move |x| {
                            if st.numfmt != base.numfmt {
                                x.numfmt = st.numfmt.clone();
                            }
                            if st.bold != base.bold {
                                x.bold = st.bold;
                            }
                            if st.italic != base.italic {
                                x.italic = st.italic;
                            }
                            if st.underline != base.underline {
                                x.underline = st.underline;
                            }
                            if st.strike != base.strike {
                                x.strike = st.strike;
                            }
                            if st.font != base.font {
                                x.font = st.font.clone();
                            }
                            if st.size10 != base.size10 {
                                x.size10 = st.size10;
                            }
                            if st.color != base.color {
                                x.color = st.color;
                            }
                            if st.fill != base.fill {
                                x.fill = st.fill;
                            }
                            if st.halign != base.halign {
                                x.halign = st.halign;
                            }
                            if st.valign != base.valign {
                                x.valign = st.valign;
                            }
                            if st.wrap != base.wrap {
                                x.wrap = st.wrap;
                            }
                            if st.border != base.border {
                                x.border = st.border;
                            }
                            if st.locked != base.locked {
                                x.locked = st.locked;
                            }
                        });
                        close = true;
                    }
                }
                Dialog::Find { find, replace, formulas, case, whole } => {
                    egui::Grid::new("fr").show(ui, |ui| {
                        ui.label(t("Find:", "Найти:"));
                        ui.text_edit_singleline(find);
                        ui.end_row();
                        ui.label(t("Replace with:", "Заменить на:"));
                        ui.text_edit_singleline(replace);
                        ui.end_row();
                    });
                    ui.checkbox(formulas, t("Search in formulas", "Искать в формулах"));
                    ui.checkbox(case, t("Match case", "С учётом регистра"));
                    ui.checkbox(whole, t("Entire cell", "Ячейка целиком"));
                    ui.horizontal(|ui| {
                        if ui.button(t("Find Next", "Найти далее")).clicked() {
                            let (f, fo, c, w) = (find.clone(), *formulas, *case, *whole);
                            self.find_next(&f, fo, c, w);
                        }
                        if ui.button(t("Replace All", "Заменить все")).clicked() {
                            let (f, r, c, w) = (find.clone(), replace.clone(), *case, *whole);
                            let n = self.replace_all(&f, &r, c, w);
                            self.status = format!("{} {n}", t("Replacements:", "Замен:"));
                        }
                    });
                }
                Dialog::Chart { kind, title, legend, edit } => {
                    ui.horizontal_wrapped(|ui| {
                        for k in ChartKind::ALL {
                            ui.selectable_value(kind, k, k.name());
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(t("Title:", "Заголовок:"));
                        ui.text_edit_singleline(title);
                    });
                    ui.checkbox(legend, t("Legend", "Легенда"));
                    if edit.is_none() {
                        let (r0, c0, r1, c1) = self.sel.rect();
                        ui.label(format!("{} {}:{}", t("Data:", "Данные:"), addr(r0, c0), addr(r1, c1)));
                    }
                    if ui.button(t("OK", "ОК")).clicked() {
                        self.checkpoint(t("Chart", "Диаграмма"));
                        match edit {
                            Some(id) => {
                                let id = *id;
                                if let Some(ch) = self.sh_mut().charts.iter_mut().find(|c| c.id == id) {
                                    ch.kind = *kind;
                                    ch.title = title.clone();
                                    ch.legend = *legend;
                                }
                            }
                            None => {
                                let (mut r0, mut c0, mut r1, mut c1) = self.sel.rect();
                                if self.sel.single() {
                                    let (ur, uc) = self.sh().used().unwrap_or((5, 2));
                                    r0 = 0;
                                    c0 = 0;
                                    r1 = ur;
                                    c1 = uc;
                                }
                                let anchor = (r0, c1 + 2);
                                let ch = Chart { id: rand::random(), kind: *kind, range: (r0, c0, r1, c1), title: title.clone(), anchor, size: (480.0, 300.0), legend: *legend };
                                self.selected_chart = Some(ch.id);
                                self.sh_mut().charts.push(ch);
                            }
                        }
                        close = true;
                    }
                }
                Dialog::Rename(name) => {
                    let r = ui.text_edit_singleline(name);
                    r.request_focus();
                    if ui.button(t("OK", "ОК")).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        let new = name.trim().to_string();
                        let bad = new.is_empty() || new.len() > 31 || new.contains(['\\', '/', '?', '*', '[', ']', ':']) || self.wb.sheets.iter().enumerate().any(|(i, s)| i != self.sheet && s.name.eq_ignore_ascii_case(&new));
                        if bad {
                            kui::error(APP, t("Sheet names must be unique, up to 31 characters, without \\ / ? * [ ] :", "Имя листа должно быть уникальным, до 31 символа, без \\ / ? * [ ] :"));
                        } else {
                            self.checkpoint(t("Rename Sheet", "Переименование листа"));
                            let old = self.sh().name.clone();
                            for sh in &mut self.wb.sheets {
                                for cell in sh.cells.values_mut() {
                                    if cell.is_formula() {
                                        let nf = formula::rename_sheet_in(&cell.input, &old, &new);
                                        if nf != cell.input {
                                            cell.input = nf;
                                            cell.expr = None;
                                        }
                                    }
                                }
                            }
                            self.sh_mut().name = new;
                            self.recalc();
                            close = true;
                        }
                    }
                }
                Dialog::Functions(filter) => {
                    ui.add(egui::TextEdit::singleline(filter).hint_text(t("Search functions", "Поиск функций")));
                    ui.label(format!("{}: {}", t("Supported functions", "Поддерживаемых функций"), functions::FUNCTIONS.len()));
                    let f = filter.to_uppercase();
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for name in functions::FUNCTIONS.iter().filter(|n| f.is_empty() || n.contains(&f)) {
                                if ui.button(*name).clicked() {
                                    let mut text = self.editing.clone().unwrap_or_else(|| "=".into());
                                    if !text.starts_with('=') {
                                        text = "=".into();
                                    }
                                    text.push_str(&format!("{name}("));
                                    self.start_edit(Some(text));
                                    close = true;
                                }
                            }
                        });
                    });
                    ui.label(egui::RichText::new(t("Russian names (СУММ, ЕСЛИ, ВПР…) are accepted too.", "Русские имена (СУММ, ЕСЛИ, ВПР…) тоже принимаются.")).small());
                }
                Dialog::Export { all, selection, landscape, fit, grid } => {
                    ui.radio_value(all, false, t("Active sheet", "Активный лист"));
                    ui.radio_value(all, true, t("All sheets", "Все листы"));
                    ui.add_enabled(!*all, egui::Checkbox::new(selection, t("Selection only", "Только выделение")));
                    ui.horizontal(|ui| {
                        ui.label(t("Orientation:", "Ориентация:"));
                        ui.selectable_value(landscape, 0, t("Auto", "Авто"));
                        ui.selectable_value(landscape, 1, t("Portrait", "Книжная"));
                        ui.selectable_value(landscape, 2, t("Landscape", "Альбомная"));
                    });
                    ui.checkbox(fit, t("Fit to page width", "Вписать по ширине"));
                    ui.checkbox(grid, t("Gridlines", "Сетка"));
                    if ui.button(t("Export…", "Экспорт…")).clicked() {
                        let stem = self.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Book".into());
                        if let Some(p) = kui::open_dialog(t("Export as PDF", "Экспорт в PDF"), &[("PDF", &["pdf"])]).set_file_name(format!("{stem}.pdf")).save_file() {
                            self.commit_edit();
                            let o = export::Options {
                                all_sheets: *all,
                                selection: if *selection { Some(self.sel.rect()) } else { None },
                                landscape: match landscape {
                                    1 => Some(false),
                                    2 => Some(true),
                                    _ => None,
                                },
                                fit_width: *fit,
                                gridlines: *grid,
                            };
                            let b = export::export(&self.wb, self.sheet, &o);
                            if let Err(e) = kit42::write_atomic(&p, &b) {
                                kui::error(APP, &e.to_string());
                            }
                            close = true;
                        }
                    }
                }
                Dialog::Filter { col, values } => {
                    ui.label(format!("{} {}", t("Column", "Столбец"), col_name(*col)));
                    ui.horizontal(|ui| {
                        if ui.small_button(t("Select All", "Выбрать всё")).clicked() {
                            values.iter_mut().for_each(|v| v.1 = true);
                        }
                        if ui.small_button(t("Clear", "Снять всё")).clicked() {
                            values.iter_mut().for_each(|v| v.1 = false);
                        }
                    });
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for (v, on) in values.iter_mut() {
                            ui.checkbox(on, if v.is_empty() { t("(Blanks)", "(Пустые)").to_string() } else { v.clone() });
                        }
                    });
                    if ui.button(t("OK", "ОК")).clicked() {
                        let keep: std::collections::HashSet<String> = values.iter().filter(|v| v.1).map(|v| v.0.clone()).collect();
                        self.apply_filter(*col, &keep);
                        close = true;
                    }
                }
                Dialog::Sort { col, desc, header } => {
                    ui.horizontal(|ui| {
                        ui.label(t("Column:", "Столбец:"));
                        let mut name = col_name(*col);
                        if ui.add(egui::TextEdit::singleline(&mut name).desired_width(50.0)).changed() {
                            if let Some(c) = col_from_name(&name) {
                                *col = c;
                            }
                        }
                    });
                    ui.radio_value(desc, false, t("Ascending (A→Z, 1→9)", "По возрастанию (А→Я, 1→9)"));
                    ui.radio_value(desc, true, t("Descending", "По убыванию"));
                    ui.checkbox(header, t("My data has headers", "Данные содержат заголовки"));
                    if ui.button(t("Sort", "Сортировать")).clicked() {
                        let (c, ds, h) = (*col, *desc, *header);
                        self.sort(c, ds, h);
                        close = true;
                    }
                }
                Dialog::GoTo(target) => {
                    let r = ui.add(egui::TextEdit::singleline(target).hint_text("B12, Sheet2!A1, A1:C5"));
                    r.request_focus();
                    if ui.button(t("Go", "Перейти")).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        let tg = target.clone();
                        self.goto(&tg);
                        close = true;
                    }
                }
            }
        });
        if open && !close {
            self.dialog = Some(d);
        }
    }

    pub fn open_filter(&mut self, col: u32) {
        let Some((r0, _, r1, _)) = self.sh().filter else { return };
        let sh = self.sh();
        let mut vals: Vec<String> = (r0 + 1..=r1).map(|r| sh.value(r, col).as_text()).collect();
        vals.sort();
        vals.dedup();
        let visible: std::collections::HashSet<String> = (r0 + 1..=r1).filter(|r| !sh.hidden_rows.contains(r)).map(|r| sh.value(r, col).as_text()).collect();
        self.dialog = Some(Dialog::Filter { col, values: vals.into_iter().map(|v| { let on = visible.contains(&v); (v, on) }).collect() });
    }

    fn apply_filter(&mut self, col: u32, keep: &std::collections::HashSet<String>) {
        let Some((r0, _, r1, _)) = self.sh().filter else { return };
        self.checkpoint(t("Filter", "Фильтр"));
        for r in r0 + 1..=r1 {
            let v = self.sh().value(r, col).as_text();
            if keep.contains(&v) {
                self.sh_mut().hidden_rows.remove(&r);
            } else {
                self.sh_mut().hidden_rows.insert(r);
            }
        }
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a == "--convert" || a == "--eval").unwrap_or(false) {
        kit42::attach_console();
        kit42::settings::load_global();
        let code = (|| -> Result<String, String> {
            if args[0] == "--convert" && args.get(1).map(|a| a == "--demo").unwrap_or(false) {
                let out = args.get(2).ok_or("usage: --convert --demo out.xlsx")?;
                let mut app_wb = Workbook::default();
                app_wb.sheets[0].name = t("Sales", "Продажи").into();
                let mut tmp = App {
                    wb: app_wb, path: None, history: History::new(1), sheet: 0, sel: Sel { active: (0, 0), other: (0, 0) }, editing: None, edit_cell: None,
                    edit_focus_cell: false, zoom: 1.0, scroll_to: None, clip: None, drag: None, selected_chart: None, dialog: None, status: String::new(),
                    csv_delim: ',', encoding: encoding_rs::UTF_8, picker: Default::default(), request_editor_focus: false,
                };
                let rows = [
                    ["Отчёт о продажах 2024", "", "", "", ""],
                    ["Месяц", "Выручка", "Расходы", "Прибыль", "Маржа"],
                    ["Январь", "120000", "80000", "=B3-C3", "=D3/B3"],
                    ["Февраль", "135000", "82000", "=B4-C4", "=D4/B4"],
                    ["Март", "150500", "91000", "=B5-C5", "=D5/B5"],
                    ["Апрель", "142000", "88000", "=B6-C6", "=D6/B6"],
                    ["Итого", "=SUM(B3:B6)", "=СУММ(C3:C6)", "=SUM(D3:D6)", "=IFERROR(D7/B7,0)"],
                    ["Среднее", "=AVERAGE(B3:B6)", "=AVERAGE(C3:C6)", "=AVERAGE(D3:D6)", ""],
                    ["Лучший месяц", "=INDEX(A3:A6,MATCH(MAX(D3:D6),D3:D6,0))", "", "", ""],
                    ["Дата отчёта", "15.11.2024", "", "", ""],
                ];
                for (r, row) in rows.iter().enumerate() {
                    for (c, v) in row.iter().enumerate() {
                        if !v.is_empty() {
                            tmp.put(0, r as u32, c as u32, v);
                        }
                    }
                }
                tmp.sel = Sel { active: (0, 0), other: (0, 4) };
                tmp.merge();
                tmp.style_sel("", |s| { s.bold = true; s.size10 = 160; s.color = [31, 78, 121]; });
                tmp.sel = Sel { active: (1, 0), other: (1, 4) };
                tmp.style_sel("", |s| { s.bold = true; s.fill = Some([221, 235, 247]); s.border = [true; 4]; s.halign = HAlign::Center; });
                tmp.sel = Sel { active: (2, 1), other: (7, 3) };
                tmp.style_sel("", |s| s.numfmt = "#,##0 \"₽\"".into());
                tmp.sel = Sel { active: (2, 4), other: (6, 4) };
                tmp.style_sel("", |s| s.numfmt = "0.0%".into());
                tmp.sel = Sel { active: (6, 0), other: (6, 4) };
                tmp.style_sel("", |s| { s.bold = true; s.border[1] = true; });
                tmp.wb.sheets[0].col_w.insert(0, 120.0);
                tmp.wb.sheets[0].col_w.insert(1, 110.0);
                tmp.wb.sheets[0].freeze = (2, 1);
                tmp.wb.sheets[0].charts.push(Chart { id: 1, kind: ChartKind::Column, range: (1, 0, 5, 3), title: "Выручка и расходы".into(), anchor: (11, 0), size: (480.0, 260.0), legend: true });
                tmp.recalc();
                let p = std::path::PathBuf::from(out);
                let b = tmp.serialize(&p)?;
                kit42::write_atomic(&p, &b).map_err(|e| e.to_string())?;
                return Ok(format!("demo -> {out}"));
            }
            if args[0] == "--eval" {
                // --eval "=formula" [file]: evaluates a formula (against a workbook when given).
                let f = args.get(1).ok_or("usage: --eval =FORMULA [file]")?;
                let mut wb = match args.get(2) {
                    Some(p) => App::load(std::path::Path::new(p))?.0,
                    None => Workbook::default(),
                };
                let s = 0;
                let (r, c) = (MAX_ROWS - 1, MAX_COLS - 1);
                wb.sheets[s].cells.insert((r, c), Cell { input: f.clone(), value: Value::Empty, style: 0, expr: None });
                formula::recalc(&mut wb);
                return Ok(wb.sheets[s].value(r, c).as_text());
            }
            let (i, o) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?);
            let (wb, d, _) = App::load(std::path::Path::new(i))?;
            let app = App {
                wb,
                path: None,
                history: History::new(1),
                sheet: 0,
                sel: Sel { active: (0, 0), other: (0, 0) },
                editing: None,
                edit_cell: None,
                edit_focus_cell: false,
                zoom: 1.0,
                scroll_to: None,
                clip: None,
                drag: None,
                selected_chart: None,
                dialog: None,
                status: String::new(),
                csv_delim: d,
                encoding: encoding_rs::UTF_8,
                picker: Default::default(),
                request_editor_focus: false,
            };
            let out = std::path::PathBuf::from(o);
            let b = app.serialize(&out)?;
            kit42::write_atomic(&out, &b).map_err(|e| e.to_string())?;
            let cells: usize = app.wb.sheets.iter().map(|s| s.cells.len()).sum();
            Ok(format!("{} sheets, {cells} cells -> {o}", app.wb.sheets.len()))
        })();
        match code {
            Ok(m) => {
                println!("{m}");
                std::process::exit(0)
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1)
            }
        }
    }
    kit42::run(kit42::AppSpec { id: "xls42", title: APP, size: [1280.0, 860.0] }, App::new)
}
