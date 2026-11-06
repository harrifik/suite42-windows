//! OpenDocument Text, RTF, HTML, Markdown and plain text for DOC42.

use crate::model::*;
use kit42::ooxml::{esc, parse, NodeExt, Package, PackageWriter};
use kit42::richtext::{Align2, CharFmt, ListInfo, ListKind, Para, ParaStyle, Run};
use std::collections::HashMap;
use std::path::Path;

// ───────────────────────── units ─────────────────────────

pub fn length_pt(s: &str) -> Option<f32> {
    let s = s.trim();
    let (num, unit): (String, String) = s.chars().partition(|c| c.is_ascii_digit() || *c == '.' || *c == '-');
    let v: f32 = num.parse().ok()?;
    Some(match unit.as_str() {
        "pt" | "" => v,
        "in" => v * 72.0,
        "cm" => v * 72.0 / 2.54,
        "mm" => v * 72.0 / 25.4,
        "pc" => v * 12.0,
        "px" => v * 0.75,
        "em" => v * 12.0,
        _ => v,
    })
}

fn parse_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() == 6 {
        let v = u32::from_str_radix(s, 16).ok()?;
        return Some([(v >> 16) as u8, (v >> 8) as u8, v as u8]);
    }
    if s.len() == 3 {
        let v: Vec<u8> = s.chars().filter_map(|c| c.to_digit(16)).map(|d| (d * 17) as u8).collect();
        if v.len() == 3 {
            return Some([v[0], v[1], v[2]]);
        }
    }
    match s.to_ascii_lowercase().as_str() {
        "black" => Some([0, 0, 0]),
        "white" => Some([255, 255, 255]),
        "red" => Some([255, 0, 0]),
        "green" => Some([0, 128, 0]),
        "blue" => Some([0, 0, 255]),
        "yellow" => Some([255, 255, 0]),
        "gray" | "grey" => Some([128, 128, 128]),
        _ => None,
    }
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

// ───────────────────────── ODT ─────────────────────────

#[derive(Default, Clone)]
struct OdtStyle {
    parent: Option<String>,
    r: Vec<(String, String)>,
    p: Vec<(String, String)>,
}

fn odt_text_props(f: &mut CharFmt, props: &[(String, String)]) {
    for (k, v) in props {
        match k.as_str() {
            "font-weight" => f.bold = v == "bold" || v.parse::<u32>().map(|w| w >= 600).unwrap_or(false),
            "font-style" => f.italic = v == "italic" || v == "oblique",
            "text-underline-style" => f.underline = v != "none",
            "text-line-through-style" => f.strike = v != "none",
            "font-size" => {
                if let Some(pt) = length_pt(v) {
                    f.size = pt;
                }
            }
            "color" => {
                if let Some(c) = parse_color(v) {
                    f.color = c;
                }
            }
            "font-name" | "font-family" => f.font = v.trim_matches('\'').to_string(),
            "background-color" => f.highlight = if v == "transparent" { None } else { parse_color(v) },
            "text-position" => f.script = if v.starts_with("super") || v.starts_with("33%") { 1 } else if v.starts_with("sub") || v.starts_with("-33%") { -1 } else { 0 },
            _ => {}
        }
    }
}

fn odt_para_props(p: &mut Para, props: &[(String, String)]) {
    for (k, v) in props {
        match k.as_str() {
            "text-align" => {
                p.align = match v.as_str() {
                    "center" => Align2::Center,
                    "end" | "right" => Align2::Right,
                    "justify" => Align2::Justify,
                    _ => Align2::Left,
                }
            }
            "margin-left" => p.indent_left = length_pt(v).unwrap_or(0.0),
            "margin-right" => p.indent_right = length_pt(v).unwrap_or(0.0),
            "text-indent" => p.indent_first = length_pt(v).unwrap_or(0.0),
            "margin-top" => p.space_before = length_pt(v).unwrap_or(0.0),
            "margin-bottom" => p.space_after = length_pt(v).unwrap_or(0.0),
            "line-height" if v.ends_with('%') => p.line_spacing = v.trim_end_matches('%').parse::<f32>().unwrap_or(100.0) / 100.0,
            "break-before" => p.page_break_before = v == "page",
            "background-color" if v != "transparent" => p.shading = parse_color(v),
            _ => {}
        }
    }
}

fn collect_styles(node: roxmltree::Node, map: &mut HashMap<String, OdtStyle>) {
    for s in node.descendants().filter(|n| n.is("style")) {
        let name = s.attr("name").unwrap_or("").to_string();
        let mut st = OdtStyle { parent: s.attr("parent-style-name").map(|v| v.to_string()), ..Default::default() };
        if let Some(tp) = s.child("text-properties") {
            st.r = tp.attributes().map(|a| (a.name().to_string(), a.value().to_string())).collect();
        }
        if let Some(pp) = s.child("paragraph-properties") {
            st.p = pp.attributes().map(|a| (a.name().to_string(), a.value().to_string())).collect();
        }
        map.insert(name, st);
    }
}

fn style_props(map: &HashMap<String, OdtStyle>, name: &str) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let mut chain = vec![];
    let mut cur = Some(name.to_string());
    let mut guard = 0;
    while let Some(c) = cur {
        guard += 1;
        if guard > 16 {
            break;
        }
        match map.get(&c) {
            Some(s) => {
                chain.push(s.clone());
                cur = s.parent.clone();
            }
            None => break,
        }
    }
    let mut r = vec![];
    let mut p = vec![];
    for s in chain.iter().rev() {
        r.extend(s.r.clone());
        p.extend(s.p.clone());
    }
    (r, p)
}

pub fn read_odt(bytes: &[u8]) -> Result<Document, String> {
    let pkg = Package::read(bytes)?;
    let content = pkg.text("content.xml").ok_or("content.xml is missing")?;
    let doc = parse(&content)?;
    let mut styles = HashMap::new();
    let styles_xml = pkg.text("styles.xml").unwrap_or_default();
    let sd = parse(&styles_xml).ok();
    if let Some(sd) = &sd {
        collect_styles(sd.root_element(), &mut styles);
    }
    collect_styles(doc.root_element(), &mut styles);
    let mut out = Document::default();
    out.blocks.clear();
    // Page layout.
    if let Some(sd) = &sd {
        if let Some(pl) = sd.root_element().descendant("page-layout-properties") {
            let g = |k: &str| pl.attr(k).and_then(length_pt);
            if let (Some(w), Some(h)) = (g("page-width"), g("page-height")) {
                out.setup.width = w;
                out.setup.height = h;
            }
            out.setup.margins = [g("margin-left").unwrap_or(72.0), g("margin-top").unwrap_or(72.0), g("margin-right").unwrap_or(72.0), g("margin-bottom").unwrap_or(72.0)];
        }
        // Default paragraph text properties.
        if let Some(def) = sd.root_element().descendants().find(|n| n.is("default-style") && n.attr("family") == Some("paragraph")) {
            if let Some(tp) = def.child("text-properties") {
                let props: Vec<(String, String)> = tp.attributes().map(|a| (a.name().to_string(), a.value().to_string())).collect();
                odt_text_props(&mut out.body, &props);
            }
        }
    }
    if let Some(m) = pkg.text("meta.xml").and_then(|x| parse(&x).ok().map(|d| (d.root_element().descendant("title").map(|n| n.text_all()), d.root_element().descendant("initial-creator").map(|n| n.text_all())))) {
        out.meta.title = m.0.unwrap_or_default();
        out.meta.author = m.1.unwrap_or_default();
    }
    let text = doc.root_element().descendant("text").ok_or("office:text is missing")?;
    odt_blocks(&pkg, &styles, text, &mut out, None);
    out.normalize();
    Ok(out)
}

fn odt_blocks(pkg: &Package, styles: &HashMap<String, OdtStyle>, node: roxmltree::Node, out: &mut Document, list: Option<(ListKind, u8)>) {
    for n in node.children().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "p" | "h" => {
                let sname = n.attr("style-name").unwrap_or("");
                let (rp, pp) = style_props(styles, sname);
                let mut base = out.body.clone();
                odt_text_props(&mut base, &rp);
                let mut p = Para::new("", base.clone());
                p.runs.clear();
                p.space_after = 6.0;
                odt_para_props(&mut p, &pp);
                if n.is("h") {
                    let lvl: u8 = n.attr("outline-level").and_then(|v| v.parse().ok()).unwrap_or(1);
                    p.style = ParaStyle::Heading(lvl.clamp(1, 6));
                } else {
                    let lower = sname.to_lowercase();
                    let parent = styles.get(sname).and_then(|s| s.parent.clone()).unwrap_or_default().to_lowercase();
                    for s in [&lower, &parent] {
                        if s == "title" {
                            p.style = ParaStyle::Title;
                        } else if s.contains("quotation") || s == "quote" {
                            p.style = ParaStyle::Quote;
                        } else if s.contains("preformatted") {
                            p.style = ParaStyle::Code;
                        }
                    }
                }
                if let Some((k, l)) = list {
                    p.list = Some(ListInfo { kind: k, level: l, checked: false });
                }
                let mut pics = vec![];
                odt_spans(pkg, styles, n, &base, None, &mut p.runs, &mut pics);
                if p.runs.is_empty() {
                    p.runs.push(Run { text: String::new(), fmt: base });
                }
                p.normalize();
                let only_pic = p.is_empty() && !pics.is_empty();
                if !only_pic {
                    out.blocks.push(Block::Para(p));
                }
                out.blocks.extend(pics.into_iter().map(Block::Picture));
            }
            "list" => {
                let sname = n.attr("style-name").unwrap_or("");
                let kind = list.map(|l| l.0).unwrap_or_else(|| {
                    if styles.get(sname).is_some() || sname.to_lowercase().contains("num") {
                        if sname.to_lowercase().contains("bullet") {
                            ListKind::Bullet
                        } else {
                            ListKind::Number
                        }
                    } else {
                        ListKind::Bullet
                    }
                });
                // Number lists declare text:list-level-style-number in their list style.
                let level = list.map(|l| l.1 + 1).unwrap_or(0);
                for item in n.children().filter(|c| c.is("list-item") || c.is("list-header")) {
                    odt_blocks(pkg, styles, item, out, Some((kind, level)));
                }
            }
            "table" => {
                let mut rows = vec![];
                for row in n.descendants().filter(|r| r.is("table-row")) {
                    let mut cells = vec![];
                    for c in row.children().filter(|c| c.is("table-cell") || c.is("covered-table-cell")) {
                        let mut tmp = Document { blocks: vec![], ..Document::default() };
                        tmp.body = out.body.clone();
                        odt_blocks(pkg, styles, c, &mut tmp, None);
                        let mut para: Option<Para> = None;
                        for b in tmp.blocks {
                            if let Block::Para(p) = b {
                                match &mut para {
                                    None => para = Some(p),
                                    Some(acc) => {
                                        let n2 = acc.char_len();
                                        let f = acc.fmt_at(n2);
                                        acc.splice(n2, 0, "\n", &f);
                                        acc.append(p);
                                    }
                                }
                            }
                        }
                        let mut para = para.unwrap_or_else(|| Para::new("", out.body.clone()));
                        para.space_after = 0.0;
                        let rep: usize = c.attr("number-columns-repeated").and_then(|v| v.parse().ok()).unwrap_or(1).min(64);
                        for _ in 0..rep {
                            cells.push(Cell { para: para.clone(), shading: None });
                        }
                    }
                    rows.push(cells);
                }
                let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                if cols > 0 {
                    for r in &mut rows {
                        while r.len() < cols {
                            r.push(Cell::new("", &out.body));
                        }
                    }
                    let w = out.setup.text_width();
                    out.blocks.push(Block::Table(Table { rows, widths: vec![w / cols as f32; cols], borders: true, header: false }));
                }
            }
            "section" | "index-body" | "table-of-content" => odt_blocks(pkg, styles, n, out, list),
            "soft-page-break" => {}
            _ => {}
        }
    }
}

