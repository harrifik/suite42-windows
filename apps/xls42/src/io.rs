//! Workbook files: XLSX (read/write with styles, merges, freeze panes, charts), ODS (read/write),
//! CSV/TSV (read/write) and legacy XLS (read via calamine).

use crate::formula::{self, tokenize, Tok};
use crate::model::*;
use kit42::ooxml::{esc, parse, relationships, NodeExt, Package, PackageWriter};
use std::collections::{BTreeMap, HashMap};

fn px_from_chars(w: f32) -> f32 {
    (w * 7.0 + 5.0).round()
}

fn chars_from_px(px: f32) -> f32 {
    ((px - 5.0) / 7.0 * 100.0).round() / 100.0
}

fn set_cell(sh: &mut Sheet, r: u32, c: u32, input: String, value: Value, style: u32) {
    sh.cells.insert((r, c), Cell { input, value, style, expr: None });
}

fn hex_rgb(s: &str) -> Option<[u8; 3]> {
    let s = s.trim_start_matches('#');
    let s = if s.len() == 8 { &s[2..] } else { s };
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

fn tint(c: [u8; 3], t: f32) -> [u8; 3] {
    let f = |x: u8| -> u8 {
        let x = x as f32;
        (if t < 0.0 { x * (1.0 + t) } else { x + (255.0 - x) * t }).round().clamp(0.0, 255.0) as u8
    };
    [f(c[0]), f(c[1]), f(c[2])]
}

const INDEXED: [[u8; 3]; 16] = [
    [0, 0, 0], [255, 255, 255], [255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0], [255, 0, 255], [0, 255, 255],
    [0, 0, 0], [255, 255, 255], [255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0], [255, 0, 255], [0, 255, 255],
];

fn builtin_numfmt(id: u32) -> &'static str {
    match id {
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        14 => "dd.mm.yyyy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "hh:mm",
        21 => "hh:mm:ss",
        22 => "dd.mm.yyyy hh:mm",
        37 => "#,##0;-#,##0",
        38 => "#,##0;[Red]-#,##0",
        39 => "#,##0.00;-#,##0.00",
        40 => "#,##0.00;[Red]-#,##0.00",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mm:ss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => "General",
    }
}

