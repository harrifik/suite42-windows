//! Word .docx reader and writer (WordprocessingML).

use crate::model::*;
use kit42::ooxml::{esc, parse, relationships, NodeExt, Package, PackageWriter};
use kit42::richtext::{Align2, CharFmt, ListInfo, ListKind, Para, ParaStyle, Run};
use roxmltree::Node;
use std::collections::HashMap;
use std::sync::Arc;

const TW: f32 = 20.0; // twips per point

#[derive(Clone, Default)]
struct RunProps {
    font: Option<String>,
    size: Option<f32>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    color: Option<[u8; 3]>,
    highlight: Option<Option<[u8; 3]>>,
    script: Option<i8>,
}

impl RunProps {
    fn merge(&mut self, o: &RunProps) {
        macro_rules! m {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        m!(font, size, bold, italic, underline, strike, color, highlight, script);
    }

    fn apply(&self, f: &mut CharFmt) {
        if let Some(v) = &self.font {
            f.font = v.clone();
        }
        if let Some(v) = self.size {
            f.size = v;
        }
        if let Some(v) = self.bold {
            f.bold = v;
        }
        if let Some(v) = self.italic {
            f.italic = v;
        }
        if let Some(v) = self.underline {
            f.underline = v;
        }
        if let Some(v) = self.strike {
            f.strike = v;
        }
        if let Some(v) = self.color {
            f.color = v;
        }
        if let Some(v) = self.highlight {
            f.highlight = v;
        }
        if let Some(v) = self.script {
            f.script = v;
        }
    }
}

#[derive(Clone, Default)]
struct ParaProps {
    align: Option<Align2>,
    ind: [Option<f32>; 3],
    before: Option<f32>,
    after: Option<f32>,
    line: Option<f32>,
    num: Option<(String, u8)>,
    page_break: Option<bool>,
    shading: Option<[u8; 3]>,
}

impl ParaProps {
    fn merge(&mut self, o: &ParaProps) {
        if o.align.is_some() {
            self.align = o.align;
        }
        for i in 0..3 {
            if o.ind[i].is_some() {
                self.ind[i] = o.ind[i];
            }
        }
        if o.before.is_some() {
            self.before = o.before;
        }
        if o.after.is_some() {
            self.after = o.after;
        }
        if o.line.is_some() {
            self.line = o.line;
        }
        if o.num.is_some() {
            self.num = o.num.clone();
        }
        if o.page_break.is_some() {
            self.page_break = o.page_break;
        }
        if o.shading.is_some() {
            self.shading = o.shading;
        }
    }
}

struct Style {
    kind: ParaStyle,
    based_on: Option<String>,
    r: RunProps,
    p: ParaProps,
}

fn hex(s: &str) -> Option<[u8; 3]> {
    if s.len() != 6 || s.eq_ignore_ascii_case("auto") {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

fn on(n: Node) -> bool {
    !matches!(n.attr("val"), Some("0") | Some("false") | Some("none") | Some("off"))
}

fn highlight_color(name: &str) -> Option<[u8; 3]> {
    Some(match name {
        "yellow" => [255, 255, 0],
        "green" => [0, 255, 0],
        "cyan" => [0, 255, 255],
        "magenta" => [255, 0, 255],
        "blue" => [0, 0, 255],
        "red" => [255, 0, 0],
        "darkBlue" => [0, 0, 128],
        "darkCyan" => [0, 128, 128],
        "darkGreen" => [0, 128, 0],
        "darkMagenta" => [128, 0, 128],
        "darkRed" => [128, 0, 0],
        "darkYellow" => [128, 128, 0],
        "darkGray" => [128, 128, 128],
        "lightGray" => [192, 192, 192],
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        _ => return None,
    })
}

struct Ctx<'a> {
    pkg: &'a Package,
    styles: HashMap<String, Style>,
    default_r: RunProps,
    default_p: ParaProps,
    theme: (String, String),
    numbering: HashMap<String, Vec<ListKind>>,
    rels: std::collections::BTreeMap<String, (String, String)>,
}

impl<'a> Ctx<'a> {
    fn font_name(&self, rf: Node) -> Option<String> {
        if let Some(a) = rf.attr("ascii").or_else(|| rf.attr("hAnsi")).or_else(|| rf.attr("cs")) {
            return Some(a.to_string());
        }
        match rf.attr("asciiTheme").or_else(|| rf.attr("hAnsiTheme")) {
            Some(t) if t.starts_with("major") => Some(self.theme.0.clone()),
            Some(_) => Some(self.theme.1.clone()),
            None => None,
        }
    }

    fn run_props(&self, rpr: Option<Node>) -> RunProps {
        let mut r = RunProps::default();
        let Some(rpr) = rpr else { return r };
        for c in rpr.children().filter(|n| n.is_element()) {
            match c.tag_name().name() {
                "rFonts" => r.font = self.font_name(c),
                "sz" => r.size = c.attr("val").and_then(|v| v.parse::<f32>().ok()).map(|v| v / 2.0),
                "b" => r.bold = Some(on(c)),
                "i" => r.italic = Some(on(c)),
                "u" => r.underline = Some(c.attr("val").map(|v| v != "none").unwrap_or(true)),
                "strike" | "dstrike" => r.strike = Some(on(c)),
                "color" => {
                    if let Some(v) = c.attr("val").and_then(hex) {
                        r.color = Some(v);
                    }
                }
                "highlight" => r.highlight = Some(c.attr("val").and_then(highlight_color)),
                "shd" => {
                    if let Some(v) = c.attr("fill").and_then(hex) {
                        if v != [255, 255, 255] {
                            r.highlight = Some(Some(v));
                        }
                    }
                }
                "vertAlign" => {
                    r.script = Some(match c.attr("val") {
                        Some("superscript") => 1,
                        Some("subscript") => -1,
                        _ => 0,
                    })
                }
                "rStyle" => {
                    if let Some(s) = c.attr("val").and_then(|id| self.styles.get(id)) {
                        let mut base = s.r.clone();
                        base.merge(&r);
                        r = base;
                    }
                }
                _ => {}
            }
        }
        r
    }

    fn para_props(&self, ppr: Option<Node>) -> (ParaProps, Option<String>) {
        let mut p = ParaProps::default();
        let mut style = None;
        let Some(ppr) = ppr else { return (p, style) };
        for c in ppr.children().filter(|n| n.is_element()) {
            match c.tag_name().name() {
                "pStyle" => style = c.attr("val").map(|s| s.to_string()),
                "jc" => {
                    p.align = Some(match c.attr("val") {
                        Some("center") => Align2::Center,
                        Some("right") | Some("end") => Align2::Right,
                        Some("both") | Some("distribute") => Align2::Justify,
                        _ => Align2::Left,
                    })
                }
                "ind" => {
                    let g = |k: &str| c.attr(k).and_then(|v| v.parse::<f32>().ok()).map(|v| v / TW);
                    p.ind[0] = g("left").or_else(|| g("start"));
                    p.ind[1] = g("right").or_else(|| g("end"));
                    p.ind[2] = g("firstLine").or_else(|| g("hanging").map(|h| -h));
                }
                "spacing" => {
                    let g = |k: &str| c.attr(k).and_then(|v| v.parse::<f32>().ok());
                    p.before = g("before").map(|v| v / TW);
                    p.after = g("after").map(|v| v / TW);
                    if let Some(l) = g("line") {
                        p.line = Some(match c.attr("lineRule") {
                            Some("exact") | Some("atLeast") => (l / TW / 13.2).max(0.8),
                            _ => l / 240.0,
                        });
                    }
                }
                "numPr" => {
                    let id = c.child("numId").and_then(|n| n.attr("val")).unwrap_or("0").to_string();
                    let lvl = c.child("ilvl").and_then(|n| n.attr("val")).and_then(|v| v.parse().ok()).unwrap_or(0);
                    p.num = Some((id, lvl));
                }
                "pageBreakBefore" => p.page_break = Some(on(c)),
                "shd" => p.shading = c.attr("fill").and_then(hex).filter(|c| *c != [255, 255, 255]),
                _ => {}
            }
        }
        (p, style)
    }

    /// Resolved run and paragraph properties of a style (following basedOn).
    fn style_chain(&self, id: &str) -> (RunProps, ParaProps, ParaStyle) {
        let mut chain = vec![];
        let mut cur = Some(id.to_string());
        let mut guard = 0;
        while let Some(c) = cur {
            guard += 1;
            if guard > 20 {
                break;
            }
            if let Some(s) = self.styles.get(&c) {
                chain.push(s);
                cur = s.based_on.clone();
            } else {
                break;
            }
        }
        let mut r = self.default_r.clone();
        let mut p = self.default_p.clone();
        for s in chain.iter().rev() {
            r.merge(&s.r);
            p.merge(&s.p);
        }
        let kind = chain.first().map(|s| s.kind).unwrap_or(ParaStyle::Body);
        (r, p, kind)
    }
}

pub fn read(bytes: &[u8]) -> Result<Document, String> {
    let pkg = Package::read(bytes)?;
    // Main part from the package relationships.
    let main = relationships(&pkg, "")
        .values()
        .find(|(t, _)| t == "officeDocument")
        .map(|(_, p)| p.clone())
        .unwrap_or_else(|| "word/document.xml".into());
    let xml = pkg.text(&main).ok_or("word/document.xml is missing")?;
    let doc = parse(&xml)?;
    let mut ctx = Ctx {
        pkg: &pkg,
        styles: HashMap::new(),
        default_r: RunProps::default(),
        default_p: ParaProps::default(),
        theme: ("Calibri Light".into(), "Calibri".into()),
        numbering: HashMap::new(),
        rels: relationships(&pkg, &main),
    };
    // Theme fonts.
    if let Some(theme_path) = ctx.rels.values().find(|(t, _)| t == "theme").map(|(_, p)| p.clone()) {
        if let Some(tx) = pkg.text(&theme_path) {
            if let Ok(t) = parse(&tx) {
                if let Some(m) = t.root_element().descendant("majorFont").and_then(|n| n.child("latin")).and_then(|n| n.attr("typeface")) {
                    ctx.theme.0 = m.to_string();
                }
                if let Some(m) = t.root_element().descendant("minorFont").and_then(|n| n.child("latin")).and_then(|n| n.attr("typeface")) {
                    ctx.theme.1 = m.to_string();
                }
            }
        }
    }
    // Styles.
    let styles_path = ctx.rels.values().find(|(t, _)| t == "styles").map(|(_, p)| p.clone()).unwrap_or_else(|| "word/styles.xml".into());
    if let Some(sx) = pkg.text(&styles_path) {
        if let Ok(sd) = parse(&sx) {
            let root = sd.root_element();
            if let Some(dd) = root.child("docDefaults") {
                ctx.default_r = ctx.run_props(dd.child("rPrDefault").and_then(|n| n.child("rPr")));
                ctx.default_p = ctx.para_props(dd.child("pPrDefault").and_then(|n| n.child("pPr"))).0;
            }
            let mut list = vec![];
            for s in root.children_named("style") {
                let id = s.attr("styleId").unwrap_or("").to_string();
                let name = s.child("name").and_then(|n| n.attr("val")).unwrap_or("").to_string();
                let based = s.child("basedOn").and_then(|n| n.attr("val")).map(|v| v.to_string());
                let kind = {
                    let k = ParaStyle::from_docx_id(&name);
                    if k == ParaStyle::Body {
                        ParaStyle::from_docx_id(&id)
                    } else {
                        k
                    }
                };
                list.push((id, s, based, kind));
            }
            for (id, s, based, kind) in list {
                let r = ctx.run_props(s.child("rPr"));
                let p = ctx.para_props(s.child("pPr")).0;
                ctx.styles.insert(id, Style { kind, based_on: based, r, p });
            }
        }
    }
    // Numbering formats.
    let num_path = ctx.rels.values().find(|(t, _)| t == "numbering").map(|(_, p)| p.clone()).unwrap_or_else(|| "word/numbering.xml".into());
    if let Some(nx) = pkg.text(&num_path) {
        if let Ok(nd) = parse(&nx) {
            let root = nd.root_element();
            let mut abs: HashMap<String, Vec<ListKind>> = HashMap::new();
            for a in root.children_named("abstractNum") {
                let id = a.attr("abstractNumId").unwrap_or("").to_string();
                let mut kinds = vec![ListKind::Bullet; 9];
                for l in a.children_named("lvl") {
                    let i: usize = l.attr("ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
                    let fmt = l.child("numFmt").and_then(|n| n.attr("val")).unwrap_or("bullet");
                    if i < 9 {
                        kinds[i] = match fmt {
                            "decimal" | "decimalZero" => ListKind::Number,
                            "lowerLetter" => ListKind::LowerLetter,
                            "upperLetter" => ListKind::UpperLetter,
                            "lowerRoman" => ListKind::LowerRoman,
                            "upperRoman" => ListKind::UpperRoman,
                            "none" => ListKind::Bullet,
                            _ => ListKind::Bullet,
                        };
                    }
                }
                abs.insert(id, kinds);
            }
            for n in root.children_named("num") {
                let id = n.attr("numId").unwrap_or("").to_string();
                if let Some(a) = n.child("abstractNumId").and_then(|x| x.attr("val")) {
                    if let Some(k) = abs.get(a) {
                        ctx.numbering.insert(id, k.clone());
                    }
                }
            }
        }
    }
    let mut out = Document::default();
    out.blocks.clear();
    let (dr, _, _) = ctx.style_chain("Normal");
    let mut body = CharFmt::default();
    if dr.font.is_none() && ctx.default_r.font.is_none() {
        body.font = ctx.theme.1.clone();
    }
    dr.apply(&mut body);
    if dr.size.is_none() && ctx.default_r.size.is_none() {
        body.size = 10.0;
    }
    out.body = body;
    let body_node = doc.root_element().child("body").ok_or("no body")?;
    read_blocks(&ctx, body_node, &mut out, &main);
    // Section (last sectPr).
    if let Some(sp) = body_node.child("sectPr") {
        if let Some(sz) = sp.child("pgSz") {
            let g = |k: &str| sz.attr(k).and_then(|v| v.parse::<f32>().ok()).map(|v| v / TW);
            if let (Some(w), Some(h)) = (g("w"), g("h")) {
                out.setup.width = w;
                out.setup.height = h;
            }
        }
        if let Some(m) = sp.child("pgMar") {
            let g = |k: &str| m.attr(k).and_then(|v| v.parse::<f32>().ok()).map(|v| v / TW);
            out.setup.margins = [g("left").unwrap_or(72.0), g("top").unwrap_or(72.0), g("right").unwrap_or(72.0), g("bottom").unwrap_or(72.0)];
        }
        for (tag, slot) in [("headerReference", 0), ("footerReference", 1)] {
            let refs = sp.children_named(tag);
            let r = refs.iter().find(|r| r.attr("type") == Some("default")).or(refs.first());
            if let Some(id) = r.and_then(|r| r.attr("id")) {
                if let Some((_, part)) = ctx.rels.get(id) {
                    if let Some(x) = pkg.text(part) {
                        if let Ok(d) = parse(&x) {
                            let mut text = vec![];
                            for p in d.root_element().descendants_named("p") {
                                let mut s = String::new();
                                for n in p.descendants() {
                                    if n.is("t") {
                                        s.push_str(n.text().unwrap_or(""));
                                    } else if n.is("instrText") && n.text().unwrap_or("").contains("NUMPAGES") {
                                        s.push_str("{pages}");
                                    } else if n.is("instrText") && n.text().unwrap_or("").contains("PAGE") {
                                        s.push_str("{page}");
                                    } else if n.is("fldSimple") {
                                        let ins = n.attr("instr").unwrap_or("");
                                        if ins.contains("NUMPAGES") {
                                            s.push_str("{pages}");
                                        } else if ins.contains("PAGE") {
                                            s.push_str("{page}");
                                        }
                                    }
                                }
                                if !s.trim().is_empty() {
                                    text.push(s);
                                }
                            }
                            let joined = text.join(" ");
                            if slot == 0 {
                                out.setup.header = joined;
                            } else {
                                out.setup.footer = joined;
                            }
                        }
                    }
                }
            }
        }
    }
    // Core properties.
    if let Some(cx) = pkg.text("docProps/core.xml") {
        if let Ok(cd) = parse(&cx) {
            let r = cd.root_element();
            out.meta.title = r.child("title").map(|n| n.text_all()).unwrap_or_default();
            out.meta.author = r.child("creator").map(|n| n.text_all()).unwrap_or_default();
            out.meta.subject = r.child("subject").map(|n| n.text_all()).unwrap_or_default();
            out.meta.keywords = r.child("keywords").map(|n| n.text_all()).unwrap_or_default();
        }
    }
    out.normalize();
    Ok(out)
}

fn read_blocks(ctx: &Ctx, parent: Node, out: &mut Document, part: &str) {
    for n in parent.children().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "p" => read_para(ctx, n, out, part),
            "tbl" => {
                if let Some(t) = read_table(ctx, n, out, part) {
                    out.blocks.push(Block::Table(t));
                }
            }
            "sdt" => {
                if let Some(c) = n.child("sdtContent") {
                    read_blocks(ctx, c, out, part);
                }
            }
            _ => {}
        }
    }
}

fn read_table(ctx: &Ctx, n: Node, out: &mut Document, part: &str) -> Option<Table> {
    let widths: Vec<f32> = n
        .child("tblGrid")
        .map(|g| g.children_named("gridCol").iter().map(|c| c.attr("w").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1440.0) / TW).collect())
        .unwrap_or_default();
    let mut rows = vec![];
    for tr in n.children_named("tr") {
        let mut row = vec![];
        for tc in tr.children_named("tc") {
            let span: usize = tc.child("tcPr").and_then(|p| p.child("gridSpan")).and_then(|g| g.attr("val")).and_then(|v| v.parse().ok()).unwrap_or(1);
            let shading = tc.child("tcPr").and_then(|p| p.child("shd")).and_then(|s| s.attr("fill")).and_then(hex).filter(|c| *c != [255, 255, 255]);
            let mut tmp = Document { blocks: vec![], ..Document::default() };
            tmp.body = out.body.clone();
            read_blocks(ctx, tc, &mut tmp, part);
            let mut para: Option<Para> = None;
            for b in tmp.blocks {
                if let Block::Para(p) = b {
                    match &mut para {
                        None => para = Some(p),
                        Some(acc) => {
                            let f = acc.fmt_at(acc.char_len());
                            let n = acc.char_len();
                            acc.splice(n, 0, "\n", &f);
                            acc.append(p);
                        }
                    }
                }
            }
            let mut para = para.unwrap_or_else(|| Para::new("", out.body.clone()));
            para.space_after = 0.0;
            para.space_before = 0.0;
            row.push(Cell { para, shading });
            for _ in 1..span {
                row.push(Cell::new("", &out.body));
            }
        }
        if !row.is_empty() {
            rows.push(row);
        }
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0).max(widths.len());
    if cols == 0 || rows.is_empty() {
        return None;
    }
    for r in &mut rows {
        while r.len() < cols {
            r.push(Cell::new("", &out.body));
        }
    }
    let mut widths = widths;
    widths.resize(cols, out.setup.text_width() / cols as f32);
    let borders = n.child("tblPr").and_then(|p| p.child("tblBorders")).map(|b| b.children().any(|c| c.is_element() && c.attr("val") != Some("none") && c.attr("val") != Some("nil"))).unwrap_or(true);
    Some(Table { rows, widths, borders, header: false })
}

