//! Excel number formats: sections, colours, thousands separators, decimals, percent,
//! scientific notation, currency literals, dates/times and elapsed hours.

use crate::model::{fmt_general, serial_date, Value};

/// Formats a value; returns the text and an optional colour from [Red]/[Blue]/… .
pub fn format(v: &Value, code: &str) -> (String, Option<[u8; 3]>) {
    match v {
        Value::Num(n) => format_num(*n, code),
        Value::Str(s) => {
            // A 4th section formats text: "@" is replaced.
            let secs = split_sections(code);
            if secs.len() >= 4 {
                let (c, color) = take_color(&secs[3]);
                return (c.replace('@', s).replace('"', ""), color);
            }
            (s.clone(), None)
        }
        Value::Bool(b) => (if *b { "TRUE" } else { "FALSE" }.into(), None),
        Value::Err(e) => (e.clone(), None),
        Value::Empty => (String::new(), None),
    }
}

fn split_sections(code: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut q = false;
    let mut br = false;
    for c in code.chars() {
        match c {
            '"' => q = !q,
            '[' if !q => br = true,
            ']' if !q => br = false,
            ';' if !q && !br => {
                out.push(String::new());
                continue;
            }
            _ => {}
        }
        out.last_mut().unwrap().push(c);
    }
    out
}