fn odt_spans(pkg: &Package, styles: &HashMap<String, OdtStyle>, node: roxmltree::Node, base: &CharFmt, link: Option<String>, runs: &mut Vec<Run>, pics: &mut Vec<Picture>) {
    for c in node.children() {
        if c.is_text() {
            let t = c.text().unwrap_or("").replace('\n', " ");
            if !t.is_empty() {
                let mut f = base.clone();
                f.link = link.clone();
                runs.push(Run { text: t, fmt: f });
            }
            continue;
        }
        if !c.is_element() {
            continue;
        }
        let mut f = base.clone();
        f.link = link.clone();
        match c.tag_name().name() {
            "span" => {
                let (rp, _) = style_props(styles, c.attr("style-name").unwrap_or(""));
                odt_text_props(&mut f, &rp);
                odt_spans(pkg, styles, c, &f, link.clone(), runs, pics);
            }
            "a" => {
                let href = c.attr("href").map(|s| s.to_string());
                odt_spans(pkg, styles, c, base, href, runs, pics);
            }
            "s" => {
                let n: usize = c.attr("c").and_then(|v| v.parse().ok()).unwrap_or(1).min(1000);
                runs.push(Run { text: " ".repeat(n), fmt: f });
            }
            "tab" => runs.push(Run { text: "\t".into(), fmt: f }),
            "line-break" => runs.push(Run { text: "\n".into(), fmt: f }),
            "frame" => {
                if let Some(img) = c.descendant("image") {
                    if let Some(data) = img.attr("href").and_then(|h| pkg.get(h.trim_start_matches("./"))) {
                        if let Some(mut pic) = Picture::new(data.clone(), 10000.0) {
                            if let (Some(w), Some(h)) = (c.attr("width").and_then(length_pt), c.attr("height").and_then(length_pt)) {
                                pic.width = w;
                                pic.height = h;
                            }
                            pics.push(pic);
                        }
                    }
                }
            }
            "note" | "annotation" | "bookmark" | "bookmark-start" | "bookmark-end" => {}
            _ => odt_spans(pkg, styles, c, &f, link.clone(), runs, pics),
        }
    }
}