fn read_para(ctx: &Ctx, n: Node, out: &mut Document, part: &str) {
    let (pp, style_id) = ctx.para_props(n.child("pPr"));
    let (sr, sp, kind) = ctx.style_chain(style_id.as_deref().unwrap_or("Normal"));
    let mut props = sp.clone();
    props.merge(&pp);
    let mut base = out.body.clone();
    sr.apply(&mut base);
    let para_rpr = ctx.run_props(n.child("pPr").and_then(|p| p.child("rPr")));
    let mut p = Para::new("", base.clone());
    p.runs.clear();
    p.style = kind;
    p.align = props.align.unwrap_or(Align2::Left);
    p.indent_left = props.ind[0].unwrap_or(0.0);
    p.indent_right = props.ind[1].unwrap_or(0.0);
    p.indent_first = props.ind[2].unwrap_or(0.0);
    p.space_before = props.before.unwrap_or(0.0);
    p.space_after = props.after.unwrap_or(0.0);
    p.line_spacing = props.line.unwrap_or(1.0);
    p.page_break_before = props.page_break.unwrap_or(false);
    p.shading = props.shading;
    if let Some((id, lvl)) = &props.num {
        if id != "0" {
            let kind = ctx.numbering.get(id).and_then(|k| k.get(*lvl as usize).cloned()).unwrap_or(ListKind::Bullet);
            p.list = Some(ListInfo { kind, level: *lvl, checked: false });
            p.indent_left = (p.indent_left - 18.0 * (*lvl as f32 + 1.0)).max(0.0);
        }
    }
    let mut pictures = vec![];
    let mut page_break_after = false;
    collect_runs(ctx, n, &base, &sr, None, &mut p.runs, &mut pictures, &mut page_break_after, part, &mut false);
    if p.runs.is_empty() {
        let mut f = base.clone();
        para_rpr.apply(&mut f);
        p.runs.push(Run { text: String::new(), fmt: f });
    }
    p.normalize();
    // A paragraph that is only a picture becomes a picture block.
    let only_pic = p.is_empty() && !pictures.is_empty();
    if !only_pic {
        out.blocks.push(Block::Para(p.clone()));
    }
    for mut pic in pictures {
        pic.align = if only_pic { p.align } else { Align2::Center };
        out.blocks.push(Block::Picture(pic));
    }
    if page_break_after {
        out.blocks.push(Block::PageBreak);
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_runs(
    ctx: &Ctx,
    node: Node,
    base: &CharFmt,
    _style_r: &RunProps,
    link: Option<String>,
    runs: &mut Vec<Run>,
    pics: &mut Vec<Picture>,
    page_break: &mut bool,
    part: &str,
    in_field: &mut bool,
) {
    for c in node.children().filter(|n| n.is_element()) {
        match c.tag_name().name() {
            "r" => {
                let rp = ctx.run_props(c.child("rPr"));
                let mut f = base.clone();
                rp.apply(&mut f);
                f.link = link.clone();
                for t in c.children().filter(|n| n.is_element()) {
                    match t.tag_name().name() {
                        "t" if !*in_field => runs.push(Run { text: t.text().unwrap_or("").to_string(), fmt: f.clone() }),
                        "tab" => runs.push(Run { text: "\t".into(), fmt: f.clone() }),
                        "br" => {
                            if t.attr("type") == Some("page") {
                                *page_break = true;
                            } else {
                                runs.push(Run { text: "\n".into(), fmt: f.clone() });
                            }
                        }
                        "fldChar" => match t.attr("fldCharType") {
                            Some("begin") => *in_field = false,
                            Some("separate") => *in_field = false,
                            _ => {}
                        },
                        "drawing" | "pict" | "object" => {
                            if let Some(pic) = read_picture(ctx, t) {
                                pics.push(pic);
                            }
                        }
                        "sym" => {
                            if let Some(ch) = t.attr("char").and_then(|v| u32::from_str_radix(v, 16).ok()).and_then(char::from_u32) {
                                runs.push(Run { text: ch.to_string(), fmt: f.clone() });
                            }
                        }
                        "noBreakHyphen" => runs.push(Run { text: "‑".into(), fmt: f.clone() }),
                        _ => {}
                    }
                }
            }
            "hyperlink" => {
                let target = c
                    .attr("id")
                    .and_then(|id| ctx.rels.get(id))
                    .map(|(_, t)| t.clone())
                    .or_else(|| c.attr("anchor").map(|a| format!("#{a}")));
                collect_runs(ctx, c, base, _style_r, target, runs, pics, page_break, part, in_field);
            }
            "ins" | "smartTag" | "fldSimple" | "customXml" | "sdtContent" | "bdo" | "dir" => {
                collect_runs(ctx, c, base, _style_r, link.clone(), runs, pics, page_break, part, in_field)
            }
            "sdt" => {
                if let Some(cc) = c.child("sdtContent") {
                    collect_runs(ctx, cc, base, _style_r, link.clone(), runs, pics, page_break, part, in_field);
                }
            }
            _ => {}
        }
    }
}

fn read_picture(ctx: &Ctx, n: Node) -> Option<Picture> {
    let blip = n.descendant("blip").and_then(|b| b.attr("embed")).or_else(|| n.descendant("imagedata").and_then(|i| i.attr("id")))?;
    let (_, path) = ctx.rels.get(blip)?;
    let data = ctx.pkg.get(path)?.clone();
    let mut pic = Picture::new(data, 10000.0)?;
    if let Some(ext) = n.descendant("extent") {
        let cx = ext.attr("cx").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) / 12700.0;
        let cy = ext.attr("cy").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) / 12700.0;
        if cx > 0.0 && cy > 0.0 {
            pic.width = cx;
            pic.height = cy;
        }
    }
    pic.alt = n.descendant("docPr").and_then(|d| d.attr("descr")).unwrap_or("").to_string();
    Some(pic)
}