fn take_color(sec: &str) -> (String, Option<[u8; 3]>) {
    let mut color = None;
    let mut out = String::new();
    let mut rest = sec;
    while let Some(i) = rest.find('[') {
        let Some(j) = rest[i..].find(']') else { break };
        let tag = &rest[i + 1..i + j];
        let c = match tag.to_lowercase().as_str() {
            "red" | "красный" => Some([220, 0, 0]),
            "blue" | "синий" => Some([0, 0, 220]),
            "green" | "зеленый" | "зелёный" => Some([0, 140, 0]),
            "black" | "черный" => Some([0, 0, 0]),
            "magenta" => Some([200, 0, 200]),
            "cyan" => Some([0, 160, 180]),
            "yellow" => Some([200, 170, 0]),
            "white" => Some([255, 255, 255]),
            _ => None,
        };
        out.push_str(&rest[..i]);
        if c.is_some() {
            color = c;
        } else if tag.starts_with('$') || tag.starts_with("h") || tag.starts_with("m") || tag.starts_with("s") || tag.starts_with('H') {
            // Currency/locale tags like [$₽-419] keep their symbol; elapsed time tags stay.
            if let Some(sym) = tag.strip_prefix('$') {
                out.push_str(sym.split('-').next().unwrap_or(""));
            } else {
                out.push('[');
                out.push_str(tag);
                out.push(']');
            }
        }
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    (out, color)
}

fn is_date_code(c: &str) -> bool {
    let mut q = false;
    for ch in c.chars() {
        if ch == '"' {
            q = !q;
            continue;
        }
        if !q && matches!(ch.to_ascii_lowercase(), 'y' | 'd' | 'h' | 's') {
            return true;
        }
        if !q && ch.to_ascii_lowercase() == 'm' && !c.contains('0') && !c.contains('#') {
            return true;
        }
    }
    false
}

pub fn format_num(n: f64, code: &str) -> (String, Option<[u8; 3]>) {
    let code = if code.is_empty() { "General" } else { code };
    let secs = split_sections(code);
    let (sec, neg_handled) = if n < 0.0 && secs.len() >= 2 {
        (secs[1].clone(), true)
    } else if n == 0.0 && secs.len() >= 3 {
        (secs[2].clone(), false)
    } else {
        (secs[0].clone(), false)
    };
    let (sec, color) = take_color(&sec);
    let v = if neg_handled { n.abs() } else { n };
    if sec.trim().eq_ignore_ascii_case("general") || sec.trim().eq_ignore_ascii_case("основной") {
        return (fmt_general(v), color);
    }
    if sec.trim() == "@" {
        return (fmt_general(v), color);
    }
    if is_date_code(&sec) {
        return (format_date(v, &sec), color);
    }
    (format_number(v, &sec), color)
}

fn format_number(n: f64, sec: &str) -> String {
    // Split literal parts and the numeric picture.
    let mut prefix = String::new();
    let mut suffix = String::new();
    let mut picture = String::new();
    let mut q = false;
    let mut seen_digit = false;
    let mut chars = sec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            q = !q;
            continue;
        }
        if c == '\\' {
            if let Some(nc) = chars.next() {
                if seen_digit {
                    suffix.push(nc);
                } else {
                    prefix.push(nc);
                }
            }
            continue;
        }
        if c == '_' || c == '*' {
            chars.next();
            if c == '_' {
                if seen_digit {
                    suffix.push(' ');
                } else {
                    prefix.push(' ');
                }
            }
            continue;
        }
        if !q && "0#?.,%Ee+-".contains(c) && (c != '-' && c != '+' || picture.ends_with(['E', 'e'])) && !(c == ',' && !seen_digit && picture.is_empty()) {
            if "0#?".contains(c) {
                seen_digit = true;
            }
            if (c == 'E' || c == 'e') && !seen_digit {
                prefix.push(c);
                continue;
            }
            if !suffix.is_empty() && c != '%' {
                suffix.push(c);
                continue;
            }
            picture.push(c);
        } else if seen_digit {
            suffix.push(c);
        } else {
            prefix.push(c);
        }
    }
    let percent = picture.contains('%') || suffix.contains('%');
    let picture = picture.replace('%', "");
    let mut n = if percent { n * 100.0 } else { n };
    let neg = n < 0.0;
    n = n.abs();
    // Scientific.
    if let Some(ei) = picture.find(['E', 'e']) {
        let mant_pic = &picture[..ei];
        let dec = mant_pic.split('.').nth(1).map(|d| d.len()).unwrap_or(0);
        let exp_digits = picture[ei + 1..].trim_start_matches(['+', '-']).len().max(1);
        let (mut m, mut e) = if n == 0.0 { (0.0, 0) } else { (n / 10f64.powi(n.log10().floor() as i32), n.log10().floor() as i32) };
        if (m * 10f64.powi(dec as i32)).round() >= 10f64.powi(dec as i32 + 1) {
            m /= 10.0;
            e += 1;
        }
        let s = format!("{}{}{:.*}E{}{:0w$}{}", if neg { "-" } else { "" }, prefix, dec, m, if e < 0 { "-" } else { "+" }, e.abs(), suffix, w = exp_digits);
        return s;
    }
    // Trailing commas scale by thousands.
    let mut pic = picture.clone();
    while pic.ends_with(',') {
        pic.pop();
        n /= 1000.0;
    }
    let thousands = pic.contains(',');
    let (int_pic, dec_pic) = match pic.split_once('.') {
        Some((i, d)) => (i.to_string(), d.to_string()),
        None => (pic.clone(), String::new()),
    };
    let dec_required = dec_pic.chars().filter(|c| *c == '0').count();
    let dec_total = dec_pic.chars().filter(|c| "0#?".contains(*c)).count();
    let s = format!("{:.*}", dec_total, n);
    let (ip, dp) = s.split_once('.').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((s.clone(), String::new()));
    let mut dp = dp;
    while dp.len() > dec_required && dp.ends_with('0') {
        dp.pop();
    }
    let min_int = int_pic.chars().filter(|c| *c == '0').count();
    let mut ip = if ip == "0" && min_int == 0 { String::new() } else { ip };
    while ip.len() < min_int {
        ip.insert(0, '0');
    }
    if thousands {
        let digits: Vec<char> = ip.chars().collect();
        let mut out = String::new();
        for (i, c) in digits.iter().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                out.push('\u{a0}');
            }
            out.push(*c);
        }
        ip = out;
        if kit42::lang() == kit42::Lang::En {
            ip = ip.replace('\u{a0}', ",");
        }
    }
    let dec_sep = if kit42::lang() == kit42::Lang::Ru { "," } else { "." };
    let body = if dp.is_empty() { ip } else { format!("{ip}{dec_sep}{dp}") };
    let body = if body.is_empty() { "0".into() } else { body };
    format!("{}{}{}{}{}", if neg { "-" } else { "" }, prefix, body, if percent { "%" } else { "" }, suffix)
}

