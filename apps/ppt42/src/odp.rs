//! OpenDocument Presentation (.odp/.otp) reader and writer.

use crate::model::*;
use kit42::ooxml::{esc, parse, NodeExt, Package, PackageWriter};
use kit42::richtext::{Align2, CharFmt, ListInfo, ListKind, Para, Run};
use std::collections::HashMap;
use std::sync::Arc;

fn parse_transform(s: &str) -> Option<(f32, f32, f32)> {
    let a = s.split("rotate(").nth(1)?.split(')').next()?.trim().parse::<f32>().ok()?;
    let tr = s.split("translate(").nth(1)?.split(')').next()?;
    let mut it = tr.split(|c: char| c == ' ' || c == ',').filter(|x| !x.is_empty());
    Some((a, len_pt(it.next()?), len_pt(it.next()?)))
}

fn len_pt(s: &str) -> f32 {
    let s = s.trim();
    let (n, u): (String, String) = s.chars().partition(|c| c.is_ascii_digit() || *c == '.' || *c == '-');
    let v: f32 = n.parse().unwrap_or(0.0);
    match u.as_str() {
        "cm" => v * 72.0 / 2.54,
        "mm" => v * 72.0 / 25.4,
        "in" => v * 72.0,
        "px" => v * 0.75,
        _ => v,
    }
}

fn color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

#[derive(Default, Clone)]
struct St {
    fill: Option<Option<[u8; 3]>>,
    stroke: Option<Option<([u8; 3], f32)>>,
    text: Vec<(String, String)>,
    align: Option<Align2>,
    anchor: Option<Anchor>,
    parent: Option<String>,
}

