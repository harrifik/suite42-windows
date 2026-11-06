//! PDF export and printing of sheets: columns and rows paginated at cell boundaries,
//! automatic orientation, fit to width, gridlines, repeated frozen rows, page numbers.

use crate::model::*;
use kit42::pdfgen::{self, Doc, Page, TextStyle};

pub struct Options {
    pub all_sheets: bool,
    pub selection: Option<(u32, u32, u32, u32)>,
    pub landscape: Option<bool>,
    pub fit_width: bool,
    pub gridlines: bool,
}

pub fn export(wb: &Workbook, active: usize, o: &Options) -> Vec<u8> {
    let mut d = Doc::new();
    let sheets: Vec<usize> = if o.all_sheets { (0..wb.sheets.len()).collect() } else { vec![active] };
    let mut fonts = std::collections::HashMap::new();
    for &si in &sheets {
        let sh = &wb.sheets[si];
        let (r0, c0, r1, c1) = match o.selection {
            Some(s) if !o.all_sheets => s,
            _ => match sh.used() {
                Some((r, c)) => {
                    // Charts extend the printed area.
                    let mut rr = r;
                    let mut cc = c;
                    for ch in &sh.charts {
                        rr = rr.max(sh.row_at(sh.row_y(ch.anchor.0) + ch.size.1));
                        cc = cc.max(sh.col_at(sh.col_x(ch.anchor.1) + ch.size.0));
                    }
                    (0, 0, rr, cc)
                }
                None => continue,
            },
        };
        let px = 0.75; // points per pixel
        let total_w: f32 = (c0..=c1).map(|c| sh.col_width(c) * px).sum();
        let landscape = o.landscape.unwrap_or(total_w > pdfgen::A4.0 - 72.0 && total_w < (pdfgen::A4.1 - 72.0) * 1.6);
        let (pw, ph) = if landscape { (pdfgen::A4.1, pdfgen::A4.0) } else { pdfgen::A4 };
        let m = 36.0;
        let avail_w = pw - 2.0 * m;
        let scale = if o.fit_width && total_w > avail_w { avail_w / total_w } else { 1.0 };
        // Column pages.
        let mut col_pages: Vec<(u32, u32)> = vec![];
        let mut start = c0;
        let mut w = 0.0;
        for c in c0..=c1 {
            let cw = sh.col_width(c) * px * scale;
            if w + cw > avail_w && c > start {
                col_pages.push((start, c - 1));
                start = c;
                w = 0.0;
            }
            w += cw;
        }
        col_pages.push((start, c1));
        // Repeated header rows (frozen rows), unless they would take most of the page.
        let header_rows: Vec<u32> = (0..sh.freeze.0).filter(|r| *r >= r0).collect();
        let header_h: f32 = header_rows.iter().map(|r| sh.row_height(*r) * px * scale).sum();
        let avail_h = ph - 2.0 * m - 16.0;
        let repeat = header_h < avail_h * 0.8;
        let mut row_pages: Vec<(u32, u32)> = vec![];
        let mut start = r0;
        let mut h = if repeat { header_h } else { 0.0 };
        for r in r0..=r1 {
            let rh = sh.row_height(r) * px * scale;
            if h + rh > avail_h && r > start {
                row_pages.push((start, r - 1));
                start = r;
                h = if repeat { header_h } else { 0.0 };
            }
            h += rh;
        }
        row_pages.push((start, r1));
        let total_pages = row_pages.len() * col_pages.len();
        let mut page_no = 0;
        for &(ca, cb) in &col_pages {
            for &(ra, rb) in &row_pages {
                page_no += 1;
                let mut page = Page::new(pw, ph);
                let hf = *fonts.entry(("Calibri".to_string(), false, false)).or_insert_with(|| d.font("Calibri", false, false));
                let mut st = TextStyle::new(hf, 8.0);
                st.color = [120, 120, 120];
                d.text(&mut page, st, m, m - 10.0, &sh.name);
                let label = format!("{page_no} / {total_pages}");
                let lw = d.measure(hf, 8.0, &label);
                d.text(&mut page, st, pw - m - lw, ph - m + 14.0, &label);
                let mut rows: Vec<u32> = vec![];
                if repeat && ra > sh.freeze.0 {
                    rows.extend(header_rows.iter().cloned());
                }
                rows.extend(ra..=rb);
                let mut y = m;
                for r in rows {
                    if sh.hidden_rows.contains(&r) {
                        continue;
                    }
                    let rh = sh.row_height(r) * px * scale;
                    let mut x = m;
                    for c in ca..=cb {
                        if sh.hidden_cols.contains(&c) {
                            continue;
                        }
                        let own_w = sh.col_width(c) * px * scale;
                        let mut cw = own_w;
                        let mut rh = rh;
                        if let Some(mg) = sh.merge_at(r, c) {
                            if (mg.0, mg.1) != (r, c) {
                                x += own_w;
                                continue;
                            }
                            cw = (mg.1..=mg.3.min(cb)).map(|k| sh.col_width(k) * px * scale).sum();
                            rh = (mg.0..=mg.2).map(|k| sh.row_height(k) * px * scale).sum();
                        }
                        let cell = sh.cells.get(&(r, c));
                        let style = cell.map(|x| wb.style(x.style).clone()).unwrap_or_default();
                        if let Some(f) = style.fill {
                            page.fill_rect(x, y, cw, rh, f);
                        }
                        if o.gridlines {
                            page.stroke_rect(x, y, cw, rh, [210, 210, 210], 0.3);
                        }
                        let v = sh.value(r, c);
                        if !v.is_empty() {
                            let (text, color) = crate::numfmt::format(&v, &style.numfmt);
                            let key = (style.font.clone(), style.bold, style.italic);
                            let f = *fonts.entry(key).or_insert_with(|| d.font(&style.font, style.bold, style.italic));
                            let size = style.size() * scale;
                            let mut ts = TextStyle::new(f, size);
                            ts.color = color.unwrap_or(style.color);
                            ts.underline = style.underline;
                            ts.strike = style.strike;
                            let tw = d.measure(f, size, &text);
                            let is_num = matches!(v, Value::Num(_));
                            let align = match style.halign {
                                HAlign::General => if is_num { HAlign::Right } else { HAlign::Left },
                                a => a,
                            };
                            let (text, tw) = if is_num && tw > cw - 3.0 { ("#".repeat(((cw - 3.0) / (size * 0.55)).max(1.0) as usize), cw - 3.0) } else { (text, tw) };
                            let tx = match align {
                                HAlign::Right => x + cw - tw - 2.0,
                                HAlign::Center => x + (cw - tw) / 2.0,
                                _ => x + 2.0,
                            };
                            if style.wrap {
                                let lines = d.wrap(f, size, &text, cw - 4.0);
                                for (k, l) in lines.iter().enumerate() {
                                    d.text(&mut page, ts, x + 2.0, y + size * (k as f32 + 1.0), l);
                                }
                            } else {
                                page.save();
                                page.raw(&format!("{} {} {} {} re W n", x, ph - y - rh, if align == HAlign::Left && !is_num { cw * 4.0 } else { cw }, rh));
                                d.text(&mut page, ts, tx, y + rh - (rh - size) / 2.0 - size * 0.2, &text);
                                page.restore();
                            }
                        }
                        let b = style.border;
                        if sh.merge_at(r, c).is_some() {
                            x += own_w;
                            continue;
                        }
                        if b[0] {
                            page.line(x, y, x, y + rh, [0, 0, 0], 0.6);
                        }
                        if b[1] {
                            page.line(x, y, x + cw, y, [0, 0, 0], 0.6);
                        }
                        if b[2] {
                            page.line(x + cw, y, x + cw, y + rh, [0, 0, 0], 0.6);
                        }
                        if b[3] {
                            page.line(x, y + rh, x + cw, y + rh, [0, 0, 0], 0.6);
                        }
                        x += own_w;
                    }
                    y += sh.row_height(r) * px * scale;
                }
                // Charts anchored on this page.
                for ch in &sh.charts {
                    if ch.anchor.0 >= ra && ch.anchor.0 <= rb && ch.anchor.1 >= ca && ch.anchor.1 <= cb {
                        let img = crate::charts::render_image(sh, ch, (ch.size.0 * 2.0) as u32, (ch.size.1 * 2.0) as u32);
                        let id = d.add_image_rgba(&img);
                        let cx = m + (sh.col_x(ch.anchor.1) - sh.col_x(ca)) * px * scale;
                        let cy = m + (sh.row_y(ch.anchor.0) - sh.row_y(ra)) * px * scale;
                        page.image(id, cx, cy, ch.size.0 * px * scale, ch.size.1 * px * scale);
                    }
                }
                d.add_page(page);
            }
        }
    }
    d.finish()
}