// ─────────────────────────── writer ───────────────────────────

fn tw(v: f32) -> i64 {
    (v * TW).round() as i64
}

fn rpr(f: &CharFmt) -> String {
    let mut s = String::from("<w:rPr>");
    s.push_str(&format!("<w:rFonts w:ascii=\"{0}\" w:hAnsi=\"{0}\" w:cs=\"{0}\" w:eastAsia=\"{0}\"/>", esc(&f.font)));
    if f.bold {
        s.push_str("<w:b/><w:bCs/>");
    }
    if f.italic {
        s.push_str("<w:i/><w:iCs/>");
    }
    if f.strike {
        s.push_str("<w:strike/>");
    }
    s.push_str(&format!("<w:color w:val=\"{:02X}{:02X}{:02X}\"/>", f.color[0], f.color[1], f.color[2]));
    s.push_str(&format!("<w:sz w:val=\"{0}\"/><w:szCs w:val=\"{0}\"/>", (f.size * 2.0).round() as i64));
    if let Some(h) = f.highlight {
        s.push_str(&format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{:02X}{:02X}{:02X}\"/>", h[0], h[1], h[2]));
    }
    if f.underline || f.link.is_some() {
        s.push_str("<w:u w:val=\"single\"/>");
    }
    match f.script {
        1 => s.push_str("<w:vertAlign w:val=\"superscript\"/>"),
        -1 => s.push_str("<w:vertAlign w:val=\"subscript\"/>"),
        _ => {}
    }
    s.push_str("</w:rPr>");
    s
}