pub fn read_xlsx(bytes: &[u8]) -> Result<Workbook, String> {
    let pkg = Package::read(bytes)?;
    let wb_path = relationships(&pkg, "").values().find(|(t, _)| t == "officeDocument").map(|(_, p)| p.clone()).unwrap_or_else(|| "xl/workbook.xml".into());
    let wb_xml = pkg.text(&wb_path).ok_or("workbook.xml is missing")?;
    let wbd = parse(&wb_xml)?;
    let rels = relationships(&pkg, &wb_path);
    let mut wb = Workbook { sheets: vec![], styles: vec![], date1904: false, names: BTreeMap::new() };
    wb.date1904 = wbd.root_element().descendant("workbookPr").and_then(|n| n.attr("date1904")).map(|v| v == "1" || v == "true").unwrap_or(false);
    // Theme colours.
    let mut theme: Vec<[u8; 3]> = vec![[255, 255, 255], [0, 0, 0], [0xE7, 0xE6, 0xE6], [0x44, 0x54, 0x6A], [0x44, 0x72, 0xC4], [0xED, 0x7D, 0x31], [0xA5, 0xA5, 0xA5], [0xFF, 0xC0, 0x00], [0x5B, 0x9B, 0xD5], [0x70, 0xAD, 0x47]];
    if let Some(tp) = rels.values().find(|(t, _)| t == "theme").map(|(_, p)| p.clone()) {
        if let Some(tx) = pkg.text(&tp) {
            if let Ok(td) = parse(&tx) {
                if let Some(cs) = td.root_element().descendant("clrScheme") {
                    let get = |name: &str| -> Option<[u8; 3]> {
                        let n = cs.child(name)?;
                        n.child("srgbClr").and_then(|x| x.attr("val")).and_then(hex_rgb).or_else(|| n.child("sysClr").and_then(|x| x.attr("lastClr")).and_then(hex_rgb))
                    };
                    let order = ["lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6"];
                    for (i, name) in order.iter().enumerate() {
                        if let Some(c) = get(name) {
                            theme[i] = c;
                        }
                    }
                }
            }
        }
    }
    let color_of = |n: roxmltree::Node| -> Option<[u8; 3]> {
        let base = if let Some(rgb) = n.attr("rgb") {
            hex_rgb(rgb)
        } else if let Some(th) = n.attr("theme").and_then(|t| t.parse::<usize>().ok()) {
            theme.get(th).cloned()
        } else if let Some(ix) = n.attr("indexed").and_then(|t| t.parse::<usize>().ok()) {
            if ix == 64 {
                None
            } else {
                INDEXED.get(ix % 16).cloned()
            }
        } else {
            None
        }?;
        Some(match n.attr("tint").and_then(|t| t.parse::<f32>().ok()) {
            Some(t) => tint(base, t),
            None => base,
        })
    };
    // Shared strings.
    let mut sst: Vec<String> = vec![];
    if let Some(sp) = rels.values().find(|(t, _)| t == "sharedStrings").map(|(_, p)| p.clone()) {
        if let Some(x) = pkg.text(&sp) {
            let d = parse(&x)?;
            for si in d.root_element().children_named("si") {
                let mut s = String::new();
                for t in si.descendants().filter(|n| n.is("t")) {
                    if t.ancestors().any(|a| a.is("rPh")) {
                        continue;
                    }
                    s.push_str(t.text().unwrap_or(""));
                }
                sst.push(s);
            }
        }
    }
    // Styles → our Style table indexed like cellXfs.
    let mut xf_styles: Vec<Style> = vec![];
    if let Some(sp) = rels.values().find(|(t, _)| t == "styles").map(|(_, p)| p.clone()) {
        if let Some(x) = pkg.text(&sp) {
            let d = parse(&x)?;
            let root = d.root_element();
            let mut numfmts: HashMap<u32, String> = HashMap::new();
            if let Some(nf) = root.child("numFmts") {
                for f in nf.children_named("numFmt") {
                    if let (Some(id), Some(code)) = (f.attr("numFmtId").and_then(|v| v.parse().ok()), f.attr("formatCode")) {
                        numfmts.insert(id, code.to_string());
                    }
                }
            }
            let fonts: Vec<roxmltree::Node> = root.child("fonts").map(|f| f.children_named("font")).unwrap_or_default();
            let fills: Vec<roxmltree::Node> = root.child("fills").map(|f| f.children_named("fill")).unwrap_or_default();
            let borders: Vec<roxmltree::Node> = root.child("borders").map(|f| f.children_named("border")).unwrap_or_default();
            if let Some(xfs) = root.child("cellXfs") {
                for xf in xfs.children_named("xf") {
                    let mut st = Style::default();
                    let id = |k: &str| xf.attr(k).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                    if let Some(f) = fonts.get(id("fontId")) {
                        st.bold = f.child("b").map(|b| b.attr("val") != Some("0")).unwrap_or(false);
                        st.italic = f.child("i").map(|b| b.attr("val") != Some("0")).unwrap_or(false);
                        st.underline = f.child("u").map(|u| u.attr("val") != Some("none")).unwrap_or(false);
                        st.strike = f.child("strike").map(|b| b.attr("val") != Some("0")).unwrap_or(false);
                        if let Some(sz) = f.child("sz").and_then(|s| s.attr("val")).and_then(|v| v.parse::<f32>().ok()) {
                            st.size10 = (sz * 10.0) as u32;
                        }
                        if let Some(n) = f.child("name").and_then(|s| s.attr("val")) {
                            st.font = n.to_string();
                        }
                        if let Some(c) = f.child("color").and_then(color_of) {
                            st.color = c;
                        }
                    }
                    if let Some(fl) = fills.get(id("fillId")) {
                        if let Some(pf) = fl.child("patternFill") {
                            if pf.attr("patternType").map(|p| p != "none" && p != "gray125").unwrap_or(false) {
                                st.fill = pf.child("fgColor").and_then(color_of).or_else(|| pf.child("bgColor").and_then(color_of));
                            }
                        }
                    }
                    if let Some(bd) = borders.get(id("borderId")) {
                        for (i, side) in ["left", "top", "right", "bottom"].iter().enumerate() {
                            st.border[i] = bd.child(side).and_then(|s| s.attr("style")).map(|s| s != "none").unwrap_or(false);
                        }
                    }
                    let nf = id("numFmtId") as u32;
                    st.numfmt = numfmts.get(&nf).cloned().unwrap_or_else(|| builtin_numfmt(nf).to_string());
                    if let Some(al) = xf.child("alignment") {
                        st.halign = match al.attr("horizontal") {
                            Some("left") => HAlign::Left,
                            Some("center") | Some("centerContinuous") => HAlign::Center,
                            Some("right") => HAlign::Right,
                            _ => HAlign::General,
                        };
                        st.valign = match al.attr("vertical") {
                            Some("top") => VAlign::Top,
                            Some("center") => VAlign::Center,
                            _ => VAlign::Bottom,
                        };
                        st.wrap = al.attr("wrapText").map(|v| v == "1" || v == "true").unwrap_or(false);
                    }
                    if let Some(p) = xf.child("protection") {
                        st.locked = p.attr("locked") != Some("0");
                    }
                    xf_styles.push(st);
                }
            }
        }
    }
    if xf_styles.is_empty() {
        xf_styles.push(Style::default());
    }
    let mut style_map: Vec<u32> = vec![];
    for st in &xf_styles {
        style_map.push(wb.style_id(st));
    }
    // Defined names.
    if let Some(dn) = wbd.root_element().descendant("definedNames") {
        for n in dn.children_named("definedName") {
            if let Some(name) = n.attr("name") {
                if !name.starts_with("_xlnm") {
                    wb.names.insert(name.to_uppercase(), format!("={}", n.text().unwrap_or("")));
                }
            }
        }
    }
    let sheets_node = wbd.root_element().descendant("sheets").ok_or("no sheets")?;
    for s in sheets_node.children_named("sheet") {
        let name = s.attr("name").unwrap_or("Sheet").to_string();
        let rid = s.attributes().find(|a| a.name() == "id").map(|a| a.value()).unwrap_or("");
        let Some((_, path)) = rels.get(rid) else { continue };
        let mut sh = Sheet::new(&name);
        let Some(x) = pkg.text(path) else {
            wb.sheets.push(sh);
            continue;
        };
        let d = parse(&x)?;
        let root = d.root_element();
        let mut shared: HashMap<String, (u32, u32, String)> = HashMap::new();
        if let Some(sd) = root.child("sheetData") {
            for (ri, row) in sd.children_named("row").into_iter().enumerate() {
                let r: u32 = row.attr("r").and_then(|v| v.parse::<u32>().ok()).map(|v| v - 1).unwrap_or(ri as u32);
                if let Some(ht) = row.attr("ht").and_then(|v| v.parse::<f32>().ok()) {
                    if row.attr("customHeight").map(|v| v == "1" || v == "true").unwrap_or(false) || (ht - 15.0).abs() > 0.5 {
                        sh.row_h.insert(r, (ht * 4.0 / 3.0).round());
                    }
                }
                if row.attr("hidden").map(|v| v == "1" || v == "true").unwrap_or(false) {
                    sh.hidden_rows.insert(r);
                }
                let mut col_cursor = 0u32;
                for c in row.children_named("c") {
                    let (rr, cc) = c.attr("r").and_then(parse_addr).unwrap_or((r, col_cursor));
                    col_cursor = cc + 1;
                    let style = c.attr("s").and_then(|v| v.parse::<usize>().ok()).and_then(|i| style_map.get(i).cloned()).unwrap_or(0);
                    let ty = c.attr("t").unwrap_or("n");
                    let v = c.child("v").and_then(|v| v.text()).unwrap_or("");
                    let value = match ty {
                        "s" => Value::Str(v.parse::<usize>().ok().and_then(|i| sst.get(i).cloned()).unwrap_or_default()),
                        "str" => Value::Str(v.to_string()),
                        "inlineStr" => Value::Str(c.child("is").map(|n| n.descendants().filter(|x| x.is("t")).filter_map(|x| x.text()).collect()).unwrap_or_default()),
                        "b" => Value::Bool(v == "1"),
                        "e" => Value::Err(v.to_string()),
                        _ => v.parse::<f64>().map(Value::Num).unwrap_or(Value::Empty),
                    };
                    let mut input = String::new();
                    if let Some(f) = c.child("f") {
                        let text = f.text().unwrap_or("").to_string();
                        if f.attr("t") == Some("shared") {
                            let si = f.attr("si").unwrap_or("").to_string();
                            if !text.is_empty() {
                                shared.insert(si, (rr, cc, text.clone()));
                                input = format!("={text}");
                            } else if let Some((r0, c0, base)) = shared.get(&si) {
                                input = formula::shift_formula(&format!("={base}"), rr as i64 - *r0 as i64, cc as i64 - *c0 as i64);
                            }
                        } else if !text.is_empty() {
                            input = format!("={text}");
                        }
                    }
                    if input.is_empty() {
                        input = match &value {
                            Value::Num(n) => fmt_general(*n),
                            Value::Str(s) => {
                                if parse_input(s).0 != Value::Str(s.clone()) || s.starts_with('=') {
                                    format!("'{s}")
                                } else {
                                    s.clone()
                                }
                            }
                            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
                            Value::Err(e) => e.clone(),
                            Value::Empty => String::new(),
                        };
                    }
                    if input.is_empty() && style == 0 {
                        continue;
                    }
                    set_cell(&mut sh, rr, cc, input, value, style);
                }
            }
        }
        if let Some(cols) = root.child("cols") {
            for col in cols.children_named("col") {
                let min: u32 = col.attr("min").and_then(|v| v.parse().ok()).unwrap_or(1);
                let max: u32 = col.attr("max").and_then(|v| v.parse().ok()).unwrap_or(min).min(min + 300);
                let w = col.attr("width").and_then(|v| v.parse::<f32>().ok());
                let hidden = col.attr("hidden").map(|v| v == "1" || v == "true").unwrap_or(false);
                for c in min..=max {
                    if let Some(w) = w {
                        let px = px_from_chars(w);
                        if (px - DEF_COL_W).abs() > 1.0 {
                            sh.col_w.insert(c - 1, px);
                        }
                    }
                    if hidden {
                        sh.hidden_cols.insert(c - 1);
                    }
                }
            }
        }
        if let Some(mc) = root.child("mergeCells") {
            for m in mc.children_named("mergeCell") {
                if let Some((a, b)) = m.attr("ref").and_then(|r| r.split_once(':')) {
                    if let (Some((r0, c0)), Some((r1, c1))) = (parse_addr(a), parse_addr(b)) {
                        sh.merges.push((r0, c0, r1, c1));
                    }
                }
            }
        }
        if let Some(pane) = root.descendant("pane") {
            if pane.attr("state").map(|s| s.starts_with("frozen")).unwrap_or(false) {
                let x = pane.attr("xSplit").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) as u32;
                let y = pane.attr("ySplit").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) as u32;
                sh.freeze = (y, x);
            }
        }
        if root.descendant("sheetView").and_then(|v| v.attr("showGridLines")) == Some("0") {
            sh.gridlines = false;
        }
        if let Some(tc) = root.descendant("tabColor").and_then(color_of) {
            sh.tab_color = Some(tc);
        }
        if let Some(af) = root.child("autoFilter").and_then(|a| a.attr("ref")) {
            if let Some((a, b)) = af.split_once(':') {
                if let (Some((r0, c0)), Some((r1, c1))) = (parse_addr(a), parse_addr(b)) {
                    sh.filter = Some((r0, c0, r1, c1));
                }
            }
        }
        // Charts through the drawing part.
        let srels = relationships(&pkg, path);
        if let Some((_, dpath)) = srels.values().find(|(t, _)| t == "drawing") {
            if let Some(dx) = pkg.text(dpath) {
                if let Ok(dd) = parse(&dx) {
                    let drels = relationships(&pkg, dpath);
                    for anchor in dd.root_element().children().filter(|n| n.is("twoCellAnchor") || n.is("oneCellAnchor")) {
                        let pos = |tag: &str| -> Option<(u32, u32)> {
                            let n = anchor.child(tag)?;
                            Some((n.child("row")?.text()?.parse().ok()?, n.child("col")?.text()?.parse().ok()?))
                        };
                        let Some(chart_rid) = anchor.descendant("chart").and_then(|c| c.attributes().find(|a| a.name() == "id").map(|a| a.value().to_string())) else { continue };
                        let Some((_, cpath)) = drels.get(&chart_rid) else { continue };
                        let Some(cx) = pkg.text(cpath) else { continue };
                        let Ok(cd) = parse(&cx) else { continue };
                        let plot = cd.root_element().descendant("plotArea");
                        let (kind, node) = ["barChart", "bar3DChart", "lineChart", "line3DChart", "areaChart", "pieChart", "pie3DChart", "doughnutChart", "scatterChart"]
                            .iter()
                            .find_map(|k| plot.and_then(|p| p.child(k)).map(|n| (*k, n)))
                            .unwrap_or(("barChart", cd.root_element()));
                        let kind = match kind {
                            "barChart" | "bar3DChart" => {
                                if node.child("barDir").and_then(|b| b.attr("val")) == Some("bar") {
                                    ChartKind::Bar
                                } else {
                                    ChartKind::Column
                                }
                            }
                            "lineChart" | "line3DChart" => ChartKind::Line,
                            "areaChart" => ChartKind::Area,
                            "scatterChart" => ChartKind::Scatter,
                            _ => ChartKind::Pie,
                        };
                        let mut rng: Option<(u32, u32, u32, u32)> = None;
                        for f in node.descendants().filter(|n| n.is("f")) {
                            let t = f.text().unwrap_or("");
                            let refpart = t.rsplit('!').next().unwrap_or(t).replace('$', "");
                            let (a, b) = refpart.split_once(':').unwrap_or((&refpart, &refpart));
                            if let (Some((r0, c0)), Some((r1, c1))) = (parse_addr(a), parse_addr(b)) {
                                rng = Some(match rng {
                                    None => (r0, c0, r1, c1),
                                    Some((a0, b0, a1, b1)) => (a0.min(r0), b0.min(c0), a1.max(r1), b1.max(c1)),
                                });
                            }
                        }
                        let title = cd.root_element().descendant("title").map(|t| t.descendants().filter(|n| n.is("t")).filter_map(|n| n.text()).collect::<String>()).unwrap_or_default();
                        if let Some(range) = rng {
                            let from = pos("from").unwrap_or((0, 0));
                            let to = pos("to").unwrap_or((from.0 + 15, from.1 + 7));
                            let w = sh.col_x(to.1) - sh.col_x(from.1);
                            let h = sh.row_y(to.0) - sh.row_y(from.0);
                            sh.charts.push(Chart { id: rand::random(), kind, range, title, anchor: from, size: (w.max(200.0), h.max(150.0)), legend: true });
                        }
                    }
                }
            }
        }
        wb.sheets.push(sh);
    }
    if wb.sheets.is_empty() {
        wb.sheets.push(Sheet::new("Sheet1"));
    }
    formula::recalc(&mut wb);
    Ok(wb)
}

