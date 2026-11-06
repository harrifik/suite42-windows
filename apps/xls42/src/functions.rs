//! Spreadsheet function library.

use crate::formula::{compare, num, scalar_op, truthy, Eval, Expr, Rng, V};
use crate::model::*;

fn e(s: &str) -> Value {
    Value::Err(s.into())
}

macro_rules! t {
    ($x:expr) => {
        match $x {
            Ok(v) => v,
            Err(err) => return V::S(err),
        }
    };
}

fn n(v: f64) -> V {
    if v.is_finite() {
        V::S(Value::Num(v))
    } else {
        V::S(e("#NUM!"))
    }
}

fn s(v: String) -> V {
    V::S(Value::Str(v))
}

fn b(v: bool) -> V {
    V::S(Value::Bool(v))
}

/// Criteria like ">5", "<>x", "a*", "=10" used by COUNTIF/SUMIF/… .
pub fn criteria_match(crit: &Value, v: &Value) -> bool {
    let text = crit.as_text();
    let (op, rest) = ["<>", ">=", "<=", "=", ">", "<"]
        .iter()
        .find(|o| text.starts_with(**o))
        .map(|o| (*o, &text[o.len()..]))
        .unwrap_or(("=", text.as_str()));
    let target = if matches!(crit, Value::Num(_)) && op == "=" && rest == text { crit.clone() } else { parse_input(rest).0 };
    match (&target, v) {
        (Value::Num(t), _) => match v {
            Value::Num(x) => cmp_op(op, x.partial_cmp(t).unwrap_or(std::cmp::Ordering::Equal)),
            Value::Str(sx) if op == "<>" => sx.parse::<f64>().map(|x| x != *t).unwrap_or(true),
            Value::Empty => op == "<>",
            _ => op == "<>",
        },
        (Value::Bool(t), Value::Bool(x)) => cmp_op(op, x.cmp(t)),
        (Value::Empty, _) => match op {
            "=" => v.is_empty() || v.as_text().is_empty(),
            "<>" => !(v.is_empty() || v.as_text().is_empty()),
            _ => false,
        },
        _ => {
            let pat = rest.to_lowercase();
            let hay = v.as_text().to_lowercase();
            if matches!(v, Value::Num(_)) && op != "<>" && op != "=" {
                return false;
            }
            match op {
                "=" => wildcard(&pat, &hay),
                "<>" => !wildcard(&pat, &hay),
                _ => cmp_op(op, hay.cmp(&pat)),
            }
        }
    }
}

fn cmp_op(op: &str, o: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::*;
    match op {
        "=" => o == Equal,
        "<>" => o != Equal,
        ">" => o == Greater,
        "<" => o == Less,
        ">=" => o != Less,
        "<=" => o != Greater,
        _ => false,
    }
}

/// Excel wildcards: * any run, ? one char, ~ escapes.
pub fn wildcard(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = s.chars().collect();
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => (0..=t.len()).any(|i| go(&p[1..], &t[i..])),
            Some('?') => !t.is_empty() && go(&p[1..], &t[1..]),
            Some('~') if p.len() > 1 => !t.is_empty() && t[0] == p[1] && go(&p[2..], &t[1..]),
            Some(c) => !t.is_empty() && t[0] == *c && go(&p[1..], &t[1..]),
        }
    }
    if p.len() > 200 || t.len() > 10000 {
        return pat == s;
    }
    go(&p, &t)
}

fn stats(xs: &[f64]) -> (f64, f64) {
    let m = xs.iter().sum::<f64>() / xs.len() as f64;
    let ss = xs.iter().map(|x| (x - m).powi(2)).sum::<f64>();
    (m, ss)
}