pub fn read(bytes: &[u8]) -> Result<Presentation, String> {
    let pkg = Package::read(bytes)?;
    let xml = pkg.text("content.xml").ok_or("content.xml is missing")?;
    let d = parse(&xml)?;
    let mut styles: HashMap<String, St> = HashMap::new();
    let styles_xml = pkg.text("styles.xml").unwrap_or_default();
    let sd = parse(&styles_xml).ok();
    let mut page_w = 28.0 * 72.0 / 2.54;
    let mut page_h = 15.75 * 72.0 / 2.54;
    let mut collect = |root: roxmltree::Node| {
        for s in root.descendants().filter(|n| n.is("style")) {
            let mut st = St { parent: s.attr("parent-style-name").map(|x| x.to_string()), ..Default::default() };
            if let Some(g) = s.child("graphic-properties") {
                st.fill = g.attr("fill").map(|f| if f == "none" { None } else { g.attr("fill-color").and_then(color) });
                st.stroke = g.attr("stroke").map(|f| if f == "none" { None } else { Some((g.attr("stroke-color").and_then(color).unwrap_or([0, 0, 0]), g.attr("stroke-width").map(len_pt).unwrap_or(1.0).max(0.75))) });
                st.anchor = g.attr("textarea-vertical-align").map(|a| match a {
                    "middle" => Anchor::Middle,
                    "bottom" => Anchor::Bottom,
                    _ => Anchor::Top,
                });
            }
            if let Some(t) = s.child("text-properties") {
                st.text = t.attributes().map(|a| (a.name().to_string(), a.value().to_string())).collect();
            }
            if let Some(p) = s.child("paragraph-properties") {
                st.align = p.attr("text-align").map(|a| match a {
                    "center" => Align2::Center,
                    "end" | "right" => Align2::Right,
                    "justify" => Align2::Justify,
                    _ => Align2::Left,
                });
            }
            styles.insert(s.attr("name").unwrap_or("").to_string(), st);
        }
    };
    if let Some(sd) = &sd {
        collect(sd.root_element());
        if let Some(pl) = sd.root_element().descendant("page-layout-properties") {
            if let (Some(w), Some(h)) = (pl.attr("page-width"), pl.attr("page-height")) {
                page_w = len_pt(w);
                page_h = len_pt(h);
            }
        }
    }
    collect(d.root_element());
    let scale = 960.0 / page_w;
    let mut pres = Presentation::default();
    pres.slides.clear();
    pres.height = page_h * scale;
    let fmt_from = |name: Option<&str>, base: &CharFmt| -> CharFmt {
        let mut f = base.clone();
        let mut chain = vec![];
        let mut cur = name.map(|s| s.to_string());
        let mut g = 0;
        while let Some(c) = cur {
            g += 1;
            if g > 10 {
                break;
            }
            if let Some(s) = styles.get(&c) {
                chain.push(s.clone());
                cur = s.parent.clone();
            } else {
                break;
            }
        }
        for s in chain.iter().rev() {
            for (k, v) in &s.text {
                match k.as_str() {
                    "font-size" => f.size = len_pt(v) * scale,
                    "font-weight" => f.bold = v == "bold",
                    "font-style" => f.italic = v == "italic",
                    "color" => f.color = color(v).unwrap_or(f.color),
                    "font-name" | "font-family" => f.font = v.trim_matches('\'').to_string(),
                    "text-underline-style" => f.underline = v != "none",
                    "text-line-through-style" => f.strike = v != "none",
                    _ => {}
                }
            }
        }
        f
    };
    for page in d.root_element().descendants().filter(|n| n.is("page")) {
        let mut slide = Slide::new();
        for fr in page.children().filter(|n| n.is_element()) {
            let x = fr.attr("x").map(len_pt).unwrap_or(0.0) * scale;
            let y = fr.attr("y").map(len_pt).unwrap_or(0.0) * scale;
            let w = fr.attr("width").map(len_pt).unwrap_or(100.0) * scale;
            let h = fr.attr("height").map(len_pt).unwrap_or(50.0) * scale;
            // draw:transform="rotate(a) translate(x y)" rotates about the top-left corner.
            let (x, y, rot) = match fr.attr("transform").and_then(parse_transform) {
                Some((a, tx, ty)) => {
                    let (tx, ty) = (tx * scale, ty * scale);
                    let deg = (-a.to_degrees()).rem_euclid(360.0);
                    let (sn, cs) = deg.to_radians().sin_cos();
                    let (cx, cy) = (tx + w / 2.0 * cs - h / 2.0 * sn, ty + w / 2.0 * sn + h / 2.0 * cs);
                    (cx - w / 2.0, cy - h / 2.0, deg)
                }
                None => (x, y, 0.0),
            };
            let first_new = slide.shapes.len();
            let st = fr.attr("style-name").and_then(|s| styles.get(s)).cloned().unwrap_or_default();
            let class = fr.attr("class").unwrap_or("");
            let base = pres.fmt(if class == "title" { 40.0 } else { 20.0 }, class == "title");
            let base = fmt_from(fr.attr("text-style-name").or(fr.attr("style-name")), &base);
            let read_paras = |node: roxmltree::Node, base: &CharFmt| -> Vec<Para> {
                let mut out = vec![];
                let mut walk_list = vec![(node, None::<(ListKind, u8)>)];
                while let Some((n, list)) = walk_list.pop() {
                    for c in n.children().filter(|x| x.is_element()) {
                        if c.is("p") || c.is("h") {
                            let pf = fmt_from(c.attr("style-name"), base);
                            let mut p = para("", pf.clone());
                            p.runs.clear();
                            p.align = c.attr("style-name").and_then(|s| styles.get(s)).and_then(|s| s.align).or(st.align).unwrap_or(Align2::Left);
                            if let Some((k, l)) = list {
                                p.list = Some(ListInfo { kind: k, level: l, checked: false });
                            }
                            for t in c.children() {
                                if t.is_text() {
                                    p.runs.push(Run { text: t.text().unwrap_or("").to_string(), fmt: pf.clone() });
                                } else if t.is("span") {
                                    p.runs.push(Run { text: t.text_all(), fmt: fmt_from(t.attr("style-name"), &pf) });
                                } else if t.is("s") {
                                    p.runs.push(Run { text: " ".into(), fmt: pf.clone() });
                                } else if t.is("line-break") {
                                    p.runs.push(Run { text: "\n".into(), fmt: pf.clone() });
                                } else if t.is("a") {
                                    p.runs.push(Run { text: t.text_all(), fmt: pf.clone() });
                                }
                            }
                            if p.runs.is_empty() {
                                p.runs.push(Run { text: String::new(), fmt: pf });
                            }
                            p.normalize();
                            out.push(p);
                        } else if c.is("list") {
                            let lv = list.map(|l| l.1 + 1).unwrap_or(0);
                            for item in c.children().filter(|x| x.is("list-item")) {
                                walk_list.insert(0, (item, Some((ListKind::Bullet, lv))));
                            }
                        }
                    }
                }
                out
            };
            match fr.tag_name().name() {
                "frame" => {
                    if let Some(img) = fr.child("image") {
                        if let Some(data) = img.attr("href").and_then(|h| pkg.get(h.trim_start_matches("./"))) {
                            slide.shapes.push(Shape::new(Content::Picture(Arc::new(data.clone())), x, y, w, h));
                        }
                    } else if let Some(tb) = fr.child("text-box") {
                        let mut s = Shape::new(Content::Shape(Geom::Rect), x, y, w, h);
                        s.text = read_paras(tb, &base);
                        s.placeholder = match class {
                            "title" => Some(Placeholder::Title),
                            "subtitle" => Some(Placeholder::Subtitle),
                            "outline" => Some(Placeholder::Body),
                            _ => None,
                        };
                        if class == "outline" {
                            for p in &mut s.text {
                                if p.list.is_none() {
                                    p.list = Some(ListInfo { kind: ListKind::Bullet, level: 0, checked: false });
                                }
                            }
                        }
                        s.anchor = st.anchor.unwrap_or(if class == "title" { Anchor::Middle } else { Anchor::Top });
                        s.fill = st.fill.flatten();
                        s.line = st.stroke.flatten();
                        slide.shapes.push(s);
                    } else if let Some(tbl) = fr.child("table") {
                        let rows: Vec<Vec<Para>> = tbl.descendants().filter(|n| n.is("table-row")).map(|r| r.children().filter(|c| c.is("table-cell")).map(|c| read_paras(c, &pres.fmt(14.0, false)).into_iter().next().unwrap_or_else(|| para("", pres.fmt(14.0, false)))).collect()).collect();
                        if !rows.is_empty() {
                            slide.shapes.push(Shape::new(Content::Table(rows), x, y, w, h));
                        }
                    }
                }
                "custom-shape" | "rect" | "ellipse" | "line" | "connector" => {
                    let geom = match fr.tag_name().name() {
                        "ellipse" => Geom::Ellipse,
                        "line" | "connector" => Geom::Line,
                        "rect" => Geom::Rect,
                        _ => match fr.descendant("enhanced-geometry").and_then(|g| g.attr("type")).unwrap_or("rectangle") {
                            "ellipse" => Geom::Ellipse,
                            "isosceles-triangle" => Geom::Triangle,
                            "right-triangle" => Geom::RightTriangle,
                            "diamond" => Geom::Diamond,
                            "pentagon" => Geom::Pentagon,
                            "hexagon" => Geom::Hexagon,
                            "right-arrow" => Geom::RightArrow,
                            "left-arrow" => Geom::LeftArrow,
                            "up-arrow" => Geom::UpArrow,
                            "down-arrow" => Geom::DownArrow,
                            "star5" => Geom::Star5,
                            "heart" => Geom::Heart,
                            "chevron" => Geom::Chevron,
                            "cross" => Geom::Plus,
                            "round-rectangle" => Geom::RoundRect,
                            _ => Geom::Rect,
                        },
                    };
                    let (x, y, w, h) = if geom == Geom::Line {
                        let x1 = fr.attr("x1").map(len_pt).unwrap_or(0.0) * scale;
                        let y1 = fr.attr("y1").map(len_pt).unwrap_or(0.0) * scale;
                        let x2 = fr.attr("x2").map(len_pt).unwrap_or(0.0) * scale;
                        let y2 = fr.attr("y2").map(len_pt).unwrap_or(0.0) * scale;
                        (x1.min(x2), y1.min(y2), (x2 - x1).abs().max(1.0), (y2 - y1).abs().max(1.0))
                    } else {
                        (x, y, w, h)
                    };
                    let mut s = Shape::new(Content::Shape(geom), x, y, w, h);
                    s.fill = st.fill.unwrap_or(Some(pres.theme.accents[0]));
                    s.line = st.stroke.unwrap_or(Some((pres.theme.accents[0], 1.0)));
                    s.text = read_paras(fr, &CharFmt { color: [255, 255, 255], ..base.clone() });
                    s.anchor = Anchor::Middle;
                    for p in &mut s.text {
                        if p.align == Align2::Left {
                            p.align = Align2::Center;
                        }
                    }
                    slide.shapes.push(s);
                }
                "notes" => {
                    slide.notes = fr.descendants().filter(|n| n.is("p")).map(|p| p.text_all()).collect::<Vec<_>>().join("\n");
                }
                _ => {}
            }
            for s in &mut slide.shapes[first_new..] {
                s.rot = rot;
            }
        }
        if let Some(t) = page.attr("style-name").and_then(|s| d.root_element().descendants().find(|n| n.is("style") && n.attr("name") == Some(s))) {
            if let Some(dp) = t.child("drawing-page-properties") {
                if dp.attr("fill") == Some("solid") {
                    slide.background = dp.attr("fill-color").and_then(color).map(Background::Solid);
                }
                if dp.attr("transition-type").is_some() || dp.attr("type").is_some() {
                    slide.transition = Transition::Fade;
                }
            }
        }
        pres.slides.push(slide);
    }
    if pres.slides.is_empty() {
        pres.slides.push(Slide::new());
    }
    Ok(pres)
}