pub fn write_odt(doc: &Document) -> Result<Vec<u8>, String> {
    let mut auto = String::new();
    let mut tstyles: Vec<CharFmt> = vec![];
    let mut pstyles: Vec<String> = vec![];
    let mut tname = |f: &CharFmt, auto: &mut String| -> String {
        if let Some(i) = tstyles.iter().position(|x| x == f) {
            return format!("T{}", i + 1);
        }
        tstyles.push(f.clone());
        let n = format!("T{}", tstyles.len());
        auto.push_str(&format!(
            "<style:style style:name=\"{n}\" style:family=\"text\"><style:text-properties style:font-name=\"{font}\" fo:font-family=\"'{font}'\" fo:font-size=\"{size}pt\" fo:font-weight=\"{w}\" fo:font-style=\"{st}\" style:text-underline-style=\"{u}\" style:text-line-through-style=\"{s}\" fo:color=\"{c}\"{bg}{pos}/></style:style>",
            font = esc(&f.font),
            size = f.size,
            w = if f.bold { "bold" } else { "normal" },
            st = if f.italic { "italic" } else { "normal" },
            u = if f.underline { "solid" } else { "none" },
            s = if f.strike { "solid" } else { "none" },
            c = hex(f.color),
            bg = f.highlight.map(|h| format!(" fo:background-color=\"{}\"", hex(h))).unwrap_or_default(),
            pos = match f.script {
                1 => " style:text-position=\"super 58%\"",
                -1 => " style:text-position=\"sub 58%\"",
                _ => "",
            }
        ));
        n
    };
    let mut pname = |p: &Para, auto: &mut String| -> String {
        let props = format!(
            "<style:paragraph-properties fo:text-align=\"{}\" fo:margin-left=\"{}pt\" fo:margin-right=\"{}pt\" fo:text-indent=\"{}pt\" fo:margin-top=\"{}pt\" fo:margin-bottom=\"{}pt\" fo:line-height=\"{}%\"{}{}/>",
            match p.align {
                Align2::Left => "start",
                Align2::Center => "center",
                Align2::Right => "end",
                Align2::Justify => "justify",
            },
            p.indent_left, p.indent_right, p.indent_first, p.space_before, p.space_after, (p.line_spacing * 100.0).round(),
            if p.page_break_before { " fo:break-before=\"page\"" } else { "" },
            p.shading.map(|c| format!(" fo:background-color=\"{}\"", hex(c))).unwrap_or_default()
        );
        let parent = match p.style {
            ParaStyle::Heading(n) => format!("Heading_20_{n}"),
            ParaStyle::Title => "Title".into(),
            ParaStyle::Quote => "Quotations".into(),
            ParaStyle::Code => "Preformatted_20_Text".into(),
            _ => "Standard".into(),
        };
        let key = format!("{parent}|{props}");
        if let Some(i) = pstyles.iter().position(|x| *x == key) {
            return format!("P{}", i + 1);
        }
        pstyles.push(key);
        let n = format!("P{}", pstyles.len());
        auto.push_str(&format!("<style:style style:name=\"{n}\" style:family=\"paragraph\" style:parent-style-name=\"{parent}\">{props}</style:style>"));
        n
    };
    let mut body = String::new();
    let mut media = vec![];
    let para_xml = |p: &Para, auto: &mut String, tname: &mut dyn FnMut(&CharFmt, &mut String) -> String, pname: &mut dyn FnMut(&Para, &mut String) -> String| -> String {
        let ps = pname(p, auto);
        let mut inner = String::new();
        for r in &p.runs {
            if r.text.is_empty() {
                continue;
            }
            let ts = tname(&r.fmt, auto);
            let mut t = String::new();
            for ch in r.text.chars() {
                match ch {
                    '\t' => t.push_str("<text:tab/>"),
                    '\n' => t.push_str("<text:line-break/>"),
                    c => t.push_str(&esc(&c.to_string())),
                }
            }
            let t = t.replace("  ", " <text:s/>");
            let span = format!("<text:span text:style-name=\"{ts}\">{t}</text:span>");
            match &r.fmt.link {
                Some(l) => inner.push_str(&format!("<text:a xlink:type=\"simple\" xlink:href=\"{}\">{span}</text:a>", esc(l))),
                None => inner.push_str(&span),
            }
        }
        match p.style {
            ParaStyle::Heading(n) => format!("<text:h text:style-name=\"{ps}\" text:outline-level=\"{n}\">{inner}</text:h>"),
            _ => format!("<text:p text:style-name=\"{ps}\">{inner}</text:p>"),
        }
    };
    let mut i = 0;
    let blocks = &doc.blocks;
    while i < blocks.len() {
        match &blocks[i] {
            Block::Para(p) if p.list.is_some() => {
                // Consecutive list paragraphs become one list (levels nest).
                let numbered = !matches!(p.list.as_ref().unwrap().kind, ListKind::Bullet | ListKind::Check);
                body.push_str(&format!("<text:list text:style-name=\"{}\">", if numbered { "LNum" } else { "LBul" }));
                while i < blocks.len() {
                    let Block::Para(q) = &blocks[i] else { break };
                    let Some(l) = &q.list else { break };
                    let mut x = para_xml(q, &mut auto, &mut tname, &mut pname);
                    for _ in 0..l.level {
                        x = format!("<text:list><text:list-item>{x}</text:list-item></text:list>");
                    }
                    body.push_str(&format!("<text:list-item>{x}</text:list-item>"));
                    i += 1;
                }
                body.push_str("</text:list>");
                continue;
            }
            Block::Para(p) => body.push_str(&para_xml(p, &mut auto, &mut tname, &mut pname)),
            Block::PageBreak => {
                let mut p = Para::new("", doc.body.clone());
                p.page_break_before = true;
                if let Some(Block::Para(next)) = blocks.get(i + 1) {
                    let mut n = next.clone();
                    n.page_break_before = true;
                    body.push_str(&para_xml(&n, &mut auto, &mut tname, &mut pname));
                    i += 2;
                    continue;
                }
                body.push_str(&para_xml(&p, &mut auto, &mut tname, &mut pname));
            }
            Block::Table(t) => {
                body.push_str(&format!("<table:table table:name=\"Table{i}\" table:style-name=\"Tbl\">"));
                for w in &t.widths {
                    let cs = format!("Col{}", (*w * 10.0) as i64);
                    if !auto.contains(&format!("\"{cs}\"")) {
                        auto.push_str(&format!("<style:style style:name=\"{cs}\" style:family=\"table-column\"><style:table-column-properties style:column-width=\"{w}pt\"/></style:style>"));
                    }
                    body.push_str(&format!("<table:table-column table:style-name=\"{cs}\"/>"));
                }
                for row in &t.rows {
                    body.push_str("<table:table-row>");
                    for c in row {
                        body.push_str(&format!("<table:table-cell table:style-name=\"{}\" office:value-type=\"string\">", if t.borders { "CellB" } else { "Cell" }));
                        body.push_str(&para_xml(&c.para, &mut auto, &mut tname, &mut pname));
                        body.push_str("</table:table-cell>");
                    }
                    body.push_str("</table:table-row>");
                }
                body.push_str("</table:table>");
            }
            Block::Picture(pic) => {
                let (mime, ext) = kit42::ooxml::image_mime(&pic.data);
                let name = format!("Pictures/img{}.{}", media.len() + 1, ext);
                media.push((name.clone(), mime, pic.data.clone()));
                let align = match pic.align {
                    Align2::Center => "PCenter",
                    Align2::Right => "PRight",
                    _ => "Standard",
                };
                body.push_str(&format!(
                    "<text:p text:style-name=\"{align}\"><draw:frame draw:name=\"Image{n}\" text:anchor-type=\"as-char\" svg:width=\"{w}pt\" svg:height=\"{h}pt\"><draw:image xlink:href=\"{name}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/></draw:frame></text:p>",
                    n = media.len(), w = pic.width, h = pic.height
                ));
            }
        }
        i += 1;
    }
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" office:version="1.3""#;
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ns}><office:automatic-styles><style:style style:name="Tbl" style:family="table"><style:table-properties table:align="left"/></style:style><style:style style:name="CellB" style:family="table-cell"><style:table-cell-properties fo:padding="0.05in" fo:border="0.5pt solid #808080"/></style:style><style:style style:name="Cell" style:family="table-cell"><style:table-cell-properties fo:padding="0.05in" fo:border="none"/></style:style><style:style style:name="PCenter" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:text-align="center"/></style:style><style:style style:name="PRight" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:text-align="end"/></style:style><text:list-style style:name="LBul"><text:list-level-style-bullet text:level="1" text:bullet-char="•"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" fo:text-indent="-0.25in" fo:margin-left="0.25in"/></style:list-level-properties></text:list-level-style-bullet><text:list-level-style-bullet text:level="2" text:bullet-char="◦"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" fo:text-indent="-0.25in" fo:margin-left="0.5in"/></style:list-level-properties></text:list-level-style-bullet></text:list-style><text:list-style style:name="LNum"><text:list-level-style-number text:level="1" style:num-suffix="." style:num-format="1"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" fo:text-indent="-0.25in" fo:margin-left="0.25in"/></style:list-level-properties></text:list-level-style-number><text:list-level-style-number text:level="2" style:num-suffix="." style:num-format="a"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" fo:text-indent="-0.25in" fo:margin-left="0.5in"/></style:list-level-properties></text:list-level-style-number></text:list-style>{auto}</office:automatic-styles><office:body><office:text>{body}</office:text></office:body></office:document-content>"#
    );
    let s = &doc.setup;
    let mut named = String::new();
    for st in ParaStyle::ALL {
        let f = st.base_fmt(&doc.body);
        let name = match st {
            ParaStyle::Heading(n) => format!("Heading_20_{n}"),
            ParaStyle::Title => "Title".into(),
            ParaStyle::Quote => "Quotations".into(),
            ParaStyle::Code => "Preformatted_20_Text".into(),
            ParaStyle::Body => "Standard".into(),
            ParaStyle::Subtitle => "Subtitle".into(),
            ParaStyle::Caption => "Caption".into(),
        };
        let display = name.replace("_20_", " ");
        named.push_str(&format!(
            "<style:style style:name=\"{name}\" style:display-name=\"{display}\" style:family=\"paragraph\"{}><style:text-properties style:font-name=\"{font}\" fo:font-family=\"'{font}'\" fo:font-size=\"{size}pt\" fo:font-weight=\"{w}\" fo:color=\"{c}\"/></style:style>",
            if matches!(st, ParaStyle::Heading(_)) { format!(" style:default-outline-level=\"{}\"", match st { ParaStyle::Heading(n) => n, _ => 1 }) } else { String::new() },
            font = esc(&f.font), size = f.size, w = if f.bold { "bold" } else { "normal" }, c = hex(f.color)
        ));
    }
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles {ns}><office:styles><style:default-style style:family="paragraph"><style:text-properties style:font-name="{font}" fo:font-family="'{font}'" fo:font-size="{size}pt"/></style:default-style>{named}</office:styles><office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="{w}pt" fo:page-height="{h}pt" fo:margin-left="{ml}pt" fo:margin-top="{mt}pt" fo:margin-right="{mr}pt" fo:margin-bottom="{mb}pt" style:print-orientation="{orient}"/></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1">{hdr}{ftr}</style:master-page></office:master-styles></office:document-styles>"#,
        font = esc(&doc.body.font), size = doc.body.size, w = s.width, h = s.height, ml = s.margins[0], mt = s.margins[1], mr = s.margins[2], mb = s.margins[3],
        orient = if s.width > s.height { "landscape" } else { "portrait" },
        hdr = if s.header.is_empty() { String::new() } else { format!("<style:header><text:p>{}</text:p></style:header>", odt_fields(&s.header)) },
        ftr = if s.footer.is_empty() { String::new() } else { format!("<style:footer><text:p>{}</text:p></style:footer>", odt_fields(&s.footer)) },
    );
    let meta = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-meta {ns}><office:meta><meta:generator>DOC42 for Windows</meta:generator><dc:title>{}</dc:title><meta:initial-creator>{}</meta:initial-creator></office:meta></office:document-meta>"#,
        esc(&doc.meta.title),
        esc(&doc.meta.author)
    );
    let mut manifest = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.text"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/>"#);
    for (name, mime, _) in &media {
        manifest.push_str(&format!("<manifest:file-entry manifest:full-path=\"{name}\" manifest:media-type=\"{mime}\"/>"));
    }
    manifest.push_str("</manifest:manifest>");
    let mut w = PackageWriter::new();
    w.stored("mimetype", b"application/vnd.oasis.opendocument.text")?;
    w.add_str("content.xml", &content)?;
    w.add_str("styles.xml", &styles)?;
    w.add_str("meta.xml", &meta)?;
    w.add_str("META-INF/manifest.xml", &manifest)?;
    for (name, _, data) in &media {
        w.add(name, data)?;
    }
    w.finish()
}

fn odt_fields(s: &str) -> String {
    esc(s).replace("{page}", "<text:page-number text:select-page=\"current\">1</text:page-number>").replace("{pages}", "<text:page-count>1</text:page-count>")
}

// ───────────────────────── RTF ─────────────────────────