fn run_xml(text: &str, f: &CharFmt) -> String {
    let mut s = String::new();
    s.push_str("<w:r>");
    s.push_str(&rpr(f));
    let mut buf = String::new();
    let flush = |buf: &mut String, s: &mut String| {
        if !buf.is_empty() {
            s.push_str(&format!("<w:t xml:space=\"preserve\">{}</w:t>", esc(buf)));
            buf.clear();
        }
    };
    for ch in text.chars() {
        match ch {
            '\t' => {
                flush(&mut buf, &mut s);
                s.push_str("<w:tab/>");
            }
            '\n' => {
                flush(&mut buf, &mut s);
                s.push_str("<w:br/>");
            }
            c => buf.push(c),
        }
    }
    flush(&mut buf, &mut s);
    s.push_str("</w:r>");
    s
}

struct Writer {
    rels: Vec<(String, String, String, bool)>,
    media: Vec<(String, Arc<Vec<u8>>)>,
    pic_n: usize,
}

impl Writer {
    fn rel(&mut self, ty: &str, target: &str, external: bool) -> String {
        if let Some((id, ..)) = self.rels.iter().find(|(_, t, tg, _)| t == ty && tg == target) {
            return id.clone();
        }
        let id = format!("rId{}", self.rels.len() + 10);
        self.rels.push((id.clone(), ty.into(), target.into(), external));
        id
    }

