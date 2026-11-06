//! Workbook model: sheets of cells with styles, sizes, merges, freeze panes and charts.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Empty,
    Num(f64),
    Str(String),
    Bool(bool),
    Err(String),
}

impl Value {
    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Bool(b) => Some(*b as i32 as f64),
            Value::Empty => Some(0.0),
            Value::Str(s) => s.trim().parse().ok(),
            Value::Err(_) => None,
        }
    }
    pub fn as_text(&self) -> String {
        match self {
            Value::Empty => String::new(),
            Value::Num(n) => fmt_general(*n),
            Value::Str(s) => s.clone(),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
            Value::Err(e) => e.clone(),
        }
    }
}

/// Excel "General" number display: up to 11 significant digits, scientific for extremes.
pub fn fmt_general(n: f64) -> String {
    if n == 0.0 {
        return "0".into();
    }
    if !n.is_finite() {
        return "#NUM!".into();
    }
    let a = n.abs();
    if !(1e-9..1e11).contains(&a) {
        let s = format!("{:.5E}", n);
        let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        let e: i32 = e.parse().unwrap_or(0);
        return format!("{m}E{}{:02}", if e < 0 { "-" } else { "+" }, e.abs());
    }
    let digits = 11 - (a.log10().floor() as i32 + 1).max(1);
    let s = format!("{:.*}", digits.clamp(0, 10) as usize, n);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
pub enum HAlign {
    #[default]
    General,
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
pub enum VAlign {
    Top,
    Center,
    #[default]
    Bottom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Hash, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub font: String,
    /// Font size in tenths of a point.
    pub size10: u32,
    pub color: [u8; 3],
    pub fill: Option<[u8; 3]>,
    pub halign: HAlign,
    pub valign: VAlign,
    pub wrap: bool,
    pub numfmt: String,
    /// left, top, right, bottom
    pub border: [bool; 4],
    pub locked: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            font: "Calibri".into(),
            size10: 110,
            color: [0, 0, 0],
            fill: None,
            halign: HAlign::General,
            valign: VAlign::Bottom,
            wrap: false,
            numfmt: "General".into(),
            border: [false; 4],
            locked: true,
        }
    }
}

impl Style {
    pub fn size(&self) -> f32 {
        self.size10 as f32 / 10.0
    }
}

#[derive(Clone, Debug)]
pub struct Cell {
    /// What the user typed ("=SUM(A1:A3)", "12", "text").
    pub input: String,
    pub value: Value,
    pub style: u32,
    pub expr: Option<Arc<crate::formula::Expr>>,
}

