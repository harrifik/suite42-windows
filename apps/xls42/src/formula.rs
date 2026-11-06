//! Formula engine: tokenizer, parser, evaluator, reference shifting and recalculation.

use crate::model::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Str(String),
    Bool(bool),
    Err(String),
    Ref { sheet: Option<String>, r: u32, c: u32 },
    Range { sheet: Option<String>, r0: u32, c0: u32, r1: u32, c1: u32 },
    Name(String),
    Array(Vec<Vec<Expr>>),
    Neg(Box<Expr>),
    Percent(Box<Expr>),
    Bin(String, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Missing,
}

// ───────────────────────── tokenizer ─────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Sheet(String),
    Err(String),
    Op(String),
    LParen,
    RParen,
    Comma,
    Semi,
    Colon,
    LBrace,
    RBrace,
}

pub fn tokenize(s: &str) -> Result<Vec<(Tok, usize, usize)>, String> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let start = i;
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                i += 1;
                continue;
            }
            '"' => {
                let mut t = String::new();
                i += 1;
                loop {
                    if i >= cs.len() {
                        return Err("unterminated string".into());
                    }
                    if cs[i] == '"' {
                        if i + 1 < cs.len() && cs[i + 1] == '"' {
                            t.push('"');
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    t.push(cs[i]);
                    i += 1;
                }
                out.push((Tok::Str(t), start, i));
                continue;
            }
            '\'' => {
                let mut t = String::new();
                i += 1;
                while i < cs.len() {
                    if cs[i] == '\'' {
                        if i + 1 < cs.len() && cs[i + 1] == '\'' {
                            t.push('\'');
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    t.push(cs[i]);
                    i += 1;
                }
                i += 1;
                if i < cs.len() && cs[i] == '!' {
                    i += 1;
                    out.push((Tok::Sheet(t), start, i));
                } else {
                    return Err("bad sheet reference".into());
                }
                continue;
            }
            '#' => {
                let mut t = String::from("#");
                i += 1;
                while i < cs.len() && (cs[i].is_alphanumeric() || "/!?".contains(cs[i])) {
                    t.push(cs[i]);
                    i += 1;
                    if t.ends_with('!') || t.ends_with('?') || t == "#N/A" {
                        break;
                    }
                }
                out.push((Tok::Err(t.to_uppercase()), start, i));
                continue;
            }
            '(' => out.push((Tok::LParen, i, i + 1)),
            ')' => out.push((Tok::RParen, i, i + 1)),
            ',' => out.push((Tok::Comma, i, i + 1)),
            ';' => out.push((Tok::Semi, i, i + 1)),
            ':' => out.push((Tok::Colon, i, i + 1)),
            '{' => out.push((Tok::LBrace, i, i + 1)),
            '}' => out.push((Tok::RBrace, i, i + 1)),
            '<' | '>' => {
                if i + 1 < cs.len() && (cs[i + 1] == '=' || (c == '<' && cs[i + 1] == '>')) {
                    out.push((Tok::Op(format!("{c}{}", cs[i + 1])), i, i + 2));
                    i += 2;
                    continue;
                }
                out.push((Tok::Op(c.to_string()), i, i + 1));
            }
            '+' | '-' | '*' | '/' | '^' | '&' | '=' | '%' => out.push((Tok::Op(c.to_string()), i, i + 1)),
            d if d.is_ascii_digit() || (d == '.' && i + 1 < cs.len() && cs[i + 1].is_ascii_digit()) => {
                let mut t = String::new();
                while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                    t.push(cs[i]);
                    i += 1;
                }
                if i < cs.len() && (cs[i] == 'e' || cs[i] == 'E') && i + 1 < cs.len() && (cs[i + 1].is_ascii_digit() || ((cs[i + 1] == '-' || cs[i + 1] == '+') && i + 2 < cs.len() && cs[i + 2].is_ascii_digit())) {
                    t.push('e');
                    i += 1;
                    if cs[i] == '-' || cs[i] == '+' {
                        t.push(cs[i]);
                        i += 1;
                    }
                    while i < cs.len() && cs[i].is_ascii_digit() {
                        t.push(cs[i]);
                        i += 1;
                    }
                }
                // "1:3" whole rows or "A1" never start with digits; a digit run followed by letters is an identifier (e.g. sheet names are quoted).
                out.push((Tok::Num(t.parse().map_err(|_| format!("bad number {t}"))?), start, i));
                continue;
            }
            a if a.is_alphabetic() || a == '_' || a == '$' || a == '\\' => {
                let mut t = String::new();
                while i < cs.len() && (cs[i].is_alphanumeric() || "_.$\\".contains(cs[i])) {
                    t.push(cs[i]);
                    i += 1;
                }
                if i < cs.len() && cs[i] == '!' {
                    i += 1;
                    out.push((Tok::Sheet(t), start, i));
                } else {
                    out.push((Tok::Ident(t), start, i));
                }
                continue;
            }
            other => return Err(format!("unexpected '{other}'")),
        }
        i += 1;
    }
    Ok(out)
}

/// A cell reference text like "$B$7" → (row, col, row_abs, col_abs).
pub fn parse_ref(s: &str) -> Option<(u32, u32, bool, bool)> {
    let ca = s.starts_with('$');
    let rest = s.trim_start_matches('$');
    let i = rest.find(|c: char| c.is_ascii_digit() || c == '$')?;
    let col = col_from_name(&rest[..i])?;
    let rp = &rest[i..];
    let ra = rp.starts_with('$');
    let row: u32 = rp.trim_start_matches('$').parse().ok()?;
    if row == 0 || row > MAX_ROWS {
        return None;
    }
    Some((row - 1, col, ra, ca))
}

fn parse_col_ref(s: &str) -> Option<u32> {
    col_from_name(s.trim_start_matches('$'))
}