    fn para(&mut self, p: &Para) -> String {
        let mut s = String::from("<w:p><w:pPr>");
        s.push_str(&format!("<w:pStyle w:val=\"{}\"/>", p.style.docx_id()));
        if p.page_break_before {
            s.push_str("<w:pageBreakBefore/>");
        }
        if let Some(l) = &p.list {
            let num = match l.kind {
                ListKind::Bullet | ListKind::Check => 1,
                ListKind::Number => 2,
                ListKind::LowerLetter => 3,
                ListKind::UpperLetter => 4,
                ListKind::LowerRoman => 5,
                ListKind::UpperRoman => 6,
            };
            s.push_str(&format!("<w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{num}\"/></w:numPr>", l.level));
        }
        if let Some(sh) = p.shading {
            s.push_str(&format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{:02X}{:02X}{:02X}\"/>", sh[0], sh[1], sh[2]));
        }
        s.push_str(&format!(
            "<w:spacing w:before=\"{}\" w:after=\"{}\" w:line=\"{}\" w:lineRule=\"auto\"/>",
            tw(p.space_before),
            tw(p.space_after),
            (p.line_spacing * 240.0).round() as i64
        ));
        let list_indent = p.list.as_ref().map(|l| 18.0 * (l.level as f32 + 1.0)).unwrap_or(0.0);
        let first = if p.list.is_some() { format!(" w:hanging=\"{}\"", tw(18.0)) } else if p.indent_first >= 0.0 { format!(" w:firstLine=\"{}\"", tw(p.indent_first)) } else { format!(" w:hanging=\"{}\"", tw(-p.indent_first)) };
        s.push_str(&format!("<w:ind w:left=\"{}\" w:right=\"{}\"{first}/>", tw(p.indent_left + list_indent), tw(p.indent_right)));
        s.push_str(match p.align {
            Align2::Left => "",
            Align2::Center => "<w:jc w:val=\"center\"/>",
            Align2::Right => "<w:jc w:val=\"right\"/>",
            Align2::Justify => "<w:jc w:val=\"both\"/>",
        });
        if let Some(r) = p.runs.first() {
            s.push_str(&rpr(&r.fmt));
        }
        s.push_str("</w:pPr>");
        for r in &p.runs {
            if r.text.is_empty() {
                continue;
            }
            if let Some(link) = &r.fmt.link {
                if let Some(anchor) = link.strip_prefix('#') {
                    s.push_str(&format!("<w:hyperlink w:anchor=\"{}\">{}</w:hyperlink>", esc(anchor), run_xml(&r.text, &r.fmt)));
                } else {
                    let id = self.rel("hyperlink", link, true);
                    s.push_str(&format!("<w:hyperlink r:id=\"{id}\">{}</w:hyperlink>", run_xml(&r.text, &r.fmt)));
                }
            } else {
                s.push_str(&run_xml(&r.text, &r.fmt));
            }
        }
        s.push_str("</w:p>");
        s
    }

    fn picture(&mut self, pic: &Picture) -> String {
        self.pic_n += 1;
        let (_, ext) = kit42::ooxml::image_mime(&pic.data);
        let name = format!("media/image{}.{}", self.pic_n, ext);
        self.media.push((name.clone(), pic.data.clone()));
        let id = self.rel("image", &name, false);
        let (cx, cy) = ((pic.width * 12700.0) as i64, (pic.height * 12700.0) as i64);
        let jc = match pic.align {
            Align2::Center => "<w:jc w:val=\"center\"/>",
            Align2::Right => "<w:jc w:val=\"right\"/>",
            _ => "",
        };
        format!(
            r#"<w:p><w:pPr><w:spacing w:after="120"/>{jc}</w:pPr><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{n}" name="Picture {n}" descr="{alt}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{n}" name="image{n}.{ext}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{id}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
            n = self.pic_n,
            alt = esc(&pic.alt)
        )
    }

    fn table(&mut self, t: &Table) -> String {
        let mut s = String::from("<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/>");
        if t.borders {
            s.push_str("<w:tblBorders>");
            for b in ["top", "left", "bottom", "right", "insideH", "insideV"] {
                s.push_str(&format!("<w:{b} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"808080\"/>"));
            }
            s.push_str("</w:tblBorders>");
        } else {
            s.push_str("<w:tblBorders><w:top w:val=\"nil\"/><w:left w:val=\"nil\"/><w:bottom w:val=\"nil\"/><w:right w:val=\"nil\"/><w:insideH w:val=\"nil\"/><w:insideV w:val=\"nil\"/></w:tblBorders>");
        }
        s.push_str("<w:tblLayout w:type=\"fixed\"/></w:tblPr><w:tblGrid>");
        for w in &t.widths {
            s.push_str(&format!("<w:gridCol w:w=\"{}\"/>", tw(*w)));
        }
        s.push_str("</w:tblGrid>");
        for (ri, row) in t.rows.iter().enumerate() {
            s.push_str("<w:tr>");
            if ri == 0 && t.header {
                s.push_str("<w:trPr><w:tblHeader/></w:trPr>");
            }
            for (ci, c) in row.iter().enumerate() {
                s.push_str(&format!("<w:tc><w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/>", tw(t.widths.get(ci).cloned().unwrap_or(72.0))));
                if let Some(sh) = c.shading {
                    s.push_str(&format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{:02X}{:02X}{:02X}\"/>", sh[0], sh[1], sh[2]));
                }
                s.push_str("</w:tcPr>");
                // Line breaks inside a cell become separate paragraphs.
                let mut p = c.para.clone();
                let text = p.text();
                if text.contains('\n') {
                    let mut parts = vec![];
                    let mut rest = p.clone();
                    loop {
                        let t2 = rest.text();
                        match t2.chars().position(|ch| ch == '\n') {
                            Some(pos) => {
                                let mut tail = rest.split_off(pos);
                                tail.splice(0, 1, "", &tail.fmt_at(0));
                                parts.push(rest);
                                rest = tail;
                            }
                            None => {
                                parts.push(rest);
                                break;
                            }
                        }
                    }
                    for part in parts {
                        s.push_str(&self.para(&part));
                    }
                } else {
                    p.space_after = 0.0;
                    s.push_str(&self.para(&p));
                }
                s.push_str("</w:tc>");
            }
            s.push_str("</w:tr>");
        }
        s.push_str("</w:tbl>");
        s
    }
}