fn format_date(v: f64, sec: &str) -> String {
    use chrono::{Datelike, Timelike};
    let Some(dt) = serial_date(v) else { return "#".repeat(8) };
    let months_en = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let months_ru = ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"];
    let days_en = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    let days_ru = ["воскресенье", "понедельник", "вторник", "среда", "четверг", "пятница", "суббота"];
    let ru = kit42::lang() == kit42::Lang::Ru;
    let ampm = sec.to_lowercase().contains("am/pm");
    let cs: Vec<char> = sec.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut last_was_hour = false;
    while i < cs.len() {
        let c = cs[i];
        if c == '"' {
            i += 1;
            while i < cs.len() && cs[i] != '"' {
                out.push(cs[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        if c == '\\' {
            if i + 1 < cs.len() {
                out.push(cs[i + 1]);
            }
            i += 2;
            continue;
        }
        if c == '[' {
            // Elapsed [h], [m], [s]
            let end = cs[i..].iter().position(|x| *x == ']').map(|p| i + p).unwrap_or(cs.len() - 1);
            let tag: String = cs[i + 1..end].iter().collect::<String>().to_lowercase();
            let total = v * 86400.0;
            out.push_str(&match tag.chars().next() {
                Some('h') => format!("{:0w$}", (total / 3600.0).floor() as i64, w = tag.len()),
                Some('m') => format!("{:0w$}", (total / 60.0).floor() as i64, w = tag.len()),
                _ => format!("{:0w$}", total.floor() as i64, w = tag.len()),
            });
            last_was_hour = tag.starts_with('h');
            i = end + 1;
            continue;
        }
        let lc = c.to_ascii_lowercase();
        let mut run = 1;
        while i + run < cs.len() && cs[i + run].to_ascii_lowercase() == lc {
            run += 1;
        }
        match lc {
            'y' => out.push_str(&if run <= 2 { format!("{:02}", dt.year() % 100) } else { format!("{:04}", dt.year()) }),
            'd' => out.push_str(&match run {
                1 => dt.day().to_string(),
                2 => format!("{:02}", dt.day()),
                3 => {
                    let d = (if ru { days_ru } else { days_en })[dt.weekday().num_days_from_sunday() as usize];
                    d.chars().take(if ru { 2 } else { 3 }).collect()
                }
                _ => (if ru { days_ru } else { days_en })[dt.weekday().num_days_from_sunday() as usize].to_string(),
            }),
            'm' => {
                // Minutes when after hours or before seconds.
                let next_is_sec = cs[i + run..].iter().find(|x| x.is_alphabetic()).map(|x| x.to_ascii_lowercase() == 's').unwrap_or(false);
                if (last_was_hour || next_is_sec) && run <= 2 {
                    out.push_str(&if run == 2 { format!("{:02}", dt.minute()) } else { dt.minute().to_string() });
                } else {
                    out.push_str(&match run {
                        1 => dt.month().to_string(),
                        2 => format!("{:02}", dt.month()),
                        3 => {
                            let m = (if ru { months_ru } else { months_en })[dt.month0() as usize];
                            m.chars().take(3).collect()
                        }
                        5 => months_en[dt.month0() as usize].chars().take(1).collect(),
                        _ => (if ru { months_ru } else { months_en })[dt.month0() as usize].to_string(),
                    });
                }
            }
            'h' => {
                let h = if ampm { (dt.hour() + 11) % 12 + 1 } else { dt.hour() };
                out.push_str(&if run >= 2 { format!("{:02}", h) } else { h.to_string() });
                last_was_hour = true;
                i += run;
                continue;
            }
            's' => out.push_str(&if run >= 2 { format!("{:02}", dt.second()) } else { dt.second().to_string() }),
            'a' if sec[i..].to_lowercase().starts_with("am/pm") => {
                out.push_str(if dt.hour() < 12 { "AM" } else { "PM" });
                i += 5;
                continue;
            }
            _ => {
                for _ in 0..run {
                    out.push(c);
                }
            }
        }
        if lc != ' ' && lc != ':' {
            last_was_hour = false;
        }
        i += run;
    }
    out
}

/// Presets offered in the toolbar.
pub fn presets() -> Vec<(&'static str, &'static str)> {
    use kit42::t;
    vec![
        (t("General", "Общий"), "General"),
        (t("Number", "Числовой"), "#,##0.00"),
        (t("Integer", "Целое"), "#,##0"),
        (t("Currency ₽", "Денежный ₽"), "#,##0.00 \"₽\""),
        (t("Currency $", "Денежный $"), "$#,##0.00"),
        (t("Currency €", "Денежный €"), "#,##0.00 \"€\""),
        (t("Accounting (red negatives)", "Финансовый (отрицательные красным)"), "#,##0.00;[Red]-#,##0.00"),
        (t("Percent", "Процентный"), "0%"),
        (t("Percent 0.00", "Процентный 0,00"), "0.00%"),
        (t("Date", "Дата"), "dd.mm.yyyy"),
        (t("Long date", "Длинная дата"), "d mmmm yyyy"),
        (t("ISO date", "Дата ISO"), "yyyy-mm-dd"),
        (t("Time", "Время"), "hh:mm"),
        (t("Time with seconds", "Время с секундами"), "hh:mm:ss"),
        (t("Date and time", "Дата и время"), "dd.mm.yyyy hh:mm"),
        (t("Elapsed hours", "Прошедшие часы"), "[h]:mm:ss"),
        (t("Scientific", "Экспоненциальный"), "0.00E+00"),
        (t("Text", "Текстовый"), "@"),
    ]
}