pub fn write(pres: &Presentation, template: bool) -> Result<Vec<u8>, String> {
    let k = 28.0 * 72.0 / 2.54 / pres.width; // points per unit for a 28 cm wide page
    let cm = |v: f32| format!("{:.3}cm", v * k / 72.0 * 2.54);
    let mut auto = String::new();
    let mut tstyles: Vec<CharFmt> = vec![];
    let mut body = String::new();
    let mut media = vec![];
    let mut tname = |f: &CharFmt, auto: &mut String| -> String {
        if let Some(i) = tstyles.iter().position(|x| x == f) {
            return format!("T{}", i + 1);
        }
        tstyles.push(f.clone());
        let n = format!("T{}", tstyles.len());
        auto.push_str(&format!(
            "<style:style style:name=\"{n}\" style:family=\"text\"><style:text-properties fo:font-size=\"{:.1}pt\" fo:font-weight=\"{}\" fo:font-style=\"{}\" fo:color=\"#{:02x}{:02x}{:02x}\" style:font-name=\"{}\" fo:font-family=\"'{}'\" style:text-underline-style=\"{}\"/></style:style>",
            f.size * k,
            if f.bold { "bold" } else { "normal" },
            if f.italic { "italic" } else { "normal" },
            f.color[0], f.color[1], f.color[2], esc(&f.font), esc(&f.font),
            if f.underline { "solid" } else { "none" }
        ));
        n
    };
    let paras = |ps: &[Para], auto: &mut String, tname: &mut dyn FnMut(&CharFmt, &mut String) -> String| -> String {
        let mut s = String::new();
        for p in ps {
            let pa = match p.align {
                Align2::Center => "PC",
                Align2::Right => "PR",
                Align2::Justify => "PJ",
                Align2::Left => "PL",
            };
            let mut inner = String::new();
            for r in &p.runs {
                let tn = tname(&r.fmt, auto);
                inner.push_str(&format!("<text:span text:style-name=\"{tn}\">{}</text:span>", esc(&r.text).replace('\n', "<text:line-break/>")));
            }
            let para = format!("<text:p text:style-name=\"{pa}\">{inner}</text:p>");
            match &p.list {
                Some(l) => {
                    let mut x = format!("<text:list-item>{para}</text:list-item>");
                    for _ in 0..l.level {
                        x = format!("<text:list-item><text:list>{x}</text:list></text:list-item>");
                    }
                    s.push_str(&format!("<text:list text:style-name=\"LB\">{x}</text:list>"));
                }
                None => s.push_str(&para),
            }
        }
        s
    };
    for (si, slide) in pres.slides.iter().enumerate() {
        let bg = match pres.background_of(slide) {
            Background::Solid(c) | Background::Gradient(c, _) => format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]),
            Background::Picture(_) => "#ffffff".into(),
        };
        auto.push_str(&format!("<style:style style:name=\"dp{si}\" style:family=\"drawing-page\"><style:drawing-page-properties draw:fill=\"solid\" draw:fill-color=\"{bg}\"{} presentation:visibility=\"{}\"/></style:style>", if slide.transition != Transition::None { " presentation:transition-type=\"automatic\" smil:type=\"fade\"" } else { "" }, if slide.hidden { "hidden" } else { "visible" }));
        body.push_str(&format!("<draw:page draw:name=\"page{}\" draw:style-name=\"dp{si}\" draw:master-page-name=\"Default\">", si + 1));
        for (i, sh) in slide.shapes.iter().enumerate() {
            let gs = format!("gr{si}_{i}");
            auto.push_str(&format!(
                "<style:style style:name=\"{gs}\" style:family=\"graphic\"><style:graphic-properties draw:fill=\"{}\"{} draw:stroke=\"{}\"{} draw:textarea-vertical-align=\"{}\"/></style:style>",
                if sh.fill.is_some() { "solid" } else { "none" },
                sh.fill.map(|c| format!(" draw:fill-color=\"#{:02x}{:02x}{:02x}\"", c[0], c[1], c[2])).unwrap_or_default(),
                if sh.line.is_some() { "solid" } else { "none" },
                sh.line.map(|(c, w)| format!(" svg:stroke-color=\"#{:02x}{:02x}{:02x}\" svg:stroke-width=\"{:.2}pt\"", c[0], c[1], c[2], w * k)).unwrap_or_default(),
                match sh.anchor {
                    Anchor::Top => "top",
                    Anchor::Middle => "middle",
                    Anchor::Bottom => "bottom",
                }
            ));
            let pos = format!("svg:x=\"{}\" svg:y=\"{}\" svg:width=\"{}\" svg:height=\"{}\"", cm(sh.x), cm(sh.y), cm(sh.w), cm(sh.h));
            let rot = if sh.rot != 0.0 {
                let (sn, cs) = sh.rot.to_radians().sin_cos();
                let (cx, cy) = sh.center();
                let (tx, ty) = (cx - sh.w / 2.0 * cs + sh.h / 2.0 * sn, cy - sh.w / 2.0 * sn - sh.h / 2.0 * cs);
                format!(" draw:transform=\"rotate({}) translate({} {})\"", -sh.rot.to_radians(), cm(tx), cm(ty))
            } else {
                String::new()
            };
            let pos = if sh.rot != 0.0 { format!("svg:width=\"{}\" svg:height=\"{}\"{rot}", cm(sh.w), cm(sh.h)) } else { pos };
            match &sh.content {
                Content::Picture(d) => {
                    let (mime, ext) = kit42::ooxml::image_mime(d);
                    let name = format!("Pictures/img{}.{ext}", media.len() + 1);
                    media.push((name.clone(), mime, d.clone()));
                    body.push_str(&format!("<draw:frame draw:style-name=\"{gs}\" {pos}><draw:image xlink:href=\"{name}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/></draw:frame>"));
                }
                Content::Table(rows) => {
                    let cols = rows.first().map(|r| r.len()).unwrap_or(1);
                    let mut t = format!("<table:table>{}", "<table:table-column/>".repeat(cols));
                    for r in rows {
                        t.push_str("<table:table-row>");
                        for c in r {
                            t.push_str(&format!("<table:table-cell>{}</table:table-cell>", paras(std::slice::from_ref(c), &mut auto, &mut tname)));
                        }
                        t.push_str("</table:table-row>");
                    }
                    t.push_str("</table:table>");
                    body.push_str(&format!("<draw:frame {pos}>{t}</draw:frame>"));
                }
                Content::Shape(g) if sh.is_line() => {
                    let _ = g;
                    body.push_str(&format!("<draw:line draw:style-name=\"{gs}\" svg:x1=\"{}\" svg:y1=\"{}\" svg:x2=\"{}\" svg:y2=\"{}\"/>", cm(sh.x), cm(sh.y), cm(sh.x + sh.w), cm(sh.y + sh.h)));
                }
                Content::Shape(g) => {
                    let text = paras(&sh.text, &mut auto, &mut tname);
                    if sh.placeholder.is_some() || (sh.fill.is_none() && *g == Geom::Rect) {
                        let class = match sh.placeholder {
                            Some(Placeholder::Title) => " presentation:class=\"title\"",
                            Some(Placeholder::Subtitle) => " presentation:class=\"subtitle\"",
                            Some(Placeholder::Body) => " presentation:class=\"outline\"",
                            None => "",
                        };
                        body.push_str(&format!("<draw:frame draw:style-name=\"{gs}\"{class} {pos}><draw:text-box>{text}</draw:text-box></draw:frame>"));
                    } else {
                        let ty = match g {
                            Geom::Ellipse => "ellipse",
                            Geom::Triangle => "isosceles-triangle",
                            Geom::RightTriangle => "right-triangle",
                            Geom::Diamond => "diamond",
                            Geom::Pentagon => "pentagon",
                            Geom::Hexagon => "hexagon",
                            Geom::RightArrow => "right-arrow",
                            Geom::LeftArrow => "left-arrow",
                            Geom::UpArrow => "up-arrow",
                            Geom::DownArrow => "down-arrow",
                            Geom::Star5 => "star5",
                            Geom::Heart => "heart",
                            Geom::Chevron => "chevron",
                            Geom::Plus => "cross",
                            Geom::RoundRect => "round-rectangle",
                            _ => "rectangle",
                        };
                        body.push_str(&format!("<draw:custom-shape draw:style-name=\"{gs}\" {pos}>{text}<draw:enhanced-geometry svg:viewBox=\"0 0 21600 21600\" draw:type=\"{ty}\"/></draw:custom-shape>"));
                    }
                }
            }
        }
        if !slide.notes.is_empty() {
            body.push_str(&format!("<presentation:notes><draw:frame presentation:class=\"notes\" svg:x=\"2cm\" svg:y=\"14cm\" svg:width=\"17cm\" svg:height=\"10cm\"><draw:text-box>{}</draw:text-box></draw:frame></presentation:notes>", slide.notes.lines().map(|l| format!("<text:p>{}</text:p>", esc(l))).collect::<String>()));
        }
        body.push_str("</draw:page>");
    }
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:smil="urn:oasis:names:tc:opendocument:xmlns:smil-compatible:1.0" office:version="1.3""#;
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ns}><office:automatic-styles><style:style style:name="PL" style:family="paragraph"><style:paragraph-properties fo:text-align="start"/></style:style><style:style style:name="PC" style:family="paragraph"><style:paragraph-properties fo:text-align="center"/></style:style><style:style style:name="PR" style:family="paragraph"><style:paragraph-properties fo:text-align="end"/></style:style><style:style style:name="PJ" style:family="paragraph"><style:paragraph-properties fo:text-align="justify"/></style:style><text:list-style style:name="LB"><text:list-level-style-bullet text:level="1" text:bullet-char="•"><style:list-level-properties text:space-before="0.3cm" text:min-label-width="0.6cm"/></text:list-level-style-bullet><text:list-level-style-bullet text:level="2" text:bullet-char="–"><style:list-level-properties text:space-before="1.2cm" text:min-label-width="0.6cm"/></text:list-level-style-bullet></text:list-style>{auto}</office:automatic-styles><office:body><office:presentation>{body}</office:presentation></office:body></office:document-content>"#
    );
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles {ns}><office:automatic-styles><style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="28cm" fo:page-height="{}" style:print-orientation="landscape"/></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Default" style:page-layout-name="PM1"/></office:master-styles></office:document-styles>"#,
        cm(pres.height)
    );
    let mime = if template { "application/vnd.oasis.opendocument.presentation-template" } else { "application/vnd.oasis.opendocument.presentation" };
    let mut manifest = format!(r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="{mime}"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/>"#);
    for (n, m, _) in &media {
        manifest.push_str(&format!("<manifest:file-entry manifest:full-path=\"{n}\" manifest:media-type=\"{m}\"/>"));
    }
    manifest.push_str("</manifest:manifest>");
    let mut w = PackageWriter::new();
    w.stored("mimetype", mime.as_bytes())?;
    w.add_str("content.xml", &content)?;
    w.add_str("styles.xml", &styles)?;
    w.add_str("META-INF/manifest.xml", &manifest)?;
    for (n, _, d) in &media {
        w.add(n, d)?;
    }
    w.finish()
}