fn header_footer(tag: &str, text: &str) -> String {
    // {page} and {pages} become PAGE / NUMPAGES fields.
    let mut runs = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let next = [("{page}", "PAGE"), ("{pages}", "NUMPAGES")].iter().filter_map(|(k, f)| rest.find(k).map(|i| (i, *k, *f))).min_by_key(|x| x.0);
        match next {
            Some((i, k, f)) => {
                if i > 0 {
                    runs.push_str(&format!("<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(&rest[..i])));
                }
                runs.push_str(&format!("<w:fldSimple w:instr=\" {f} \"><w:r><w:t>1</w:t></w:r></w:fldSimple>"));
                rest = &rest[i + k.len()..];
            }
            None => {
                runs.push_str(&format!("<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(rest)));
                break;
            }
        }
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{tag} xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:p><w:pPr><w:jc w:val="center"/></w:pPr>{runs}</w:p></w:{tag}>"#
    )
}

fn styles_xml(body: &CharFmt) -> String {
    let mut s = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#);
    s.push_str(&format!(
        r#"<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{0}" w:hAnsi="{0}" w:cs="{0}" w:eastAsia="{0}"/><w:sz w:val="{1}"/><w:szCs w:val="{1}"/><w:lang w:val="ru-RU"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>"#,
        esc(&body.font),
        (body.size * 2.0) as i64
    ));
    for st in ParaStyle::ALL {
        let f = st.base_fmt(body);
        let id = st.docx_id();
        let name = match st {
            ParaStyle::Body => "Normal".to_string(),
            ParaStyle::Heading(n) => format!("heading {n}"),
            other => other.docx_id(),
        };
        let (b, a) = st.spacing();
        let outline = match st {
            ParaStyle::Heading(n) => format!("<w:outlineLvl w:val=\"{}\"/>", n - 1),
            _ => String::new(),
        };
        s.push_str(&format!(
            r#"<w:style w:type="paragraph" w:styleId="{id}"{def}><w:name w:val="{name}"/>{based}<w:qFormat/><w:pPr><w:keepNext w:val="{keep}"/><w:spacing w:before="{b}" w:after="{a}"/>{outline}</w:pPr>{rpr}</w:style>"#,
            def = if st == ParaStyle::Body { " w:default=\"1\"" } else { "" },
            based = if st == ParaStyle::Body { String::new() } else { "<w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/>".into() },
            keep = matches!(st, ParaStyle::Heading(_) | ParaStyle::Title) as u8,
            b = tw(b),
            a = tw(a),
            rpr = rpr(&f)
        ));
    }
    s.push_str(r#"<w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders><w:tblCellMar><w:left w:w="108" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>"#);
    s.push_str("</w:styles>");
    s
}

fn numbering_xml() -> String {
    let mut s = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#);
    let kinds = [("bullet", ""), ("decimal", ""), ("lowerLetter", ""), ("upperLetter", ""), ("lowerRoman", ""), ("upperRoman", "")];
    for (i, (fmt, _)) in kinds.iter().enumerate() {
        s.push_str(&format!("<w:abstractNum w:abstractNumId=\"{i}\"><w:multiLevelType w:val=\"hybridMultilevel\"/>"));
        for l in 0..9 {
            let (f, text, font) = if *fmt == "bullet" {
                let b = ["•", "o", "▪"][l % 3];
                ("bullet", b.to_string(), if l % 3 == 1 { "<w:rPr><w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\"/></w:rPr>" } else { "" })
            } else {
                (*fmt, format!("%{}.", l + 1), "")
            };
            s.push_str(&format!(
                "<w:lvl w:ilvl=\"{l}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{f}\"/><w:lvlText w:val=\"{}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{}\" w:hanging=\"360\"/></w:pPr>{font}</w:lvl>",
                esc(&text),
                360 * (l + 1)
            ));
        }
        s.push_str("</w:abstractNum>");
    }
    for i in 0..kinds.len() {
        s.push_str(&format!("<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{i}\"/></w:num>", i + 1));
    }
    s.push_str("</w:numbering>");
    s
}

pub fn write(doc: &Document) -> Result<Vec<u8>, String> {
    let mut w = Writer { rels: vec![], media: vec![], pic_n: 0 };
    let mut body = String::new();
    for b in &doc.blocks {
        match b {
            Block::Para(p) => body.push_str(&w.para(p)),
            Block::Table(t) => {
                body.push_str(&w.table(t));
            }
            Block::Picture(pic) => body.push_str(&w.picture(pic)),
            Block::PageBreak => body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"),
        }
    }
    let s = &doc.setup;
    let mut hf = String::new();
    let has_header = !s.header.trim().is_empty();
    let has_footer = !s.footer.trim().is_empty();
    if has_header {
        let id = w.rel("header", "header1.xml", false);
        hf.push_str(&format!("<w:headerReference w:type=\"default\" r:id=\"{id}\"/>"));
    }
    if has_footer {
        let id = w.rel("footer", "footer1.xml", false);
        hf.push_str(&format!("<w:footerReference w:type=\"default\" r:id=\"{id}\"/>"));
    }
    let orient = if s.width > s.height { " w:orient=\"landscape\"" } else { "" };
    body.push_str(&format!(
        "<w:sectPr>{hf}<w:pgSz w:w=\"{}\" w:h=\"{}\"{orient}/><w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/>{}</w:sectPr>",
        tw(s.width), tw(s.height), tw(s.margins[1]), tw(s.margins[2]), tw(s.margins[3]), tw(s.margins[0]),
        if s.first_page_different { "<w:titlePg/>" } else { "" }
    ));
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>{body}</w:body></w:document>"#
    );
    let mut rels = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/>"#);
    for (id, ty, target, ext) in &w.rels {
        rels.push_str(&format!(
            "<Relationship Id=\"{id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/{ty}\" Target=\"{}\"{}/>",
            esc(target),
            if *ext { " TargetMode=\"External\"" } else { "" }
        ));
    }
    rels.push_str("</Relationships>");
    let mut ct = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="jpeg" ContentType="image/jpeg"/><Default Extension="gif" ContentType="image/gif"/><Default Extension="bmp" ContentType="image/bmp"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#);
    if has_header {
        ct.push_str(r#"<Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/>"#);
    }
    if has_footer {
        ct.push_str(r#"<Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/>"#);
    }
    ct.push_str("</Types>");
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let core = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title><dc:subject>{}</dc:subject><dc:creator>{}</dc:creator><cp:keywords>{}</cp:keywords><dcterms:created xsi:type="dcterms:W3CDTF">{now}</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">{now}</dcterms:modified></cp:coreProperties>"#,
        esc(&doc.meta.title), esc(&doc.meta.subject), esc(&doc.meta.author), esc(&doc.meta.keywords)
    );
    let mut pkg = PackageWriter::new();
    pkg.add_str("[Content_Types].xml", &ct)?;
    pkg.add_str("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/></Relationships>"#)?;
    pkg.add_str("word/document.xml", &document)?;
    pkg.add_str("word/_rels/document.xml.rels", &rels)?;
    pkg.add_str("word/styles.xml", &styles_xml(&doc.body))?;
    pkg.add_str("word/numbering.xml", &numbering_xml())?;
    pkg.add_str("word/settings.xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:defaultTabStop w:val="708"/><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"#)?;
    pkg.add_str("docProps/core.xml", &core)?;
    pkg.add_str("docProps/app.xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>DOC42 for Windows</Application></Properties>"#)?;
    if has_header {
        pkg.add_str("word/header1.xml", &header_footer("hdr", &s.header))?;
    }
    if has_footer {
        pkg.add_str("word/footer1.xml", &header_footer("ftr", &s.footer))?;
    }
    for (name, data) in &w.media {
        pkg.add(&format!("word/{name}"), data)?;
    }
    pkg.finish()
}