pub fn write_xlsx(wb: &Workbook) -> Result<Vec<u8>, String> {
    // Styles.
    let mut fonts: Vec<String> = vec![];
    let mut fills: Vec<String> = vec!["<fill><patternFill patternType=\"none\"/></fill>".into(), "<fill><patternFill patternType=\"gray125\"/></fill>".into()];
    let mut borders: Vec<String> = vec!["<border><left/><right/><top/><bottom/><diagonal/></border>".into()];
    let mut numfmts: Vec<(u32, String)> = vec![];
    let mut xfs = String::new();
    let idx = |list: &mut Vec<String>, s: String| -> usize {
        match list.iter().position(|x| *x == s) {
            Some(i) => i,
            None => {
                list.push(s);
                list.len() - 1
            }
        }
    };
    for st in &wb.styles {
        let font = format!(
            "<font>{}{}{}{}<sz val=\"{}\"/><color rgb=\"FF{:02X}{:02X}{:02X}\"/><name val=\"{}\"/></font>",
            if st.bold { "<b/>" } else { "" },
            if st.italic { "<i/>" } else { "" },
            if st.strike { "<strike/>" } else { "" },
            if st.underline { "<u/>" } else { "" },
            st.size(),
            st.color[0], st.color[1], st.color[2],
            esc(&st.font)
        );
        let fi = idx(&mut fonts, font);
        let fill = match st.fill {
            Some(c) => format!("<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FF{:02X}{:02X}{:02X}\"/><bgColor indexed=\"64\"/></patternFill></fill>", c[0], c[1], c[2]),
            None => fills[0].clone(),
        };
        let fl = idx(&mut fills, fill);
        let side = |on: bool, name: &str| if on { format!("<{name} style=\"thin\"><color auto=\"1\"/></{name}>") } else { format!("<{name}/>") };
        let border = format!("<border>{}{}{}{}<diagonal/></border>", side(st.border[0], "left"), side(st.border[2], "right"), side(st.border[1], "top"), side(st.border[3], "bottom"));
        let bi = idx(&mut borders, border);
        let builtin = (0..50u32).find(|i| builtin_numfmt(*i) == st.numfmt && (*i == 0 || builtin_numfmt(*i) != "General"));
        let nf = match builtin {
            Some(i) => i,
            None => match numfmts.iter().find(|(_, c)| *c == st.numfmt) {
                Some((i, _)) => *i,
                None => {
                    let id = 164 + numfmts.len() as u32;
                    numfmts.push((id, st.numfmt.clone()));
                    id
                }
            },
        };
        let h = match st.halign {
            HAlign::General => "",
            HAlign::Left => " horizontal=\"left\"",
            HAlign::Center => " horizontal=\"center\"",
            HAlign::Right => " horizontal=\"right\"",
        };
        let v = match st.valign {
            VAlign::Top => " vertical=\"top\"",
            VAlign::Center => " vertical=\"center\"",
            VAlign::Bottom => "",
        };
        let wrap = if st.wrap { " wrapText=\"1\"" } else { "" };
        let align = if !h.is_empty() || !v.is_empty() || st.wrap { format!("<alignment{h}{v}{wrap}/>") } else { String::new() };
        let prot = if !st.locked { "<protection locked=\"0\"/>" } else { "" };
        xfs.push_str(&format!(
            "<xf numFmtId=\"{nf}\" fontId=\"{fi}\" fillId=\"{fl}\" borderId=\"{bi}\" xfId=\"0\" applyNumberFormat=\"1\" applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\" applyAlignment=\"1\"{}>{align}{prot}</xf>",
            if prot.is_empty() { "" } else { " applyProtection=\"1\"" }
        ));
    }
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="{}">{}</numFmts><fonts count="{}">{}</fonts><fills count="{}">{}</fills><borders count="{}">{}</borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="{}">{}</cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#,
        numfmts.len(),
        numfmts.iter().map(|(i, c)| format!("<numFmt numFmtId=\"{i}\" formatCode=\"{}\"/>", esc(c))).collect::<String>(),
        fonts.len(),
        fonts.concat(),
        fills.len(),
        fills.concat(),
        borders.len(),
        borders.concat(),
        wb.styles.len(),
        xfs
    );
    // Shared strings.
    let mut sst: Vec<String> = vec![];
    let mut sst_idx: HashMap<String, usize> = HashMap::new();
    let mut pkg = PackageWriter::new();
    let mut ct_extra = String::new();
    let mut wb_rels = String::new();
    let mut sheets_xml = String::new();
    let mut chart_n = 0;
    for (si, sh) in wb.sheets.iter().enumerate() {
        let mut x = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#);
        if let Some(tc) = sh.tab_color {
            x.push_str(&format!("<sheetPr><tabColor rgb=\"FF{:02X}{:02X}{:02X}\"/></sheetPr>", tc[0], tc[1], tc[2]));
        }
        if let Some((r, c)) = sh.used() {
            x.push_str(&format!("<dimension ref=\"A1:{}\"/>", addr(r, c)));
        }
        x.push_str(&format!("<sheetViews><sheetView workbookViewId=\"0\"{}{}>", if si == 0 { " tabSelected=\"1\"" } else { "" }, if sh.gridlines { "" } else { " showGridLines=\"0\"" }));
        if sh.freeze != (0, 0) {
            let (fr, fc) = sh.freeze;
            let pane = match (fr > 0, fc > 0) {
                (true, true) => "bottomRight",
                (true, false) => "bottomLeft",
                _ => "topRight",
            };
            x.push_str(&format!(
                "<pane{}{} topLeftCell=\"{}\" activePane=\"{pane}\" state=\"frozen\"/>",
                if fc > 0 { format!(" xSplit=\"{fc}\"") } else { String::new() },
                if fr > 0 { format!(" ySplit=\"{fr}\"") } else { String::new() },
                addr(fr, fc)
            ));
        }
        x.push_str("</sheetView></sheetViews><sheetFormatPr defaultRowHeight=\"15\"/>");
        let mut cols: Vec<u32> = sh.col_w.keys().cloned().chain(sh.hidden_cols.iter().cloned()).collect();
        cols.sort();
        cols.dedup();
        if !cols.is_empty() {
            x.push_str("<cols>");
            for c in cols {
                let w = sh.col_w.get(&c).cloned().unwrap_or(DEF_COL_W);
                x.push_str(&format!(
                    "<col min=\"{0}\" max=\"{0}\" width=\"{1}\" customWidth=\"1\"{2}/>",
                    c + 1,
                    chars_from_px(w),
                    if sh.hidden_cols.contains(&c) { " hidden=\"1\"" } else { "" }
                ));
            }
            x.push_str("</cols>");
        }
        x.push_str("<sheetData>");
        let mut rows: BTreeMap<u32, Vec<(u32, &Cell)>> = BTreeMap::new();
        for (&(r, c), cell) in &sh.cells {
            rows.entry(r).or_default().push((c, cell));
        }
        for r in sh.row_h.keys().chain(sh.hidden_rows.iter()) {
            rows.entry(*r).or_default();
        }
        for (r, cells) in rows {
            let ht = sh.row_h.get(&r).map(|h| format!(" ht=\"{}\" customHeight=\"1\"", h * 0.75)).unwrap_or_default();
            let hid = if sh.hidden_rows.contains(&r) { " hidden=\"1\"" } else { "" };
            x.push_str(&format!("<row r=\"{}\"{ht}{hid}>", r + 1));
            for (c, cell) in cells {
                let a = addr(r, c);
                let s = if cell.style != 0 { format!(" s=\"{}\"", cell.style) } else { String::new() };
                if cell.is_formula() {
                    let f = esc(&cell.input[1..]);
                    match &cell.value {
                        Value::Num(n) => x.push_str(&format!("<c r=\"{a}\"{s}><f>{f}</f><v>{n}</v></c>")),
                        Value::Str(t) => x.push_str(&format!("<c r=\"{a}\"{s} t=\"str\"><f>{f}</f><v>{}</v></c>", esc(t))),
                        Value::Bool(b) => x.push_str(&format!("<c r=\"{a}\"{s} t=\"b\"><f>{f}</f><v>{}</v></c>", *b as u8)),
                        Value::Err(e) => x.push_str(&format!("<c r=\"{a}\"{s} t=\"e\"><f>{f}</f><v>{}</v></c>", esc(e))),
                        Value::Empty => x.push_str(&format!("<c r=\"{a}\"{s}><f>{f}</f></c>")),
                    }
                } else {
                    match &cell.value {
                        Value::Num(n) => x.push_str(&format!("<c r=\"{a}\"{s}><v>{n}</v></c>")),
                        Value::Str(t) => {
                            let i = *sst_idx.entry(t.clone()).or_insert_with(|| {
                                sst.push(t.clone());
                                sst.len() - 1
                            });
                            x.push_str(&format!("<c r=\"{a}\"{s} t=\"s\"><v>{i}</v></c>"));
                        }
                        Value::Bool(b) => x.push_str(&format!("<c r=\"{a}\"{s} t=\"b\"><v>{}</v></c>", *b as u8)),
                        Value::Err(e) => x.push_str(&format!("<c r=\"{a}\"{s} t=\"e\"><v>{}</v></c>", esc(e))),
                        Value::Empty => {
                            if cell.style != 0 {
                                x.push_str(&format!("<c r=\"{a}\"{s}/>"));
                            }
                        }
                    }
                }
            }
            x.push_str("</row>");
        }
        x.push_str("</sheetData>");
        if let Some((r0, c0, r1, c1)) = sh.filter {
            x.push_str(&format!("<autoFilter ref=\"{}:{}\"/>", addr(r0, c0), addr(r1, c1)));
        }
        if !sh.merges.is_empty() {
            x.push_str(&format!("<mergeCells count=\"{}\">", sh.merges.len()));
            for (r0, c0, r1, c1) in &sh.merges {
                x.push_str(&format!("<mergeCell ref=\"{}:{}\"/>", addr(*r0, *c0), addr(*r1, *c1)));
            }
            x.push_str("</mergeCells>");
        }
        x.push_str("<pageMargins left=\"0.7\" right=\"0.7\" top=\"0.75\" bottom=\"0.75\" header=\"0.3\" footer=\"0.3\"/>");
        let mut srels = String::new();
        if !sh.charts.is_empty() {
            x.push_str("<drawing r:id=\"rIdDr\"/>");
            let dn = si + 1;
            srels = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdDr" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing{dn}.xml"/></Relationships>"#);
            let mut drawing = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">"#);
            let mut drels = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
            for (k, ch) in sh.charts.iter().enumerate() {
                chart_n += 1;
                let to_c = sh.col_at(sh.col_x(ch.anchor.1) + ch.size.0);
                let to_r = sh.row_at(sh.row_y(ch.anchor.0) + ch.size.1);
                drawing.push_str(&format!(
                    r#"<xdr:twoCellAnchor><xdr:from><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>{to_c}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{to_r}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{}" name="Chart {}"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart r:id="rIdC{k}"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor>"#,
                    ch.anchor.1, ch.anchor.0, k + 2, k + 1
                ));
                drels.push_str(&format!("<Relationship Id=\"rIdC{k}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart\" Target=\"../charts/chart{chart_n}.xml\"/>"));
                pkg.add_str(&format!("xl/charts/chart{chart_n}.xml"), &chart_xml(sh, ch))?;
                ct_extra.push_str(&format!("<Override PartName=\"/xl/charts/chart{chart_n}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawingml.chart+xml\"/>"));
            }
            drawing.push_str("</xdr:wsDr>");
            drels.push_str("</Relationships>");
            pkg.add_str(&format!("xl/drawings/drawing{dn}.xml"), &drawing)?;
            pkg.add_str(&format!("xl/drawings/_rels/drawing{dn}.xml.rels"), &drels)?;
            ct_extra.push_str(&format!("<Override PartName=\"/xl/drawings/drawing{dn}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawing+xml\"/>"));
        }
        x.push_str("</worksheet>");
        pkg.add_str(&format!("xl/worksheets/sheet{}.xml", si + 1), &x)?;
        if !srels.is_empty() {
            pkg.add_str(&format!("xl/worksheets/_rels/sheet{}.xml.rels", si + 1), &srels)?;
        }
        ct_extra.push_str(&format!("<Override PartName=\"/xl/worksheets/sheet{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>", si + 1));
        wb_rels.push_str(&format!("<Relationship Id=\"rIdS{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{}.xml\"/>", si + 1, si + 1));
        sheets_xml.push_str(&format!("<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rIdS{}\"/>", esc(&sh.name), si + 1, si + 1));
    }
    let names: String = wb.names.iter().map(|(k, v)| format!("<definedName name=\"{}\">{}</definedName>", esc(k), esc(v.trim_start_matches('=')))).collect();
    let workbook = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><workbookPr{}/><bookViews><workbookView/></bookViews><sheets>{sheets_xml}</sheets>{}<calcPr calcId="191029" fullCalcOnLoad="1"/></workbook>"#,
        if wb.date1904 { " date1904=\"1\"" } else { "" },
        if names.is_empty() { String::new() } else { format!("<definedNames>{names}</definedNames>") }
    );
    let sst_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{0}" uniqueCount="{0}">{1}</sst>"#,
        sst.len(),
        sst.iter().map(|s| format!("<si><t xml:space=\"preserve\">{}</t></si>", esc(s))).collect::<String>()
    );
    pkg.add_str("[Content_Types].xml", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>{ct_extra}</Types>"#))?;
    pkg.add_str("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/></Relationships>"#)?;
    pkg.add_str("xl/workbook.xml", &workbook)?;
    pkg.add_str("xl/_rels/workbook.xml.rels", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{wb_rels}<Relationship Id="rIdSt" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rIdSs" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#))?;
    pkg.add_str("xl/styles.xml", &styles)?;
    pkg.add_str("xl/sharedStrings.xml", &sst_xml)?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    pkg.add_str("docProps/core.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dcterms:created xsi:type="dcterms:W3CDTF">{now}</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">{now}</dcterms:modified></cp:coreProperties>"#))?;
    pkg.add_str("docProps/app.xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>XLS42 for Windows</Application></Properties>"#)?;
    pkg.finish()
}

/// Series of a chart: (name, category labels, values per point) from its range.
pub fn chart_series(sh: &Sheet, ch: &Chart) -> (Vec<String>, Vec<(String, Vec<f64>)>, bool) {
    let (r0, c0, r1, c1) = ch.range;
    let header = (c0..=c1).skip(1).any(|c| matches!(sh.value(r0, c), Value::Str(_)));
    let cat_col = (r0..=r1).skip(if header { 1 } else { 0 }).any(|r| matches!(sh.value(r, c0), Value::Str(_))) || (c1 > c0 && matches!(ch.kind, ChartKind::Scatter));
    let data_r0 = if header { r0 + 1 } else { r0 };
    let data_c0 = if cat_col { c0 + 1 } else { c0 };
    let cats: Vec<String> = (data_r0..=r1).map(|r| if cat_col { sh.value(r, c0).as_text() } else { (r - data_r0 + 1).to_string() }).collect();
    let series = (data_c0..=c1)
        .map(|c| {
            let name = if header { sh.value(r0, c).as_text() } else { format!("{} {}", kit42::t("Series", "Ряд"), c - data_c0 + 1) };
            (name, (data_r0..=r1).map(|r| sh.value(r, c).as_num().unwrap_or(0.0)).collect())
        })
        .collect();
    (cats, series, cat_col)
}

fn chart_xml(sh: &Sheet, ch: &Chart) -> String {
    let (r0, c0, r1, c1) = ch.range;
    let (cats, series, cat_col) = chart_series(sh, ch);
    let header = (c0..=c1).skip(1).any(|c| matches!(sh.value(r0, c), Value::Str(_)));
    let data_r0 = if header { r0 + 1 } else { r0 };
    let data_c0 = if cat_col { c0 + 1 } else { c0 };
    let sn = quote_sheet(&sh.name);
    let abs = |r: u32, c: u32| format!("${}${}", col_name(c), r + 1);
    let mut ser = String::new();
    for (i, (name, vals)) in series.iter().enumerate() {
        let c = data_c0 + i as u32;
        let tx = if header { format!("<c:tx><c:strRef><c:f>{sn}!{}</c:f><c:strCache><c:ptCount val=\"1\"/><c:pt idx=\"0\"><c:v>{}</c:v></c:pt></c:strCache></c:strRef></c:tx>", abs(r0, c), esc(name)) } else { String::new() };
        let cat = if cat_col {
            format!(
                "<c:{tag}><c:strRef><c:f>{sn}!{}:{}</c:f><c:strCache><c:ptCount val=\"{}\"/>{}</c:strCache></c:strRef></c:{tag}>",
                abs(data_r0, c0), abs(r1, c0), cats.len(),
                cats.iter().enumerate().map(|(k, v)| format!("<c:pt idx=\"{k}\"><c:v>{}</c:v></c:pt>", esc(v))).collect::<String>(),
                tag = if ch.kind == ChartKind::Scatter { "xVal" } else { "cat" }
            )
        } else {
            String::new()
        };
        let val = format!(
            "<c:{tag}><c:numRef><c:f>{sn}!{}:{}</c:f><c:numCache><c:ptCount val=\"{}\"/>{}</c:numCache></c:numRef></c:{tag}>",
            abs(data_r0, c), abs(r1, c), vals.len(),
            vals.iter().enumerate().map(|(k, v)| format!("<c:pt idx=\"{k}\"><c:v>{v}</c:v></c:pt>")).collect::<String>(),
            tag = if ch.kind == ChartKind::Scatter { "yVal" } else { "val" }
        );
        ser.push_str(&format!("<c:ser><c:idx val=\"{i}\"/><c:order val=\"{i}\"/>{tx}{cat}{val}</c:ser>"));
    }
    let axes = "<c:axId val=\"1001\"/><c:axId val=\"1002\"/>";
    let (plot, need_axes) = match ch.kind {
        ChartKind::Column | ChartKind::Bar => (format!("<c:barChart><c:barDir val=\"{}\"/><c:grouping val=\"clustered\"/><c:varyColors val=\"0\"/>{ser}<c:gapWidth val=\"150\"/>{axes}</c:barChart>", if ch.kind == ChartKind::Bar { "bar" } else { "col" }), true),
        ChartKind::Line => (format!("<c:lineChart><c:grouping val=\"standard\"/><c:varyColors val=\"0\"/>{ser}<c:marker val=\"1\"/>{axes}</c:lineChart>"), true),
        ChartKind::Area => (format!("<c:areaChart><c:grouping val=\"standard\"/><c:varyColors val=\"0\"/>{ser}{axes}</c:areaChart>"), true),
        ChartKind::Pie => (format!("<c:pieChart><c:varyColors val=\"1\"/>{ser}<c:firstSliceAng val=\"0\"/></c:pieChart>"), false),
        ChartKind::Scatter => (format!("<c:scatterChart><c:scatterStyle val=\"lineMarker\"/><c:varyColors val=\"0\"/>{ser}{axes}</c:scatterChart>"), true),
    };
    let axes_xml = if need_axes {
        let first = if ch.kind == ChartKind::Scatter { "<c:valAx><c:axId val=\"1001\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:crossAx val=\"1002\"/></c:valAx>" } else { "<c:catAx><c:axId val=\"1001\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:crossAx val=\"1002\"/></c:catAx>" };
        format!("{first}<c:valAx><c:axId val=\"1002\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"l\"/><c:majorGridlines/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/><c:crossAx val=\"1001\"/></c:valAx>")
    } else {
        String::new()
    };
    let title = if ch.title.is_empty() { "<c:autoTitleDeleted val=\"1\"/>".to_string() } else { format!("<c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val=\"0\"/></c:title><c:autoTitleDeleted val=\"0\"/>", esc(&ch.title)) };
    let legend = if ch.legend { "<c:legend><c:legendPos val=\"r\"/><c:overlay val=\"0\"/></c:legend>" } else { "" };
    format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><c:chart>{title}<c:plotArea><c:layout/>{plot}{axes_xml}</c:plotArea>{legend}<c:plotVisOnly val="1"/></c:chart></c:chartSpace>"#)
}

// ───────────────────────── ODS ─────────────────────────

/// OpenFormula → Excel syntax: of:=SUM([.A1:.B2];[Sheet2.C3]) → =SUM(A1:B2,Sheet2!C3)
pub fn from_odf_formula(f: &str) -> String {
    let body = f.trim_start_matches("of:").trim_start_matches("oooc:").trim_start_matches('=');
    let mut out = String::from("=");
    let cs: Vec<char> = body.chars().collect();
    let mut i = 0;
    let mut in_str = false;
    while i < cs.len() {
        let c = cs[i];
        if c == '"' {
            in_str = !in_str;
            out.push(c);
            i += 1;
            continue;
        }
        if in_str {
            out.push(c);
            i += 1;
            continue;
        }
        if c == '[' {
            let end = cs[i..].iter().position(|x| *x == ']').map(|p| i + p).unwrap_or(cs.len());
            let inner: String = cs[i + 1..end].iter().collect();
            let conv = |part: &str| -> String {
                let p = part.trim_start_matches('$');
                match p.rsplit_once('.') {
                    Some((sheet, cell)) if !sheet.is_empty() => format!("{}!{}", quote_sheet(sheet.trim_matches('\'').trim_start_matches('$')), cell),
                    Some((_, cell)) => cell.to_string(),
                    None => p.to_string(),
                }
            };
            let text = match inner.split_once(':') {
                Some((a, b)) => format!("{}:{}", conv(a), conv(b).rsplit('!').next().unwrap_or("")),
                None => conv(&inner),
            };
            out.push_str(&text);
            i = end + 1;
            continue;
        }
        out.push(if c == ';' { ',' } else { c });
        i += 1;
    }
    out
}

pub fn to_odf_formula(f: &str) -> String {
    let body = &f[1..];
    let Ok(toks) = tokenize(body) else { return format!("of:{f}") };
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::from("of:=");
    let mut i = 0;
    let mut last = 0;
    let mut depth_brace = 0;
    while i < toks.len() {
        let (t, s, e) = &toks[i];
        match t {
            Tok::LBrace => depth_brace += 1,
            Tok::RBrace => depth_brace -= 1,
            _ => {}
        }
        let (sheet, j) = if let Tok::Sheet(n) = t { (Some(n.clone()), i + 1) } else { (None, i) };
        if let Some((Tok::Ident(id), _, mut end)) = toks.get(j).cloned() {
            let is_func = matches!(toks.get(j + 1), Some((Tok::LParen, ..)));
            if !is_func && formula::parse_ref(&id).is_some() {
                let mut text = id.clone();
                let mut jj = j;
                if let (Some((Tok::Colon, ..)), Some((Tok::Ident(id2), _, e2))) = (toks.get(j + 1), toks.get(j + 2)) {
                    if formula::parse_ref(id2).is_some() {
                        text = format!("{id}:.{id2}");
                        end = *e2;
                        jj = j + 2;
                    }
                }
                out.extend(&chars[last..*s]);
                match &sheet {
                    Some(sn) => out.push_str(&format!("[${}.{}]", if sn.chars().all(|c| c.is_alphanumeric()) { sn.clone() } else { format!("'{sn}'") }, text)),
                    None => out.push_str(&format!("[.{text}]")),
                }
                last = end;
                i = jj + 1;
                continue;
            }
        }
        if *t == Tok::Comma && depth_brace == 0 {
            out.extend(&chars[last..*s]);
            out.push(';');
            last = *e;
        }
        i += 1;
    }
    out.extend(&chars[last..]);
    out
}

pub fn read_ods(bytes: &[u8]) -> Result<Workbook, String> {
    let pkg = Package::read(bytes)?;
    let xml = pkg.text("content.xml").ok_or("content.xml is missing")?;
    let d = parse(&xml)?;
    let mut wb = Workbook { sheets: vec![], styles: vec![Style::default()], date1904: false, names: BTreeMap::new() };
    // Automatic styles: cell styles, column widths, row heights.
    let mut cell_styles: HashMap<String, Style> = HashMap::new();
    let mut col_widths: HashMap<String, f32> = HashMap::new();
    let mut row_heights: HashMap<String, f32> = HashMap::new();
    for s in d.root_element().descendants().filter(|n| n.is("style") && n.parent().map(|p| p.is("automatic-styles")).unwrap_or(false)) {
        let name = s.attr("name").unwrap_or("").to_string();
        match s.attr("family") {
            Some("table-cell") => {
                let mut st = Style::default();
                if let Some(tp) = s.child("text-properties") {
                    st.bold = tp.attr("font-weight") == Some("bold");
                    st.italic = tp.attr("font-style") == Some("italic");
                    st.underline = tp.attr("text-underline-style").map(|v| v != "none").unwrap_or(false);
                    if let Some(c) = tp.attr("color").and_then(hex_rgb) {
                        st.color = c;
                    }
                    if let Some(sz) = tp.attr("font-size").and_then(|v| v.trim_end_matches("pt").parse::<f32>().ok()) {
                        st.size10 = (sz * 10.0) as u32;
                    }
                }
                if let Some(cp) = s.child("table-cell-properties") {
                    st.fill = cp.attr("background-color").filter(|v| *v != "transparent").and_then(hex_rgb);
                    st.wrap = cp.attr("wrap-option") == Some("wrap");
                    if cp.attr("border").map(|b| b != "none").unwrap_or(false) {
                        st.border = [true; 4];
                    }
                }
                if let Some(pp) = s.child("paragraph-properties") {
                    st.halign = match pp.attr("text-align") {
                        Some("center") => HAlign::Center,
                        Some("end") | Some("right") => HAlign::Right,
                        Some("start") | Some("left") => HAlign::Left,
                        _ => HAlign::General,
                    };
                }
                cell_styles.insert(name, st);
            }
            Some("table-column") => {
                if let Some(w) = s.child("table-column-properties").and_then(|p| p.attr("column-width")).and_then(odf_len) {
                    col_widths.insert(name, w * 96.0 / 72.0);
                }
            }
            Some("table-row") => {
                if let Some(h) = s.child("table-row-properties").and_then(|p| p.attr("row-height")).and_then(odf_len) {
                    row_heights.insert(name, h * 96.0 / 72.0);
                }
            }
            _ => {}
        }
    }
    let mut style_ids: HashMap<String, u32> = HashMap::new();
    for (k, v) in &cell_styles {
        style_ids.insert(k.clone(), wb.style_id(v));
    }
    let mut total_cells = 0usize;
    for table in d.root_element().descendants().filter(|n| n.is("table") && n.tag_name().namespace().map(|ns| ns.contains("table")).unwrap_or(false)) {
        let mut sh = Sheet::new(table.attr("name").unwrap_or("Sheet"));
        let mut c = 0u32;
        for col in table.descendants().filter(|n| n.is("table-column")) {
            let rep: u32 = col.attr("number-columns-repeated").and_then(|v| v.parse().ok()).unwrap_or(1).min(1024);
            if let Some(w) = col.attr("style-name").and_then(|s| col_widths.get(s)) {
                for k in 0..rep {
                    if (w - DEF_COL_W).abs() > 1.0 {
                        sh.col_w.insert(c + k, *w);
                    }
                }
            }
            c += rep;
        }
        let mut r = 0u32;
        for row in table.descendants().filter(|n| n.is("table-row")) {
            let rep: u32 = row.attr("number-rows-repeated").and_then(|v| v.parse().ok()).unwrap_or(1);
            let has_content = row.children().any(|c| c.is_element() && (c.attr("value-type").is_some() || c.attr("formula").is_some() || c.has_children()));
            let rep = if has_content { rep.min(10_000) } else { rep };
            if let Some(h) = row.attr("style-name").and_then(|s| row_heights.get(s)) {
                if (h - DEF_ROW_H).abs() > 1.0 && rep < 1000 {
                    for k in 0..rep {
                        sh.row_h.insert(r + k, *h);
                    }
                }
            }
            if row.attr("visibility") == Some("collapse") && rep < 1000 {
                for k in 0..rep {
                    sh.hidden_rows.insert(r + k);
                }
            }
            if has_content {
                for k in 0..rep {
                    let mut c = 0u32;
                    for cell in row.children().filter(|n| n.is("table-cell") || n.is("covered-table-cell")) {
                        let crep: u32 = cell.attr("number-columns-repeated").and_then(|v| v.parse().ok()).unwrap_or(1);
                        let empty = cell.attr("value-type").is_none() && cell.attr("formula").is_none() && !cell.has_children();
                        let style = cell.attr("style-name").and_then(|s| style_ids.get(s)).cloned().unwrap_or(0);
                        if empty {
                            c += crep;
                            continue;
                        }
                        let crep = crep.min(1024);
                        for kk in 0..crep {
                            total_cells += 1;
                            if total_cells > 1_000_000 {
                                return Err(kit42::t("The spreadsheet expands to more than 1,000,000 cells.", "Таблица разворачивается более чем в 1 000 000 ячеек.").into());
                            }
                            let text: String = cell.children().filter(|n| n.is("p")).map(|p| p.text_all()).collect::<Vec<_>>().join("\n");
                            let vt = cell.attr("value-type").unwrap_or("string");
                            let value = match vt {
                                "float" | "percentage" | "currency" => cell.attr("value").and_then(|v| v.parse().ok()).map(Value::Num).unwrap_or(Value::Empty),
                                "date" => cell.attr("date-value").and_then(|v| {
                                    let (d, t) = v.split_once('T').unwrap_or((v, ""));
                                    let p: Vec<i32> = d.split('-').filter_map(|x| x.parse().ok()).collect();
                                    let base = date_serial(*p.first()?, *p.get(1)? as u32, *p.get(2)? as u32)?;
                                    let tp: Vec<f64> = t.split(':').filter_map(|x| x.parse().ok()).collect();
                                    Some(base + tp.first().unwrap_or(&0.0) / 24.0 + tp.get(1).unwrap_or(&0.0) / 1440.0 + tp.get(2).unwrap_or(&0.0) / 86400.0)
                                }).map(Value::Num).unwrap_or(Value::Str(text.clone())),
                                "time" => cell.attr("time-value").map(|v| {
                                    let v = v.trim_start_matches("PT");
                                    let h: f64 = v.split('H').next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
                                    let m: f64 = v.split('H').nth(1).and_then(|x| x.split('M').next()).and_then(|x| x.parse().ok()).unwrap_or(0.0);
                                    let s: f64 = v.split('M').nth(1).and_then(|x| x.trim_end_matches('S').parse().ok()).unwrap_or(0.0);
                                    Value::Num((h * 3600.0 + m * 60.0 + s) / 86400.0)
                                }).unwrap_or(Value::Empty),
                                "boolean" => Value::Bool(cell.attr("boolean-value") == Some("true")),
                                _ => {
                                    if text.is_empty() {
                                        Value::Empty
                                    } else {
                                        Value::Str(text.clone())
                                    }
                                }
                            };
                            let input = match cell.attr("formula") {
                                Some(f) => from_odf_formula(f),
                                None => match &value {
                                    Value::Num(n) => fmt_general(*n),
                                    Value::Str(s) => s.clone(),
                                    Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
                                    _ => String::new(),
                                },
                            };
                            let mut style = style;
                            if vt == "date" || vt == "percentage" || vt == "time" {
                                let mut st = wb.style(style).clone();
                                st.numfmt = match vt {
                                    "date" => "dd.mm.yyyy",
                                    "time" => "hh:mm:ss",
                                    _ => "0.00%",
                                }
                                .into();
                                style = wb.style_id(&st);
                            }
                            set_cell(&mut sh, r + k, c + kk, input, value, style);
                            let cs: u32 = cell.attr("number-columns-spanned").and_then(|v| v.parse().ok()).unwrap_or(1);
                            let rs: u32 = cell.attr("number-rows-spanned").and_then(|v| v.parse().ok()).unwrap_or(1);
                            if cs > 1 || rs > 1 {
                                sh.merges.push((r + k, c + kk, r + k + rs - 1, c + kk + cs - 1));
                            }
                        }
                        c += crep;
                    }
                }
            }
            r = r.saturating_add(rep);
        }
        wb.sheets.push(sh);
    }
    if wb.sheets.is_empty() {
        wb.sheets.push(Sheet::new("Sheet1"));
    }
    formula::recalc(&mut wb);
    Ok(wb)
}

fn odf_len(s: &str) -> Option<f32> {
    let s = s.trim();
    let (n, u): (String, String) = s.chars().partition(|c| c.is_ascii_digit() || *c == '.' || *c == '-');
    let v: f32 = n.parse().ok()?;
    Some(match u.as_str() {
        "in" => v * 72.0,
        "cm" => v * 72.0 / 2.54,
        "mm" => v * 72.0 / 25.4,
        "px" => v * 0.75,
        _ => v,
    })
}

pub fn write_ods(wb: &Workbook) -> Result<Vec<u8>, String> {
    let mut auto = String::new();
    for (i, st) in wb.styles.iter().enumerate() {
        auto.push_str(&format!(
            "<style:style style:name=\"ce{i}\" style:family=\"table-cell\"><style:table-cell-properties{}{}{}/><style:paragraph-properties fo:text-align=\"{}\"/><style:text-properties fo:font-weight=\"{}\" fo:font-style=\"{}\" fo:color=\"#{:02x}{:02x}{:02x}\" fo:font-size=\"{}pt\" style:font-name=\"{}\"{}/></style:style>",
            st.fill.map(|c| format!(" fo:background-color=\"#{:02x}{:02x}{:02x}\"", c[0], c[1], c[2])).unwrap_or_default(),
            if st.wrap { " fo:wrap-option=\"wrap\"" } else { "" },
            if st.border.iter().any(|b| *b) { " fo:border=\"0.06pt solid #000000\"" } else { "" },
            match st.halign {
                HAlign::Center => "center",
                HAlign::Right => "end",
                _ => "start",
            },
            if st.bold { "bold" } else { "normal" },
            if st.italic { "italic" } else { "normal" },
            st.color[0], st.color[1], st.color[2],
            st.size(),
            esc(&st.font),
            if st.underline { " style:text-underline-style=\"solid\"" } else { "" }
        ));
    }
    let mut body = String::new();
    for (si, sh) in wb.sheets.iter().enumerate() {
        body.push_str(&format!("<table:table table:name=\"{}\">", esc(&sh.name)));
        let (ur, uc) = sh.used().unwrap_or((0, 0));
        for c in 0..=uc {
            let w = sh.col_width(c);
            let name = format!("co{si}_{c}");
            auto.push_str(&format!("<style:style style:name=\"{name}\" style:family=\"table-column\"><style:table-column-properties style:column-width=\"{}pt\"/></style:style>", w * 0.75));
            body.push_str(&format!("<table:table-column table:style-name=\"{name}\"/>"));
        }
        for r in 0..=ur {
            let rs = if sh.row_h.contains_key(&r) {
                let name = format!("ro{si}_{r}");
                auto.push_str(&format!("<style:style style:name=\"{name}\" style:family=\"table-row\"><style:table-row-properties style:row-height=\"{}pt\" style:use-optimal-row-height=\"false\"/></style:style>", sh.row_h[&r] * 0.75));
                format!(" table:style-name=\"{name}\"")
            } else {
                String::new()
            };
            body.push_str(&format!("<table:table-row{rs}{}>", if sh.hidden_rows.contains(&r) { " table:visibility=\"collapse\"" } else { "" }));
            for c in 0..=uc {
                let merge = sh.merges.iter().find(|m| m.0 == r && m.1 == c);
                let covered = !merge.is_some() && sh.merges.iter().any(|m| r >= m.0 && r <= m.2 && c >= m.1 && c <= m.3);
                let tag = if covered { "table:covered-table-cell" } else { "table:table-cell" };
                let span = merge.map(|m| format!(" table:number-columns-spanned=\"{}\" table:number-rows-spanned=\"{}\"", m.3 - m.1 + 1, m.2 - m.0 + 1)).unwrap_or_default();
                match sh.cells.get(&(r, c)) {
                    None => body.push_str(&format!("<{tag}{span}/>")),
                    Some(cell) => {
                        let st = format!(" table:style-name=\"ce{}\"", cell.style);
                        let f = if cell.is_formula() { format!(" table:formula=\"{}\"", esc(&to_odf_formula(&cell.input))) } else { String::new() };
                        let (vt, text) = match &cell.value {
                            Value::Num(n) => (format!(" office:value-type=\"float\" office:value=\"{n}\""), crate::numfmt::format(&cell.value, &wb.style(cell.style).numfmt).0),
                            Value::Str(s) => (" office:value-type=\"string\"".to_string(), s.clone()),
                            Value::Bool(b) => (format!(" office:value-type=\"boolean\" office:boolean-value=\"{b}\""), if *b { "TRUE" } else { "FALSE" }.into()),
                            Value::Err(e) => (" office:value-type=\"string\"".to_string(), e.clone()),
                            Value::Empty => (String::new(), String::new()),
                        };
                        let p = if text.is_empty() { String::new() } else { text.split('\n').map(|l| format!("<text:p>{}</text:p>", esc(l))).collect() };
                        body.push_str(&format!("<{tag}{st}{f}{vt}{span}>{p}</{tag}>"));
                    }
                }
            }
            body.push_str("</table:table-row>");
        }
        body.push_str("</table:table>");
    }
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2" office:version="1.3""#;
    let content = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ns}><office:automatic-styles>{auto}</office:automatic-styles><office:body><office:spreadsheet>{body}</office:spreadsheet></office:body></office:document-content>"#);
    let mut w = PackageWriter::new();
    w.stored("mimetype", b"application/vnd.oasis.opendocument.spreadsheet")?;
    w.add_str("content.xml", &content)?;
    w.add_str("styles.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles {ns}><office:styles/></office:document-styles>"#))?;
    w.add_str("META-INF/manifest.xml", r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/></manifest:manifest>"#)?;
    w.finish()
}

// ───────────────────────── CSV ─────────────────────────

pub fn detect_delimiter(text: &str) -> char {
    let lines: Vec<&str> = text.lines().take(20).collect();
    let mut best = (',', 0.0f32);
    for d in [',', ';', '\t', '|'] {
        let counts: Vec<usize> = lines.iter().map(|l| split_csv_line(l, d).len()).collect();
        if counts.is_empty() {
            continue;
        }
        let first = counts[0];
        if first < 2 {
            continue;
        }
        let consistent = counts.iter().filter(|c| **c == first).count() as f32 / counts.len() as f32;
        let score = consistent * first as f32;
        if score > best.1 {
            best = (d, score);
        }
    }
    best.0
}

fn split_csv_line(line: &str, d: char) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut q = false;
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        if q {
            if c == '"' {
                if it.peek() == Some(&'"') {
                    cur.push('"');
                    it.next();
                } else {
                    q = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            q = true;
        } else if c == d {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// Parses CSV text with quoted fields that may contain newlines.
pub fn parse_csv(text: &str, d: char) -> Vec<Vec<String>> {
    let mut rows = vec![];
    let mut row = vec![];
    let mut cur = String::new();
    let mut q = false;
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if q {
            if c == '"' {
                if it.peek() == Some(&'"') {
                    cur.push('"');
                    it.next();
                } else {
                    q = false;
                }
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' if cur.is_empty() => q = true,
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cur));
                rows.push(std::mem::take(&mut row));
            }
            c if c == d => row.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    if !cur.is_empty() || !row.is_empty() {
        row.push(cur);
        rows.push(row);
    }
    rows
}

pub fn read_csv(bytes: &[u8], name: &str) -> (Workbook, char, &'static encoding_rs::Encoding) {
    let (text, enc) = kit42::ooxml::decode_text(bytes);
    let d = if name.to_lowercase().ends_with(".tsv") || name.to_lowercase().ends_with(".tab") { '\t' } else { detect_delimiter(&text) };
    let mut wb = Workbook { sheets: vec![Sheet::new(name.rsplit(['\\', '/']).next().unwrap_or("Sheet1").rsplit_once('.').map(|x| x.0).unwrap_or("Sheet1"))], ..Workbook::default() };
    let date_style = {
        let st = Style { numfmt: "dd.mm.yyyy".into(), ..Style::default() };
        wb.style_id(&st)
    };
    let pct_style = {
        let st = Style { numfmt: "0%".into(), ..Style::default() };
        wb.style_id(&st)
    };
    for (r, row) in parse_csv(&text, d).into_iter().enumerate() {
        for (c, field) in row.into_iter().enumerate() {
            if field.is_empty() {
                continue;
            }
            // Equals-prefixed fields stay literal text (never executed as formulas).
            if field.starts_with('=') {
                set_cell(&mut wb.sheets[0], r as u32, c as u32, format!("'{field}"), Value::Str(field), 0);
                continue;
            }
            let (v, fmt) = parse_input(&field);
            let style = match fmt {
                Some(f) if f.contains('d') || f.contains('y') => date_style,
                Some("0%") => pct_style,
                _ => 0,
            };
            set_cell(&mut wb.sheets[0], r as u32, c as u32, field, v, style);
        }
    }
    (wb, d, enc)
}

pub fn write_csv(wb: &Workbook, sheet: usize, d: char) -> String {
    let sh = &wb.sheets[sheet];
    let Some((ur, uc)) = sh.used() else { return String::new() };
    let mut out = String::new();
    for r in 0..=ur {
        let fields: Vec<String> = (0..=uc)
            .map(|c| {
                let v = sh.value(r, c);
                let st = sh.cells.get(&(r, c)).map(|cell| wb.style(cell.style).numfmt.clone()).unwrap_or_default();
                let text = match &v {
                    Value::Num(_) if st.contains('d') || st.contains('y') || st.contains('h') => crate::numfmt::format(&v, &st).0,
                    Value::Num(n) => fmt_general(*n),
                    _ => v.as_text(),
                };
                if text.contains(d) || text.contains('"') || text.contains('\n') || text.starts_with(' ') {
                    format!("\"{}\"", text.replace('"', "\"\""))
                } else {
                    text
                }
            })
            .collect();
        out.push_str(&fields.join(&d.to_string()));
        out.push_str("\r\n");
    }
    out
}

// ───────────────────────── XLS (97–2003) ─────────────────────────

pub fn read_xls(bytes: &[u8]) -> Result<Workbook, String> {
    use calamine::{Data, Reader};
    let mut x = calamine::Xls::new(std::io::Cursor::new(bytes.to_vec())).map_err(|e| e.to_string())?;
    let mut wb = Workbook { sheets: vec![], ..Workbook::default() };
    let date_style = {
        let st = Style { numfmt: "dd.mm.yyyy".into(), ..Style::default() };
        wb.style_id(&st)
    };
    for name in x.sheet_names() {
        let mut sh = Sheet::new(&name);
        if let Ok(range) = x.worksheet_range(&name) {
            let (r0, c0) = range.start().unwrap_or((0, 0));
            for (r, c, v) in range.cells() {
                let (r, c) = (r as u32 + r0, c as u32 + c0);
                let (value, style) = match v {
                    Data::Int(i) => (Value::Num(*i as f64), 0),
                    Data::Float(f) => (Value::Num(*f), 0),
                    Data::String(s) => (Value::Str(s.clone()), 0),
                    Data::Bool(b) => (Value::Bool(*b), 0),
                    Data::DateTime(d) => (Value::Num(d.as_f64()), date_style),
                    Data::DateTimeIso(s) | Data::DurationIso(s) => (Value::Str(s.clone()), 0),
                    Data::Error(e) => (Value::Err(format!("#{e:?}").to_uppercase()), 0),
                    Data::Empty => continue,
                };
                let input = match &value {
                    Value::Num(n) => fmt_general(*n),
                    other => other.as_text(),
                };
                set_cell(&mut sh, r, c, input, value, style);
            }
        }
        if let Ok(fr) = x.worksheet_formula(&name) {
            let (r0, c0) = fr.start().unwrap_or((0, 0));
            for (r, c, f) in fr.cells() {
                if f.is_empty() {
                    continue;
                }
                let (r, c) = (r as u32 + r0, c as u32 + c0);
                let cached = sh.cells.get(&(r, c)).map(|x| (x.value.clone(), x.style)).unwrap_or((Value::Empty, 0));
                // Undecodable formulas keep their cached value.
                let input = format!("={f}");
                if formula::parse(&input).is_ok() {
                    set_cell(&mut sh, r, c, input, cached.0, cached.1);
                }
            }
        }
        wb.sheets.push(sh);
    }
    if wb.sheets.is_empty() {
        wb.sheets.push(Sheet::new("Sheet1"));
    }
    formula::recalc(&mut wb);
    Ok(wb)
}