// ───────────────────────── parser ─────────────────────────

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn is_op(&self, ops: &[&str]) -> Option<String> {
        match self.peek() {
            Some(Tok::Op(o)) if ops.contains(&o.as_str()) => Some(o.clone()),
            _ => None,
        }
    }
    fn expr(&mut self) -> Result<Expr, String> {
        self.comparison()
    }
    fn comparison(&mut self) -> Result<Expr, String> {
        let mut l = self.concat()?;
        while let Some(op) = self.is_op(&["=", "<>", "<", ">", "<=", ">="]) {
            self.pos += 1;
            let r = self.concat()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn concat(&mut self) -> Result<Expr, String> {
        let mut l = self.additive()?;
        while self.is_op(&["&"]).is_some() {
            self.pos += 1;
            let r = self.additive()?;
            l = Expr::Bin("&".into(), Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn additive(&mut self) -> Result<Expr, String> {
        let mut l = self.mult()?;
        while let Some(op) = self.is_op(&["+", "-"]) {
            self.pos += 1;
            let r = self.mult()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn mult(&mut self) -> Result<Expr, String> {
        let mut l = self.power()?;
        while let Some(op) = self.is_op(&["*", "/"]) {
            self.pos += 1;
            let r = self.power()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn power(&mut self) -> Result<Expr, String> {
        let mut l = self.unary()?;
        while self.is_op(&["^"]).is_some() {
            self.pos += 1;
            let r = self.unary()?;
            l = Expr::Bin("^".into(), Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn unary(&mut self) -> Result<Expr, String> {
        if self.is_op(&["-"]).is_some() {
            self.pos += 1;
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        if self.is_op(&["+"]).is_some() {
            self.pos += 1;
            return self.unary();
        }
        let mut e = self.primary()?;
        while self.is_op(&["%"]).is_some() {
            self.pos += 1;
            e = Expr::Percent(Box::new(e));
        }
        Ok(e)
    }
    fn reference(&mut self, sheet: Option<String>, first: &str) -> Result<Expr, String> {
        // first is an identifier or number text
        if self.peek() == Some(&Tok::Colon) {
            let save = self.pos;
            self.pos += 1;
            let second = match self.next() {
                Some(Tok::Ident(s)) => s,
                Some(Tok::Num(n)) => format!("{}", n as u64),
                _ => {
                    self.pos = save;
                    String::new()
                }
            };
            if !second.is_empty() {
                if let (Some((r0, c0, ..)), Some((r1, c1, ..))) = (parse_ref(first), parse_ref(&second)) {
                    return Ok(Expr::Range { sheet, r0: r0.min(r1), c0: c0.min(c1), r1: r0.max(r1), c1: c0.max(c1) });
                }
                if let (Some(c0), Some(c1)) = (parse_col_ref(first), parse_col_ref(&second)) {
                    return Ok(Expr::Range { sheet, r0: 0, c0: c0.min(c1), r1: MAX_ROWS - 1, c1: c0.max(c1) });
                }
                let rows = |s: &str| s.trim_start_matches('$').parse::<u32>().ok().filter(|r| *r >= 1);
                if let (Some(r0), Some(r1)) = (rows(first), rows(&second)) {
                    return Ok(Expr::Range { sheet, r0: r0.min(r1) - 1, c0: 0, r1: r0.max(r1) - 1, c1: MAX_COLS - 1 });
                }
                return Err(format!("bad range {first}:{second}"));
            }
        }
        if let Some((r, c, ..)) = parse_ref(first) {
            return Ok(Expr::Ref { sheet, r, c });
        }
        Err(format!("bad reference {first}"))
    }
    fn args(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = vec![];
        if self.peek() == Some(&Tok::RParen) {
            self.pos += 1;
            return Ok(args);
        }
        loop {
            if matches!(self.peek(), Some(Tok::Comma) | Some(Tok::Semi)) {
                args.push(Expr::Missing);
            } else if self.peek() == Some(&Tok::RParen) {
                args.push(Expr::Missing);
            } else {
                args.push(self.expr()?);
            }
            match self.next() {
                Some(Tok::Comma) | Some(Tok::Semi) => continue,
                Some(Tok::RParen) => break,
                _ => return Err("expected , or )".into()),
            }
        }
        Ok(args)
    }
    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Num(n)) => {
                if self.peek() == Some(&Tok::Colon) && n.fract() == 0.0 {
                    return self.reference(None, &format!("{}", n as u64));
                }
                Ok(Expr::Num(n))
            }
            Some(Tok::Str(s)) => Ok(Expr::Str(s)),
            Some(Tok::Err(e)) => Ok(Expr::Err(e)),
            Some(Tok::LParen) => {
                let e = self.expr()?;
                if self.next() != Some(Tok::RParen) {
                    return Err("missing )".into());
                }
                Ok(e)
            }
            Some(Tok::LBrace) => {
                let mut rows = vec![vec![]];
                loop {
                    let neg = if self.is_op(&["-"]).is_some() {
                        self.pos += 1;
                        true
                    } else {
                        false
                    };
                    let v = match self.next() {
                        Some(Tok::Num(n)) => Expr::Num(if neg { -n } else { n }),
                        Some(Tok::Str(s)) => Expr::Str(s),
                        Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("TRUE") => Expr::Bool(true),
                        Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("FALSE") => Expr::Bool(false),
                        Some(Tok::Err(e)) => Expr::Err(e),
                        _ => return Err("bad array constant".into()),
                    };
                    rows.last_mut().unwrap().push(v);
                    match self.next() {
                        Some(Tok::Comma) => {}
                        Some(Tok::Semi) => rows.push(vec![]),
                        Some(Tok::RBrace) => break,
                        _ => return Err("bad array constant".into()),
                    }
                }
                Ok(Expr::Array(rows))
            }
            Some(Tok::Sheet(s)) => match self.next() {
                Some(Tok::Ident(r)) => self.reference(Some(s), &r),
                Some(Tok::Num(n)) => self.reference(Some(s), &format!("{}", n as u64)),
                _ => Err("bad sheet reference".into()),
            },
            Some(Tok::Ident(id)) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let args = self.args()?;
                    return Ok(Expr::Call(canonical(&id), args));
                }
                let up = id.to_uppercase();
                if up == "TRUE" || up == "ИСТИНА" {
                    return Ok(Expr::Bool(true));
                }
                if up == "FALSE" || up == "ЛОЖЬ" {
                    return Ok(Expr::Bool(false));
                }
                if parse_ref(&id).is_some() || (self.peek() == Some(&Tok::Colon) && parse_col_ref(&id).is_some()) {
                    return self.reference(None, &id);
                }
                Ok(Expr::Name(id))
            }
            t => Err(format!("unexpected {t:?}")),
        }
    }
}

pub fn parse(formula: &str) -> Result<Expr, String> {
    let body = formula.strip_prefix('=').unwrap_or(formula);
    let toks: Vec<Tok> = tokenize(body)?.into_iter().map(|t| t.0).collect();
    let mut p = Parser { toks, pos: 0 };
    let e = p.expr()?;
    if p.pos < p.toks.len() {
        return Err("unexpected text after formula".into());
    }
    Ok(e)
}

/// Russian and legacy aliases → canonical function names.
pub fn canonical(name: &str) -> String {
    let up = name.to_uppercase().replace("_XLFN.", "").replace("_XLWS.", "");
    let map: &[(&str, &str)] = &[
        ("СУММ", "SUM"), ("ЕСЛИ", "IF"), ("ВПР", "VLOOKUP"), ("ГПР", "HLOOKUP"), ("СРЗНАЧ", "AVERAGE"), ("МАКС", "MAX"), ("МИН", "MIN"),
        ("СЧЁТ", "COUNT"), ("СЧЕТ", "COUNT"), ("СЧЁТЗ", "COUNTA"), ("СЧЕТЗ", "COUNTA"), ("СЧЁТЕСЛИ", "COUNTIF"), ("СЧЕТЕСЛИ", "COUNTIF"),
        ("СУММЕСЛИ", "SUMIF"), ("ОКРУГЛ", "ROUND"), ("И", "AND"), ("ИЛИ", "OR"), ("НЕ", "NOT"), ("ЕСЛИОШИБКА", "IFERROR"),
        ("СЦЕПИТЬ", "CONCATENATE"), ("СЦЕП", "CONCAT"), ("ЛЕВСИМВ", "LEFT"), ("ПРАВСИМВ", "RIGHT"), ("ПСТР", "MID"), ("ДЛСТР", "LEN"),
        ("СЕГОДНЯ", "TODAY"), ("ТДАТА", "NOW"), ("ДАТА", "DATE"), ("ГОД", "YEAR"), ("МЕСЯЦ", "MONTH"), ("ДЕНЬ", "DAY"), ("ИНДЕКС", "INDEX"),
        ("ПОИСКПОЗ", "MATCH"), ("ПРОИЗВЕД", "PRODUCT"), ("СТЕПЕНЬ", "POWER"), ("КОРЕНЬ", "SQRT"), ("ЦЕЛОЕ", "INT"), ("ОСТАТ", "MOD"),
        ("ТЕКСТ", "TEXT"), ("ЗНАЧЕН", "VALUE"), ("ПРОПИСН", "UPPER"), ("СТРОЧН", "LOWER"), ("ПРОПНАЧ", "PROPER"), ("СЖПРОБЕЛЫ", "TRIM"),
        ("ПОДСТАВИТЬ", "SUBSTITUTE"), ("НАЙТИ", "FIND"), ("ПОИСК", "SEARCH"), ("МЕДИАНА", "MEDIAN"), ("СТАНДОТКЛОН", "STDEV"),
        ("СТАНДОТКЛОН.В", "STDEV.S"), ("СТАНДОТКЛОН.Г", "STDEV.P"), ("ДИСП", "VAR"), ("ДИСП.В", "VAR.S"), ("ДИСП.Г", "VAR.P"),
        ("НАИБОЛЬШИЙ", "LARGE"), ("НАИМЕНЬШИЙ", "SMALL"), ("РАНГ", "RANK"), ("РАНГ.РВ", "RANK.EQ"), ("ПЛТ", "PMT"), ("БС", "FV"), ("ПС", "PV"),
        ("ЧПС", "NPV"), ("ВСД", "IRR"), ("СТАВКА", "RATE"), ("КПЕР", "NPER"), ("ПРПЛТ", "IPMT"), ("ОСПЛТ", "PPMT"), ("ЕПУСТО", "ISBLANK"),
        ("ЕЧИСЛО", "ISNUMBER"), ("ЕТЕКСТ", "ISTEXT"), ("ЕОШИБКА", "ISERROR"), ("ЕОШ", "ISERR"), ("ЕНД", "ISNA"), ("ВЫБОР", "CHOOSE"),
        ("СТРОКА", "ROW"), ("СТОЛБЕЦ", "COLUMN"), ("СУММПРОИЗВ", "SUMPRODUCT"), ("СЧИТАТЬПУСТОТЫ", "COUNTBLANK"), ("СРЗНАЧЕСЛИ", "AVERAGEIF"),
        ("СРЗНАЧЕСЛИМН", "AVERAGEIFS"), ("СУММЕСЛИМН", "SUMIFS"), ("СЧЁТЕСЛИМН", "COUNTIFS"), ("СЧЕТЕСЛИМН", "COUNTIFS"), ("ДЕНЬНЕД", "WEEKDAY"),
        ("ДАТАМЕС", "EDATE"), ("КОНМЕСЯЦА", "EOMONTH"), ("ЧИСТРАБДНИ", "NETWORKDAYS"), ("РАБДЕНЬ", "WORKDAY"), ("ОКРУГЛВВЕРХ", "ROUNDUP"),
        ("ОКРУГЛВНИЗ", "ROUNDDOWN"), ("ПИ", "PI"), ("СЛЧИС", "RAND"), ("СЛУЧМЕЖДУ", "RANDBETWEEN"), ("ПОВТОР", "REPT"), ("СИМВОЛ", "CHAR"),
        ("КОДСИМВ", "CODE"), ("СОВПАД", "EXACT"), ("ЗАМЕНИТЬ", "REPLACE"), ("ТРАНСП", "TRANSPOSE"), ("ДВССЫЛ", "INDIRECT"), ("СМЕЩ", "OFFSET"),
        ("АДРЕС", "ADDRESS"), ("ПРОСМОТРX", "XLOOKUP"), ("ПРОСМОТР", "LOOKUP"), ("ЧСТРОК", "ROWS"), ("ЧИСЛСТОЛБ", "COLUMNS"), ("ЕСЛИМН", "IFS"),
        ("ОБЪЕДИНИТЬ", "TEXTJOIN"), ("ABS", "ABS"), ("МАКСЕСЛИ", "MAXIFS"), ("МИНЕСЛИ", "MINIFS"), ("ЧАС", "HOUR"), ("МИНУТЫ", "MINUTE"),
        ("СЕКУНДЫ", "SECOND"), ("ВРЕМЯ", "TIME"), ("НОМНЕДЕЛИ", "WEEKNUM"), ("РАЗНДАТ", "DATEDIF"), ("ДНИ", "DAYS"), ("ПРОМЕЖУТОЧНЫЕ.ИТОГИ", "SUBTOTAL"),
        ("ОКРВВЕРХ", "CEILING"), ("ОКРВНИЗ", "FLOOR"), ("ОТБР", "TRUNC"), ("ЗНАК", "SIGN"), ("ФАКТР", "FACT"), ("ЧИСЛКОМБ", "COMBIN"),
        ("СРГЕОМ", "GEOMEAN"), ("КОРРЕЛ", "CORREL"), ("НАКЛОН", "SLOPE"), ("ОТРЕЗОК", "INTERCEPT"), ("ПРЕДСКАЗ", "FORECAST"), ("ПЕРСЕНТИЛЬ", "PERCENTILE"),
        ("КВАРТИЛЬ", "QUARTILE"), ("МОДА", "MODE"), ("УНИК", "UNIQUE"), ("СОРТ", "SORT"), ("ФИЛЬТР", "FILTER"), ("ПОСЛЕД", "SEQUENCE"),
        ("ЕЧЁТН", "ISEVEN"), ("ЕНЕЧЁТ", "ISODD"), ("ЕЛОГИЧ", "ISLOGICAL"), ("НД", "NA"), ("ТИП", "TYPE"), ("АПЛ", "SLN"), ("ДНЕЙ360", "DAYS360"),
        ("ДОЛЯГОДА", "YEARFRAC"), ("ДАТАЗНАЧ", "DATEVALUE"), ("ВРЕМЗНАЧ", "TIMEVALUE"), ("ПЕЧСИМВ", "CLEAN"), ("ФИКСИРОВАННЫЙ", "FIXED"),
        ("РИМСКОЕ", "ROMAN"), ("НОД", "GCD"), ("НОК", "LCM"), ("ЧЁТН", "EVEN"), ("НЕЧЁТ", "ODD"), ("ЧАСТНОЕ", "QUOTIENT"), ("ГРАДУСЫ", "DEGREES"),
        ("РАДИАНЫ", "RADIANS"), ("ЕНД", "ISNA"), ("ЕСНД", "IFNA"), ("ПЕРЕКЛЮЧ", "SWITCH"), ("ИСКЛИЛИ", "XOR"), ("СУММКВ", "SUMSQ"),
    ];
    map.iter().find(|(k, _)| *k == up).map(|(_, v)| v.to_string()).unwrap_or(up)
}

// ───────────────────────── evaluation ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rng {
    pub sheet: usize,
    pub r0: u32,
    pub c0: u32,
    pub r1: u32,
    pub c1: u32,
}

#[derive(Clone, Debug)]
pub enum V {
    S(Value),
    R(Rng),
    A(Vec<Vec<Value>>),
}

pub struct Eval<'a> {
    pub wb: &'a Workbook,
    pub memo: HashMap<(usize, u32, u32), Value>,
    pub arrays: HashMap<(usize, u32, u32), Vec<Vec<Value>>>,
    visiting: HashSet<(usize, u32, u32)>,
    cur: (usize, u32, u32),
    pub volatile_seed: u64,
}

const E_VALUE: &str = "#VALUE!";
const E_DIV: &str = "#DIV/0!";
const E_NA: &str = "#N/A";
const E_REF: &str = "#REF!";
const E_NUM: &str = "#NUM!";
const E_NAME: &str = "#NAME?";

fn err(e: &str) -> Value {
    Value::Err(e.into())
}

macro_rules! tryv {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return V::S(e),
        }
    };
}

impl<'a> Eval<'a> {
    pub fn new(wb: &'a Workbook) -> Self {
        Self { wb, memo: HashMap::new(), arrays: HashMap::new(), visiting: HashSet::new(), cur: (0, 0, 0), volatile_seed: rand::random() }
    }

    pub fn cell(&mut self, s: usize, r: u32, c: u32) -> Value {
        if let Some(v) = self.memo.get(&(s, r, c)) {
            return v.clone();
        }
        let Some(sheet) = self.wb.sheets.get(s) else { return err(E_REF) };
        let Some(cell) = sheet.cells.get(&(r, c)) else {
            return sheet.spill.get(&(r, c)).cloned().unwrap_or(Value::Empty);
        };
        let Some(expr) = cell.expr.clone() else {
            if cell.input.is_empty() {
                return sheet.spill.get(&(r, c)).cloned().unwrap_or(Value::Empty);
            }
            return cell.value.clone();
        };
        if self.visiting.contains(&(s, r, c)) {
            return err("#CYCLE!");
        }
        self.visiting.insert((s, r, c));
        let saved = self.cur;
        self.cur = (s, r, c);
        let v = self.eval(&expr);
        self.cur = saved;
        self.visiting.remove(&(s, r, c));
        let out = match v {
            V::S(v) => v,
            V::R(rg) => {
                let arr = self.materialize(rg);
                let first = arr.first().and_then(|r| r.first()).cloned().unwrap_or(Value::Empty);
                if arr.len() * arr.first().map(|r| r.len()).unwrap_or(0) > 1 {
                    self.arrays.insert((s, r, c), arr);
                }
                first
            }
            V::A(a) => {
                let first = a.first().and_then(|r| r.first()).cloned().unwrap_or(Value::Empty);
                if a.len() * a.first().map(|r| r.len()).unwrap_or(0) > 1 {
                    self.arrays.insert((s, r, c), a);
                }
                first
            }
        };
        let out = match out {
            Value::Empty => Value::Num(0.0),
            o => o,
        };
        self.memo.insert((s, r, c), out.clone());
        out
    }

    fn sheet_of(&self, name: &Option<String>) -> Option<usize> {
        match name {
            None => Some(self.cur.0),
            Some(n) => self.wb.sheet_index(n),
        }
    }

    /// Clips whole-column/row ranges to the used area.
    fn clip(&self, mut g: Rng) -> Rng {
        if let Some(sh) = self.wb.sheets.get(g.sheet) {
            let (ur, uc) = sh.used().unwrap_or((0, 0));
            let (sr, sc) = sh.spill.keys().fold((0, 0), |a, k| (a.0.max(k.0), a.1.max(k.1)));
            g.r1 = g.r1.min(ur.max(sr));
            g.c1 = g.c1.min(uc.max(sc));
            if g.r1 < g.r0 {
                g.r1 = g.r0;
            }
            if g.c1 < g.c0 {
                g.c1 = g.c0;
            }
        }
        g
    }

    pub fn materialize(&mut self, g: Rng) -> Vec<Vec<Value>> {
        let g = self.clip(g);
        let rows = (g.r1 - g.r0 + 1) as usize;
        let cols = (g.c1 - g.c0 + 1) as usize;
        if rows * cols > 2_000_000 {
            return vec![vec![err(E_NUM)]];
        }
        let mut out = vec![vec![Value::Empty; cols]; rows];
        // Visit only stored cells (sparse).
        let keys: Vec<(u32, u32)> = self.wb.sheets[g.sheet]
            .cells
            .range((g.r0, 0)..=(g.r1, MAX_COLS))
            .filter(|(k, _)| k.1 >= g.c0 && k.1 <= g.c1)
            .map(|(k, _)| *k)
            .chain(self.wb.sheets[g.sheet].spill.keys().filter(|k| k.0 >= g.r0 && k.0 <= g.r1 && k.1 >= g.c0 && k.1 <= g.c1).cloned())
            .collect();
        for (r, c) in keys {
            out[(r - g.r0) as usize][(c - g.c0) as usize] = self.cell(g.sheet, r, c);
        }
        out
    }

    /// Values of a range in row-major order, sparse (only stored cells).
    fn range_values(&mut self, g: Rng) -> Vec<Value> {
        let g = self.clip(g);
        let keys: Vec<(u32, u32)> = self.wb.sheets[g.sheet]
            .cells
            .range((g.r0, 0)..=(g.r1, MAX_COLS))
            .filter(|(k, _)| k.1 >= g.c0 && k.1 <= g.c1)
            .map(|(k, _)| *k)
            .chain(self.wb.sheets[g.sheet].spill.keys().filter(|k| k.0 >= g.r0 && k.0 <= g.r1 && k.1 >= g.c0 && k.1 <= g.c1).cloned())
            .collect();
        keys.into_iter().map(|(r, c)| self.cell(g.sheet, r, c)).collect()
    }

    pub fn eval(&mut self, e: &Expr) -> V {
        match e {
            Expr::Num(n) => V::S(Value::Num(*n)),
            Expr::Str(s) => V::S(Value::Str(s.clone())),
            Expr::Bool(b) => V::S(Value::Bool(*b)),
            Expr::Err(e) => V::S(Value::Err(e.clone())),
            Expr::Missing => V::S(Value::Empty),
            Expr::Ref { sheet, r, c } => match self.sheet_of(sheet) {
                Some(s) => V::R(Rng { sheet: s, r0: *r, c0: *c, r1: *r, c1: *c }),
                None => V::S(err(E_REF)),
            },
            Expr::Range { sheet, r0, c0, r1, c1 } => match self.sheet_of(sheet) {
                Some(s) => V::R(Rng { sheet: s, r0: *r0, c0: *c0, r1: *r1, c1: *c1 }),
                None => V::S(err(E_REF)),
            },
            Expr::Name(n) => match self.wb.names.get(&n.to_uppercase()).cloned() {
                Some(f) => match parse(&f) {
                    Ok(e) => self.eval(&e),
                    Err(_) => V::S(err(E_NAME)),
                },
                None => V::S(err(E_NAME)),
            },
            Expr::Array(rows) => {
                let a = rows.iter().map(|r| r.iter().map(|x| self.scalar(x)).collect()).collect();
                V::A(a)
            }
            Expr::Neg(x) => self.map1(x, |v| match v.as_num() {
                Some(n) if !matches!(v, Value::Str(_)) || v.as_num().is_some() => Value::Num(-n),
                _ => if let Value::Err(e) = v { Value::Err(e) } else { err(E_VALUE) },
            }),
            Expr::Percent(x) => self.map1(x, |v| match num(&v) {
                Ok(n) => Value::Num(n / 100.0),
                Err(e) => e,
            }),
            Expr::Bin(op, a, b) => {
                let va = self.eval(a);
                let vb = self.eval(b);
                self.binop(op, va, vb)
            }
            Expr::Call(name, args) => self.call(name, args),
        }
    }

    /// Single value of an expression (top-left of ranges, implicit intersection).
    pub fn scalar(&mut self, e: &Expr) -> Value {
        let v = self.eval(e);
        self.to_scalar(v)
    }

    pub fn to_scalar(&mut self, v: V) -> Value {
        match v {
            V::S(s) => s,
            V::R(g) => {
                if g.r0 == g.r1 && g.c0 == g.c1 {
                    return self.cell(g.sheet, g.r0, g.c0);
                }
                // Implicit intersection with the formula's row/column.
                let (_, cr, cc) = self.cur;
                if g.c0 == g.c1 && cr >= g.r0 && cr <= g.r1 {
                    return self.cell(g.sheet, cr, g.c0);
                }
                if g.r0 == g.r1 && cc >= g.c0 && cc <= g.c1 {
                    return self.cell(g.sheet, g.r0, cc);
                }
                self.cell(g.sheet, g.r0, g.c0)
            }
            V::A(a) => a.first().and_then(|r| r.first()).cloned().unwrap_or(Value::Empty),
        }
    }

    fn to_array(&mut self, v: V) -> Vec<Vec<Value>> {
        match v {
            V::S(s) => vec![vec![s]],
            V::R(g) => self.materialize(g),
            V::A(a) => a,
        }
    }

    fn map1(&mut self, x: &Expr, f: impl Fn(Value) -> Value) -> V {
        match self.eval(x) {
            V::S(s) => V::S(f(s)),
            v => {
                let a = self.to_array(v);
                V::A(a.into_iter().map(|r| r.into_iter().map(&f).collect()).collect())
            }
        }
    }

    fn binop(&mut self, op: &str, a: V, b: V) -> V {
        let is_arr = |v: &V| matches!(v, V::A(_)) || matches!(v, V::R(g) if g.r0 != g.r1 || g.c0 != g.c1);
        if is_arr(&a) || is_arr(&b) {
            // Element-wise with broadcasting (Excel 365 arrays).
            let aa = if is_arr(&a) { self.to_array(a) } else { let s = self.to_scalar(a); vec![vec![s]] };
            let bb = if is_arr(&b) { self.to_array(b) } else { let s = self.to_scalar(b); vec![vec![s]] };
            let rows = aa.len().max(bb.len());
            let cols = aa[0].len().max(bb[0].len());
            let get = |m: &Vec<Vec<Value>>, r: usize, c: usize| -> Value {
                let rr = if m.len() == 1 { 0 } else { r };
                let cc = if m[0].len() == 1 { 0 } else { c };
                m.get(rr).and_then(|x| x.get(cc)).cloned().unwrap_or(err(E_NA))
            };
            let out = (0..rows).map(|r| (0..cols).map(|c| scalar_op(op, &get(&aa, r, c), &get(&bb, r, c))).collect()).collect();
            return V::A(out);
        }
        let x = self.to_scalar(a);
        let y = self.to_scalar(b);
        V::S(scalar_op(op, &x, &y))
    }

    fn nums(&mut self, args: &[Expr], include_text: bool) -> Result<Vec<f64>, Value> {
        let mut out = vec![];
        for a in args {
            match self.eval(a) {
                V::S(Value::Err(e)) => return Err(Value::Err(e)),
                V::S(v) => match v {
                    Value::Num(n) => out.push(n),
                    Value::Bool(b) => out.push(b as i32 as f64),
                    Value::Str(s) => match s.trim().replace(',', ".").parse() {
                        Ok(n) => out.push(n),
                        Err(_) => return Err(err(E_VALUE)),
                    },
                    Value::Empty => {}
                    _ => {}
                },
                V::R(g) => {
                    for v in self.range_values(g) {
                        match v {
                            Value::Num(n) => out.push(n),
                            Value::Err(e) => return Err(Value::Err(e)),
                            Value::Bool(b) if include_text => out.push(b as i32 as f64),
                            Value::Str(_) if include_text => out.push(0.0),
                            _ => {}
                        }
                    }
                }
                V::A(a) => {
                    for v in a.into_iter().flatten() {
                        match v {
                            Value::Num(n) => out.push(n),
                            Value::Err(e) => return Err(Value::Err(e)),
                            Value::Bool(b) if include_text => out.push(b as i32 as f64),
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    fn arg_num(&mut self, args: &[Expr], i: usize, default: Option<f64>) -> Result<f64, Value> {
        match args.get(i) {
            None | Some(Expr::Missing) => default.ok_or(err(E_VALUE)),
            Some(e) => {
                let v = self.scalar(e);
                if v.is_empty() {
                    return Ok(default.unwrap_or(0.0));
                }
                num(&v)
            }
        }
    }

    fn arg_str(&mut self, args: &[Expr], i: usize, default: &str) -> Result<String, Value> {
        match args.get(i) {
            None | Some(Expr::Missing) => Ok(default.to_string()),
            Some(e) => match self.scalar(e) {
                Value::Err(e) => Err(Value::Err(e)),
                v => Ok(v.as_text()),
            },
        }
    }

    fn arg_bool(&mut self, args: &[Expr], i: usize, default: bool) -> Result<bool, Value> {
        match args.get(i) {
            None | Some(Expr::Missing) => Ok(default),
            Some(e) => truthy(&self.scalar(e)),
        }
    }

    fn arg_range(&mut self, args: &[Expr], i: usize) -> Option<Rng> {
        match args.get(i).map(|e| self.eval(e)) {
            Some(V::R(g)) => Some(self.clip(g)),
            _ => None,
        }
    }

    fn arg_array(&mut self, args: &[Expr], i: usize) -> Vec<Vec<Value>> {
        match args.get(i) {
            Some(e) => {
                let v = self.eval(e);
                self.to_array(v)
            }
            None => vec![vec![]],
        }
    }

    fn call(&mut self, name: &str, args: &[Expr]) -> V {
        crate::functions::call(self, name, args)
    }

    // Accessors used by the function library.
    pub fn nums_pub(&mut self, args: &[Expr], include_text: bool) -> Result<Vec<f64>, Value> {
        self.nums(args, include_text)
    }
    pub fn num_arg(&mut self, args: &[Expr], i: usize, default: Option<f64>) -> Result<f64, Value> {
        self.arg_num(args, i, default)
    }
    pub fn str_arg(&mut self, args: &[Expr], i: usize, default: &str) -> Result<String, Value> {
        self.arg_str(args, i, default)
    }
    pub fn bool_arg(&mut self, args: &[Expr], i: usize, default: bool) -> Result<bool, Value> {
        self.arg_bool(args, i, default)
    }
    pub fn range_arg(&mut self, args: &[Expr], i: usize) -> Option<Rng> {
        self.arg_range(args, i)
    }
    pub fn array_arg(&mut self, args: &[Expr], i: usize) -> Vec<Vec<Value>> {
        self.arg_array(args, i)
    }
    pub fn values_of(&mut self, v: V) -> Vec<Value> {
        match v {
            V::S(s) => vec![s],
            V::R(g) => self.range_values(g),
            V::A(a) => a.into_iter().flatten().collect(),
        }
    }
    pub fn array_of(&mut self, v: V) -> Vec<Vec<Value>> {
        self.to_array(v)
    }
    pub fn current(&self) -> (usize, u32, u32) {
        self.cur
    }
    pub fn clip_pub(&self, g: Rng) -> Rng {
        self.clip(g)
    }
}

pub fn num(v: &Value) -> Result<f64, Value> {
    match v {
        Value::Num(n) => Ok(*n),
        Value::Bool(b) => Ok(*b as i32 as f64),
        Value::Empty => Ok(0.0),
        Value::Str(s) => {
            let (pv, _) = parse_input(s);
            match pv {
                Value::Num(n) => Ok(n),
                _ => Err(err(E_VALUE)),
            }
        }
        Value::Err(e) => Err(Value::Err(e.clone())),
    }
}

pub fn truthy(v: &Value) -> Result<bool, Value> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Num(n) => Ok(*n != 0.0),
        Value::Empty => Ok(false),
        Value::Str(s) => match s.to_uppercase().as_str() {
            "TRUE" | "ИСТИНА" => Ok(true),
            "FALSE" | "ЛОЖЬ" => Ok(false),
            _ => Err(err(E_VALUE)),
        },
        Value::Err(e) => Err(Value::Err(e.clone())),
    }
}

/// Excel ordering: numbers < text < booleans; text compares case-insensitively.
pub fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    let rank = |v: &Value| match v {
        Value::Empty => 0,
        Value::Num(_) => 1,
        Value::Str(_) => 2,
        Value::Bool(_) => 3,
        Value::Err(_) => 4,
    };
    match (a, b) {
        (Value::Num(x), Value::Num(y)) => x.partial_cmp(y).unwrap_or(Equal),
        (Value::Str(x), Value::Str(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Empty, Value::Num(y)) => 0.0f64.partial_cmp(y).unwrap_or(Equal),
        (Value::Num(x), Value::Empty) => x.partial_cmp(&0.0).unwrap_or(Equal),
        (Value::Empty, Value::Str(y)) => "".cmp(&y.to_lowercase().as_str()),
        (Value::Str(x), Value::Empty) => x.to_lowercase().as_str().cmp(""),
        _ => rank(a).cmp(&rank(b)),
    }
}

pub fn scalar_op(op: &str, x: &Value, y: &Value) -> Value {
    if let Value::Err(e) = x {
        return Value::Err(e.clone());
    }
    if let Value::Err(e) = y {
        return Value::Err(e.clone());
    }
    match op {
        "&" => Value::Str(format!("{}{}", x.as_text(), y.as_text())),
        "=" | "<>" | "<" | ">" | "<=" | ">=" => {
            let o = compare(x, y);
            use std::cmp::Ordering::*;
            Value::Bool(match op {
                "=" => o == Equal,
                "<>" => o != Equal,
                "<" => o == Less,
                ">" => o == Greater,
                "<=" => o != Greater,
                _ => o != Less,
            })
        }
        _ => {
            let a = match num(x) {
                Ok(n) => n,
                Err(e) => return e,
            };
            let b = match num(y) {
                Ok(n) => n,
                Err(e) => return e,
            };
            let r = match op {
                "+" => a + b,
                "-" => a - b,
                "*" => a * b,
                "/" => {
                    if b == 0.0 {
                        return err(E_DIV);
                    }
                    a / b
                }
                "^" => a.powf(b),
                _ => return err(E_VALUE),
            };
            if r.is_finite() {
                Value::Num(r)
            } else {
                err(E_NUM)
            }
        }
    }
}

// ───────────────────────── recalculation ─────────────────────────

/// Parses formulas whose AST is missing.
pub fn prepare(wb: &mut Workbook) {
    for sh in &mut wb.sheets {
        for cell in sh.cells.values_mut() {
            if cell.is_formula() {
                if cell.expr.is_none() {
                    match parse(&cell.input) {
                        Ok(e) => cell.expr = Some(Arc::new(e)),
                        Err(_) => {
                            cell.expr = None;
                            cell.value = Value::Err("#NAME?".into());
                        }
                    }
                }
            } else {
                cell.expr = None;
            }
        }
    }
}

/// Recalculates the whole workbook and spills dynamic arrays.
pub fn recalc(wb: &mut Workbook) {
    prepare(wb);
    for pass in 0..2 {
        let (memo, arrays) = {
            let mut ev = Eval::new(wb);
            let keys: Vec<(usize, u32, u32)> = wb
                .sheets
                .iter()
                .enumerate()
                .flat_map(|(s, sh)| sh.cells.iter().filter(|(_, c)| c.expr.is_some()).map(move |(k, _)| (s, k.0, k.1)))
                .collect();
            for (s, r, c) in keys {
                ev.cell(s, r, c);
            }
            (ev.memo, ev.arrays)
        };
        for ((s, r, c), v) in memo {
            if let Some(cell) = wb.sheets[s].cells.get_mut(&(r, c)) {
                if cell.expr.is_some() {
                    cell.value = v;
                }
            }
        }
        let mut changed = false;
        let mut new_spill: Vec<std::collections::BTreeMap<(u32, u32), Value>> = vec![Default::default(); wb.sheets.len()];
        for ((s, r, c), a) in arrays {
            let sh = &wb.sheets[s];
            let blocked = a.iter().enumerate().any(|(i, row)| {
                row.iter().enumerate().any(|(j, _)| {
                    (i > 0 || j > 0) && sh.cells.get(&(r + i as u32, c + j as u32)).map(|x| !x.input.is_empty()).unwrap_or(false)
                })
            });
            if blocked {
                if let Some(cell) = wb.sheets[s].cells.get_mut(&(r, c)) {
                    cell.value = Value::Err("#SPILL!".into());
                }
                continue;
            }
            for (i, row) in a.iter().enumerate() {
                for (j, v) in row.iter().enumerate() {
                    if i > 0 || j > 0 {
                        new_spill[s].insert((r + i as u32, c + j as u32), v.clone());
                    }
                }
            }
        }
        for (s, sp) in new_spill.into_iter().enumerate() {
            if wb.sheets[s].spill != sp {
                changed = true;
                wb.sheets[s].spill = sp;
            }
        }
        if !changed || pass == 1 {
            break;
        }
    }
}

// ───────────────────────── reference rewriting ─────────────────────────

fn rebuild(formula: &str, f: &mut dyn FnMut(Option<&str>, &str) -> Option<String>) -> String {
    // Rewrites every reference token (optionally with sheet prefix); f returns None for #REF!.
    let body = &formula[1..];
    let Ok(toks) = tokenize(body) else { return formula.to_string() };
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::from("=");
    let mut last = 0;
    let mut i = 0;
    while i < toks.len() {
        let (t, s, e) = &toks[i];
        let mut sheet: Option<String> = None;
        let mut start = *s;
        let mut j = i;
        if let Tok::Sheet(name) = t {
            sheet = Some(name.clone());
            j += 1;
        }
        let Some((tok, _, mut end)) = toks.get(j).cloned() else {
            i += 1;
            continue;
        };
        let first = match &tok {
            Tok::Ident(x) => Some(x.clone()),
            Tok::Num(n) if toks.get(j + 1).map(|t| t.0 == Tok::Colon).unwrap_or(false) => Some(format!("{}", *n as u64)),
            _ => None,
        };
        let is_func = matches!(toks.get(j + 1), Some((Tok::LParen, ..)));
        if let Some(first) = first.filter(|_| !is_func) {
            let mut text = first.clone();
            if let (Some((Tok::Colon, ..)), Some((t2, _, e2))) = (toks.get(j + 1), toks.get(j + 2)) {
                let second = match t2 {
                    Tok::Ident(x) => Some(x.clone()),
                    Tok::Num(n) => Some(format!("{}", *n as u64)),
                    _ => None,
                };
                if let Some(sec) = second {
                    text = format!("{first}:{sec}");
                    end = *e2;
                    j += 2;
                }
            }
            let looks_ref = text.split(':').all(|p| parse_ref(p).is_some() || parse_col_ref(p).is_some() || p.trim_start_matches('$').parse::<u32>().is_ok());
            if looks_ref && (text.contains(':') || parse_ref(&text).is_some()) {
                out.extend(&chars[last..start.min(chars.len())]);
                let prefix: String = chars[start..toks[if sheet.is_some() { i + 1 } else { i }].1].iter().collect();
                match f(sheet.as_deref(), &text) {
                    Some(new) => {
                        out.push_str(&prefix);
                        out.push_str(&new);
                    }
                    None => out.push_str("#REF!"),
                }
                last = end;
                i = j + 1;
                continue;
            }
        }
        let _ = (&mut start, e);
        i += 1;
    }
    out.extend(&chars[last.min(chars.len())..]);
    out
}

fn shift_part(p: &str, dr: i64, dc: i64) -> Option<String> {
    if let Some((r, c, ra, ca)) = parse_ref(p) {
        let nr = if ra { r as i64 } else { r as i64 + dr };
        let nc = if ca { c as i64 } else { c as i64 + dc };
        if nr < 0 || nc < 0 || nr >= MAX_ROWS as i64 || nc >= MAX_COLS as i64 {
            return None;
        }
        return Some(format!("{}{}{}{}", if ca { "$" } else { "" }, col_name(nc as u32), if ra { "$" } else { "" }, nr + 1));
    }
    if let Some(c) = parse_col_ref(p) {
        let abs = p.starts_with('$');
        let nc = if abs { c as i64 } else { c as i64 + dc };
        if nc < 0 {
            return None;
        }
        return Some(format!("{}{}", if abs { "$" } else { "" }, col_name(nc as u32)));
    }
    let abs = p.starts_with('$');
    let r: i64 = p.trim_start_matches('$').parse().ok()?;
    let nr = if abs { r } else { r + dr };
    (nr >= 1).then(|| format!("{}{nr}", if abs { "$" } else { "" }))
}

/// Shifts relative references when a formula is copied by (dr, dc).
pub fn shift_formula(f: &str, dr: i64, dc: i64) -> String {
    if !f.starts_with('=') {
        return f.to_string();
    }
    rebuild(f, &mut |_, text| {
        let parts: Vec<Option<String>> = text.split(':').map(|p| shift_part(p, dr, dc)).collect();
        if parts.iter().any(|p| p.is_none()) {
            return None;
        }
        Some(parts.into_iter().map(|p| p.unwrap()).collect::<Vec<_>>().join(":"))
    })
}

/// Adjusts references after inserting (count > 0) or deleting (count < 0) rows or columns.
/// `on_sheet` is the edited sheet; `formula_sheet` the sheet that owns the formula.
pub fn adjust_formula(f: &str, formula_sheet: &str, on_sheet: &str, rows: bool, at: u32, count: i64) -> String {
    if !f.starts_with('=') {
        return f.to_string();
    }
    rebuild(f, &mut |sheet, text| {
        let target = sheet.unwrap_or(formula_sheet);
        if !target.eq_ignore_ascii_case(on_sheet) {
            return Some(text.to_string());
        }
        let parts: Vec<&str> = text.split(':').collect();
        let fix = |p: &str, is_end: bool| -> Option<String> {
            let (pos, abs_marker) = if let Some((r, c, ra, ca)) = parse_ref(p) {
                (if rows { r } else { c }, (ra, ca))
            } else if !rows && parse_col_ref(p).is_some() {
                (parse_col_ref(p)?, (false, p.starts_with('$')))
            } else if rows {
                (p.trim_start_matches('$').parse::<u32>().ok()?.checked_sub(1)?, (p.starts_with('$'), false))
            } else {
                return Some(p.to_string());
            };
            let _ = abs_marker;
            let new = if count > 0 {
                if pos >= at { pos as i64 + count } else { pos as i64 }
            } else {
                let del_end = at as i64 - count - 1;
                if (pos as i64) < at as i64 {
                    pos as i64
                } else if pos as i64 > del_end {
                    pos as i64 + count
                } else if parts.len() == 2 {
                    // Range edge inside the deleted block shrinks to the block boundary.
                    if is_end { at as i64 - 1 } else { at as i64 }
                } else {
                    return None;
                }
            };
            if new < 0 {
                return None;
            }
            let (dr, dc) = if rows { (new - pos as i64, 0) } else { (0, new - pos as i64) };
            // Shift ignoring $ (structural edits move absolute references too).
            let plain = p.replace('$', "");
            let shifted = shift_part(&plain, dr, dc)?;
            // Restore $ markers.
            Some(if let Some((_, _, ra, ca)) = parse_ref(p) {
                let (r, c, ..) = parse_ref(&shifted)?;
                format!("{}{}{}{}", if ca { "$" } else { "" }, col_name(c), if ra { "$" } else { "" }, r + 1)
            } else if p.starts_with('$') {
                format!("${shifted}")
            } else {
                shifted
            })
        };
        if parts.len() == 2 {
            let a = fix(parts[0], false)?;
            let b = fix(parts[1], true)?;
            // A deleted whole range collapses.
            if let (Some((r0, c0, ..)), Some((r1, c1, ..))) = (parse_ref(&a), parse_ref(&b)) {
                if r1 < r0 || c1 < c0 {
                    return None;
                }
            }
            Some(format!("{a}:{b}"))
        } else {
            fix(parts[0], false)
        }
    })
}

/// Renames sheet references inside a formula.
pub fn rename_sheet_in(f: &str, old: &str, new: &str) -> String {
    if !f.starts_with('=') {
        return f.to_string();
    }
    let body = &f[1..];
    let Ok(toks) = tokenize(body) else { return f.to_string() };
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::from("=");
    let mut last = 0;
    for (t, s, e) in toks {
        if let Tok::Sheet(name) = t {
            if name.eq_ignore_ascii_case(old) {
                out.extend(&chars[last..s]);
                out.push_str(&quote_sheet(new));
                out.push('!');
                last = e;
            }
        }
    }
    out.extend(&chars[last..]);
    out
}

/// Cell references in a formula (for highlighting while editing).
pub fn references(f: &str) -> Vec<(Option<String>, u32, u32, u32, u32)> {
    let mut v = vec![];
    if let Ok(e) = parse(f) {
        fn walk(e: &Expr, v: &mut Vec<(Option<String>, u32, u32, u32, u32)>) {
            match e {
                Expr::Ref { sheet, r, c } => v.push((sheet.clone(), *r, *c, *r, *c)),
                Expr::Range { sheet, r0, c0, r1, c1 } => v.push((sheet.clone(), *r0, *c0, *r1, *c1)),
                Expr::Neg(x) | Expr::Percent(x) => walk(x, v),
                Expr::Bin(_, a, b) => {
                    walk(a, v);
                    walk(b, v);
                }
                Expr::Call(_, args) => args.iter().for_each(|a| walk(a, v)),
                _ => {}
            }
        }
        walk(&e, &mut v);
    }
    v
}