impl Cell {
    pub fn is_formula(&self) -> bool {
        self.input.starts_with('=') && self.input.len() > 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChartKind {
    Column,
    Bar,
    Line,
    Area,
    Pie,
    Scatter,
}

impl ChartKind {
    pub const ALL: [ChartKind; 6] = [ChartKind::Column, ChartKind::Bar, ChartKind::Line, ChartKind::Area, ChartKind::Pie, ChartKind::Scatter];
    pub fn name(self) -> &'static str {
        use kit42::t;
        match self {
            ChartKind::Column => t("Column", "Гистограмма"),
            ChartKind::Bar => t("Bar", "Линейчатая"),
            ChartKind::Line => t("Line", "График"),
            ChartKind::Area => t("Area", "С областями"),
            ChartKind::Pie => t("Pie", "Круговая"),
            ChartKind::Scatter => t("Scatter", "Точечная"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub id: u64,
    pub kind: ChartKind,
    /// Data range (r0, c0, r1, c1) on the chart's sheet.
    pub range: (u32, u32, u32, u32),
    pub title: String,
    /// Top-left anchor cell and size in pixels at zoom 1.
    pub anchor: (u32, u32),
    pub size: (f32, f32),
    pub legend: bool,
}

#[derive(Clone, Debug)]
pub struct Sheet {
    pub name: String,
    pub cells: BTreeMap<(u32, u32), Cell>,
    pub col_w: BTreeMap<u32, f32>,
    pub row_h: BTreeMap<u32, f32>,
    pub merges: Vec<(u32, u32, u32, u32)>,
    pub freeze: (u32, u32),
    pub hidden_rows: BTreeSet<u32>,
    pub hidden_cols: BTreeSet<u32>,
    pub charts: Vec<Chart>,
    pub tab_color: Option<[u8; 3]>,
    pub filter: Option<(u32, u32, u32, u32)>,
    pub spill: BTreeMap<(u32, u32), Value>,
    pub gridlines: bool,
}

pub const DEF_COL_W: f32 = 72.0;
pub const DEF_ROW_H: f32 = 20.0;
pub const MAX_ROWS: u32 = 1_048_576;
pub const MAX_COLS: u32 = 16_384;

impl Sheet {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            cells: BTreeMap::new(),
            col_w: BTreeMap::new(),
            row_h: BTreeMap::new(),
            merges: vec![],
            freeze: (0, 0),
            hidden_rows: BTreeSet::new(),
            hidden_cols: BTreeSet::new(),
            charts: vec![],
            tab_color: None,
            filter: None,
            spill: BTreeMap::new(),
            gridlines: true,
        }
    }

    pub fn col_width(&self, c: u32) -> f32 {
        if self.hidden_cols.contains(&c) {
            0.0
        } else {
            *self.col_w.get(&c).unwrap_or(&DEF_COL_W)
        }
    }

    pub fn row_height(&self, r: u32) -> f32 {
        if self.hidden_rows.contains(&r) {
            0.0
        } else {
            *self.row_h.get(&r).unwrap_or(&DEF_ROW_H)
        }
    }

    /// x position (at zoom 1) of the left edge of column c.
    pub fn col_x(&self, c: u32) -> f32 {
        let mut x = c as f32 * DEF_COL_W;
        for &k in self.col_w.keys().take_while(|k| **k < c) {
            x += self.col_width(k) - DEF_COL_W;
        }
        for &k in self.hidden_cols.range(..c) {
            if !self.col_w.contains_key(&k) {
                x -= DEF_COL_W;
            }
        }
        x
    }

    pub fn row_y(&self, r: u32) -> f32 {
        let mut y = r as f32 * DEF_ROW_H;
        for &k in self.row_h.keys().take_while(|k| **k < r) {
            y += self.row_height(k) - DEF_ROW_H;
        }
        for &k in self.hidden_rows.range(..r) {
            if !self.row_h.contains_key(&k) {
                y -= DEF_ROW_H;
            }
        }
        y
    }

    /// Column at x (binary search).
    pub fn col_at(&self, x: f32) -> u32 {
        let (mut lo, mut hi) = (0u32, MAX_COLS - 1);
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            if self.col_x(mid) <= x {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    pub fn row_at(&self, y: f32) -> u32 {
        let (mut lo, mut hi) = (0u32, MAX_ROWS - 1);
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            if self.row_y(mid) <= y {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    /// Last used row and column (inclusive), or None when empty.
    pub fn used(&self) -> Option<(u32, u32)> {
        let mut r = 0;
        let mut c = 0;
        let mut any = false;
        for (&(rr, cc), cell) in &self.cells {
            if !cell.input.is_empty() || cell.style != 0 {
                r = r.max(rr);
                c = c.max(cc);
                any = true;
            }
        }
        any.then_some((r, c))
    }

    pub fn value(&self, r: u32, c: u32) -> Value {
        match self.cells.get(&(r, c)) {
            Some(cell) if !matches!(cell.value, Value::Empty) || !cell.input.is_empty() => cell.value.clone(),
            _ => self.spill.get(&(r, c)).cloned().unwrap_or(Value::Empty),
        }
    }

    pub fn merge_at(&self, r: u32, c: u32) -> Option<(u32, u32, u32, u32)> {
        self.merges.iter().find(|m| r >= m.0 && r <= m.2 && c >= m.1 && c <= m.3).cloned()
    }
}

#[derive(Clone, Debug)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
    pub styles: Vec<Style>,
    pub date1904: bool,
    pub names: BTreeMap<String, String>,
}

impl Default for Workbook {
    fn default() -> Self {
        Self { sheets: vec![Sheet::new(&format!("{}1", kit42::t("Sheet", "Лист")))], styles: vec![Style::default()], date1904: false, names: BTreeMap::new() }
    }
}

impl Workbook {
    pub fn style_id(&mut self, s: &Style) -> u32 {
        if let Some(i) = self.styles.iter().position(|x| x == s) {
            return i as u32;
        }
        self.styles.push(s.clone());
        (self.styles.len() - 1) as u32
    }

    pub fn style(&self, id: u32) -> &Style {
        self.styles.get(id as usize).unwrap_or(&self.styles[0])
    }

    pub fn sheet_index(&self, name: &str) -> Option<usize> {
        self.sheets.iter().position(|s| s.name.eq_ignore_ascii_case(name))
    }
}

// ───────────────────────── addresses ─────────────────────────

pub fn col_name(mut c: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push((b'A' + (c % 26) as u8) as char);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.iter().rev().collect()
}

pub fn col_from_name(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 3 {
        return None;
    }
    let mut c: u32 = 0;
    for ch in s.chars() {
        if !ch.is_ascii_alphabetic() {
            return None;
        }
        c = c * 26 + (ch.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
    }
    (c >= 1 && c <= MAX_COLS).then(|| c - 1)
}

pub fn addr(r: u32, c: u32) -> String {
    format!("{}{}", col_name(c), r + 1)
}

/// Parses "B7" (with optional $) into (row, col).
pub fn parse_addr(s: &str) -> Option<(u32, u32)> {
    let s = s.replace('$', "");
    let i = s.find(|c: char| c.is_ascii_digit())?;
    let c = col_from_name(&s[..i])?;
    let r: u32 = s[i..].parse().ok()?;
    (r >= 1 && r <= MAX_ROWS).then(|| (r - 1, c))
}

pub fn quote_sheet(name: &str) -> String {
    if name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

// ───────────────────────── dates ─────────────────────────

/// Serial number (1900 system) for a date.
pub fn date_serial(y: i32, m: u32, d: u32) -> Option<f64> {
    let date = chrono::NaiveDate::from_ymd_opt(y, m, d)?;
    let base = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?;
    Some((date - base).num_days() as f64)
}

pub fn serial_date(v: f64) -> Option<chrono::NaiveDateTime> {
    if !v.is_finite() || v < 0.0 || v > 2_958_465.0 {
        return None;
    }
    let base = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?.and_hms_opt(0, 0, 0)?;
    let days = v.floor() as i64;
    let secs = ((v - v.floor()) * 86400.0).round() as i64;
    Some(base + chrono::Duration::days(days) + chrono::Duration::seconds(secs))
}

/// Interprets typed text: number, percentage, date, time, boolean or text.
pub fn parse_input(s: &str) -> (Value, Option<&'static str>) {
    let t = s.trim();
    if t.is_empty() {
        return (Value::Empty, None);
    }
    if t.starts_with('\'') {
        return (Value::Str(t[1..].to_string()), None);
    }
    let up = t.to_uppercase();
    if up == "TRUE" || up == "ИСТИНА" {
        return (Value::Bool(true), None);
    }
    if up == "FALSE" || up == "ЛОЖЬ" {
        return (Value::Bool(false), None);
    }
    let clean: String = t.chars().filter(|c| !c.is_whitespace() && *c != '\u{a0}').collect();
    if let Some(p) = clean.strip_suffix('%') {
        if let Ok(v) = p.replace(',', ".").parse::<f64>() {
            return (Value::Num(v / 100.0), Some("0%"));
        }
    }
    for cur in ["₽", "$", "€", "р.", "руб."] {
        let stripped = clean.trim_start_matches(cur).trim_end_matches(cur);
        if stripped.len() != clean.len() {
            if let Ok(v) = stripped.replace(',', ".").parse::<f64>() {
                return (Value::Num(v), Some(if cur == "$" { "$#,##0.00" } else if cur == "€" { "#,##0.00 €" } else { "#,##0.00 ₽" }));
            }
        }
    }
    if let Ok(v) = clean.parse::<f64>() {
        if !clean.starts_with('0') || clean.starts_with("0.") || clean == "0" || clean.contains('e') || clean.contains('E') {
            return (Value::Num(v), None);
        }
        if clean.len() > 1 && clean.starts_with('0') {
            // Leading zeros (codes, phone numbers) stay text.
            return (Value::Str(t.to_string()), None);
        }
        return (Value::Num(v), None);
    }
    // Decimal comma: "3,14" (but not thousands like "1,234,567").
    if clean.matches(',').count() == 1 && !clean.contains('.') {
        if let Ok(v) = clean.replace(',', ".").parse::<f64>() {
            return (Value::Num(v), None);
        }
    }
    // Dates: dd.mm.yyyy, yyyy-mm-dd, dd/mm/yyyy.
    let parts: Vec<&str> = t.split(|c| c == '.' || c == '-' || c == '/').collect();
    if parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        let n: Vec<i32> = parts.iter().map(|p| p.parse().unwrap_or(0)).collect();
        let (y, m, d) = if parts[0].len() == 4 { (n[0], n[1], n[2]) } else if t.contains('/') && n[0] <= 12 && n[1] > 12 { (n[2], n[0], n[1]) } else { (n[2], n[1], n[0]) };
        let y = if y < 100 { y + if y < 30 { 2000 } else { 1900 } } else { y };
        if let Some(sv) = date_serial(y, m as u32, d as u32) {
            return (Value::Num(sv), Some(if parts[0].len() == 4 { "yyyy-mm-dd" } else { "dd.mm.yyyy" }));
        }
    }
    // Time hh:mm[:ss]
    let tp: Vec<&str> = t.split(':').collect();
    if (2..=3).contains(&tp.len()) && tp.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        let h: f64 = tp[0].parse().unwrap_or(0.0);
        let m: f64 = tp[1].parse().unwrap_or(0.0);
        let s2: f64 = tp.get(2).and_then(|x| x.parse().ok()).unwrap_or(0.0);
        if m < 60.0 && s2 < 60.0 {
            return (Value::Num((h * 3600.0 + m * 60.0 + s2) / 86400.0), Some(if tp.len() == 3 { "hh:mm:ss" } else { "hh:mm" }));
        }
    }
    (Value::Str(t.to_string()), None)
}