pub fn read_rtf(bytes: &[u8]) -> Result<Document, String> {
    let src: Vec<u8> = bytes.to_vec();
    let mut out = Document::default();
    out.blocks.clear();
    #[derive(Clone)]
    struct St {
        fmt: CharFmt,
        skip: bool,
        uc: usize,
        font_idx: i32,
    }
    let mut fonts: HashMap<i32, String> = HashMap::new();
    let mut colors: Vec<[u8; 3]> = vec![];
    let mut stack: Vec<St> = vec![];
    let mut st = St { fmt: out.body.clone(), skip: false, uc: 1, font_idx: 0 };
    let mut para = Para::new("", out.body.clone());
    para.runs.clear();
    para.space_after = 6.0;
    let mut codepage = encoding_rs::WINDOWS_1252;
    let mut i = 0;
    let mut dest: Vec<String> = vec![];
    let mut fonttbl_name = String::new();
    let mut fonttbl_idx = 0;
    let mut color_cur = [0u8; 3];
    let mut pict: Option<(String, Vec<u8>, f32, f32)> = None;
    let mut pending_bytes: Vec<u8> = vec![];
    let mut skip_chars = 0usize;
    let mut in_table = false;
    let mut row: Vec<Cell> = vec![];
    let mut rows: Vec<Vec<Cell>> = vec![];
    let flush_bytes = |pending: &mut Vec<u8>, para: &mut Para, fmt: &CharFmt, cp: &'static encoding_rs::Encoding| {
        if !pending.is_empty() {
            let (s, _, _) = cp.decode(pending);
            para.runs.push(Run { text: s.into_owned(), fmt: fmt.clone() });
            pending.clear();
        }
    };
    let push_text = |para: &mut Para, s: &str, fmt: &CharFmt| {
        if let Some(r) = para.runs.last_mut() {
            if r.fmt == *fmt {
                r.text.push_str(s);
                return;
            }
        }
        para.runs.push(Run { text: s.to_string(), fmt: fmt.clone() });
    };
    while i < src.len() {
        let c = src[i];
        match c {
            b'{' => {
                flush_bytes(&mut pending_bytes, &mut para, &st.fmt, codepage);
                stack.push(st.clone());
                dest.push(dest.last().cloned().unwrap_or_default());
                i += 1;
            }
            b'}' => {
                flush_bytes(&mut pending_bytes, &mut para, &st.fmt, codepage);
                let d = dest.pop().unwrap_or_default();
                if d == "fonttbl_entry" || (d == "fonttbl" && !fonttbl_name.is_empty()) {
                    let name = fonttbl_name.trim().trim_end_matches(';').to_string();
                    if !name.is_empty() {
                        fonts.insert(fonttbl_idx, name);
                    }
                    fonttbl_name.clear();
                }
                if d == "pict" {
                    if let Some((_, hexdata, w, h)) = pict.take() {
                        let data: Vec<u8> = hexdata.chunks(2).filter_map(|c| std::str::from_utf8(c).ok().and_then(|h| u8::from_str_radix(h, 16).ok())).collect();
                        if let Some(mut p) = Picture::new(data, 450.0) {
                            if w > 0.0 && h > 0.0 {
                                p.width = w;
                                p.height = h;
                            }
                            if !para.is_empty() {
                                out.blocks.push(Block::Para(std::mem::replace(&mut para, Para::new("", st.fmt.clone()))));
                                para.runs.clear();
                            }
                            out.blocks.push(Block::Picture(p));
                        }
                    }
                }
                if let Some(s) = stack.pop() {
                    st = s;
                }
                i += 1;
            }
            b'\\' => {
                i += 1;
                if i >= src.len() {
                    break;
                }
                let n = src[i];
                if n == b'\'' {
                    let h = std::str::from_utf8(&src[i + 1..(i + 3).min(src.len())]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                    i += 3;
                    if skip_chars > 0 {
                        skip_chars -= 1;
                        continue;
                    }
                    if let Some(b) = h {
                        if !st.skip && dest.last().map(|d| d.is_empty()).unwrap_or(true) {
                            pending_bytes.push(b);
                        } else if dest.last().map(|d| d == "fonttbl" || d == "fonttbl_entry").unwrap_or(false) {
                            fonttbl_name.push(b as char);
                        }
                    }
                    continue;
                }
                if !n.is_ascii_alphabetic() {
                    // Control symbols.
                    i += 1;
                    flush_bytes(&mut pending_bytes, &mut para, &st.fmt, codepage);
                    match n {
                        b'*' => st.skip = true,
                        b'~' => push_text(&mut para, "\u{a0}", &st.fmt),
                        b'-' => {}
                        b'_' => push_text(&mut para, "‑", &st.fmt),
                        b'\\' | b'{' | b'}' if !st.skip => push_text(&mut para, &(n as char).to_string(), &st.fmt),
                        b'\n' | b'\r' if !st.skip => {
                            flush_bytes(&mut pending_bytes, &mut para, &st.fmt, codepage);
                            out.blocks.push(Block::Para(std::mem::replace(&mut para, Para::new("", st.fmt.clone()))));
                            para.runs.clear();
                        }
                        _ => {}
                    }
                    continue;
                }
                let start = i;
                while i < src.len() && src[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let word = std::str::from_utf8(&src[start..i]).unwrap_or("").to_string();
                let nstart = i;
                if i < src.len() && (src[i] == b'-' || src[i].is_ascii_digit()) {
                    i += 1;
                    while i < src.len() && src[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let param: Option<i32> = std::str::from_utf8(&src[nstart..i]).ok().and_then(|s| s.parse().ok());
                if i < src.len() && src[i] == b' ' {
                    i += 1;
                }
                flush_bytes(&mut pending_bytes, &mut para, &st.fmt, codepage);
                let p1 = param.unwrap_or(1);
                match word.as_str() {
                    "fonttbl" => {
                        dest.last_mut().map(|d| *d = "fonttbl".into());
                    }
                    "colortbl" => {
                        dest.last_mut().map(|d| *d = "colortbl".into());
                    }
                    "stylesheet" | "info" | "header" | "footer" | "headerl" | "headerr" | "footerl" | "footerr" | "footnote" | "fldinst" | "xmlnstbl" | "listtable" | "listoverridetable" | "rsidtbl" | "generator" | "themedata" | "colorschememapping" | "latentstyles" | "datastore" | "mmathPr" | "pgdsctbl" | "object" | "nonshppict" => st.skip = true,
                    "pict" => {
                        dest.last_mut().map(|d| *d = "pict".into());
                        pict = Some((String::new(), vec![], 0.0, 0.0));
                    }
                    "picwgoal" => {
                        if let Some(p) = &mut pict {
                            p.2 = p1 as f32 / 20.0;
                        }
                    }
                    "pichgoal" => {
                        if let Some(p) = &mut pict {
                            p.3 = p1 as f32 / 20.0;
                        }
                    }
                    "ansicpg" => {
                        codepage = encoding_rs::Encoding::for_label(format!("windows-{p1}").as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252);
                    }
                    "f" => {
                        if dest.last().map(|d| d == "fonttbl" || d == "fonttbl_entry").unwrap_or(false) {
                            fonttbl_idx = p1;
                            fonttbl_name.clear();
                            dest.last_mut().map(|d| *d = "fonttbl_entry".into());
                        } else {
                            st.font_idx = p1;
                            if let Some(n) = fonts.get(&p1) {
                                st.fmt.font = n.clone();
                            }
                        }
                    }
                    "red" => color_cur[0] = p1 as u8,
                    "green" => color_cur[1] = p1 as u8,
                    "blue" => color_cur[2] = p1 as u8,
                    "fs" => st.fmt.size = p1 as f32 / 2.0,
                    "b" => st.fmt.bold = param != Some(0),
                    "i" => st.fmt.italic = param != Some(0),
                    "ul" => st.fmt.underline = param != Some(0),
                    "ulnone" => st.fmt.underline = false,
                    "strike" => st.fmt.strike = param != Some(0),
                    "super" => st.fmt.script = 1,
                    "sub" => st.fmt.script = -1,
                    "nosupersub" => st.fmt.script = 0,
                    "cf" => st.fmt.color = colors.get(p1 as usize).cloned().unwrap_or([0, 0, 0]),
                    "highlight" | "cb" | "chcbpat" => st.fmt.highlight = if p1 == 0 { None } else { colors.get(p1 as usize).cloned() },
                    "plain" => {
                        let font = st.fmt.font.clone();
                        st.fmt = out.body.clone();
                        st.fmt.font = font;
                    }
                    "pard" => {
                        para.style = ParaStyle::Body;
                        para.page_break_before = false;
                        para.align = Align2::Left;
                        para.indent_left = 0.0;
                        para.indent_first = 0.0;
                        para.indent_right = 0.0;
                        para.list = None;
                        in_table = false;
                    }
                    "intbl" => in_table = true,
                    "ql" => para.align = Align2::Left,
                    "qc" => para.align = Align2::Center,
                    "qr" => para.align = Align2::Right,
                    "qj" => para.align = Align2::Justify,
                    "li" => para.indent_left = p1 as f32 / 20.0,
                    "ri" => para.indent_right = p1 as f32 / 20.0,
                    "fi" => para.indent_first = p1 as f32 / 20.0,
                    "sb" => para.space_before = p1 as f32 / 20.0,
                    "sa" => para.space_after = p1 as f32 / 20.0,
                    "pagebb" => para.page_break_before = true,
                    "outlinelevel" => para.style = ParaStyle::Heading((p1 + 1).clamp(1, 6) as u8),
                    "pnlvlblt" => para.list = Some(ListInfo { kind: ListKind::Bullet, level: 0, checked: false }),
                    "pnlvlbody" => para.list = Some(ListInfo { kind: ListKind::Number, level: 0, checked: false }),
                    "uc" => st.uc = p1.max(0) as usize,
                    "u" => {
                        let mut v = param.unwrap_or(0);
                        if v < 0 {
                            v += 65536;
                        }
                        if !st.skip {
                            if let Some(ch) = char::from_u32(v as u32) {
                                push_text(&mut para, &ch.to_string(), &st.fmt);
                            }
                        }
                        skip_chars = st.uc;
                    }
                    "par" | "sect" if !st.skip => {
                        para.normalize();
                        if in_table {
                            // Paragraph inside a cell: keep as a line break.
                            push_text(&mut para, "\n", &st.fmt);
                        } else {
                            let next = Para { runs: vec![], ..para.clone() };
                            let p = std::mem::replace(&mut para, next);
                            out.blocks.push(Block::Para(p));
                            para.page_break_before = false;
                        }
                    }
                    "line" if !st.skip => push_text(&mut para, "\n", &st.fmt),
                    "tab" if !st.skip => push_text(&mut para, "\t", &st.fmt),
                    "emdash" => push_text(&mut para, "—", &st.fmt),
                    "endash" => push_text(&mut para, "–", &st.fmt),
                    "bullet" => push_text(&mut para, "•", &st.fmt),
                    "lquote" => push_text(&mut para, "‘", &st.fmt),
                    "rquote" => push_text(&mut para, "’", &st.fmt),
                    "ldblquote" => push_text(&mut para, "“", &st.fmt),
                    "rdblquote" => push_text(&mut para, "”", &st.fmt),
                    "page" if !st.skip => {
                        if !para.is_empty() {
                            let next = Para { runs: vec![], ..para.clone() };
                            out.blocks.push(Block::Para(std::mem::replace(&mut para, next)));
                        }
                        out.blocks.push(Block::PageBreak);
                    }
                    "cell" => {
                        let mut p = std::mem::replace(&mut para, Para { runs: vec![], ..Para::new("", st.fmt.clone()) });
                        if p.runs.is_empty() {
                            p.runs.push(Run { text: String::new(), fmt: st.fmt.clone() });
                        }
                        // Trim trailing line breaks.
                        while p.text().ends_with('\n') {
                            let n2 = p.char_len();
                            p.splice(n2 - 1, 1, "", &st.fmt);
                        }
                        p.space_after = 0.0;
                        row.push(Cell { para: p, shading: None });
                    }
                    "row" => {
                        if !row.is_empty() {
                            rows.push(std::mem::take(&mut row));
                        }
                        // A table ends when the next paragraph is not in a table.
                        let look = std::str::from_utf8(&src[i..(i + 400).min(src.len())]).unwrap_or("");
                        if !look.contains("\\trowd") || look.find("\\par").map(|p| p < look.find("\\trowd").unwrap_or(usize::MAX)).unwrap_or(false) {
                            let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1);
                            for r in &mut rows {
                                while r.len() < cols {
                                    r.push(Cell::new("", &out.body));
                                }
                            }
                            let w = out.setup.text_width();
                            out.blocks.push(Block::Table(Table { rows: std::mem::take(&mut rows), widths: vec![w / cols as f32; cols], borders: true, header: false }));
                        }
                        in_table = false;
                    }
                    "pngblip" | "jpegblip" => {
                        if let Some(p) = &mut pict {
                            p.0 = word.clone();
                        }
                    }
                    "paperw" => out.setup.width = p1 as f32 / 20.0,
                    "paperh" => out.setup.height = p1 as f32 / 20.0,
                    "margl" => out.setup.margins[0] = p1 as f32 / 20.0,
                    "margt" => out.setup.margins[1] = p1 as f32 / 20.0,
                    "margr" => out.setup.margins[2] = p1 as f32 / 20.0,
                    "margb" => out.setup.margins[3] = p1 as f32 / 20.0,
                    _ => {}
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                if skip_chars > 0 {
                    skip_chars -= 1;
                    i += 1;
                    continue;
                }
                let d = dest.last().cloned().unwrap_or_default();
                if d == "colortbl" {
                    if c == b';' {
                        colors.push(color_cur);
                        color_cur = [0, 0, 0];
                    }
                } else if d == "fonttbl" || d == "fonttbl_entry" {
                    fonttbl_name.push(c as char);
                } else if d == "pict" {
                    if let Some(p) = &mut pict {
                        if c.is_ascii_hexdigit() {
                            p.1.push(c);
                        }
                    }
                } else if !st.skip {
                    pending_bytes.push(c);
                }
                i += 1;
            }
        }
    }
    flush_bytes(&mut pending_bytes, &mut para, &st.fmt, codepage);
    if !para.runs.is_empty() {
        out.blocks.push(Block::Para(para));
    }
    // Pictures: decode hex collected inside pict groups.
    for b in &mut out.blocks {
        if let Block::Para(p) = b {
            if p.runs.is_empty() {
                p.runs.push(Run { text: String::new(), fmt: out.body.clone() });
            }
            p.normalize();
        }
    }
    out.normalize();
    Ok(out)
}

fn rtf_text(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\t' => out.push_str("\\tab "),
            '\n' => out.push_str("\\line "),
            c if (c as u32) < 128 => out.push(c),
            c => {
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) {
                    out.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    out
}

pub fn write_rtf(doc: &Document) -> Vec<u8> {
    let mut fonts: Vec<String> = vec![doc.body.font.clone()];
    let mut colors: Vec<[u8; 3]> = vec![[0, 0, 0]];
    let mut idx_font = |f: &str, fonts: &mut Vec<String>| match fonts.iter().position(|x| x == f) {
        Some(i) => i,
        None => {
            fonts.push(f.into());
            fonts.len() - 1
        }
    };
    let mut idx_color = |c: [u8; 3], colors: &mut Vec<[u8; 3]>| match colors.iter().position(|x| *x == c) {
        Some(i) => i + 1,
        None => {
            colors.push(c);
            colors.len()
        }
    };
    let mut body = String::new();
    let para = |p: &Para, body: &mut String, fonts: &mut Vec<String>, colors: &mut Vec<[u8; 3]>, intbl: bool, idx_font: &mut dyn FnMut(&str, &mut Vec<String>) -> usize, idx_color: &mut dyn FnMut([u8; 3], &mut Vec<[u8; 3]>) -> usize| {
        body.push_str("\\pard");
        if intbl {
            body.push_str("\\intbl");
        }
        body.push_str(match p.align {
            Align2::Left => "\\ql",
            Align2::Center => "\\qc",
            Align2::Right => "\\qr",
            Align2::Justify => "\\qj",
        });
        if p.page_break_before {
            body.push_str("\\pagebb");
        }
        let li = p.indent_left + p.list.as_ref().map(|l| 18.0 * (l.level as f32 + 1.0)).unwrap_or(0.0);
        body.push_str(&format!("\\li{}\\ri{}\\fi{}\\sb{}\\sa{}", (li * 20.0) as i64, (p.indent_right * 20.0) as i64, (p.indent_first * 20.0) as i64, (p.space_before * 20.0) as i64, (p.space_after * 20.0) as i64));
        if let ParaStyle::Heading(n) = p.style {
            body.push_str(&format!("\\outlinelevel{}", n - 1));
        }
        body.push(' ');
        if let Some(l) = &p.list {
            body.push_str(&rtf_text(&format!("{}\t", kit42::richtext::list_marker(l.kind, l.level, 1, l.checked))));
        }
        for r in &p.runs {
            let f = &r.fmt;
            let fi = idx_font(&f.font, fonts);
            let ci = idx_color(f.color, colors);
            body.push_str(&format!("{{\\f{fi}\\fs{}\\cf{ci}", (f.size * 2.0) as i64));
            if f.bold {
                body.push_str("\\b");
            }
            if f.italic {
                body.push_str("\\i");
            }
            if f.underline {
                body.push_str("\\ul");
            }
            if f.strike {
                body.push_str("\\strike");
            }
            match f.script {
                1 => body.push_str("\\super"),
                -1 => body.push_str("\\sub"),
                _ => {}
            }
            if let Some(h) = f.highlight {
                let hi = idx_color(h, colors);
                body.push_str(&format!("\\chcbpat{hi}\\highlight{hi}"));
            }
            body.push(' ');
            match &f.link {
                Some(l) => body.push_str(&format!("{{\\field{{\\*\\fldinst HYPERLINK \"{}\"}}{{\\fldrslt {}}}}}", rtf_text(l), rtf_text(&r.text))),
                None => body.push_str(&rtf_text(&r.text)),
            }
            body.push('}');
        }
    };
    for b in &doc.blocks {
        match b {
            Block::Para(p) => {
                para(p, &mut body, &mut fonts, &mut colors, false, &mut idx_font, &mut idx_color);
                body.push_str("\\par\n");
            }
            Block::PageBreak => body.push_str("\\page\n"),
            Block::Table(t) => {
                for row in &t.rows {
                    body.push_str("\\trowd\\trgaph108");
                    let mut x = 0.0;
                    for w in &t.widths {
                        x += w;
                        if t.borders {
                            body.push_str("\\clbrdrt\\brdrs\\clbrdrl\\brdrs\\clbrdrb\\brdrs\\clbrdrr\\brdrs");
                        }
                        body.push_str(&format!("\\cellx{}", (x * 20.0) as i64));
                    }
                    body.push('\n');
                    for c in row {
                        para(&c.para, &mut body, &mut fonts, &mut colors, true, &mut idx_font, &mut idx_color);
                        body.push_str("\\cell ");
                    }
                    body.push_str("\\row\n");
                }
                body.push_str("\\pard\n");
            }
            Block::Picture(pic) => {
                let (_, ext) = kit42::ooxml::image_mime(&pic.data);
                let (blip, data) = if ext == "jpeg" {
                    ("\\jpegblip", (*pic.data).clone())
                } else if ext == "png" {
                    ("\\pngblip", (*pic.data).clone())
                } else {
                    let mut v = Vec::new();
                    if let Ok(img) = image::load_from_memory(&pic.data) {
                        let _ = img.write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png);
                    }
                    ("\\pngblip", v)
                };
                let (pw, ph) = image::load_from_memory(&data).map(|i| (i.width(), i.height())).unwrap_or((1, 1));
                body.push_str(&format!(
                    "\\pard\\qc{{\\pict{blip}\\picw{pw}\\pich{ph}\\picwgoal{}\\pichgoal{}\n",
                    (pic.width * 20.0) as i64,
                    (pic.height * 20.0) as i64
                ));
                for chunk in data.chunks(64) {
                    for b in chunk {
                        body.push_str(&format!("{:02x}", b));
                    }
                    body.push('\n');
                }
                body.push_str("}\\par\n");
            }
        }
    }
    let mut out = String::from("{\\rtf1\\ansi\\ansicpg1252\\deff0\\uc1\n{\\fonttbl");
    for (i, f) in fonts.iter().enumerate() {
        out.push_str(&format!("{{\\f{i}\\fnil {};}}", rtf_text(f)));
    }
    out.push_str("}\n{\\colortbl;");
    for c in &colors {
        out.push_str(&format!("\\red{}\\green{}\\blue{};", c[0], c[1], c[2]));
    }
    out.push_str("}\n");
    let s = &doc.setup;
    out.push_str(&format!(
        "\\paperw{}\\paperh{}\\margl{}\\margt{}\\margr{}\\margb{}\n",
        (s.width * 20.0) as i64, (s.height * 20.0) as i64, (s.margins[0] * 20.0) as i64, (s.margins[1] * 20.0) as i64, (s.margins[2] * 20.0) as i64, (s.margins[3] * 20.0) as i64
    ));
    out.push_str(&body);
    out.push('}');
    out.into_bytes()
}

// ───────────────────────── HTML ─────────────────────────

pub fn write_html(doc: &Document, standalone_images: bool) -> String {
    let mut h = format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{}</title><style>body{{font-family:'{}',sans-serif;font-size:{}pt;max-width:{}pt;margin:2em auto;line-height:1.35}} table{{border-collapse:collapse}} td{{border:1px solid #999;padding:4px 6px;vertical-align:top}} pre{{background:#f3f3f3;padding:6px}}</style></head><body>\n",
        esc(&doc.meta.title),
        esc(&doc.body.font),
        doc.body.size,
        doc.setup.text_width()
    );
    let runs = |p: &Para| -> String {
        let mut s = String::new();
        for r in &p.runs {
            let f = &r.fmt;
            let mut t = esc(&r.text).replace('\n', "<br>");
            let mut style = String::new();
            if f.font != doc.body.font {
                style.push_str(&format!("font-family:'{}';", esc(&f.font)));
            }
            if (f.size - doc.body.size).abs() > 0.1 && p.style == ParaStyle::Body {
                style.push_str(&format!("font-size:{}pt;", f.size));
            }
            if f.color != [0, 0, 0] {
                style.push_str(&format!("color:{};", hex(f.color)));
            }
            if let Some(hl) = f.highlight {
                style.push_str(&format!("background:{};", hex(hl)));
            }
            if f.bold && !matches!(p.style, ParaStyle::Heading(_) | ParaStyle::Title) {
                t = format!("<strong>{t}</strong>");
            }
            if f.italic {
                t = format!("<em>{t}</em>");
            }
            if f.underline {
                t = format!("<u>{t}</u>");
            }
            if f.strike {
                t = format!("<s>{t}</s>");
            }
            match f.script {
                1 => t = format!("<sup>{t}</sup>"),
                -1 => t = format!("<sub>{t}</sub>"),
                _ => {}
            }
            if !style.is_empty() {
                t = format!("<span style=\"{style}\">{t}</span>");
            }
            if let Some(l) = &f.link {
                t = format!("<a href=\"{}\">{t}</a>", esc(l));
            }
            s.push_str(&t);
        }
        s
    };
    let align = |p: &Para| match p.align {
        Align2::Left => String::new(),
        Align2::Center => " style=\"text-align:center\"".into(),
        Align2::Right => " style=\"text-align:right\"".into(),
        Align2::Justify => " style=\"text-align:justify\"".into(),
    };
    let mut i = 0;
    let mut pic_n = 0;
    while i < doc.blocks.len() {
        match &doc.blocks[i] {
            Block::Para(p) if p.list.is_some() => {
                let numbered = !matches!(p.list.as_ref().unwrap().kind, ListKind::Bullet | ListKind::Check);
                let tag = if numbered { "ol" } else { "ul" };
                h.push_str(&format!("<{tag}>\n"));
                while let Some(Block::Para(q)) = doc.blocks.get(i) {
                    let Some(l) = &q.list else { break };
                    let check = if l.kind == ListKind::Check { if l.checked { "☑ " } else { "☐ " } } else { "" };
                    h.push_str(&format!("<li style=\"margin-left:{}em\">{check}{}</li>\n", l.level as f32 * 1.5, runs(q)));
                    i += 1;
                }
                h.push_str(&format!("</{tag}>\n"));
                continue;
            }
            Block::Para(p) => {
                let (tag, extra) = match p.style {
                    ParaStyle::Title => ("h1".to_string(), " class=\"title\"".to_string()),
                    ParaStyle::Heading(n) => (format!("h{}", n.min(6)), String::new()),
                    ParaStyle::Quote => ("blockquote".into(), String::new()),
                    ParaStyle::Code => ("pre".into(), String::new()),
                    _ => ("p".into(), String::new()),
                };
                let body = runs(p);
                h.push_str(&format!("<{tag}{extra}{}>{}</{tag}>\n", align(p), if body.is_empty() { "&nbsp;".into() } else { body }));
            }
            Block::Table(t) => {
                h.push_str("<table>\n");
                for row in &t.rows {
                    h.push_str("<tr>");
                    for (ci, c) in row.iter().enumerate() {
                        let w = t.widths.get(ci).cloned().unwrap_or(50.0);
                        let bg = c.shading.map(|s| format!(";background:{}", hex(s))).unwrap_or_default();
                        h.push_str(&format!("<td style=\"width:{w}pt{bg}\">{}</td>", runs(&c.para)));
                    }
                    h.push_str("</tr>\n");
                }
                h.push_str("</table>\n");
            }
            Block::Picture(pic) => {
                pic_n += 1;
                let (mime, _) = kit42::ooxml::image_mime(&pic.data);
                let src = if standalone_images {
                    format!("data:{mime};base64,{}", base64(&pic.data))
                } else {
                    format!("image{pic_n}")
                };
                h.push_str(&format!(
                    "<p style=\"text-align:{}\"><img src=\"{src}\" alt=\"{}\" width=\"{}\" height=\"{}\"></p>\n",
                    match pic.align {
                        Align2::Center => "center",
                        Align2::Right => "right",
                        _ => "left",
                    },
                    esc(&pic.alt),
                    (pic.width / 0.75) as i64,
                    (pic.height / 0.75) as i64
                ));
            }
            Block::PageBreak => h.push_str("<hr style=\"page-break-after:always\">\n"),
        }
        i += 1;
    }
    h.push_str("</body></html>\n");
    h
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len() * 4 / 3 + 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

fn unbase64(s: &str) -> Vec<u8> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let bytes: Vec<u32> = s.bytes().filter_map(val).collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for c in bytes.chunks(4) {
        let n = c.iter().enumerate().fold(0u32, |acc, (i, v)| acc | v << (18 - 6 * i));
        out.push((n >> 16) as u8);
        if c.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if c.len() > 3 {
            out.push(n as u8);
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|&e| e < 12);
        match end {
            Some(e) => {
                let ent = &rest[1..e];
                let ch = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some('\u{a0}'),
                    "mdash" => Some('—'),
                    "ndash" => Some('–'),
                    "laquo" => Some('«'),
                    "raquo" => Some('»'),
                    "hellip" => Some('…'),
                    "copy" => Some('©'),
                    _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
                    _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
                    _ => None,
                };
                match ch {
                    Some(c) => {
                        out.push(c);
                        rest = &rest[e + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &rest[1..];
                    }
                }
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A tolerant HTML reader for the common document tags.
pub fn read_html(src: &str, base_dir: Option<&Path>) -> Document {
    let mut out = Document::default();
    out.blocks.clear();
    let body = CharFmt { font: "Calibri".into(), size: 11.0, ..CharFmt::default() };
    out.body = body.clone();
    let mut fmt_stack: Vec<CharFmt> = vec![body.clone()];
    let mut para = Para::new("", body.clone());
    para.runs.clear();
    let mut list_stack: Vec<ListKind> = vec![];
    let mut table: Option<Vec<Vec<Cell>>> = None;
    let mut in_cell = false;
    let mut in_pre = false;
    let mut skip = 0;
    let flush = |para: &mut Para, out: &mut Document, table: &mut Option<Vec<Vec<Cell>>>, in_cell: bool| {
        if in_cell {
            return;
        }
        let has_text = !para.text().trim().is_empty();
        if has_text || para.style != ParaStyle::Body {
            let mut p = std::mem::replace(para, Para::new("", para.runs.first().map(|r| r.fmt.clone()).unwrap_or_default()));
            p.runs.retain(|r| !r.text.is_empty());
            if p.runs.is_empty() {
                p.runs.push(Run { text: String::new(), fmt: CharFmt::default() });
            }
            // Trim leading whitespace of the paragraph.
            if let Some(r) = p.runs.first_mut() {
                r.text = r.text.trim_start().to_string();
            }
            p.normalize();
            out.blocks.push(Block::Para(p));
        }
        para.runs.clear();
        para.style = ParaStyle::Body;
        para.list = None;
        para.align = Align2::Left;
        let _ = table;
    };
    let mut i = 0;
    let bytes = src.as_bytes();
    while i < src.len() {
        if bytes[i] == b'<' {
            let end = match src[i..].find('>') {
                Some(e) => i + e,
                None => break,
            };
            let tag_full = &src[i + 1..end];
            i = end + 1;
            if tag_full.starts_with("!--") {
                if let Some(e) = src[i - 1..].find("-->") {
                    i = i - 1 + e + 3;
                }
                continue;
            }
            let closing = tag_full.starts_with('/');
            let tag_body = tag_full.trim_start_matches('/');
            let name: String = tag_body.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_ascii_lowercase();
            let attr = |key: &str| -> Option<String> {
                let lower = tag_body.to_ascii_lowercase();
                let pos = lower.find(&format!("{key}="))?;
                let rest = &tag_body[pos + key.len() + 1..];
                let q = rest.chars().next()?;
                if q == '"' || q == '\'' {
                    rest[1..].find(q).map(|e| rest[1..1 + e].to_string())
                } else {
                    Some(rest.split_whitespace().next().unwrap_or("").to_string())
                }
            };
            match (name.as_str(), closing) {
                ("script" | "style" | "head" | "title", false) => skip += 1,
                ("script" | "style" | "head" | "title", true) => skip = (skip - 1).max(0),
                _ if skip > 0 => {}
                ("p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "pre" | "li" | "tr" | "section" | "article", false) => {
                    flush(&mut para, &mut out, &mut table, in_cell);
                    let cur = fmt_stack.last().cloned().unwrap_or_default();
                    match name.as_str() {
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                            let n: u8 = name[1..].parse().unwrap_or(1);
                            para.style = ParaStyle::Heading(n);
                            fmt_stack.push(ParaStyle::Heading(n).base_fmt(&body));
                        }
                        "blockquote" => {
                            para.style = ParaStyle::Quote;
                            fmt_stack.push(ParaStyle::Quote.base_fmt(&body));
                        }
                        "pre" => {
                            para.style = ParaStyle::Code;
                            in_pre = true;
                            fmt_stack.push(ParaStyle::Code.base_fmt(&body));
                        }
                        "li" => {
                            para.list = Some(ListInfo { kind: *list_stack.last().unwrap_or(&ListKind::Bullet), level: list_stack.len().saturating_sub(1) as u8, checked: false });
                            fmt_stack.push(cur);
                        }
                        "tr" => {
                            if let Some(t) = &mut table {
                                t.push(vec![]);
                            }
                            fmt_stack.push(cur);
                        }
                        _ => fmt_stack.push(cur),
                    }
                    if let Some(st) = attr("style") {
                        if st.contains("text-align:center") || st.contains("text-align: center") {
                            para.align = Align2::Center;
                        } else if st.contains("text-align:right") || st.contains("text-align: right") {
                            para.align = Align2::Right;
                        } else if st.contains("justify") {
                            para.align = Align2::Justify;
                        }
                    }
                    if attr("align").as_deref() == Some("center") {
                        para.align = Align2::Center;
                    }
                }
                ("p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "pre" | "li" | "tr" | "section" | "article", true) => {
                    flush(&mut para, &mut out, &mut table, in_cell);
                    if fmt_stack.len() > 1 {
                        fmt_stack.pop();
                    }
                    if name == "pre" {
                        in_pre = false;
                    }
                }
                ("ul", false) => list_stack.push(ListKind::Bullet),
                ("ol", false) => list_stack.push(ListKind::Number),
                ("ul" | "ol", true) => {
                    list_stack.pop();
                }
                ("br", _) => para.runs.push(Run { text: "\n".into(), fmt: fmt_stack.last().cloned().unwrap_or_default() }),
                ("hr", _) => {
                    flush(&mut para, &mut out, &mut table, in_cell);
                    if tag_body.contains("page-break") {
                        out.blocks.push(Block::PageBreak);
                    }
                }
                ("table", false) => {
                    flush(&mut para, &mut out, &mut table, in_cell);
                    table = Some(vec![]);
                }
                ("table", true) => {
                    if let Some(rows) = table.take() {
                        let rows: Vec<Vec<Cell>> = rows.into_iter().filter(|r| !r.is_empty()).collect();
                        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                        if cols > 0 {
                            let mut rows = rows;
                            for r in &mut rows {
                                while r.len() < cols {
                                    r.push(Cell::new("", &body));
                                }
                            }
                            let w = out.setup.text_width();
                            out.blocks.push(Block::Table(Table { rows, widths: vec![w / cols as f32; cols], borders: true, header: false }));
                        }
                    }
                }
                ("td" | "th", false) => {
                    in_cell = true;
                    para.runs.clear();
                    let mut f = fmt_stack.last().cloned().unwrap_or_default();
                    f.bold |= name == "th";
                    fmt_stack.push(f);
                }
                ("td" | "th", true) => {
                    in_cell = false;
                    let mut p = std::mem::replace(&mut para, Para::new("", body.clone()));
                    p.runs.retain(|r| !r.text.is_empty());
                    if p.runs.is_empty() {
                        p.runs.push(Run { text: String::new(), fmt: body.clone() });
                    }
                    p.normalize();
                    p.space_after = 0.0;
                    para.runs.clear();
                    if let Some(t) = &mut table {
                        if t.is_empty() {
                            t.push(vec![]);
                        }
                        t.last_mut().unwrap().push(Cell { para: p, shading: None });
                    }
                    if fmt_stack.len() > 1 {
                        fmt_stack.pop();
                    }
                }
                ("b" | "strong" | "i" | "em" | "u" | "s" | "del" | "strike" | "sup" | "sub" | "code" | "a" | "span" | "font" | "mark", false) => {
                    let mut f = fmt_stack.last().cloned().unwrap_or_default();
                    match name.as_str() {
                        "b" | "strong" => f.bold = true,
                        "i" | "em" => f.italic = true,
                        "u" => f.underline = true,
                        "s" | "del" | "strike" => f.strike = true,
                        "sup" => f.script = 1,
                        "sub" => f.script = -1,
                        "code" => f.font = "Consolas".into(),
                        "mark" => f.highlight = Some([255, 255, 0]),
                        "a" => f.link = attr("href"),
                        _ => {}
                    }
                    if let Some(st) = attr("style") {
                        for decl in st.split(';') {
                            if let Some((k, v)) = decl.split_once(':') {
                                let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
                                match k.as_str() {
                                    "color" => f.color = parse_color(v).unwrap_or(f.color),
                                    "background" | "background-color" => f.highlight = parse_color(v),
                                    "font-weight" => f.bold = v.contains("bold") || v.parse::<u32>().map(|x| x >= 600).unwrap_or(false),
                                    "font-style" => f.italic = v.contains("italic"),
                                    "font-size" => f.size = length_pt(v).unwrap_or(f.size),
                                    "font-family" => f.font = v.split(',').next().unwrap_or("").trim().trim_matches(['\'', '"']).to_string(),
                                    "text-decoration" => {
                                        f.underline |= v.contains("underline");
                                        f.strike |= v.contains("line-through");
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    if let Some(c) = attr("color") {
                        f.color = parse_color(&c).unwrap_or(f.color);
                    }
                    fmt_stack.push(f);
                }
                ("b" | "strong" | "i" | "em" | "u" | "s" | "del" | "strike" | "sup" | "sub" | "code" | "a" | "span" | "font" | "mark", true) => {
                    if fmt_stack.len() > 1 {
                        fmt_stack.pop();
                    }
                }
                ("img", _) => {
                    if let Some(src) = attr("src") {
                        let data = if let Some(b64) = src.split_once(";base64,").map(|x| x.1) {
                            Some(unbase64(b64))
                        } else {
                            base_dir.and_then(|d| std::fs::read(d.join(src.replace('/', "\\"))).ok())
                        };
                        if let Some(pic) = data.and_then(|d| Picture::new(d, out.setup.text_width())) {
                            flush(&mut para, &mut out, &mut table, in_cell);
                            out.blocks.push(Block::Picture(pic));
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let next = src[i..].find('<').map(|e| i + e).unwrap_or(src.len());
        if skip == 0 {
            let raw = &src[i..next];
            let mut text = decode_entities(raw);
            if !in_pre {
                text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if raw.starts_with(char::is_whitespace) && !text.is_empty() {
                    text.insert(0, ' ');
                }
                if raw.ends_with(char::is_whitespace) && !text.is_empty() {
                    text.push(' ');
                }
            }
            if !text.is_empty() && (in_pre || !text.trim().is_empty() || !para.runs.is_empty()) {
                para.runs.push(Run { text, fmt: fmt_stack.last().cloned().unwrap_or_default() });
            }
        }
        i = next;
    }
    flush(&mut para, &mut out, &mut table, false);
    // Split <pre> paragraphs into one paragraph per line.
    let mut blocks = vec![];
    for b in out.blocks.drain(..) {
        match b {
            Block::Para(p) if p.style == ParaStyle::Code && p.text().contains('\n') => {
                for line in p.text().trim_end().split('\n') {
                    let mut q = Para::with_style(line, ParaStyle::Code, &body);
                    q.space_after = 0.0;
                    q.shading = Some([0xF3, 0xF3, 0xF3]);
                    blocks.push(Block::Para(q));
                }
            }
            b => blocks.push(b),
        }
    }
    out.blocks = blocks;
    out.normalize();
    out
}

// ───────────────────────── Markdown ─────────────────────────

pub fn read_markdown(src: &str, base_dir: Option<&Path>) -> Document {
    use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
    let mut out = Document::default();
    out.blocks.clear();
    let body = out.body.clone();
    let mut fmt_stack = vec![body.clone()];
    let mut para: Option<Para> = None;
    let mut lists: Vec<Option<u64>> = vec![];
    let mut table: Option<(Vec<Vec<Cell>>, bool)> = None;
    let mut in_code = false;
    let mut code_buf = String::new();
    let mut quote = 0;
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES;
    let start_para = |style: ParaStyle, lists: &Vec<Option<u64>>, quote: usize| -> Para {
        let mut p = Para::with_style("", style, &body);
        p.runs.clear();
        if let Some(l) = lists.last() {
            p.list = Some(ListInfo { kind: if l.is_some() { ListKind::Number } else { ListKind::Bullet }, level: (lists.len() - 1) as u8, checked: false });
            p.space_after = 2.0;
        }
        if quote > 0 && style == ParaStyle::Body {
            p.style = ParaStyle::Quote;
            p.indent_left = 36.0;
        }
        p
    };
    let finish = |p: Option<Para>, out: &mut Document, table: &mut Option<(Vec<Vec<Cell>>, bool)>| {
        if let Some(mut p) = p {
            if p.runs.is_empty() {
                p.runs.push(Run { text: String::new(), fmt: p.style.base_fmt(&out.body) });
            }
            p.normalize();
            if let Some((rows, _)) = table {
                if let Some(r) = rows.last_mut() {
                    p.space_after = 0.0;
                    r.push(Cell { para: p, shading: None });
                    return;
                }
            }
            out.blocks.push(Block::Para(p));
        }
    };
    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(Tag::Paragraph) => {
                if para.is_none() {
                    para = Some(start_para(ParaStyle::Body, &lists, quote));
                }
            }
            Event::End(TagEnd::Paragraph) => {
                finish(para.take(), &mut out, &mut table);
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let n = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                para = Some(start_para(ParaStyle::Heading(n), &lists, 0));
                fmt_stack.push(ParaStyle::Heading(n).base_fmt(&body));
            }
            Event::End(TagEnd::Heading(_)) => {
                fmt_stack.pop();
                finish(para.take(), &mut out, &mut table);
            }
            Event::Start(Tag::BlockQuote(_)) => quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => quote -= 1,
            Event::Start(Tag::CodeBlock(_)) => {
                in_code = true;
                code_buf.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code = false;
                for line in code_buf.trim_end_matches('\n').split('\n') {
                    let mut q = Para::with_style(line, ParaStyle::Code, &body);
                    q.space_after = 0.0;
                    q.shading = Some([0xF3, 0xF3, 0xF3]);
                    out.blocks.push(Block::Para(q));
                }
            }
            Event::Start(Tag::List(start)) => {
                finish(para.take(), &mut out, &mut table);
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                finish(para.take(), &mut out, &mut table);
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                finish(para.take(), &mut out, &mut table);
                para = Some(start_para(ParaStyle::Body, &lists, quote));
            }
            Event::End(TagEnd::Item) => finish(para.take(), &mut out, &mut table),
            Event::TaskListMarker(done) => {
                if let Some(p) = &mut para {
                    p.list = Some(ListInfo { kind: ListKind::Check, level: lists.len().saturating_sub(1) as u8, checked: done });
                }
            }
            Event::Start(Tag::Emphasis) => {
                let mut f = fmt_stack.last().cloned().unwrap();
                f.italic = true;
                fmt_stack.push(f);
            }
            Event::Start(Tag::Strong) => {
                let mut f = fmt_stack.last().cloned().unwrap();
                f.bold = true;
                fmt_stack.push(f);
            }
            Event::Start(Tag::Strikethrough) => {
                let mut f = fmt_stack.last().cloned().unwrap();
                f.strike = true;
                fmt_stack.push(f);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                let mut f = fmt_stack.last().cloned().unwrap();
                f.link = Some(dest_url.to_string());
                fmt_stack.push(f);
            }
            Event::End(TagEnd::Emphasis) | Event::End(TagEnd::Strong) | Event::End(TagEnd::Strikethrough) | Event::End(TagEnd::Link) => {
                if fmt_stack.len() > 1 {
                    fmt_stack.pop();
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let data = base_dir.and_then(|d| std::fs::read(d.join(dest_url.replace('/', "\\"))).ok());
                if let Some(pic) = data.and_then(|d| Picture::new(d, out.setup.text_width())) {
                    finish(para.take(), &mut out, &mut table);
                    out.blocks.push(Block::Picture(pic));
                    para = Some(start_para(ParaStyle::Body, &lists, quote));
                }
                fmt_stack.push(fmt_stack.last().cloned().unwrap());
            }
            Event::End(TagEnd::Image) => {
                fmt_stack.pop();
            }
            Event::Start(Tag::Table(_)) => table = Some((vec![], true)),
            Event::End(TagEnd::Table) => {
                if let Some((rows, _)) = table.take() {
                    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                    if cols > 0 {
                        let mut rows = rows;
                        for r in &mut rows {
                            while r.len() < cols {
                                r.push(Cell::new("", &body));
                            }
                        }
                        let w = out.setup.text_width();
                        out.blocks.push(Block::Table(Table { rows, widths: vec![w / cols as f32; cols], borders: true, header: true }));
                    }
                }
            }
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                if let Some((rows, _)) = &mut table {
                    rows.push(vec![]);
                }
            }
            Event::Start(Tag::TableCell) => para = Some(start_para(ParaStyle::Body, &vec![], 0)),
            Event::End(TagEnd::TableCell) => finish(para.take(), &mut out, &mut table),
            Event::Text(t) => {
                if in_code {
                    code_buf.push_str(&t);
                } else {
                    let p = para.get_or_insert_with(|| start_para(ParaStyle::Body, &lists, quote));
                    p.runs.push(Run { text: t.to_string(), fmt: fmt_stack.last().cloned().unwrap() });
                }
            }
            Event::Code(t) => {
                let p = para.get_or_insert_with(|| start_para(ParaStyle::Body, &lists, quote));
                let mut f = fmt_stack.last().cloned().unwrap();
                f.font = "Consolas".into();
                f.highlight = Some([0xEE, 0xEE, 0xEE]);
                p.runs.push(Run { text: t.to_string(), fmt: f });
            }
            Event::SoftBreak => {
                if let Some(p) = &mut para {
                    p.runs.push(Run { text: " ".into(), fmt: fmt_stack.last().cloned().unwrap() });
                }
            }
            Event::HardBreak => {
                if let Some(p) = &mut para {
                    p.runs.push(Run { text: "\n".into(), fmt: fmt_stack.last().cloned().unwrap() });
                }
            }
            Event::Rule => {
                finish(para.take(), &mut out, &mut table);
                let mut p = Para::new("", body.clone());
                p.border_bottom = true;
                out.blocks.push(Block::Para(p));
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                if h.contains("page-break") {
                    finish(para.take(), &mut out, &mut table);
                    out.blocks.push(Block::PageBreak);
                }
            }
            _ => {}
        }
    }
    finish(para.take(), &mut out, &mut table);
    out.normalize();
    out
}

fn md_escape(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if "\\`*_[]#|<>".contains(c) {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

fn md_runs(p: &Para) -> String {
    let mut s = String::new();
    for r in &p.runs {
        if r.text.is_empty() {
            continue;
        }
        let f = &r.fmt;
        let lead = r.text.len() - r.text.trim_start().len();
        let trail = r.text.len() - r.text.trim_end().len();
        let core = r.text.trim();
        if core.is_empty() {
            s.push_str(&r.text);
            continue;
        }
        let mut t = if f.font == "Consolas" { format!("`{core}`") } else { md_escape(core).replace('\n', "  \n") };
        if f.bold && !matches!(p.style, ParaStyle::Heading(_) | ParaStyle::Title) {
            t = format!("**{t}**");
        }
        if f.italic && p.style != ParaStyle::Quote {
            t = format!("*{t}*");
        }
        if f.strike {
            t = format!("~~{t}~~");
        }
        if let Some(l) = &f.link {
            t = format!("[{t}]({l})");
        }
        s.push_str(&r.text[..lead]);
        s.push_str(&t);
        s.push_str(&r.text[r.text.len() - trail..]);
    }
    s
}

/// Markdown text; pictures are written next to the file as `<stem>_images/imageN.png`.
pub fn write_markdown(doc: &Document, md_path: Option<&Path>) -> String {
    let mut out = String::new();
    let mut pic_n = 0;
    let nums = kit42::richtext::list_numbers(&doc.blocks.iter().map(|b| match b {
        Block::Para(p) => p.clone(),
        _ => Para::default(),
    }).collect::<Vec<_>>());
    for (i, b) in doc.blocks.iter().enumerate() {
        match b {
            Block::Para(p) => {
                let text = md_runs(p);
                match (&p.list, p.style) {
                    (Some(l), _) => {
                        let indent = "  ".repeat(l.level as usize);
                        let marker = match l.kind {
                            ListKind::Bullet => "-".to_string(),
                            ListKind::Check => if l.checked { "- [x]".into() } else { "- [ ]".into() },
                            _ => nums[i].clone().unwrap_or("1.".into()).replace(|c: char| !c.is_ascii_digit() && c != '.', "").to_string(),
                        };
                        let marker = if marker.is_empty() || marker == "." { "1.".into() } else { marker };
                        out.push_str(&format!("{indent}{marker} {text}\n"));
                        if !matches!(doc.blocks.get(i + 1), Some(Block::Para(q)) if q.list.is_some()) {
                            out.push('\n');
                        }
                    }
                    (None, ParaStyle::Title) => out.push_str(&format!("# {text}\n\n")),
                    (None, ParaStyle::Heading(n)) => out.push_str(&format!("{} {text}\n\n", "#".repeat(n as usize))),
                    (None, ParaStyle::Quote) => out.push_str(&format!("> {text}\n\n")),
                    (None, ParaStyle::Code) => {
                        let prev_code = i > 0 && matches!(&doc.blocks[i - 1], Block::Para(q) if q.style == ParaStyle::Code);
                        let next_code = matches!(doc.blocks.get(i + 1), Some(Block::Para(q)) if q.style == ParaStyle::Code);
                        if !prev_code {
                            out.push_str("```\n");
                        }
                        out.push_str(&p.text());
                        out.push('\n');
                        if !next_code {
                            out.push_str("```\n\n");
                        }
                    }
                    _ if p.border_bottom && p.is_empty() => out.push_str("---\n\n"),
                    _ => {
                        if !text.trim().is_empty() {
                            out.push_str(&text);
                            out.push_str("\n\n");
                        }
                    }
                }
            }
            Block::Table(t) => {
                for (ri, row) in t.rows.iter().enumerate() {
                    out.push('|');
                    for c in row {
                        out.push_str(&format!(" {} |", md_runs(&c.para).replace('\n', "<br>").replace('|', "\\|")));
                    }
                    out.push('\n');
                    if ri == 0 {
                        out.push('|');
                        for _ in row {
                            out.push_str(" --- |");
                        }
                        out.push('\n');
                    }
                }
                out.push('\n');
            }
            Block::Picture(pic) => {
                pic_n += 1;
                let (_, ext) = kit42::ooxml::image_mime(&pic.data);
                if let Some(path) = md_path {
                    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "doc".into());
                    let dir = path.with_file_name(format!("{stem}_images"));
                    std::fs::create_dir_all(&dir).ok();
                    let name = format!("image{pic_n}.{ext}");
                    let _ = std::fs::write(dir.join(&name), &*pic.data);
                    out.push_str(&format!("![{}]({stem}_images/{name})\n\n", pic.alt));
                }
            }
            Block::PageBreak => out.push_str("<div style=\"page-break-after: always\"></div>\n\n"),
        }
    }
    out
}

// ───────────────────────── plain text ─────────────────────────

pub fn read_text(text: &str, body: &CharFmt) -> Document {
    let mut d = Document { body: body.clone(), ..Document::default() };
    d.blocks = text
        .split('\x0C')
        .enumerate()
        .flat_map(|(i, page)| {
            let mut v: Vec<Block> = vec![];
            if i > 0 {
                v.push(Block::PageBreak);
            }
            for line in page.lines() {
                let mut p = Para::new(line, body.clone());
                p.space_after = 0.0;
                v.push(Block::Para(p));
            }
            v
        })
        .collect();
    d.normalize();
    d
}