fn percentile(mut xs: Vec<f64>, k: f64, exclusive: bool) -> Result<f64, Value> {
    if xs.is_empty() || !(0.0..=1.0).contains(&k) {
        return Err(e("#NUM!"));
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let nlen = xs.len() as f64;
    let pos = if exclusive {
        let p = k * (nlen + 1.0) - 1.0;
        if p < 0.0 || p > nlen - 1.0 {
            return Err(e("#NUM!"));
        }
        p
    } else {
        k * (nlen - 1.0)
    };
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(xs.len() - 1);
    Ok(xs[lo] + (xs[hi] - xs[lo]) * (pos - lo as f64))
}

fn erf(x: f64) -> f64 {
    // Abramowitz–Stegun 7.1.26 with refinement.
    let t = 1.0 / (1.0 + 0.5 * x.abs());
    let y = 1.0 - t * (-x * x - 1.26551223 + t * (1.00002368 + t * (0.37409196 + t * (0.09678418 + t * (-0.18628806 + t * (0.27886807 + t * (-1.13520398 + t * (1.48851587 + t * (-0.82215223 + t * 0.17087277))))))))).exp();
    if x >= 0.0 {
        y
    } else {
        -y
    }
}

fn norm_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

fn norm_inv(p: f64) -> f64 {
    // Acklam's algorithm.
    let a = [-3.969683028665376e+01, 2.209460984245205e+02, -2.759285104469687e+02, 1.383577518672690e+02, -3.066479806614716e+01, 2.506628277459239e+00];
    let b = [-5.447609879822406e+01, 1.615858368580409e+02, -1.556989798598866e+02, 6.680131188771972e+01, -1.328068155288572e+01];
    let c = [-7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e+00, -2.549732539343734e+00, 4.374664141464968e+00, 2.938163982698783e+00];
    let d = [7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e+00, 3.754408661907416e+00];
    let pl = 0.02425;
    if p < pl {
        let q = (-2.0 * p.ln()).sqrt();
        (((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]) / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0)
    } else if p <= 1.0 - pl {
        let q = p - 0.5;
        let r = q * q;
        (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q / (((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]) / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0)
    }
}

fn date_parts(v: f64) -> Option<(i32, u32, u32)> {
    use chrono::Datelike;
    serial_date(v).map(|d| (d.year(), d.month(), d.day()))
}

fn add_months(serial: f64, months: i32, end_of_month: bool) -> Option<f64> {
    use chrono::Datelike;
    let d = serial_date(serial)?.date();
    let total = d.year() * 12 + d.month() as i32 - 1 + months;
    let (y, m) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    let last = (1..=31).rev().find(|dd| chrono::NaiveDate::from_ymd_opt(y, m, *dd).is_some())?;
    let day = if end_of_month { last } else { d.day().min(last) };
    date_serial(y, m, day)
}

fn weekday(serial: f64) -> u32 {
    // 1 = Sunday (Excel type 1)
    (((serial.floor() as i64 - 1).rem_euclid(7)) + 1) as u32
}

fn npv(rate: f64, cfs: &[f64]) -> f64 {
    cfs.iter().enumerate().map(|(i, c)| c / (1.0 + rate).powi(i as i32 + 1)).sum()
}

fn irr(cfs: &[f64], guess: f64) -> Option<f64> {
    let mut r = guess;
    for _ in 0..100 {
        let f: f64 = cfs.iter().enumerate().map(|(i, c)| c / (1.0 + r).powi(i as i32)).sum();
        let df: f64 = cfs.iter().enumerate().map(|(i, c)| -(i as f64) * c / (1.0 + r).powi(i as i32 + 1)).sum();
        if df.abs() < 1e-14 {
            return None;
        }
        let nr = r - f / df;
        if (nr - r).abs() < 1e-10 {
            return Some(nr);
        }
        r = nr;
    }
    None
}

fn pmt(rate: f64, nper: f64, pv: f64, fv: f64, typ: f64) -> f64 {
    if rate == 0.0 {
        return -(pv + fv) / nper;
    }
    let p = (1.0 + rate).powf(nper);
    -(rate * (pv * p + fv)) / ((1.0 + rate * typ) * (p - 1.0))
}

fn fv(rate: f64, nper: f64, pmt_: f64, pv: f64, typ: f64) -> f64 {
    if rate == 0.0 {
        return -(pv + pmt_ * nper);
    }
    let p = (1.0 + rate).powf(nper);
    -(pv * p + pmt_ * (1.0 + rate * typ) * (p - 1.0) / rate)
}

fn ipmt(rate: f64, per: f64, nper: f64, pv: f64, fv_: f64, typ: f64) -> f64 {
    let p = pmt(rate, nper, pv, fv_, typ);
    let mut interest = if per == 1.0 {
        if typ == 1.0 {
            0.0
        } else {
            -pv
        }
    } else if typ == 1.0 {
        fv(rate, per - 2.0, p, pv, 1.0) - p
    } else {
        fv(rate, per - 1.0, p, pv, 0.0)
    };
    interest *= rate;
    interest
}

fn roman(mut n: i64) -> String {
    let t = [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
    let mut s = String::new();
    for (v, r) in t {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn lookup_match(hay: &[Value], key: &Value, mode: i32) -> Option<usize> {
    match mode {
        0 => {
            let wild = matches!(key, Value::Str(s) if s.contains('*') || s.contains('?'));
            hay.iter().position(|v| if wild { matches!(v, Value::Str(_)) && wildcard(&key.as_text().to_lowercase(), &v.as_text().to_lowercase()) } else { compare(v, key) == std::cmp::Ordering::Equal && std::mem::discriminant(v) == std::mem::discriminant(key) || (matches!((v, key), (Value::Num(a), Value::Num(b)) if a == b)) })
        }
        1 => {
            // Largest value <= key in ascending data (binary search like Excel).
            let mut best = None;
            for (i, v) in hay.iter().enumerate() {
                if v.is_empty() {
                    continue;
                }
                if compare(v, key) != std::cmp::Ordering::Greater && std::mem::discriminant(v) == std::mem::discriminant(key) {
                    best = Some(i);
                } else if std::mem::discriminant(v) == std::mem::discriminant(key) {
                    break;
                }
            }
            best
        }
        _ => {
            let mut best = None;
            for (i, v) in hay.iter().enumerate() {
                if compare(v, key) != std::cmp::Ordering::Less {
                    best = Some(i);
                } else {
                    break;
                }
            }
            best
        }
    }
}

pub fn call(ev: &mut Eval, name: &str, a: &[Expr]) -> V {
    match name {
        // ───────── logic ─────────
        "IF" => {
            let c = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            let cond = t!(truthy(&c));
            match (cond, a.get(1), a.get(2)) {
                (true, Some(x), _) => ev.eval(x),
                (true, None, _) => b(true),
                (false, _, Some(y)) => ev.eval(y),
                (false, _, None) => b(false),
            }
        }
        "IFS" => {
            for pair in a.chunks(2) {
                if pair.len() == 2 {
                    let c = ev.scalar(&pair[0]);
                    if t!(truthy(&c)) {
                        return ev.eval(&pair[1]);
                    }
                }
            }
            V::S(e("#N/A"))
        }
        "IFERROR" | "IFNA" => {
            let v = ev.eval(a.first().unwrap_or(&Expr::Missing));
            let sv = match &v {
                V::S(x) => x.clone(),
                _ => Value::Empty,
            };
            match sv {
                Value::Err(er) if name == "IFERROR" || er == "#N/A" => a.get(1).map(|x| ev.eval(x)).unwrap_or(V::S(Value::Str(String::new()))),
                _ => v,
            }
        }
        "AND" | "OR" | "XOR" => {
            let mut vals = vec![];
            for x in a {
                let v = ev.eval(x);
                for item in ev.values_of(v) {
                    match item {
                        Value::Err(er) => return V::S(Value::Err(er)),
                        Value::Bool(bb) => vals.push(bb),
                        Value::Num(nn) => vals.push(nn != 0.0),
                        _ => {}
                    }
                }
            }
            if vals.is_empty() {
                return V::S(e("#VALUE!"));
            }
            b(match name {
                "AND" => vals.iter().all(|x| *x),
                "OR" => vals.iter().any(|x| *x),
                _ => vals.iter().filter(|x| **x).count() % 2 == 1,
            })
        }
        "NOT" => {
            let v = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            b(!t!(truthy(&v)))
        }
        "TRUE" => b(true),
        "FALSE" => b(false),
        "SWITCH" => {
            let key = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            let rest = &a[1.min(a.len())..];
            for pair in rest.chunks(2) {
                if pair.len() == 2 {
                    let v = ev.scalar(&pair[0]);
                    if compare(&v, &key) == std::cmp::Ordering::Equal {
                        return ev.eval(&pair[1]);
                    }
                } else {
                    return ev.eval(&pair[0]);
                }
            }
            V::S(e("#N/A"))
        }
        "CHOOSE" => {
            let i = t!(ev.num_arg(a, 0, None)) as usize;
            match a.get(i) {
                Some(x) if i >= 1 => ev.eval(x),
                _ => V::S(e("#VALUE!")),
            }
        }

        // ───────── math ─────────
        "SUM" => n(t!(ev.nums_pub(a, false)).iter().sum()),
        "PRODUCT" => n(t!(ev.nums_pub(a, false)).iter().product()),
        "SUMSQ" => n(t!(ev.nums_pub(a, false)).iter().map(|x| x * x).sum()),
        "SUMPRODUCT" => {
            let arrays: Vec<Vec<Value>> = (0..a.len()).map(|i| ev.array_arg(a, i).into_iter().flatten().collect()).collect();
            if arrays.is_empty() {
                return V::S(e("#VALUE!"));
            }
            let len = arrays[0].len();
            if arrays.iter().any(|x| x.len() != len) {
                return V::S(e("#VALUE!"));
            }
            let mut total = 0.0;
            for i in 0..len {
                let mut p = 1.0;
                for arr in &arrays {
                    p *= match &arr[i] {
                        Value::Num(x) => *x,
                        Value::Bool(bb) => *bb as i32 as f64,
                        Value::Err(er) => return V::S(Value::Err(er.clone())),
                        _ => 0.0,
                    };
                }
                total += p;
            }
            n(total)
        }
        "ABS" | "SQRT" | "EXP" | "LN" | "LOG10" | "INT" | "SIGN" | "SIN" | "COS" | "TAN" | "ASIN" | "ACOS" | "ATAN" | "SINH" | "COSH" | "TANH" | "DEGREES" | "RADIANS" | "FACT" | "EVEN" | "ODD" | "ASINH" | "ACOSH" | "ATANH" | "COT" => {
            let v = ev.eval(a.first().unwrap_or(&Expr::Missing));
            let f = |x: f64| -> Value {
                let r = match name {
                    "ABS" => x.abs(),
                    "SQRT" => {
                        if x < 0.0 {
                            return e("#NUM!");
                        }
                        x.sqrt()
                    }
                    "EXP" => x.exp(),
                    "LN" => {
                        if x <= 0.0 {
                            return e("#NUM!");
                        }
                        x.ln()
                    }
                    "LOG10" => {
                        if x <= 0.0 {
                            return e("#NUM!");
                        }
                        x.log10()
                    }
                    "INT" => x.floor(),
                    "SIGN" => {
                        if x > 0.0 {
                            1.0
                        } else if x < 0.0 {
                            -1.0
                        } else {
                            0.0
                        }
                    }
                    "SIN" => x.sin(),
                    "COS" => x.cos(),
                    "TAN" => x.tan(),
                    "COT" => 1.0 / x.tan(),
                    "ASIN" => x.asin(),
                    "ACOS" => x.acos(),
                    "ATAN" => x.atan(),
                    "SINH" => x.sinh(),
                    "COSH" => x.cosh(),
                    "TANH" => x.tanh(),
                    "ASINH" => x.asinh(),
                    "ACOSH" => x.acosh(),
                    "ATANH" => x.atanh(),
                    "DEGREES" => x.to_degrees(),
                    "RADIANS" => x.to_radians(),
                    "FACT" => {
                        if !(0.0..171.0).contains(&x) {
                            return e("#NUM!");
                        }
                        (1..=x.floor() as u64).map(|k| k as f64).product()
                    }
                    "EVEN" => {
                        let c = (x.abs() / 2.0).ceil() * 2.0;
                        c * x.signum()
                    }
                    _ => {
                        let mut c = x.abs().ceil();
                        if c % 2.0 == 0.0 {
                            c += 1.0;
                        }
                        c * if x < 0.0 { -1.0 } else { 1.0 }
                    }
                };
                if r.is_finite() {
                    Value::Num(r)
                } else {
                    e("#NUM!")
                }
            };
            match v {
                V::S(x) => V::S(match num(&x) {
                    Ok(v) => f(v),
                    Err(er) => er,
                }),
                other => {
                    let arr = ev.array_of(other);
                    V::A(arr.into_iter().map(|r| r.into_iter().map(|x| num(&x).map(f).unwrap_or_else(|er| er)).collect()).collect())
                }
            }
        }
        "PI" => n(std::f64::consts::PI),
        "RAND" => n(rand::random::<f64>()),
        "RANDBETWEEN" => {
            let lo = t!(ev.num_arg(a, 0, None)).ceil();
            let hi = t!(ev.num_arg(a, 1, None)).floor();
            if hi < lo {
                return V::S(e("#NUM!"));
            }
            n((lo + (rand::random::<f64>() * (hi - lo + 1.0)).floor()).min(hi))
        }
        "ROUND" | "ROUNDUP" | "ROUNDDOWN" => {
            let x = t!(ev.num_arg(a, 0, None));
            let d = t!(ev.num_arg(a, 1, Some(0.0))).trunc() as i32;
            let m = 10f64.powi(d);
            let y = x * m;
            let y = (y * 1e9).round() / 1e9;
            let r = match name {
                "ROUND" => y.abs().round() * y.signum(),
                "ROUNDUP" => y.abs().ceil() * y.signum(),
                _ => y.abs().floor() * y.signum(),
            };
            n(r / m)
        }
        "MROUND" => {
            let x = t!(ev.num_arg(a, 0, None));
            let m = t!(ev.num_arg(a, 1, None));
            if m == 0.0 {
                return n(0.0);
            }
            if x * m < 0.0 {
                return V::S(e("#NUM!"));
            }
            n((x / m).round() * m)
        }
        "CEILING" | "CEILING.MATH" | "FLOOR" | "FLOOR.MATH" | "CEILING.PRECISE" | "FLOOR.PRECISE" => {
            let x = t!(ev.num_arg(a, 0, None));
            let sig = t!(ev.num_arg(a, 1, Some(if x < 0.0 && name == "CEILING" { -1.0 } else { 1.0 }))).abs();
            if sig == 0.0 {
                return n(0.0);
            }
            n(if name.starts_with("CEILING") { (x / sig).ceil() * sig } else { (x / sig).floor() * sig })
        }
        "TRUNC" => {
            let x = t!(ev.num_arg(a, 0, None));
            let d = t!(ev.num_arg(a, 1, Some(0.0))) as i32;
            let m = 10f64.powi(d);
            n((x * m).trunc() / m)
        }
        "MOD" => {
            let x = t!(ev.num_arg(a, 0, None));
            let d = t!(ev.num_arg(a, 1, None));
            if d == 0.0 {
                return V::S(e("#DIV/0!"));
            }
            n(x - d * (x / d).floor())
        }
        "QUOTIENT" => {
            let x = t!(ev.num_arg(a, 0, None));
            let d = t!(ev.num_arg(a, 1, None));
            if d == 0.0 {
                return V::S(e("#DIV/0!"));
            }
            n((x / d).trunc())
        }
        "POWER" => {
            let x = t!(ev.num_arg(a, 0, None));
            let y = t!(ev.num_arg(a, 1, None));
            n(x.powf(y))
        }
        "LOG" => {
            let x = t!(ev.num_arg(a, 0, None));
            let base = t!(ev.num_arg(a, 1, Some(10.0)));
            if x <= 0.0 || base <= 0.0 || base == 1.0 {
                return V::S(e("#NUM!"));
            }
            n(x.ln() / base.ln())
        }
        "ATAN2" => {
            let x = t!(ev.num_arg(a, 0, None));
            let y = t!(ev.num_arg(a, 1, None));
            if x == 0.0 && y == 0.0 {
                return V::S(e("#DIV/0!"));
            }
            n(y.atan2(x))
        }
        "GCD" | "LCM" => {
            let xs = t!(ev.nums_pub(a, false));
            fn gcd(a: u64, b: u64) -> u64 {
                if b == 0 {
                    a
                } else {
                    gcd(b, a % b)
                }
            }
            let ints: Vec<u64> = xs.iter().map(|x| x.trunc().abs() as u64).collect();
            if name == "GCD" {
                n(ints.iter().fold(0, |acc, x| gcd(acc, *x)) as f64)
            } else {
                n(ints.iter().fold(1u64, |acc, x| if *x == 0 { 0 } else { acc / gcd(acc, *x) * x }) as f64)
            }
        }
        "COMBIN" | "PERMUT" => {
            let nn = t!(ev.num_arg(a, 0, None)).floor();
            let k = t!(ev.num_arg(a, 1, None)).floor();
            if k > nn || k < 0.0 {
                return V::S(e("#NUM!"));
            }
            let mut r = 1.0;
            for i in 0..k as u64 {
                r *= nn - i as f64;
                if name == "COMBIN" {
                    r /= i as f64 + 1.0;
                }
            }
            n(r.round())
        }
        "ROMAN" => {
            let x = t!(ev.num_arg(a, 0, None)) as i64;
            if !(0..4000).contains(&x) {
                return V::S(e("#VALUE!"));
            }
            s(roman(x))
        }
        "ARABIC" => {
            let t2 = t!(ev.str_arg(a, 0, "")).to_uppercase();
            let val = |c: char| match c {
                'I' => 1,
                'V' => 5,
                'X' => 10,
                'L' => 50,
                'C' => 100,
                'D' => 500,
                'M' => 1000,
                _ => 0,
            };
            let cs: Vec<i64> = t2.chars().map(val).collect();
            let mut total = 0;
            for i in 0..cs.len() {
                if i + 1 < cs.len() && cs[i] < cs[i + 1] {
                    total -= cs[i];
                } else {
                    total += cs[i];
                }
            }
            n(total as f64)
        }
        "BASE" | "DEC2BIN" | "DEC2HEX" | "DEC2OCT" => {
            let x = t!(ev.num_arg(a, 0, None)) as i64;
            let (radix, len_i) = match name {
                "BASE" => (t!(ev.num_arg(a, 1, None)) as u32, 2),
                "DEC2BIN" => (2, 1),
                "DEC2HEX" => (16, 1),
                _ => (8, 1),
            };
            if !(2..=36).contains(&radix) || x < 0 {
                return V::S(e("#NUM!"));
            }
            let mut v = x as u64;
            let mut digits = vec![];
            loop {
                digits.push(std::char::from_digit((v % radix as u64) as u32, radix).unwrap().to_ascii_uppercase());
                v /= radix as u64;
                if v == 0 {
                    break;
                }
            }
            let mut out: String = digits.iter().rev().collect();
            if let Ok(min) = ev.num_arg(a, len_i, Some(0.0)) {
                while out.len() < min as usize {
                    out.insert(0, '0');
                }
            }
            s(out)
        }
        "DECIMAL" | "BIN2DEC" | "HEX2DEC" | "OCT2DEC" => {
            let txt = t!(ev.str_arg(a, 0, ""));
            let radix = match name {
                "DECIMAL" => t!(ev.num_arg(a, 1, None)) as u32,
                "BIN2DEC" => 2,
                "HEX2DEC" => 16,
                _ => 8,
            };
            match u64::from_str_radix(txt.trim(), radix) {
                Ok(v) => n(v as f64),
                Err(_) => V::S(e("#NUM!")),
            }
        }
        "SEQUENCE" => {
            let rows = t!(ev.num_arg(a, 0, None)) as usize;
            let cols = t!(ev.num_arg(a, 1, Some(1.0))) as usize;
            let start = t!(ev.num_arg(a, 2, Some(1.0)));
            let step = t!(ev.num_arg(a, 3, Some(1.0)));
            if rows == 0 || cols == 0 || rows * cols > 1_000_000 {
                return V::S(e("#VALUE!"));
            }
            V::A((0..rows).map(|r| (0..cols).map(|c| Value::Num(start + step * (r * cols + c) as f64)).collect()).collect())
        }
        "SUBTOTAL" | "AGGREGATE" => {
            let code = t!(ev.num_arg(a, 0, None)) as i64 % 100;
            let rest = if name == "AGGREGATE" { &a[2.min(a.len())..] } else { &a[1.min(a.len())..] };
            let fname = match code {
                1 => "AVERAGE",
                2 => "COUNT",
                3 => "COUNTA",
                4 => "MAX",
                5 => "MIN",
                6 => "PRODUCT",
                7 => "STDEV.S",
                8 => "STDEV.P",
                9 => "SUM",
                10 => "VAR.S",
                11 => "VAR.P",
                12 => "MEDIAN",
                13 => "MODE.SNGL",
                14 => "LARGE",
                15 => "SMALL",
                _ => return V::S(e("#VALUE!")),
            };
            call(ev, fname, rest)
        }

        // ───────── statistics ─────────
        "AVERAGE" | "AVERAGEA" => {
            let xs = t!(ev.nums_pub(a, name == "AVERAGEA"));
            if xs.is_empty() {
                return V::S(e("#DIV/0!"));
            }
            n(xs.iter().sum::<f64>() / xs.len() as f64)
        }
        "COUNT" => {
            let mut c = 0;
            for x in a {
                let v = ev.eval(x);
                c += ev.values_of(v).iter().filter(|v| matches!(v, Value::Num(_))).count();
            }
            n(c as f64)
        }
        "COUNTA" => {
            let mut c = 0;
            for x in a {
                let v = ev.eval(x);
                c += ev.values_of(v).iter().filter(|v| !v.is_empty()).count();
            }
            n(c as f64)
        }
        "COUNTBLANK" => {
            let Some(g) = ev.range_arg(a, 0) else { return V::S(e("#VALUE!")) };
            let total = (g.r1 - g.r0 + 1) as f64 * (g.c1 - g.c0 + 1) as f64;
            let filled = ev.values_of(V::R(g)).iter().filter(|v| !v.is_empty() && v.as_text() != "").count();
            n(total - filled as f64)
        }
        "MAX" | "MAXA" | "MIN" | "MINA" => {
            let xs = t!(ev.nums_pub(a, name.ends_with('A')));
            if xs.is_empty() {
                return n(0.0);
            }
            n(if name.starts_with("MAX") { xs.iter().cloned().fold(f64::MIN, f64::max) } else { xs.iter().cloned().fold(f64::MAX, f64::min) })
        }
        "MEDIAN" => {
            let mut xs = t!(ev.nums_pub(a, false));
            if xs.is_empty() {
                return V::S(e("#NUM!"));
            }
            xs.sort_by(|x, y| x.partial_cmp(y).unwrap());
            let m = xs.len() / 2;
            n(if xs.len() % 2 == 0 { (xs[m - 1] + xs[m]) / 2.0 } else { xs[m] })
        }
        "MODE" | "MODE.SNGL" => {
            let xs = t!(ev.nums_pub(a, false));
            let mut best = (0, f64::NAN);
            for x in &xs {
                let c = xs.iter().filter(|y| *y == x).count();
                if c > best.0 {
                    best = (c, *x);
                }
            }
            if best.0 < 2 {
                return V::S(e("#N/A"));
            }
            n(best.1)
        }
        "STDEV" | "STDEV.S" | "STDEVA" | "STDEVP" | "STDEV.P" | "VAR" | "VAR.S" | "VARP" | "VAR.P" | "DEVSQ" | "AVEDEV" => {
            let xs = t!(ev.nums_pub(a, name == "STDEVA"));
            let pop = name.ends_with('P');
            if xs.len() < if pop { 1 } else { 2 } {
                return V::S(e("#DIV/0!"));
            }
            let (m, ss) = stats(&xs);
            let var = ss / (xs.len() as f64 - if pop { 0.0 } else { 1.0 });
            n(match name {
                "DEVSQ" => ss,
                "AVEDEV" => xs.iter().map(|x| (x - m).abs()).sum::<f64>() / xs.len() as f64,
                _ if name.starts_with("VAR") => var,
                _ => var.sqrt(),
            })
        }
        "GEOMEAN" | "HARMEAN" => {
            let xs = t!(ev.nums_pub(a, false));
            if xs.is_empty() || xs.iter().any(|x| *x <= 0.0) {
                return V::S(e("#NUM!"));
            }
            n(if name == "GEOMEAN" { (xs.iter().map(|x| x.ln()).sum::<f64>() / xs.len() as f64).exp() } else { xs.len() as f64 / xs.iter().map(|x| 1.0 / x).sum::<f64>() })
        }
        "LARGE" | "SMALL" => {
            let mut xs = t!(ev.nums_pub(&a[..1.min(a.len())], false));
            let k = t!(ev.num_arg(a, 1, None)) as usize;
            if k == 0 || k > xs.len() {
                return V::S(e("#NUM!"));
            }
            xs.sort_by(|x, y| x.partial_cmp(y).unwrap());
            n(if name == "LARGE" { xs[xs.len() - k] } else { xs[k - 1] })
        }
        "RANK" | "RANK.EQ" | "RANK.AVG" => {
            let x = t!(ev.num_arg(a, 0, None));
            let xs = t!(ev.nums_pub(&a[1..2.min(a.len())], false));
            let asc = t!(ev.num_arg(a, 2, Some(0.0))) != 0.0;
            let better = xs.iter().filter(|y| if asc { **y < x } else { **y > x }).count();
            let ties = xs.iter().filter(|y| **y == x).count();
            if ties == 0 {
                return V::S(e("#N/A"));
            }
            n(if name == "RANK.AVG" { better as f64 + (ties as f64 + 1.0) / 2.0 } else { better as f64 + 1.0 })
        }
        "PERCENTILE" | "PERCENTILE.INC" | "PERCENTILE.EXC" | "QUARTILE" | "QUARTILE.INC" | "QUARTILE.EXC" => {
            let xs = t!(ev.nums_pub(&a[..1.min(a.len())], false));
            let mut k = t!(ev.num_arg(a, 1, None));
            if name.starts_with("QUARTILE") {
                k /= 4.0;
            }
            n(t!(percentile(xs, k, name.ends_with("EXC"))))
        }
        "CORREL" | "PEARSON" | "COVAR" | "COVARIANCE.P" | "COVARIANCE.S" | "SLOPE" | "INTERCEPT" | "RSQ" | "FORECAST" | "FORECAST.LINEAR" | "STEYX" => {
            let (yi, xi) = if name.starts_with("FORECAST") { (1, 2) } else if name == "SLOPE" || name == "INTERCEPT" || name == "RSQ" || name == "STEYX" { (0, 1) } else { (0, 1) };
            let ya: Vec<Value> = ev.array_arg(a, yi).into_iter().flatten().collect();
            let xa: Vec<Value> = ev.array_arg(a, xi).into_iter().flatten().collect();
            if ya.len() != xa.len() {
                return V::S(e("#N/A"));
            }
            let pairs: Vec<(f64, f64)> = ya.iter().zip(xa.iter()).filter_map(|(y, x)| match (y, x) {
                (Value::Num(y), Value::Num(x)) => Some((*y, *x)),
                _ => None,
            }).collect();
            let cnt = pairs.len() as f64;
            if cnt < 2.0 {
                return V::S(e("#DIV/0!"));
            }
            let my = pairs.iter().map(|p| p.0).sum::<f64>() / cnt;
            let mx = pairs.iter().map(|p| p.1).sum::<f64>() / cnt;
            let sxy: f64 = pairs.iter().map(|(y, x)| (x - mx) * (y - my)).sum();
            let sxx: f64 = pairs.iter().map(|(_, x)| (x - mx).powi(2)).sum();
            let syy: f64 = pairs.iter().map(|(y, _)| (y - my).powi(2)).sum();
            match name {
                "CORREL" | "PEARSON" => n(sxy / (sxx * syy).sqrt()),
                "RSQ" => n((sxy / (sxx * syy).sqrt()).powi(2)),
                "COVAR" | "COVARIANCE.P" => n(sxy / cnt),
                "COVARIANCE.S" => n(sxy / (cnt - 1.0)),
                "SLOPE" => n(sxy / sxx),
                "INTERCEPT" => n(my - sxy / sxx * mx),
                "STEYX" => n(((syy - sxy * sxy / sxx) / (cnt - 2.0)).sqrt()),
                _ => {
                    let x = t!(ev.num_arg(a, 0, None));
                    n(my + sxy / sxx * (x - mx))
                }
            }
        }
        "NORM.DIST" | "NORMDIST" => {
            let x = t!(ev.num_arg(a, 0, None));
            let m = t!(ev.num_arg(a, 1, None));
            let sd = t!(ev.num_arg(a, 2, None));
            let cum = t!(ev.bool_arg(a, 3, true));
            if sd <= 0.0 {
                return V::S(e("#NUM!"));
            }
            let z = (x - m) / sd;
            n(if cum { norm_cdf(z) } else { (-z * z / 2.0).exp() / (sd * (2.0 * std::f64::consts::PI).sqrt()) })
        }
        "NORM.S.DIST" | "NORMSDIST" => {
            let z = t!(ev.num_arg(a, 0, None));
            let cum = t!(ev.bool_arg(a, 1, true));
            n(if cum { norm_cdf(z) } else { (-z * z / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt() })
        }
        "NORM.INV" | "NORMINV" => {
            let p = t!(ev.num_arg(a, 0, None));
            let m = t!(ev.num_arg(a, 1, None));
            let sd = t!(ev.num_arg(a, 2, None));
            if p <= 0.0 || p >= 1.0 || sd <= 0.0 {
                return V::S(e("#NUM!"));
            }
            n(m + sd * norm_inv(p))
        }
        "NORM.S.INV" | "NORMSINV" => {
            let p = t!(ev.num_arg(a, 0, None));
            if p <= 0.0 || p >= 1.0 {
                return V::S(e("#NUM!"));
            }
            n(norm_inv(p))
        }
        "STANDARDIZE" => {
            let x = t!(ev.num_arg(a, 0, None));
            let m = t!(ev.num_arg(a, 1, None));
            let sd = t!(ev.num_arg(a, 2, None));
            n((x - m) / sd)
        }
        "COUNTIF" | "SUMIF" | "AVERAGEIF" => {
            let rg = ev.range_arg(a, 0);
            let crit = ev.scalar(a.get(1).unwrap_or(&Expr::Missing));
            let cells = match rg {
                Some(g) => ev.array_of(V::R(g)),
                None => ev.array_arg(a, 0),
            };
            let sums = if name != "COUNTIF" && a.len() > 2 {
                match (ev.range_arg(a, 2), rg) {
                    (Some(g), Some(src)) => Some(ev.array_of(V::R(Rng { r1: g.r0 + (src.r1 - src.r0), c1: g.c0 + (src.c1 - src.c0), ..g }))),
                    _ => Some(ev.array_arg(a, 2)),
                }
            } else {
                None
            };
            let (mut c, mut total) = (0, 0.0);
            for (i, row) in cells.iter().enumerate() {
                for (j, v) in row.iter().enumerate() {
                    if criteria_match(&crit, v) {
                        let sv = match &sums {
                            Some(m) => m.get(i).and_then(|r| r.get(j)).cloned().unwrap_or(Value::Empty),
                            None => v.clone(),
                        };
                        if name == "COUNTIF" {
                            c += 1;
                        } else if let Value::Num(x) = sv {
                            total += x;
                            c += 1;
                        }
                    }
                }
            }
            match name {
                "COUNTIF" => n(c as f64),
                "SUMIF" => n(total),
                _ => {
                    if c == 0 {
                        V::S(e("#DIV/0!"))
                    } else {
                        n(total / c as f64)
                    }
                }
            }
        }
        "COUNTIFS" | "SUMIFS" | "AVERAGEIFS" | "MAXIFS" | "MINIFS" => {
            let (target, pairs) = if name == "COUNTIFS" { (None, a) } else { (Some(ev.array_arg(a, 0)), &a[1.min(a.len())..]) };
            let mut conds = vec![];
            for p in pairs.chunks(2) {
                if p.len() < 2 {
                    return V::S(e("#VALUE!"));
                }
                let v = ev.eval(&p[0]);
                let arr = ev.array_of(v);
                let crit = ev.scalar(&p[1]);
                conds.push((arr, crit));
            }
            let (rows, cols) = conds.first().map(|(a, _)| (a.len(), a.first().map(|r| r.len()).unwrap_or(0))).unwrap_or((0, 0));
            let mut matched = vec![];
            for i in 0..rows {
                for j in 0..cols {
                    if conds.iter().all(|(arr, crit)| criteria_match(crit, arr.get(i).and_then(|r| r.get(j)).unwrap_or(&Value::Empty))) {
                        matched.push((i, j));
                    }
                }
            }
            if name == "COUNTIFS" {
                return n(matched.len() as f64);
            }
            let tg = target.unwrap();
            let vals: Vec<f64> = matched.iter().filter_map(|(i, j)| match tg.get(*i).and_then(|r| r.get(*j)) {
                Some(Value::Num(x)) => Some(*x),
                _ => None,
            }).collect();
            match name {
                "SUMIFS" => n(vals.iter().sum()),
                "AVERAGEIFS" => {
                    if vals.is_empty() {
                        V::S(e("#DIV/0!"))
                    } else {
                        n(vals.iter().sum::<f64>() / vals.len() as f64)
                    }
                }
                "MAXIFS" => n(vals.iter().cloned().fold(f64::MIN, f64::max).max(if vals.is_empty() { 0.0 } else { f64::MIN })),
                _ => n(if vals.is_empty() { 0.0 } else { vals.iter().cloned().fold(f64::MAX, f64::min) }),
            }
        }

        // ───────── lookup ─────────
        "VLOOKUP" | "HLOOKUP" => {
            let key = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            let table = ev.array_arg(a, 1);
            let idx = t!(ev.num_arg(a, 2, None)) as usize;
            let approx = t!(ev.bool_arg(a, 3, true));
            let vert = name == "VLOOKUP";
            let first: Vec<Value> = if vert { table.iter().map(|r| r.first().cloned().unwrap_or(Value::Empty)).collect() } else { table.first().cloned().unwrap_or_default() };
            let width = if vert { table.first().map(|r| r.len()).unwrap_or(0) } else { table.len() };
            if idx == 0 || idx > width {
                return V::S(e("#REF!"));
            }
            match lookup_match(&first, &key, if approx { 1 } else { 0 }) {
                Some(p) => V::S(if vert { table[p][idx - 1].clone() } else { table[idx - 1][p].clone() }),
                None => V::S(e("#N/A")),
            }
        }
        "LOOKUP" => {
            let key = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            let v1: Vec<Value> = ev.array_arg(a, 1).into_iter().flatten().collect();
            let res: Vec<Value> = if a.len() > 2 { ev.array_arg(a, 2).into_iter().flatten().collect() } else { v1.clone() };
            match lookup_match(&v1, &key, 1) {
                Some(p) => V::S(res.get(p).cloned().unwrap_or(e("#N/A"))),
                None => V::S(e("#N/A")),
            }
        }
        "MATCH" | "XMATCH" => {
            let key = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            let hay: Vec<Value> = ev.array_arg(a, 1).into_iter().flatten().collect();
            let mode = t!(ev.num_arg(a, 2, Some(if name == "MATCH" { 1.0 } else { 0.0 }))) as i32;
            let mode = if name == "XMATCH" {
                match mode {
                    -1 => 1,
                    1 => -1,
                    _ => 0,
                }
            } else {
                mode
            };
            match lookup_match(&hay, &key, mode) {
                Some(p) => n(p as f64 + 1.0),
                None => V::S(e("#N/A")),
            }
        }
        "XLOOKUP" => {
            let key = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            let look: Vec<Value> = ev.array_arg(a, 1).into_iter().flatten().collect();
            let ret = ev.array_arg(a, 2);
            let mode = t!(ev.num_arg(a, 4, Some(0.0))) as i32;
            let pos = match mode {
                0 | 2 => lookup_match(&look, &key, 0),
                -1 => {
                    let mut best: Option<usize> = None;
                    for (i, v) in look.iter().enumerate() {
                        if compare(v, &key) != std::cmp::Ordering::Greater && best.map(|b2| compare(v, &look[b2]) == std::cmp::Ordering::Greater).unwrap_or(true) {
                            best = Some(i);
                        }
                    }
                    best
                }
                _ => {
                    let mut best: Option<usize> = None;
                    for (i, v) in look.iter().enumerate() {
                        if compare(v, &key) != std::cmp::Ordering::Less && best.map(|b2| compare(v, &look[b2]) == std::cmp::Ordering::Less).unwrap_or(true) {
                            best = Some(i);
                        }
                    }
                    best
                }
            };
            match pos {
                Some(p) => {
                    if ret.len() == look.len() && ret.first().map(|r| r.len()).unwrap_or(0) >= 1 {
                        if ret[0].len() == 1 {
                            V::S(ret[p][0].clone())
                        } else {
                            V::A(vec![ret[p].clone()])
                        }
                    } else if ret.len() == 1 {
                        V::S(ret[0].get(p).cloned().unwrap_or(e("#N/A")))
                    } else {
                        V::A(ret.iter().map(|r| vec![r.get(p).cloned().unwrap_or(Value::Empty)]).collect())
                    }
                }
                None => match a.get(3) {
                    Some(x) if !matches!(x, Expr::Missing) => ev.eval(x),
                    _ => V::S(e("#N/A")),
                },
            }
        }
        "INDEX" => {
            let v = ev.eval(a.first().unwrap_or(&Expr::Missing));
            let r = t!(ev.num_arg(a, 1, Some(0.0))) as usize;
            let c = t!(ev.num_arg(a, 2, Some(0.0))) as usize;
            match v {
                V::R(g) => {
                    let g = ev.clip_pub(g);
                    let (rows, cols) = ((g.r1 - g.r0 + 1) as usize, (g.c1 - g.c0 + 1) as usize);
                    let (r, c) = if rows == 1 && c == 0 && r > 0 { (1, r) } else { (r, c) };
                    if r > rows || c > cols {
                        return V::S(e("#REF!"));
                    }
                    match (r, c) {
                        (0, 0) => V::R(g),
                        (0, c) => V::R(Rng { c0: g.c0 + c as u32 - 1, c1: g.c0 + c as u32 - 1, ..g }),
                        (r, 0) => V::R(Rng { r0: g.r0 + r as u32 - 1, r1: g.r0 + r as u32 - 1, ..g }),
                        (r, c) => V::R(Rng { r0: g.r0 + r as u32 - 1, r1: g.r0 + r as u32 - 1, c0: g.c0 + c as u32 - 1, c1: g.c0 + c as u32 - 1, ..g }),
                    }
                }
                other => {
                    let arr = ev.array_of(other);
                    let (r, c) = if arr.len() == 1 && c == 0 { (1, r.max(1)) } else { (r.max(1), c.max(1)) };
                    V::S(arr.get(r - 1).and_then(|row| row.get(c - 1)).cloned().unwrap_or(e("#REF!")))
                }
            }
        }
        "ROW" | "COLUMN" => {
            let g = match a.first() {
                None | Some(Expr::Missing) => {
                    let (sh, r, c) = ev.current();
                    Rng { sheet: sh, r0: r, c0: c, r1: r, c1: c }
                }
                Some(_) => match ev.eval(&a[0]) {
                    V::R(g) => g,
                    _ => return V::S(e("#VALUE!")),
                },
            };
            n(if name == "ROW" { g.r0 + 1 } else { g.c0 + 1 } as f64)
        }
        "ROWS" | "COLUMNS" => {
            let v = ev.eval(a.first().unwrap_or(&Expr::Missing));
            match v {
                V::R(g) => n(if name == "ROWS" { g.r1 - g.r0 + 1 } else { g.c1 - g.c0 + 1 } as f64),
                other => {
                    let arr = ev.array_of(other);
                    n(if name == "ROWS" { arr.len() } else { arr.first().map(|r| r.len()).unwrap_or(0) } as f64)
                }
            }
        }
        "OFFSET" => {
            let Some(base) = (match ev.eval(a.first().unwrap_or(&Expr::Missing)) {
                V::R(g) => Some(g),
                _ => None,
            }) else {
                return V::S(e("#VALUE!"));
            };
            let dr = t!(ev.num_arg(a, 1, Some(0.0))) as i64;
            let dc = t!(ev.num_arg(a, 2, Some(0.0))) as i64;
            let h = t!(ev.num_arg(a, 3, Some((base.r1 - base.r0 + 1) as f64))) as i64;
            let w = t!(ev.num_arg(a, 4, Some((base.c1 - base.c0 + 1) as f64))) as i64;
            let r0 = base.r0 as i64 + dr;
            let c0 = base.c0 as i64 + dc;
            if r0 < 0 || c0 < 0 || h < 1 || w < 1 {
                return V::S(e("#REF!"));
            }
            V::R(Rng { sheet: base.sheet, r0: r0 as u32, c0: c0 as u32, r1: (r0 + h - 1) as u32, c1: (c0 + w - 1) as u32 })
        }
        "INDIRECT" => {
            let text = t!(ev.str_arg(a, 0, ""));
            let a1 = t!(ev.bool_arg(a, 1, true));
            let text = if a1 {
                text
            } else {
                // R1C1 → A1
                let up = text.to_uppercase();
                let parse = |s: &str| -> Option<(u32, u32)> {
                    let s = s.strip_prefix('R')?;
                    let (r, c) = s.split_once('C')?;
                    Some((r.parse().ok()?, c.parse().ok()?))
                };
                match parse(&up) {
                    Some((r, c)) => addr(r - 1, c - 1),
                    None => return V::S(e("#REF!")),
                }
            };
            match crate::formula::parse(&format!("={text}")) {
                Ok(ex @ (Expr::Ref { .. } | Expr::Range { .. })) => ev.eval(&ex),
                _ => V::S(e("#REF!")),
            }
        }
        "ADDRESS" => {
            let r = t!(ev.num_arg(a, 0, None)) as u32;
            let c = t!(ev.num_arg(a, 1, None)) as u32;
            let abs = t!(ev.num_arg(a, 2, Some(1.0))) as u32;
            let sheet = t!(ev.str_arg(a, 4, ""));
            if r == 0 || c == 0 {
                return V::S(e("#VALUE!"));
            }
            let (ra, ca) = match abs {
                1 => (true, true),
                2 => (true, false),
                3 => (false, true),
                _ => (false, false),
            };
            let txt = format!("{}{}{}{}", if ca { "$" } else { "" }, col_name(c - 1), if ra { "$" } else { "" }, r);
            s(if sheet.is_empty() { txt } else { format!("{}!{txt}", quote_sheet(&sheet)) })
        }
        "TRANSPOSE" => {
            let arr = ev.array_arg(a, 0);
            let rows = arr.len();
            let cols = arr.first().map(|r| r.len()).unwrap_or(0);
            V::A((0..cols).map(|c| (0..rows).map(|r| arr[r][c].clone()).collect()).collect())
        }
        "UNIQUE" => {
            let arr = ev.array_arg(a, 0);
            let mut out: Vec<Vec<Value>> = vec![];
            for row in arr {
                if !out.iter().any(|r| r.len() == row.len() && r.iter().zip(&row).all(|(x, y)| compare(x, y) == std::cmp::Ordering::Equal)) {
                    out.push(row);
                }
            }
            V::A(out)
        }
        "SORT" | "SORTBY" => {
            let mut arr = ev.array_arg(a, 0);
            let (idx, desc) = if name == "SORT" {
                (t!(ev.num_arg(a, 1, Some(1.0))) as usize, t!(ev.num_arg(a, 2, Some(1.0))) < 0.0)
            } else {
                (0, t!(ev.num_arg(a, 2, Some(1.0))) < 0.0)
            };
            if name == "SORTBY" {
                let keys: Vec<Value> = ev.array_arg(a, 1).into_iter().flatten().collect();
                let mut idxs: Vec<usize> = (0..arr.len()).collect();
                idxs.sort_by(|x, y| {
                    let o = compare(&keys[*x.min(&(keys.len() - 1))], &keys[*y.min(&(keys.len() - 1))]);
                    if desc {
                        o.reverse()
                    } else {
                        o
                    }
                });
                return V::A(idxs.into_iter().map(|i| arr[i].clone()).collect());
            }
            let k = idx.max(1) - 1;
            arr.sort_by(|x, y| {
                let o = compare(x.get(k).unwrap_or(&Value::Empty), y.get(k).unwrap_or(&Value::Empty));
                if desc {
                    o.reverse()
                } else {
                    o
                }
            });
            V::A(arr)
        }
        "FILTER" => {
            let arr = ev.array_arg(a, 0);
            let inc: Vec<Value> = ev.array_arg(a, 1).into_iter().flatten().collect();
            let out: Vec<Vec<Value>> = arr.into_iter().enumerate().filter(|(i, _)| inc.get(*i).map(|v| truthy(v).unwrap_or(false)).unwrap_or(false)).map(|(_, r)| r).collect();
            if out.is_empty() {
                return match a.get(2) {
                    Some(x) => ev.eval(x),
                    None => V::S(e("#CALC!")),
                };
            }
            V::A(out)
        }
        "HSTACK" | "VSTACK" => {
            let arrays: Vec<Vec<Vec<Value>>> = (0..a.len()).map(|i| ev.array_arg(a, i)).collect();
            if name == "VSTACK" {
                let w = arrays.iter().map(|x| x.first().map(|r| r.len()).unwrap_or(0)).max().unwrap_or(0);
                V::A(arrays.into_iter().flatten().map(|mut r| {
                    r.resize(w, e("#N/A"));
                    r
                }).collect())
            } else {
                let h = arrays.iter().map(|x| x.len()).max().unwrap_or(0);
                V::A((0..h).map(|i| arrays.iter().flat_map(|x| x.get(i).cloned().unwrap_or_else(|| vec![e("#N/A"); x.first().map(|r| r.len()).unwrap_or(0)])).collect()).collect())
            }
        }

        // ───────── text ─────────
        "CONCATENATE" | "CONCAT" => {
            let mut out = String::new();
            for x in a {
                let v = ev.eval(x);
                for item in ev.values_of(v) {
                    if let Value::Err(er) = item {
                        return V::S(Value::Err(er));
                    }
                    out.push_str(&item.as_text());
                }
            }
            s(out)
        }
        "TEXTJOIN" => {
            let delim = t!(ev.str_arg(a, 0, ""));
            let skip = t!(ev.bool_arg(a, 1, true));
            let mut parts = vec![];
            for x in &a[2.min(a.len())..] {
                let v = ev.eval(x);
                for item in ev.values_of(v) {
                    let t2 = item.as_text();
                    if !(skip && t2.is_empty()) {
                        parts.push(t2);
                    }
                }
            }
            s(parts.join(&delim))
        }
        "LEFT" | "RIGHT" => {
            let t2 = t!(ev.str_arg(a, 0, ""));
            let k = t!(ev.num_arg(a, 1, Some(1.0))) as usize;
            let cs: Vec<char> = t2.chars().collect();
            let k = k.min(cs.len());
            s(if name == "LEFT" { cs[..k].iter().collect() } else { cs[cs.len() - k..].iter().collect() })
        }
        "MID" => {
            let t2 = t!(ev.str_arg(a, 0, ""));
            let st = t!(ev.num_arg(a, 1, None));
            let k = t!(ev.num_arg(a, 2, None));
            if st < 1.0 || k < 0.0 {
                return V::S(e("#VALUE!"));
            }
            s(t2.chars().skip(st as usize - 1).take(k as usize).collect())
        }
        "LEN" => n(t!(ev.str_arg(a, 0, "")).chars().count() as f64),
        "LOWER" => s(t!(ev.str_arg(a, 0, "")).to_lowercase()),
        "UPPER" => s(t!(ev.str_arg(a, 0, "")).to_uppercase()),
        "PROPER" => {
            let t2 = t!(ev.str_arg(a, 0, ""));
            let mut out = String::new();
            let mut start = true;
            for c in t2.chars() {
                if c.is_alphabetic() {
                    if start {
                        out.extend(c.to_uppercase());
                    } else {
                        out.extend(c.to_lowercase());
                    }
                    start = false;
                } else {
                    out.push(c);
                    start = true;
                }
            }
            s(out)
        }
        "TRIM" => s(t!(ev.str_arg(a, 0, "")).split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")),
        "CLEAN" => s(t!(ev.str_arg(a, 0, "")).chars().filter(|c| *c as u32 >= 32).collect()),
        "SUBSTITUTE" => {
            let t2 = t!(ev.str_arg(a, 0, ""));
            let old = t!(ev.str_arg(a, 1, ""));
            let new = t!(ev.str_arg(a, 2, ""));
            if old.is_empty() {
                return s(t2);
            }
            match a.get(3) {
                Some(_) => {
                    let k = t!(ev.num_arg(a, 3, None)) as usize;
                    let mut idx = 0;
                    let mut out = t2.clone();
                    for (i, (pos, _)) in t2.match_indices(&old).enumerate() {
                        if i + 1 == k {
                            out = format!("{}{}{}", &t2[..pos], new, &t2[pos + old.len()..]);
                            break;
                        }
                        idx = i;
                    }
                    let _ = idx;
                    s(out)
                }
                None => s(t2.replace(&old, &new)),
            }
        }
        "REPLACE" => {
            let t2: Vec<char> = t!(ev.str_arg(a, 0, "")).chars().collect();
            let st = (t!(ev.num_arg(a, 1, None)) as usize).max(1) - 1;
            let k = t!(ev.num_arg(a, 2, None)) as usize;
            let new = t!(ev.str_arg(a, 3, ""));
            let st = st.min(t2.len());
            let end = (st + k).min(t2.len());
            s(format!("{}{}{}", t2[..st].iter().collect::<String>(), new, t2[end..].iter().collect::<String>()))
        }
        "FIND" | "SEARCH" => {
            let needle = t!(ev.str_arg(a, 0, ""));
            let hay = t!(ev.str_arg(a, 1, ""));
            let st = (t!(ev.num_arg(a, 2, Some(1.0))) as usize).max(1) - 1;
            let hc: Vec<char> = hay.chars().collect();
            let (h2, n2): (Vec<char>, Vec<char>) = if name == "SEARCH" { (hay.to_lowercase().chars().collect(), needle.to_lowercase().chars().collect()) } else { (hc.clone(), needle.chars().collect()) };
            for i in st..h2.len().max(st) + 1 {
                if i + n2.len() > h2.len() {
                    break;
                }
                let seg: String = h2[i..i + n2.len()].iter().collect();
                let pat: String = n2.iter().collect();
                if (name == "SEARCH" && wildcard(&pat, &seg)) || seg == pat {
                    return n(i as f64 + 1.0);
                }
            }
            if n2.is_empty() {
                return n(st as f64 + 1.0);
            }
            V::S(e("#VALUE!"))
        }
        "REPT" => {
            let t2 = t!(ev.str_arg(a, 0, ""));
            let k = t!(ev.num_arg(a, 1, None)) as usize;
            if t2.len() * k > 32767 {
                return V::S(e("#VALUE!"));
            }
            s(t2.repeat(k))
        }
        "EXACT" => {
            let x = t!(ev.str_arg(a, 0, ""));
            let y = t!(ev.str_arg(a, 1, ""));
            b(x == y)
        }
        "CHAR" | "UNICHAR" => {
            let c = t!(ev.num_arg(a, 0, None)) as u32;
            let ch = if name == "CHAR" && c < 256 {
                encoding_rs::WINDOWS_1251.decode(&[c as u8]).0.chars().next()
            } else {
                char::from_u32(c)
            };
            match ch {
                Some(ch) => s(ch.to_string()),
                None => V::S(e("#VALUE!")),
            }
        }
        "CODE" | "UNICODE" => {
            let t2 = t!(ev.str_arg(a, 0, ""));
            match t2.chars().next() {
                Some(c) if name == "CODE" && (c as u32) > 127 => {
                    let cs = c.to_string();
                    let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode(&cs);
                    let code = *bytes.first().unwrap_or(&63);
                    n(code as f64)
                }
                Some(c) => n(c as u32 as f64),
                None => V::S(e("#VALUE!")),
            }
        }
        "T" => {
            let v = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            s(if let Value::Str(x) = v { x } else { String::new() })
        }
        "N" => {
            let v = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            n(match v {
                Value::Num(x) => x,
                Value::Bool(bb) => bb as i32 as f64,
                _ => 0.0,
            })
        }
        "VALUE" | "NUMBERVALUE" => {
            let t2 = t!(ev.str_arg(a, 0, ""));
            let t2 = if name == "NUMBERVALUE" {
                let dec = t!(ev.str_arg(a, 1, "."));
                let grp = t!(ev.str_arg(a, 2, ","));
                t2.replace(&grp, "").replace(&dec, ".")
            } else {
                t2
            };
            match parse_input(&t2).0 {
                Value::Num(x) => n(x),
                _ => V::S(e("#VALUE!")),
            }
        }
        "TEXT" => {
            let v = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            let fmt = t!(ev.str_arg(a, 1, "General"));
            s(crate::numfmt::format(&v, &fmt).0)
        }
        "FIXED" | "DOLLAR" => {
            let x = t!(ev.num_arg(a, 0, None));
            let d = t!(ev.num_arg(a, 1, Some(2.0))) as usize;
            let no_commas = t!(ev.bool_arg(a, 2, false));
            let code = format!("{}{}{}", if name == "DOLLAR" { "$" } else { "" }, if no_commas { "0" } else { "#,##0" }, if d > 0 { format!(".{}", "0".repeat(d)) } else { String::new() });
            s(crate::numfmt::format(&Value::Num(x), &code).0)
        }
        "TEXTBEFORE" | "TEXTAFTER" => {
            let t2 = t!(ev.str_arg(a, 0, ""));
            let d = t!(ev.str_arg(a, 1, ""));
            let k = t!(ev.num_arg(a, 2, Some(1.0))) as i64;
            let found: Vec<usize> = t2.match_indices(&d).map(|(i, _)| i).collect();
            let idx = if k > 0 { found.get(k as usize - 1) } else { found.len().checked_sub((-k) as usize).and_then(|i| found.get(i)) };
            match idx {
                Some(&i) => s(if name == "TEXTBEFORE" { t2[..i].to_string() } else { t2[i + d.len()..].to_string() }),
                None => V::S(e("#N/A")),
            }
        }
        "TEXTSPLIT" => {
            let t2 = t!(ev.str_arg(a, 0, ""));
            let cd = t!(ev.str_arg(a, 1, ""));
            let rd = t!(ev.str_arg(a, 2, ""));
            let rows: Vec<&str> = if rd.is_empty() { vec![t2.as_str()] } else { t2.split(rd.as_str()).collect() };
            let arr: Vec<Vec<Value>> = rows.iter().map(|r| if cd.is_empty() { vec![Value::Str(r.to_string())] } else { r.split(cd.as_str()).map(|x| Value::Str(x.to_string())).collect() }).collect();
            let w = arr.iter().map(|r| r.len()).max().unwrap_or(0);
            V::A(arr.into_iter().map(|mut r| {
                r.resize(w, e("#N/A"));
                r
            }).collect())
        }

        // ───────── dates ─────────
        "TODAY" => {
            let d = chrono::Local::now().date_naive();
            use chrono::Datelike;
            n(date_serial(d.year(), d.month(), d.day()).unwrap_or(0.0))
        }
        "NOW" => {
            let d = chrono::Local::now().naive_local();
            use chrono::{Datelike, Timelike};
            n(date_serial(d.year(), d.month(), d.day()).unwrap_or(0.0) + (d.num_seconds_from_midnight() as f64) / 86400.0)
        }
        "DATE" => {
            let y = t!(ev.num_arg(a, 0, None)) as i32;
            let m = t!(ev.num_arg(a, 1, None)) as i32;
            let d = t!(ev.num_arg(a, 2, None)) as i32;
            let y = if y < 1900 { y + 1900 } else { y };
            let total = y * 12 + m - 1;
            let (yy, mm) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
            match date_serial(yy, mm, 1) {
                Some(base) => n(base + d as f64 - 1.0),
                None => V::S(e("#NUM!")),
            }
        }
        "TIME" => {
            let h = t!(ev.num_arg(a, 0, None));
            let m = t!(ev.num_arg(a, 1, None));
            let sec = t!(ev.num_arg(a, 2, None));
            n(((h * 3600.0 + m * 60.0 + sec) / 86400.0).rem_euclid(1.0))
        }
        "YEAR" | "MONTH" | "DAY" => {
            let v = t!(ev.num_arg(a, 0, None));
            match date_parts(v) {
                Some((y, m, d)) => n(match name {
                    "YEAR" => y as f64,
                    "MONTH" => m as f64,
                    _ => d as f64,
                }),
                None => V::S(e("#NUM!")),
            }
        }
        "HOUR" | "MINUTE" | "SECOND" => {
            let v = t!(ev.num_arg(a, 0, None));
            let secs = ((v - v.floor()) * 86400.0).round() as i64;
            n(match name {
                "HOUR" => secs / 3600,
                "MINUTE" => secs / 60 % 60,
                _ => secs % 60,
            } as f64)
        }
        "WEEKDAY" => {
            let v = t!(ev.num_arg(a, 0, None));
            let typ = t!(ev.num_arg(a, 1, Some(1.0))) as i64;
            let w = weekday(v) as i64; // 1=Sun
            n(match typ {
                2 | 11 => (w + 5) % 7 + 1,
                3 => (w + 5) % 7,
                _ => w,
            } as f64)
        }
        "WEEKNUM" | "ISOWEEKNUM" => {
            use chrono::Datelike;
            let v = t!(ev.num_arg(a, 0, None));
            let Some(d) = serial_date(v) else { return V::S(e("#NUM!")) };
            if name == "ISOWEEKNUM" || t!(ev.num_arg(a, 1, Some(1.0))) as i64 == 21 {
                return n(d.iso_week().week() as f64);
            }
            let jan1 = chrono::NaiveDate::from_ymd_opt(d.year(), 1, 1).unwrap();
            let offset = jan1.weekday().num_days_from_sunday() as i64;
            n(((d.ordinal0() as i64 + offset) / 7 + 1) as f64)
        }
        "EDATE" | "EOMONTH" => {
            let v = t!(ev.num_arg(a, 0, None));
            let m = t!(ev.num_arg(a, 1, None)) as i32;
            match add_months(v, m, name == "EOMONTH") {
                Some(x) => n(x),
                None => V::S(e("#NUM!")),
            }
        }
        "DAYS" => {
            let end = t!(ev.num_arg(a, 0, None));
            let st = t!(ev.num_arg(a, 1, None));
            n((end - st).floor())
        }
        "DAYS360" => {
            let st = t!(ev.num_arg(a, 0, None));
            let en = t!(ev.num_arg(a, 1, None));
            let (Some((y1, m1, d1)), Some((y2, m2, d2))) = (date_parts(st), date_parts(en)) else { return V::S(e("#NUM!")) };
            let d1 = d1.min(30);
            let d2 = if d2 == 31 && d1 >= 30 { 30 } else { d2 };
            n(((y2 - y1) * 360 + (m2 as i32 - m1 as i32) * 30 + (d2 as i32 - d1 as i32)) as f64)
        }
        "DATEDIF" => {
            let st = t!(ev.num_arg(a, 0, None));
            let en = t!(ev.num_arg(a, 1, None));
            let unit = t!(ev.str_arg(a, 2, "D")).to_uppercase();
            if en < st {
                return V::S(e("#NUM!"));
            }
            let (Some((y1, m1, d1)), Some((y2, m2, d2))) = (date_parts(st), date_parts(en)) else { return V::S(e("#NUM!")) };
            let months = (y2 - y1) * 12 + m2 as i32 - m1 as i32 - if d2 < d1 { 1 } else { 0 };
            n(match unit.as_str() {
                "D" => (en - st).floor(),
                "M" => months as f64,
                "Y" => (months / 12) as f64,
                "YM" => (months % 12) as f64,
                "MD" => {
                    if d2 >= d1 {
                        (d2 - d1) as f64
                    } else {
                        let prev = add_months(en, -1, false).unwrap_or(en);
                        let (_, _, pd) = date_parts(prev).unwrap_or((0, 0, 1));
                        let last = add_months(prev, 0, true).and_then(date_parts).map(|x| x.2).unwrap_or(30);
                        (last - d1.min(last) + d2) as f64 + 0.0 * pd as f64
                    }
                }
                "YD" => {
                    let shifted = date_serial(y1 + (y2 - y1) - if (m2, d2) < (m1, d1) { 1 } else { 0 }, m1, d1).unwrap_or(st);
                    (en - shifted).floor()
                }
                _ => return V::S(e("#NUM!")),
            })
        }
        "NETWORKDAYS" | "NETWORKDAYS.INTL" | "WORKDAY" | "WORKDAY.INTL" => {
            let st = t!(ev.num_arg(a, 0, None)).floor();
            let second = t!(ev.num_arg(a, 1, None)).floor();
            let intl = name.ends_with("INTL");
            let weekend: Vec<u32> = if intl {
                match ev.scalar(a.get(2).unwrap_or(&Expr::Missing)) {
                    Value::Str(mask) if mask.len() == 7 => mask.chars().enumerate().filter(|(_, c)| *c == '1').map(|(i, _)| ((i + 1) % 7 + 1) as u32).collect(),
                    Value::Num(k) => match k as i32 {
                        2 => vec![1, 2],
                        3 => vec![2, 3],
                        11 => vec![1],
                        12 => vec![2],
                        17 => vec![7],
                        _ => vec![1, 7],
                    },
                    _ => vec![1, 7],
                }
            } else {
                vec![1, 7]
            };
            let hol_idx = if intl { 3 } else { 2 };
            let holidays: Vec<f64> = if a.len() > hol_idx { t!(ev.nums_pub(&a[hol_idx..hol_idx + 1], false)).into_iter().map(|x| x.floor()).collect() } else { vec![] };
            let workday = |d: f64| !weekend.contains(&weekday(d)) && !holidays.contains(&d);
            if name.starts_with("NETWORKDAYS") {
                let (lo, hi, sign) = if second >= st { (st, second, 1.0) } else { (second, st, -1.0) };
                if hi - lo > 1e6 {
                    return V::S(e("#NUM!"));
                }
                let mut c = 0.0;
                let mut d = lo;
                while d <= hi {
                    if workday(d) {
                        c += 1.0;
                    }
                    d += 1.0;
                }
                n(c * sign)
            } else {
                let mut left = second as i64;
                let step = if left >= 0 { 1.0 } else { -1.0 };
                let mut d = st;
                let mut guard = 0;
                while left != 0 && guard < 10_000_000 {
                    d += step;
                    if workday(d) {
                        left -= step as i64;
                    }
                    guard += 1;
                }
                n(d)
            }
        }
        "DATEVALUE" | "TIMEVALUE" => {
            let t2 = t!(ev.str_arg(a, 0, ""));
            match parse_input(&t2).0 {
                Value::Num(x) => n(if name == "DATEVALUE" { x.floor() } else { x - x.floor() }),
                _ => V::S(e("#VALUE!")),
            }
        }
        "YEARFRAC" => {
            let st = t!(ev.num_arg(a, 0, None));
            let en = t!(ev.num_arg(a, 1, None));
            let basis = t!(ev.num_arg(a, 2, Some(0.0))) as i32;
            let (st, en) = if st <= en { (st, en) } else { (en, st) };
            n(match basis {
                1 => (en - st) / 365.25,
                2 => (en - st) / 360.0,
                3 => (en - st) / 365.0,
                _ => {
                    let (Some((y1, m1, d1)), Some((y2, m2, d2))) = (date_parts(st), date_parts(en)) else { return V::S(e("#NUM!")) };
                    let d1 = d1.min(30) as i32;
                    let d2 = if d2 == 31 && d1 == 30 { 30 } else { d2 as i32 };
                    ((y2 - y1) * 360 + (m2 as i32 - m1 as i32) * 30 + (d2 - d1)) as f64 / 360.0
                }
            })
        }

        // ───────── information ─────────
        "ISBLANK" | "ISNUMBER" | "ISTEXT" | "ISNONTEXT" | "ISLOGICAL" | "ISERROR" | "ISERR" | "ISNA" | "ISEVEN" | "ISODD" | "ISREF" | "ISFORMULA" => {
            if name == "ISREF" {
                return b(matches!(a.first(), Some(Expr::Ref { .. }) | Some(Expr::Range { .. })));
            }
            if name == "ISFORMULA" {
                if let Some(Expr::Ref { sheet, r, c }) = a.first() {
                    let si = sheet.as_ref().and_then(|s| ev.wb.sheet_index(s)).unwrap_or(ev.current().0);
                    return b(ev.wb.sheets.get(si).and_then(|sh| sh.cells.get(&(*r, *c))).map(|c| c.is_formula()).unwrap_or(false));
                }
                return V::S(e("#VALUE!"));
            }
            let v = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            b(match name {
                "ISBLANK" => v.is_empty(),
                "ISNUMBER" => matches!(v, Value::Num(_)),
                "ISTEXT" => matches!(v, Value::Str(_)),
                "ISNONTEXT" => !matches!(v, Value::Str(_)),
                "ISLOGICAL" => matches!(v, Value::Bool(_)),
                "ISERROR" => matches!(v, Value::Err(_)),
                "ISERR" => matches!(&v, Value::Err(x) if x != "#N/A"),
                "ISNA" => matches!(&v, Value::Err(x) if x == "#N/A"),
                "ISEVEN" => t!(num(&v)).trunc() as i64 % 2 == 0,
                _ => t!(num(&v)).trunc() as i64 % 2 != 0,
            })
        }
        "NA" => V::S(e("#N/A")),
        "TYPE" => {
            let v = ev.eval(a.first().unwrap_or(&Expr::Missing));
            n(match v {
                V::S(Value::Num(_)) | V::S(Value::Empty) => 1.0,
                V::S(Value::Str(_)) => 2.0,
                V::S(Value::Bool(_)) => 4.0,
                V::S(Value::Err(_)) => 16.0,
                V::R(g) if g.r0 == g.r1 && g.c0 == g.c1 => match ev.cell(g.sheet, g.r0, g.c0) {
                    Value::Str(_) => 2.0,
                    Value::Bool(_) => 4.0,
                    Value::Err(_) => 16.0,
                    _ => 1.0,
                },
                _ => 64.0,
            })
        }
        "ERROR.TYPE" => {
            let v = ev.scalar(a.first().unwrap_or(&Expr::Missing));
            match v {
                Value::Err(x) => n(match x.as_str() {
                    "#NULL!" => 1.0,
                    "#DIV/0!" => 2.0,
                    "#VALUE!" => 3.0,
                    "#REF!" => 4.0,
                    "#NAME?" => 5.0,
                    "#NUM!" => 6.0,
                    "#N/A" => 7.0,
                    _ => 8.0,
                }),
                _ => V::S(e("#N/A")),
            }
        }
        "SHEET" | "SHEETS" => n(if name == "SHEETS" { ev.wb.sheets.len() } else { ev.current().0 + 1 } as f64),

        // ───────── finance ─────────
        "PMT" => {
            let r = t!(ev.num_arg(a, 0, None));
            let np = t!(ev.num_arg(a, 1, None));
            let pv = t!(ev.num_arg(a, 2, None));
            let fv_ = t!(ev.num_arg(a, 3, Some(0.0)));
            let ty = t!(ev.num_arg(a, 4, Some(0.0)));
            n(pmt(r, np, pv, fv_, ty))
        }
        "IPMT" | "PPMT" => {
            let r = t!(ev.num_arg(a, 0, None));
            let per = t!(ev.num_arg(a, 1, None));
            let np = t!(ev.num_arg(a, 2, None));
            let pv = t!(ev.num_arg(a, 3, None));
            let fv_ = t!(ev.num_arg(a, 4, Some(0.0)));
            let ty = t!(ev.num_arg(a, 5, Some(0.0)));
            if per < 1.0 || per > np {
                return V::S(e("#NUM!"));
            }
            let i = ipmt(r, per, np, pv, fv_, ty);
            n(if name == "IPMT" { i } else { pmt(r, np, pv, fv_, ty) - i })
        }
        "FV" => {
            let r = t!(ev.num_arg(a, 0, None));
            let np = t!(ev.num_arg(a, 1, None));
            let p = t!(ev.num_arg(a, 2, Some(0.0)));
            let pv = t!(ev.num_arg(a, 3, Some(0.0)));
            let ty = t!(ev.num_arg(a, 4, Some(0.0)));
            n(fv(r, np, p, pv, ty))
        }
        "PV" => {
            let r = t!(ev.num_arg(a, 0, None));
            let np = t!(ev.num_arg(a, 1, None));
            let p = t!(ev.num_arg(a, 2, Some(0.0)));
            let fv_ = t!(ev.num_arg(a, 3, Some(0.0)));
            let ty = t!(ev.num_arg(a, 4, Some(0.0)));
            n(if r == 0.0 { -(fv_ + p * np) } else { -(fv_ + p * (1.0 + r * ty) * ((1.0 + r).powf(np) - 1.0) / r) / (1.0 + r).powf(np) })
        }
        "NPER" => {
            let r = t!(ev.num_arg(a, 0, None));
            let p = t!(ev.num_arg(a, 1, None));
            let pv = t!(ev.num_arg(a, 2, None));
            let fv_ = t!(ev.num_arg(a, 3, Some(0.0)));
            let ty = t!(ev.num_arg(a, 4, Some(0.0)));
            if r == 0.0 {
                return n(-(pv + fv_) / p);
            }
            let num_ = p * (1.0 + r * ty) - fv_ * r;
            let den = pv * r + p * (1.0 + r * ty);
            n((num_ / den).ln() / (1.0 + r).ln())
        }
        "RATE" => {
            let np = t!(ev.num_arg(a, 0, None));
            let p = t!(ev.num_arg(a, 1, None));
            let pv = t!(ev.num_arg(a, 2, None));
            let fv_ = t!(ev.num_arg(a, 3, Some(0.0)));
            let ty = t!(ev.num_arg(a, 4, Some(0.0)));
            let mut r = t!(ev.num_arg(a, 5, Some(0.1)));
            for _ in 0..100 {
                let f = fv(r, np, p, pv, ty) - fv_;
                let h = 1e-6;
                let d = (fv(r + h, np, p, pv, ty) - fv_ - f) / h;
                if d.abs() < 1e-14 {
                    break;
                }
                let nr = r + f / d * 1.0 * -1.0;
                if (nr - r).abs() < 1e-10 {
                    return n(nr);
                }
                r = nr;
            }
            n(r)
        }
        "NPV" => {
            let r = t!(ev.num_arg(a, 0, None));
            let cfs = t!(ev.nums_pub(&a[1.min(a.len())..], false));
            n(npv(r, &cfs))
        }
        "IRR" => {
            let cfs = t!(ev.nums_pub(&a[..1.min(a.len())], false));
            let g = t!(ev.num_arg(a, 1, Some(0.1)));
            match irr(&cfs, g) {
                Some(r) => n(r),
                None => V::S(e("#NUM!")),
            }
        }
        "XNPV" | "XIRR" => {
            let (ri, vi, di) = if name == "XNPV" { (0, 1, 2) } else { (usize::MAX, 0, 1) };
            let vals: Vec<f64> = ev.array_arg(a, vi).into_iter().flatten().filter_map(|v| v.as_num()).collect();
            let dates: Vec<f64> = ev.array_arg(a, di).into_iter().flatten().filter_map(|v| v.as_num()).collect();
            if vals.len() != dates.len() || vals.is_empty() {
                return V::S(e("#NUM!"));
            }
            let f = |r: f64| vals.iter().zip(&dates).map(|(v, d)| v / (1.0 + r).powf((d - dates[0]) / 365.0)).sum::<f64>();
            if ri == 0 {
                let r = t!(ev.num_arg(a, 0, None));
                return n(f(r));
            }
            let mut r = t!(ev.num_arg(a, 2, Some(0.1)));
            for _ in 0..200 {
                let y = f(r);
                let d = (f(r + 1e-7) - y) / 1e-7;
                if d.abs() < 1e-14 {
                    break;
                }
                let nr = r - y / d;
                if (nr - r).abs() < 1e-10 {
                    return n(nr);
                }
                r = nr;
            }
            V::S(e("#NUM!"))
        }
        "SLN" => {
            let c = t!(ev.num_arg(a, 0, None));
            let sv = t!(ev.num_arg(a, 1, None));
            let l = t!(ev.num_arg(a, 2, None));
            n((c - sv) / l)
        }
        "SYD" => {
            let c = t!(ev.num_arg(a, 0, None));
            let sv = t!(ev.num_arg(a, 1, None));
            let l = t!(ev.num_arg(a, 2, None));
            let per = t!(ev.num_arg(a, 3, None));
            n((c - sv) * (l - per + 1.0) * 2.0 / (l * (l + 1.0)))
        }
        "DDB" => {
            let c = t!(ev.num_arg(a, 0, None));
            let sv = t!(ev.num_arg(a, 1, None));
            let l = t!(ev.num_arg(a, 2, None));
            let per = t!(ev.num_arg(a, 3, None));
            let f = t!(ev.num_arg(a, 4, Some(2.0)));
            let mut book = c;
            let mut dep = 0.0;
            for _ in 0..per as i64 {
                dep = (book * f / l).min(book - sv).max(0.0);
                book -= dep;
            }
            n(dep)
        }
        "EFFECT" => {
            let r = t!(ev.num_arg(a, 0, None));
            let k = t!(ev.num_arg(a, 1, None)).trunc();
            n((1.0 + r / k).powf(k) - 1.0)
        }
        "NOMINAL" => {
            let r = t!(ev.num_arg(a, 0, None));
            let k = t!(ev.num_arg(a, 1, None)).trunc();
            n(k * ((1.0 + r).powf(1.0 / k) - 1.0))
        }
        "CONVERT" => {
            let x = t!(ev.num_arg(a, 0, None));
            let from = t!(ev.str_arg(a, 1, ""));
            let to = t!(ev.str_arg(a, 2, ""));
            let unit = |u: &str| -> Option<(f64, &'static str)> {
                Some(match u {
                    "m" => (1.0, "len"),
                    "km" => (1000.0, "len"),
                    "cm" => (0.01, "len"),
                    "mm" => (0.001, "len"),
                    "in" => (0.0254, "len"),
                    "ft" => (0.3048, "len"),
                    "yd" => (0.9144, "len"),
                    "mi" => (1609.344, "len"),
                    "g" => (1.0, "mass"),
                    "kg" => (1000.0, "mass"),
                    "lbm" => (453.59237, "mass"),
                    "ozm" => (28.349523125, "mass"),
                    "l" | "L" => (1.0, "vol"),
                    "ml" => (0.001, "vol"),
                    "gal" => (3.785411784, "vol"),
                    "sec" | "s" => (1.0, "time"),
                    "min" | "mn" => (60.0, "time"),
                    "hr" => (3600.0, "time"),
                    "day" | "d" => (86400.0, "time"),
                    _ => return None,
                })
            };
            let temp = |v: f64, f: &str, t2: &str| -> Option<f64> {
                let c = match f {
                    "C" | "cel" => v,
                    "F" | "fah" => (v - 32.0) * 5.0 / 9.0,
                    "K" | "kel" => v - 273.15,
                    _ => return None,
                };
                Some(match t2 {
                    "C" | "cel" => c,
                    "F" | "fah" => c * 9.0 / 5.0 + 32.0,
                    "K" | "kel" => c + 273.15,
                    _ => return None,
                })
            };
            if let Some(v) = temp(x, &from, &to) {
                return n(v);
            }
            match (unit(&from), unit(&to)) {
                (Some((f1, k1)), Some((f2, k2))) if k1 == k2 => n(x * f1 / f2),
                _ => V::S(e("#N/A")),
            }
        }
        _ => {
            // Unknown function: maybe an operator-like name.
            let _ = scalar_op;
            V::S(e("#NAME?"))
        }
    }
}

/// Names of all supported functions (for the function list and autocomplete).
pub const FUNCTIONS: &[&str] = &[
    "ABS", "ACOS", "ACOSH", "ADDRESS", "AGGREGATE", "AND", "ARABIC", "ASIN", "ASINH", "ATAN", "ATAN2", "ATANH", "AVEDEV", "AVERAGE", "AVERAGEA", "AVERAGEIF", "AVERAGEIFS",
    "BASE", "BIN2DEC", "CEILING", "CEILING.MATH", "CHAR", "CHOOSE", "CLEAN", "CODE", "COLUMN", "COLUMNS", "COMBIN", "CONCAT", "CONCATENATE", "CONVERT", "CORREL", "COS", "COSH", "COT",
    "COUNT", "COUNTA", "COUNTBLANK", "COUNTIF", "COUNTIFS", "COVAR", "COVARIANCE.P", "COVARIANCE.S", "DATE", "DATEDIF", "DATEVALUE", "DAY", "DAYS", "DAYS360", "DDB", "DEC2BIN", "DEC2HEX",
    "DEC2OCT", "DECIMAL", "DEGREES", "DEVSQ", "DOLLAR", "EDATE", "EFFECT", "EOMONTH", "ERROR.TYPE", "EVEN", "EXACT", "EXP", "FACT", "FALSE", "FILTER", "FIND", "FIXED", "FLOOR", "FLOOR.MATH",
    "FORECAST", "FORECAST.LINEAR", "FV", "GCD", "GEOMEAN", "HARMEAN", "HEX2DEC", "HLOOKUP", "HOUR", "HSTACK", "IF", "IFERROR", "IFNA", "IFS", "INDEX", "INDIRECT", "INT", "INTERCEPT", "IPMT",
    "IRR", "ISBLANK", "ISERR", "ISERROR", "ISEVEN", "ISFORMULA", "ISLOGICAL", "ISNA", "ISNONTEXT", "ISNUMBER", "ISODD", "ISOWEEKNUM", "ISREF", "ISTEXT", "LARGE", "LCM", "LEFT", "LEN", "LN",
    "LOG", "LOG10", "LOOKUP", "LOWER", "MATCH", "MAX", "MAXA", "MAXIFS", "MEDIAN", "MID", "MIN", "MINA", "MINIFS", "MINUTE", "MOD", "MODE", "MODE.SNGL", "MONTH", "MROUND", "N", "NA",
    "NETWORKDAYS", "NETWORKDAYS.INTL", "NOMINAL", "NORM.DIST", "NORM.INV", "NORM.S.DIST", "NORM.S.INV", "NOT", "NOW", "NPER", "NPV", "NUMBERVALUE", "OCT2DEC", "ODD", "OFFSET", "OR", "PEARSON",
    "PERCENTILE", "PERCENTILE.EXC", "PERCENTILE.INC", "PERMUT", "PI", "PMT", "POWER", "PPMT", "PRODUCT", "PROPER", "PV", "QUARTILE", "QUARTILE.EXC", "QUARTILE.INC", "QUOTIENT", "RADIANS",
    "RAND", "RANDBETWEEN", "RANK", "RANK.AVG", "RANK.EQ", "RATE", "REPLACE", "REPT", "RIGHT", "ROMAN", "ROUND", "ROUNDDOWN", "ROUNDUP", "ROW", "ROWS", "RSQ", "SEARCH", "SECOND", "SEQUENCE",
    "SHEET", "SHEETS", "SIGN", "SIN", "SINH", "SLN", "SLOPE", "SMALL", "SORT", "SORTBY", "SQRT", "STANDARDIZE", "STDEV", "STDEV.P", "STDEV.S", "STDEVA", "STDEVP", "STEYX", "SUBSTITUTE", "SUBTOTAL",
    "SUM", "SUMIF", "SUMIFS", "SUMPRODUCT", "SUMSQ", "SWITCH", "SYD", "T", "TAN", "TANH", "TEXT", "TEXTAFTER", "TEXTBEFORE", "TEXTJOIN", "TEXTSPLIT", "TIME", "TIMEVALUE", "TODAY", "TRANSPOSE",
    "TRIM", "TRUE", "TRUNC", "TYPE", "UNICHAR", "UNICODE", "UNIQUE", "UPPER", "VALUE", "VAR", "VAR.P", "VAR.S", "VARP", "VLOOKUP", "VSTACK", "WEEKDAY", "WEEKNUM", "WORKDAY", "WORKDAY.INTL",
    "XIRR", "XLOOKUP", "XMATCH", "XNPV", "XOR", "YEAR", "YEARFRAC",
];
