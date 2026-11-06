//! PowerPoint .pptx / .potx reader and writer (PresentationML).

use crate::model::*;
use kit42::ooxml::{esc, parse, relationships, NodeExt, Package, PackageWriter};
use kit42::richtext::{Align2, CharFmt, ListInfo, ListKind, Para, Run};
use roxmltree::Node;
use std::collections::HashMap;
use std::sync::Arc;

const EMU: f32 = 12700.0; // EMU per point

struct Ctx<'a> {
    pkg: &'a Package,
    theme: HashMap<String, [u8; 3]>,
    major: String,
    minor: String,
}

fn hex(s: &str) -> Option<[u8; 3]> {
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

impl<'a> Ctx<'a> {
    fn color(&self, n: Node) -> Option<[u8; 3]> {
        let c = n.children().find(|c| c.is_element())?;
        let mut base = match c.tag_name().name() {
            "srgbClr" => hex(c.attr("val")?)?,
            "schemeClr" => {
                let v = c.attr("val")?;
                let key = match v {
                    "tx1" => "dk1",
                    "bg1" => "lt1",
                    "tx2" => "dk2",
                    "bg2" => "lt2",
                    o => o,
                };
                *self.theme.get(key)?
            }
            "sysClr" => hex(c.attr("lastClr").unwrap_or("000000"))?,
            "prstClr" => match c.attr("val")? {
                "white" => [255, 255, 255],
                "red" => [255, 0, 0],
                "blue" => [0, 0, 255],
                "green" => [0, 128, 0],
                "yellow" => [255, 255, 0],
                _ => [0, 0, 0],
            },
            _ => return None,
        };
        for m in c.children().filter(|x| x.is_element()) {
            let v = m.attr("val").and_then(|v| v.parse::<f32>().ok()).unwrap_or(100000.0) / 100000.0;
            match m.tag_name().name() {
                "lumMod" => base = base.map(|x| (x as f32 * v).min(255.0) as u8),
                "lumOff" => base = base.map(|x| (x as f32 + 255.0 * v).min(255.0) as u8),
                "tint" => base = base.map(|x| (255.0 - (255.0 - x as f32) * v) as u8),
                "shade" => base = base.map(|x| (x as f32 * v) as u8),
                _ => {}
            }
        }
        Some(base)
    }

    fn font(&self, n: Node) -> Option<String> {
        let t = n.child("latin").and_then(|l| l.attr("typeface")).or_else(|| n.child("cs").and_then(|l| l.attr("typeface")))?;
        Some(match t {
            "+mj-lt" => self.major.clone(),
            "+mn-lt" => self.minor.clone(),
            o => o.to_string(),
        })
    }
}

#[derive(Clone, Default)]
struct PhInfo {
    xfrm: Option<(f32, f32, f32, f32)>,
    size: Option<f32>,
    color: Option<[u8; 3]>,
    anchor: Option<Anchor>,
    align: Option<Align2>,
    bold: Option<bool>,
}

fn xfrm(n: Node) -> Option<(f32, f32, f32, f32, f32, bool, bool)> {
    let x = n.descendant("xfrm")?;
    let off = x.child("off")?;
    let ext = x.child("ext")?;
    let g = |n: Node, k: &str| n.attr(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) / EMU;
    let rot = x.attr("rot").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) / 60000.0;
    Some((g(off, "x"), g(off, "y"), g(ext, "cx"), g(ext, "cy"), rot, x.attr("flipH") == Some("1"), x.attr("flipV") == Some("1")))
}

fn ph_key(n: Node) -> Option<(String, String)> {
    let ph = n.descendant("ph")?;
    let ty = ph.attr("type").unwrap_or("body").to_string();
    let ty = match ty.as_str() {
        "ctrTitle" => "title".to_string(),
        o => o.to_string(),
    };
    Some((ty, ph.attr("idx").unwrap_or("").to_string()))
}

fn collect_ph(ctx: &Ctx, part: &str) -> HashMap<(String, String), PhInfo> {
    let mut out = HashMap::new();
    let Some(x) = ctx.pkg.text(part) else { return out };
    let Ok(d) = parse(&x) else { return out };
    for sp in d.root_element().descendants().filter(|n| n.is("sp")) {
        let Some(key) = ph_key(sp) else { continue };
        let mut info = PhInfo { xfrm: xfrm(sp).map(|v| (v.0, v.1, v.2, v.3)), ..Default::default() };
        if let Some(bp) = sp.descendant("bodyPr") {
            info.anchor = bp.attr("anchor").map(|a| match a {
                "ctr" => Anchor::Middle,
                "b" => Anchor::Bottom,
                _ => Anchor::Top,
            });
        }
        if let Some(rpr) = sp.descendant("lvl1pPr").and_then(|l| l.child("defRPr")).or_else(|| sp.descendant("defRPr")) {
            info.size = rpr.attr("sz").and_then(|v| v.parse::<f32>().ok()).map(|v| v / 100.0);
            info.bold = rpr.attr("b").map(|v| v == "1");
            info.color = rpr.child("solidFill").and_then(|f| ctx.color(f));
        }
        if let Some(pp) = sp.descendant("lvl1pPr") {
            info.align = pp.attr("algn").map(algn);
        }
        out.insert(key.clone(), info.clone());
        out.entry((key.0, String::new())).or_insert(info);
    }
    out
}

fn algn(a: &str) -> Align2 {
    match a {
        "ctr" => Align2::Center,
        "r" => Align2::Right,
        "just" | "dist" => Align2::Justify,
        _ => Align2::Left,
    }
}

fn read_text(ctx: &Ctx, body: Node, base: &CharFmt, ph: &PhInfo, is_title: bool) -> Vec<Para> {
    let mut out = vec![];
    let lst_size = |lvl: u8| -> Option<f32> {
        body.child("lstStyle")?.child(&format!("lvl{}pPr", lvl + 1))?.child("defRPr")?.attr("sz")?.parse::<f32>().ok().map(|v| v / 100.0)
    };
    for p in body.children_named("p") {
        let ppr = p.child("pPr");
        let lvl: u8 = ppr.and_then(|x| x.attr("lvl")).and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut f = base.clone();
        if let Some(sz) = lst_size(lvl).or(ph.size) {
            f.size = sz;
        } else if !is_title && lvl > 0 {
            f.size = (f.size - 4.0 * lvl as f32).max(12.0);
        }
        if let Some(c) = ph.color {
            f.color = c;
        }
        if let Some(b) = ph.bold {
            f.bold = b;
        }
        if let Some(dr) = p.child("endParaRPr").and_then(|e| e.attr("sz")).and_then(|v| v.parse::<f32>().ok()) {
            if p.children_named("r").is_empty() {
                f.size = dr / 100.0;
            }
        }
        let mut para = Para::new("", f.clone());
        para.runs.clear();
        para.space_after = 4.0;
        para.line_spacing = 1.0;
        para.align = ppr.and_then(|x| x.attr("algn")).map(algn).or(ph.align).unwrap_or(Align2::Left);
        if let Some(ppr) = ppr {
            if let Some(pct) = ppr.child("lnSpc").and_then(|l| l.child("spcPct")).and_then(|s| s.attr("val")).and_then(|v| v.parse::<f32>().ok()) {
                para.line_spacing = pct / 100000.0;
            }
            if ppr.child("buNone").is_none() {
                if ppr.child("buAutoNum").is_some() {
                    para.list = Some(ListInfo { kind: ListKind::Number, level: lvl, checked: false });
                } else if ppr.child("buChar").is_some() {
                    para.list = Some(ListInfo { kind: ListKind::Bullet, level: lvl, checked: false });
                }
            }
        }
        // Body placeholders are bulleted unless buNone.
        if para.list.is_none() && ph.xfrm.is_some() && !is_title && ppr.map(|x| x.child("buNone").is_none()).unwrap_or(true) && ph.size.is_none() && false {
            para.list = Some(ListInfo { kind: ListKind::Bullet, level: lvl, checked: false });
        }
        for r in p.children().filter(|n| n.is("r") || n.is("br") || n.is("fld")) {
            if r.is("br") {
                para.runs.push(Run { text: "\n".into(), fmt: f.clone() });
                continue;
            }
            let mut rf = f.clone();
            if let Some(rpr) = r.child("rPr") {
                if let Some(sz) = rpr.attr("sz").and_then(|v| v.parse::<f32>().ok()) {
                    rf.size = sz / 100.0;
                }
                if let Some(b) = rpr.attr("b") {
                    rf.bold = b == "1";
                }
                if let Some(i) = rpr.attr("i") {
                    rf.italic = i == "1";
                }
                if let Some(u) = rpr.attr("u") {
                    rf.underline = u != "none";
                }
                if let Some(s) = rpr.attr("strike") {
                    rf.strike = s != "noStrike";
                }
                if let Some(c) = rpr.child("solidFill").and_then(|n| ctx.color(n)) {
                    rf.color = c;
                }
                if let Some(fnt) = ctx.font(rpr) {
                    rf.font = fnt;
                }
                if let Some(b) = rpr.attr("baseline").and_then(|v| v.parse::<i32>().ok()) {
                    rf.script = b.signum() as i8;
                }
            }
            let text = r.child("t").map(|t| t.text_all()).unwrap_or_default();
            para.runs.push(Run { text, fmt: rf });
        }
        if para.runs.is_empty() {
            para.runs.push(Run { text: String::new(), fmt: f.clone() });
        }
        para.normalize();
        out.push(para);
    }
    out
}

fn fill_of(ctx: &Ctx, sppr: Option<Node>) -> (Option<Option<[u8; 3]>>, Option<Option<([u8; 3], f32)>>) {
    let Some(sppr) = sppr else { return (None, None) };
    let fill = if sppr.child("noFill").is_some() {
        Some(None)
    } else if let Some(sf) = sppr.child("solidFill") {
        Some(ctx.color(sf))
    } else if let Some(gf) = sppr.child("gradFill") {
        Some(gf.descendant("gs").and_then(|g| ctx.color(g)))
    } else {
        None
    };
    let line = sppr.child("ln").map(|ln| {
        if ln.child("noFill").is_some() {
            None
        } else {
            let w = ln.attr("w").and_then(|v| v.parse::<f32>().ok()).map(|v| v / EMU).unwrap_or(1.0);
            ln.child("solidFill").and_then(|f| ctx.color(f)).map(|c| (c, w))
        }
    });
    (fill, line)
}

/// Shapes from a spTree, applying group transforms.
fn read_tree(ctx: &Ctx, tree: Node, rels: &std::collections::BTreeMap<String, (String, String)>, layout_ph: &HashMap<(String, String), PhInfo>, master_ph: &HashMap<(String, String), PhInfo>, pres_fmt: &dyn Fn(f32, bool) -> CharFmt, out: &mut Vec<Shape>, tf: (f32, f32, f32, f32)) {
    let (ox, oy, sx, sy) = tf;
    let map = |x: f32, y: f32, w: f32, h: f32| (ox + x * sx, oy + y * sy, w * sx, h * sy);
    for n in tree.children().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "sp" | "cxnSp" => {
                let key = ph_key(n);
                let inherited = key.as_ref().map(|k| {
                    let mut info = master_ph.get(k).or_else(|| master_ph.get(&(k.0.clone(), String::new()))).cloned().unwrap_or_default();
                    if let Some(l) = layout_ph.get(k).or_else(|| layout_ph.get(&(k.0.clone(), String::new()))) {
                        if l.xfrm.is_some() {
                            info.xfrm = l.xfrm;
                        }
                        if l.size.is_some() {
                            info.size = l.size;
                        }
                        if l.anchor.is_some() {
                            info.anchor = l.anchor;
                        }
                        if l.align.is_some() {
                            info.align = l.align;
                        }
                        if l.color.is_some() {
                            info.color = l.color;
                        }
                    }
                    info
                });
                let x = xfrm(n).or_else(|| inherited.as_ref().and_then(|i| i.xfrm).map(|(a, b, c, d)| (a, b, c, d, 0.0, false, false)));
                let Some((x0, y0, w, h, rot, fh, fv)) = x else { continue };
                let (x0, y0, w, h) = map(x0, y0, w, h);
                let prst = n.descendant("prstGeom").and_then(|g| g.attr("prst")).unwrap_or(if n.is("cxnSp") { "line" } else { "rect" });
                let mut geom = Geom::from_prst(prst);
                if n.is("cxnSp") || prst.contains("Connector") || prst == "line" {
                    let tail = n.descendant("tailEnd").and_then(|t| t.attr("type")).map(|t| t != "none").unwrap_or(false);
                    geom = if tail { Geom::Arrow } else { Geom::Line };
                }
                let mut s = Shape::new(Content::Shape(geom), x0, y0, w, h);
                s.rot = rot;
                s.flip_h = fh;
                s.flip_v = fv;
                s.name = n.descendant("cNvPr").and_then(|c| c.attr("name")).unwrap_or("").to_string();
                let (fill, line) = fill_of(ctx, n.child("spPr"));
                // Style references (p:style) give default fill/line for drawn shapes.
                let style_fill = n.child("style").and_then(|st| st.child("fillRef")).and_then(|f| ctx.color(f));
                let style_line = n.child("style").and_then(|st| st.child("lnRef")).and_then(|f| ctx.color(f));
                s.fill = match fill {
                    Some(f) => f,
                    None => if key.is_none() { style_fill } else { None },
                };
                s.line = match line {
                    Some(l) => l,
                    None => if key.is_none() { style_line.map(|c| (c, 1.0)) } else { None },
                };
                if s.is_line() && s.line.is_none() {
                    s.line = Some((style_line.unwrap_or([0, 0, 0]), 1.5));
                }
                let ph_type = key.as_ref().map(|k| k.0.clone());
                s.placeholder = ph_type.as_deref().map(|t| match t {
                    "title" => Placeholder::Title,
                    "subTitle" => Placeholder::Subtitle,
                    _ => Placeholder::Body,
                });
                let is_title = s.placeholder == Some(Placeholder::Title);
                let info = inherited.unwrap_or_default();
                s.anchor = n.descendant("bodyPr").and_then(|b| b.attr("anchor")).map(|a| match a {
                    "ctr" => Anchor::Middle,
                    "b" => Anchor::Bottom,
                    _ => Anchor::Top,
                }).or(info.anchor).unwrap_or(if is_title { Anchor::Middle } else { Anchor::Top });
                if s.placeholder.is_none() && n.child("txBody").is_some() && s.fill.is_some() {
                    s.anchor = n.descendant("bodyPr").and_then(|b| b.attr("anchor")).map(|a| if a == "t" { Anchor::Top } else if a == "b" { Anchor::Bottom } else { Anchor::Middle }).unwrap_or(Anchor::Middle);
                }
                let default_size = match ph_type.as_deref() {
                    Some("title") => 44.0,
                    Some("subTitle") => 24.0,
                    Some(_) => 24.0,
                    None => 18.0,
                };
                let mut base = pres_fmt(info.size.unwrap_or(default_size), is_title);
                if s.placeholder.is_none() && s.fill.is_some() {
                    base.color = [255, 255, 255];
                }
                if let Some(tb) = n.child("txBody") {
                    let mut ph = info.clone();
                    if s.placeholder.is_none() {
                        ph.size = None;
                    }
                    s.text = read_text(ctx, tb, &base, &ph, is_title);
                    if s.placeholder == Some(Placeholder::Body) {
                        for p in &mut s.text {
                            let body_bullets = p.list.is_none() && !p.is_empty() && n.descendant("buNone").is_none() && info.align != Some(Align2::Center);
                            if body_bullets && ph_type.as_deref() == Some("body") {
                                p.list = Some(ListInfo { kind: ListKind::Bullet, level: 0, checked: false });
                            }
                        }
                    }
                    if s.placeholder.is_none() && s.fill.is_some() {
                        for p in &mut s.text {
                            if p.align == Align2::Left {
                                p.align = Align2::Center;
                            }
                        }
                    }
                }
                out.push(s);
            }
            "pic" => {
                let Some((x0, y0, w, h, rot, fh, fv)) = xfrm(n) else { continue };
                let (x0, y0, w, h) = map(x0, y0, w, h);
                let Some(rid) = n.descendant("blip").and_then(|b| b.attributes().find(|a| a.name() == "embed").map(|a| a.value().to_string())) else { continue };
                let Some((_, path)) = rels.get(&rid) else { continue };
                let Some(data) = ctx.pkg.get(path) else { continue };
                let mut s = Shape::new(Content::Picture(Arc::new(data.clone())), x0, y0, w, h);
                s.rot = rot;
                s.flip_h = fh;
                s.flip_v = fv;
                out.push(s);
            }
            "graphicFrame" => {
                let Some(tbl) = n.descendant("tbl") else { continue };
                let x = n.child("xfrm").map(|x| {
                    let g = |a: Option<Node>, k: &str| a.and_then(|n| n.attr(k)).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) / EMU;
                    (g(x.child("off"), "x"), g(x.child("off"), "y"), g(x.child("ext"), "cx"), g(x.child("ext"), "cy"))
                });
                let Some((x0, y0, w, h)) = x else { continue };
                let (x0, y0, w, mut h) = map(x0, y0, w, h);
                let rows: Vec<Vec<Para>> = tbl
                    .children_named("tr")
                    .iter()
                    .map(|tr| {
                        tr.children_named("tc").iter().map(|tc| {
                            let base = pres_fmt(14.0, false);
                            tc.child("txBody").map(|b| read_text(ctx, b, &base, &PhInfo::default(), false)).and_then(|v| v.into_iter().reduce(|mut a, b| {
                                let n2 = a.char_len();
                                let f = a.fmt_at(n2);
                                a.splice(n2, 0, "\n", &f);
                                a.append(b);
                                a
                            })).unwrap_or_else(|| para("", base))
                        }).collect()
                    })
                    .collect();
                let rh_sum: f32 = tbl.children_named("tr").iter().map(|tr| tr.attr("h").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) / EMU).sum();
                if rh_sum > h {
                    h = rh_sum;
                }
                out.push(Shape::new(Content::Table(rows), x0, y0, w, h));
            }
            "grpSp" => {
                let gx = n.child("grpSpPr").and_then(|g| g.child("xfrm"));
                let g = |a: Option<Node>, k: &str| a.and_then(|n| n.attr(k)).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) / EMU;
                let (off, ext, choff, chext) = match gx {
                    Some(x) => ((g(x.child("off"), "x"), g(x.child("off"), "y")), (g(x.child("ext"), "cx"), g(x.child("ext"), "cy")), (g(x.child("chOff"), "x"), g(x.child("chOff"), "y")), (g(x.child("chExt"), "cx"), g(x.child("chExt"), "cy"))),
                    None => ((0.0, 0.0), (1.0, 1.0), (0.0, 0.0), (1.0, 1.0)),
                };
                let gsx = if chext.0 > 0.0 { ext.0 / chext.0 } else { 1.0 };
                let gsy = if chext.1 > 0.0 { ext.1 / chext.1 } else { 1.0 };
                let child_tf = (ox + (off.0 - choff.0 * gsx) * sx, oy + (off.1 - choff.1 * gsy) * sy, sx * gsx, sy * gsy);
                read_tree(ctx, n, rels, layout_ph, master_ph, pres_fmt, out, child_tf);
            }
            "AlternateContent" => {
                if let Some(fb) = n.child("Fallback") {
                    read_tree(ctx, fb, rels, layout_ph, master_ph, pres_fmt, out, tf);
                }
            }
            _ => {}
        }
    }
}

pub fn read(bytes: &[u8]) -> Result<Presentation, String> {
    let pkg = Package::read(bytes)?;
    let main = relationships(&pkg, "").values().find(|(t, _)| t == "officeDocument").map(|(_, p)| p.clone()).unwrap_or_else(|| "ppt/presentation.xml".into());
    let xml = pkg.text(&main).ok_or("presentation.xml is missing")?;
    let d = parse(&xml)?;
    let rels = relationships(&pkg, &main);
    let mut pres = Presentation::default();
    pres.slides.clear();
    if let Some(sz) = d.root_element().child("sldSz") {
        pres.width = sz.attr("cx").and_then(|v| v.parse::<f32>().ok()).unwrap_or(12192000.0) / EMU * (96.0 / 72.0) / (96.0 / 72.0);
        pres.height = sz.attr("cy").and_then(|v| v.parse::<f32>().ok()).unwrap_or(6858000.0) / EMU;
        // Our coordinates are in points scaled so that a 16:9 slide is 960 wide.
    }
    let scale = 960.0 / pres.width;
    let mut ctx = Ctx { pkg: &pkg, theme: HashMap::new(), major: "Calibri Light".into(), minor: "Calibri".into() };
    // Theme from the first master.
    let master_path = rels.values().find(|(t, _)| t == "slideMaster").map(|(_, p)| p.clone());
    if let Some(mp) = &master_path {
        if let Some((_, tp)) = relationships(&pkg, mp).values().find(|(t, _)| t == "theme") {
            if let Some(tx) = pkg.text(tp) {
                if let Ok(td) = parse(&tx) {
                    if let Some(cs) = td.root_element().descendant("clrScheme") {
                        for c in cs.children().filter(|n| n.is_element()) {
                            if let Some(col) = ctx.color(c) {
                                ctx.theme.insert(c.tag_name().name().to_string(), col);
                            }
                        }
                    }
                    if let Some(m) = td.root_element().descendant("majorFont").and_then(|n| n.child("latin")).and_then(|n| n.attr("typeface")) {
                        ctx.major = m.to_string();
                    }
                    if let Some(m) = td.root_element().descendant("minorFont").and_then(|n| n.child("latin")).and_then(|n| n.attr("typeface")) {
                        ctx.minor = m.to_string();
                    }
                }
            }
        }
    }
    pres.theme.major = ctx.major.clone();
    pres.theme.minor = ctx.minor.clone();
    if let Some(c) = ctx.theme.get("dk1") {
        pres.theme.text = *c;
        pres.theme.title = *c;
    }
    if let Some(c) = ctx.theme.get("lt1") {
        pres.theme.background = *c;
    }
    for (i, k) in ["accent1", "accent2", "accent3", "accent4", "accent5", "accent6"].iter().enumerate() {
        if let Some(c) = ctx.theme.get(*k) {
            pres.theme.accents[i] = *c;
        }
    }
    let master_ph = master_path.as_ref().map(|m| collect_ph(&ctx, m)).unwrap_or_default();
    let master_bg = master_path.as_ref().and_then(|m| pkg.text(m)).and_then(|x| background_of(&ctx, &x, &relationships(&pkg, master_path.as_ref().unwrap())));
    let theme = pres.theme.clone();
    let fmt = |size: f32, title: bool| CharFmt { font: if title { theme.major.clone() } else { theme.minor.clone() }, size, color: if title { theme.title } else { theme.text }, ..CharFmt::default() };
    let ids = d.root_element().child("sldIdLst").map(|l| l.children_named("sldId")).unwrap_or_default();
    for id in ids {
        let Some(rid) = id.attributes().find(|a| a.name() == "id" && a.namespace().is_some()).map(|a| a.value()) else { continue };
        let Some((_, path)) = rels.get(rid) else { continue };
        let Some(sx) = pkg.text(path) else { continue };
        let sd = parse(&sx)?;
        let srels = relationships(&pkg, path);
        let layout = srels.values().find(|(t, _)| t == "slideLayout").map(|(_, p)| p.clone());
        let layout_ph = layout.as_ref().map(|l| collect_ph(&ctx, l)).unwrap_or_default();
        let mut slide = Slide::new();
        slide.hidden = sd.root_element().attr("show") == Some("0");
        if let Some(tree) = sd.root_element().descendant("spTree") {
            read_tree(&ctx, tree, &srels, &layout_ph, &master_ph, &fmt, &mut slide.shapes, (0.0, 0.0, 1.0, 1.0));
        }
        slide.background = background_of(&ctx, &sx, &srels)
            .or_else(|| layout.as_ref().and_then(|l| pkg.text(l).and_then(|x| background_of(&ctx, &x, &relationships(&pkg, l)))))
            .or(master_bg.clone());
        if let Some(tr) = sd.root_element().child("transition") {
            slide.transition = match tr.children().find(|c| c.is_element()).map(|c| c.tag_name().name()) {
                Some("push") | Some("cover") | Some("pull") => Transition::Push,
                Some("wipe") | Some("split") => Transition::Wipe,
                Some(_) => Transition::Fade,
                None => Transition::Fade,
            };
        }
        // Animations: entrance effects on shapes by spid order.
        if let Some(timing) = sd.root_element().child("timing") {
            for par in timing.descendants().filter(|n| n.is("cTn") && n.attr("presetClass") == Some("entr")) {
                let spid = par.descendant("spTgt").and_then(|s| s.attr("spid"));
                let preset = par.attr("presetID").and_then(|v| v.parse::<u32>().ok()).unwrap_or(1);
                if let Some(spid) = spid {
                    let names: Vec<String> = sd.root_element().descendants().filter(|n| n.is("cNvPr")).map(|n| n.attr("id").unwrap_or("").to_string()).collect();
                    if let Some(pos) = names.iter().position(|x| x == spid) {
                        // cNvPr index 0 is the tree itself.
                        if let Some(sh) = slide.shapes.get_mut(pos.saturating_sub(1)) {
                            sh.anim = Some(match preset {
                                10 => Anim::Fade,
                                2 => Anim::FlyIn,
                                23 | 53 => Anim::Zoom,
                                _ => Anim::Appear,
                            });
                        }
                    }
                }
            }
        }
        if let Some((_, np)) = srels.values().find(|(t, _)| t == "notesSlide") {
            if let Some(nx) = pkg.text(np) {
                if let Ok(nd) = parse(&nx) {
                    let body = nd.root_element().descendants().filter(|n| n.is("sp")).find(|sp| ph_key(*sp).map(|k| k.0 == "body").unwrap_or(false));
                    if let Some(b) = body {
                        slide.notes = b.descendants_named("p").iter().map(|p| p.descendants().filter(|x| x.is("t")).filter_map(|x| x.text()).collect::<String>()).collect::<Vec<_>>().join("\n");
                    }
                }
            }
        }
        // Scale to the 960-wide coordinate system.
        for sh in &mut slide.shapes {
            sh.x *= scale;
            sh.y *= scale;
            sh.w *= scale;
            sh.h *= scale;
            for p in &mut sh.text {
                p.apply_all(&mut |f| f.size *= scale);
            }
            if let Some((c, w)) = sh.line {
                sh.line = Some((c, w * scale));
            }
            if let Content::Table(rows) = &mut sh.content {
                for r in rows {
                    for c in r {
                        c.apply_all(&mut |f| f.size *= scale);
                    }
                }
            }
        }
        pres.slides.push(slide);
    }
    pres.height *= scale;
    pres.width = 960.0;
    if pres.slides.is_empty() {
        pres.slides.push(Slide::new());
    }
    if let Some(cx) = pkg.text("docProps/core.xml") {
        if let Ok(cd) = parse(&cx) {
            pres.title = cd.root_element().child("title").map(|n| n.text_all()).unwrap_or_default();
            pres.author = cd.root_element().child("creator").map(|n| n.text_all()).unwrap_or_default();
        }
    }
    Ok(pres)
}

fn background_of(ctx: &Ctx, xml: &str, rels: &std::collections::BTreeMap<String, (String, String)>) -> Option<Background> {
    let d = parse(xml).ok()?;
    let bg = d.root_element().descendant("bg")?;
    if let Some(bp) = bg.child("bgPr") {
        if let Some(sf) = bp.child("solidFill") {
            return ctx.color(sf).map(Background::Solid);
        }
        if let Some(gf) = bp.child("gradFill") {
            let stops: Vec<[u8; 3]> = gf.descendants_named("gs").iter().filter_map(|g| ctx.color(*g)).collect();
            if stops.len() >= 2 {
                return Some(Background::Gradient(stops[0], *stops.last().unwrap()));
            }
        }
        if let Some(rid) = bp.descendant("blip").and_then(|b| b.attributes().find(|a| a.name() == "embed").map(|a| a.value())) {
            let (_, path) = rels.get(rid)?;
            return ctx.pkg.get(path).map(|d| Background::Picture(Arc::new(d.clone())));
        }
    }
    if let Some(br) = bg.child("bgRef") {
        return ctx.color(br).map(Background::Solid);
    }
    None
}

// ───────────────────────── writer ─────────────────────────

fn e(v: f32, scale: f32) -> i64 {
    (v * scale * EMU).round() as i64
}

fn rgb(c: [u8; 3]) -> String {
    format!("<a:srgbClr val=\"{:02X}{:02X}{:02X}\"/>", c[0], c[1], c[2])
}

fn paras_xml(paras: &[Para], scale: f32) -> String {
    let mut s = String::new();
    for p in paras {
        let algn = match p.align {
            Align2::Center => " algn=\"ctr\"",
            Align2::Right => " algn=\"r\"",
            Align2::Justify => " algn=\"just\"",
            _ => "",
        };
        let lvl = p.list.as_ref().map(|l| format!(" lvl=\"{}\"", l.level)).unwrap_or_default();
        let bullet = match &p.list {
            Some(l) if l.kind == ListKind::Number => format!("<a:buFont typeface=\"+mj-lt\"/><a:buAutoNum type=\"arabicPeriod\"/>"),
            Some(_) => "<a:buFont typeface=\"Arial\"/><a:buChar char=\"•\"/>".to_string(),
            None => "<a:buNone/>".to_string(),
        };
        let marl = p.list.as_ref().map(|l| format!(" marL=\"{}\" indent=\"-228600\"", 228600 * (l.level as i64 + 1))).unwrap_or_else(|| " marL=\"0\" indent=\"0\"".into());
        s.push_str(&format!("<a:p><a:pPr{algn}{lvl}{marl}><a:lnSpc><a:spcPct val=\"{}\"/></a:lnSpc>{bullet}</a:pPr>", (p.line_spacing * 100000.0) as i64));
        for r in &p.runs {
            let f = &r.fmt;
            let attrs = format!(
                " lang=\"ru-RU\" sz=\"{}\"{}{}{}{}{}",
                (f.size / scale * 100.0).round() as i64,
                if f.bold { " b=\"1\"" } else { " b=\"0\"" },
                if f.italic { " i=\"1\"" } else { "" },
                if f.underline { " u=\"sng\"" } else { "" },
                if f.strike { " strike=\"sngStrike\"" } else { "" },
                match f.script {
                    1 => " baseline=\"30000\"",
                    -1 => " baseline=\"-25000\"",
                    _ => "",
                }
            );
            let font = format!("<a:latin typeface=\"{0}\"/><a:cs typeface=\"{0}\"/>", esc(&f.font));
            for (i, part) in r.text.split('\n').enumerate() {
                if i > 0 {
                    s.push_str("<a:br/>");
                }
                if !part.is_empty() {
                    s.push_str(&format!("<a:r><a:rPr{attrs}><a:solidFill>{}</a:solidFill>{font}</a:rPr><a:t>{}</a:t></a:r>", rgb(f.color), esc(part)));
                }
            }
        }
        let f = &p.runs[0].fmt;
        s.push_str(&format!("<a:endParaRPr lang=\"ru-RU\" sz=\"{}\"/></a:p>", (f.size / scale * 100.0).round() as i64));
    }
    if s.is_empty() {
        s.push_str("<a:p><a:endParaRPr lang=\"ru-RU\"/></a:p>");
    }
    s
}

pub fn write(pres: &Presentation, template: bool) -> Result<Vec<u8>, String> {
    // Slide size: 16:9 = 12192000×6858000 EMU = 960×540 points at 1/1.0 * 0.75… keep 1 point = 12700 EMU scaled.
    let scale = 12192000.0 / EMU / pres.width; // our units → points
    let mut pkg = PackageWriter::new();
    let mut ct = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="jpeg" ContentType="image/jpeg"/><Default Extension="gif" ContentType="image/gif"/><Default Extension="bmp" ContentType="image/bmp"/><Default Extension="webp" ContentType="image/webp"/>"#);
    ct.push_str(&format!(
        "<Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.{}.main+xml\"/>",
        if template { "template" } else { "presentation" }
    ));
    ct.push_str(r#"<Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/ppt/notesMasters/notesMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml"/><Override PartName="/ppt/theme/theme2.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#);
    let th = &pres.theme;
    let theme = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="{name}"><a:themeElements><a:clrScheme name="{name}"><a:dk1>{dk1}</a:dk1><a:lt1>{lt1}</a:lt1><a:dk2>{dk1}</a:dk2><a:lt2>{lt1}</a:lt2><a:accent1>{a1}</a:accent1><a:accent2>{a2}</a:accent2><a:accent3>{a3}</a:accent3><a:accent4>{a4}</a:accent4><a:accent5>{a5}</a:accent5><a:accent6>{a6}</a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="{name}"><a:majorFont><a:latin typeface="{major}"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="{minor}"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Office"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#,
        name = esc(th.name), dk1 = rgb(th.text), lt1 = rgb(th.background), a1 = rgb(th.accents[0]), a2 = rgb(th.accents[1]), a3 = rgb(th.accents[2]), a4 = rgb(th.accents[3]), a5 = rgb(th.accents[4]), a6 = rgb(th.accents[5]), major = esc(&th.major), minor = esc(&th.minor)
    );
    let ns = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
    let master = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sldMaster {ns}><p:cSld><p:bg><p:bgPr><a:solidFill>{bg}</a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz="4400"><a:solidFill>{tc}</a:solidFill><a:latin typeface="+mj-lt"/></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr><a:defRPr sz="2800"><a:solidFill>{tx}</a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:bodyStyle><p:otherStyle/></p:txStyles></p:sldMaster>"#,
        bg = rgb(th.background), tc = rgb(th.title), tx = rgb(th.text)
    );
    let layout = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sldLayout {ns} type="blank" preserve="1"><p:cSld name="Blank"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#);
    let notes_master = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:notesMaster {ns}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:notesMaster>"#);
    pkg.add_str("ppt/theme/theme1.xml", &theme)?;
    pkg.add_str("ppt/theme/theme2.xml", &theme)?;
    pkg.add_str("ppt/slideMasters/slideMaster1.xml", &master)?;
    pkg.add_str("ppt/slideMasters/_rels/slideMaster1.xml.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/></Relationships>"#)?;
    pkg.add_str("ppt/slideLayouts/slideLayout1.xml", &layout)?;
    pkg.add_str("ppt/slideLayouts/_rels/slideLayout1.xml.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#)?;
    pkg.add_str("ppt/notesMasters/notesMaster1.xml", &notes_master)?;
    pkg.add_str("ppt/notesMasters/_rels/notesMaster1.xml.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme2.xml"/></Relationships>"#)?;
    let mut sld_ids = String::new();
    let mut pres_rels = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdM" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rIdT" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/><Relationship Id="rIdN" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster" Target="notesMasters/notesMaster1.xml"/>"#);
    let mut media_n = 0;
    for (si, s) in pres.slides.iter().enumerate() {
        let n = si + 1;
        let mut srels = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#);
        let mut tree = String::new();
        let mut timing = String::new();
        let mut id = 2;
        let mut add_media = |data: &Arc<Vec<u8>>, srels: &mut String, pkg: &mut PackageWriter| -> Result<String, String> {
            media_n += 1;
            let (_, ext) = kit42::ooxml::image_mime(data);
            let name = format!("image{media_n}.{ext}");
            pkg.add(&format!("ppt/media/{name}"), data)?;
            let rid = format!("rIdImg{media_n}");
            srels.push_str(&format!("<Relationship Id=\"{rid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/{name}\"/>"));
            Ok(rid)
        };
        let bg = match &s.background {
            Some(Background::Solid(c)) => format!("<p:bg><p:bgPr><a:solidFill>{}</a:solidFill><a:effectLst/></p:bgPr></p:bg>", rgb(*c)),
            Some(Background::Gradient(a, b)) => format!("<p:bg><p:bgPr><a:gradFill><a:gsLst><a:gs pos=\"0\">{}</a:gs><a:gs pos=\"100000\">{}</a:gs></a:gsLst><a:lin ang=\"5400000\"/></a:gradFill><a:effectLst/></p:bgPr></p:bg>", rgb(*a), rgb(*b)),
            Some(Background::Picture(d)) => {
                let rid = add_media(d, &mut srels, &mut pkg)?;
                format!("<p:bg><p:bgPr><a:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></a:blipFill><a:effectLst/></p:bgPr></p:bg>")
            }
            None => String::new(),
        };
        let mut anim_ids = vec![];
        for sh in &s.shapes {
            id += 1;
            let xf = format!(
                "<a:xfrm{}{}{}><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>",
                if sh.rot != 0.0 { format!(" rot=\"{}\"", (sh.rot * 60000.0) as i64) } else { String::new() },
                if sh.flip_h { " flipH=\"1\"" } else { "" },
                if sh.flip_v { " flipV=\"1\"" } else { "" },
                e(sh.x, scale), e(sh.y, scale), e(sh.w, scale).max(1), e(sh.h, scale).max(1)
            );
            if sh.anim.is_some() {
                anim_ids.push((id, sh.anim.unwrap()));
            }
            let name = if sh.name.is_empty() { format!("Shape {id}") } else { sh.name.clone() };
            match &sh.content {
                Content::Picture(d) => {
                    let rid = add_media(d, &mut srels, &mut pkg)?;
                    tree.push_str(&format!(r#"<p:pic><p:nvPicPr><p:cNvPr id="{id}" name="{}"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>{xf}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#, esc(&name)));
                }
                Content::Table(rows) => {
                    let cols = rows.first().map(|r| r.len()).unwrap_or(1).max(1);
                    let cw = e(sh.w / cols as f32, scale);
                    let rh = e(sh.h / rows.len().max(1) as f32, scale);
                    let mut t = format!("<a:tbl><a:tblPr firstRow=\"1\" bandRow=\"1\"/><a:tblGrid>{}</a:tblGrid>", (0..cols).map(|_| format!("<a:gridCol w=\"{cw}\"/>")).collect::<String>());
                    for (ri, r) in rows.iter().enumerate() {
                        t.push_str(&format!("<a:tr h=\"{rh}\">"));
                        for c in r {
                            let fill = if ri == 0 { rgb(pres.theme.accents[0]) } else if ri % 2 == 0 { rgb([233, 237, 244]) } else { rgb([207, 213, 234]) };
                            t.push_str(&format!("<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody><a:tcPr anchor=\"ctr\"><a:solidFill>{fill}</a:solidFill></a:tcPr></a:tc>", paras_xml(std::slice::from_ref(c), scale)));
                        }
                        t.push_str("</a:tr>");
                    }
                    t.push_str("</a:tbl>");
                    tree.push_str(&format!(r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="{id}" name="{}"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table">{t}</a:graphicData></a:graphic></p:graphicFrame>"#, esc(&name), e(sh.x, scale), e(sh.y, scale), e(sh.w, scale), e(sh.h, scale)));
                }
                Content::Shape(g) => {
                    let fill = match sh.fill {
                        Some(c) => format!("<a:solidFill>{}</a:solidFill>", rgb(c)),
                        None => "<a:noFill/>".into(),
                    };
                    let line = match sh.line {
                        Some((c, w)) => format!("<a:ln w=\"{}\"><a:solidFill>{}</a:solidFill>{}</a:ln>", e(w, scale), rgb(c), if *g == Geom::Arrow { "<a:tailEnd type=\"triangle\"/>" } else { "" }),
                        None => "<a:ln><a:noFill/></a:ln>".into(),
                    };
                    if sh.is_line() {
                        tree.push_str(&format!(r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="{id}" name="{}"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr>{xf}<a:prstGeom prst="line"><a:avLst/></a:prstGeom>{line}</p:spPr></p:cxnSp>"#, esc(&name)));
                        continue;
                    }
                    let ph = match sh.placeholder {
                        Some(Placeholder::Title) => "<p:nvPr><p:ph type=\"title\"/></p:nvPr>".to_string(),
                        Some(Placeholder::Subtitle) => "<p:nvPr><p:ph type=\"subTitle\" idx=\"1\"/></p:nvPr>".to_string(),
                        Some(Placeholder::Body) => format!("<p:nvPr><p:ph idx=\"{}\"/></p:nvPr>", id),
                        None => "<p:nvPr/>".to_string(),
                    };
                    let anchor = match sh.anchor {
                        Anchor::Top => "t",
                        Anchor::Middle => "ctr",
                        Anchor::Bottom => "b",
                    };
                    let tx = if sh.text.is_empty() { String::new() } else { format!("<p:txBody><a:bodyPr wrap=\"square\" lIns=\"91440\" tIns=\"45720\" rIns=\"91440\" bIns=\"45720\" anchor=\"{anchor}\"><a:normAutofit/></a:bodyPr><a:lstStyle/>{}</p:txBody>", paras_xml(&sh.text, scale)) };
                    tree.push_str(&format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{}"/><p:cNvSpPr{}/>{ph}</p:nvSpPr><p:spPr>{xf}<a:prstGeom prst="{}"><a:avLst/></a:prstGeom>{fill}{line}</p:spPr>{tx}</p:sp>"#, esc(&name), if sh.placeholder.is_some() { "><a:spLocks noGrp=\"1\"/></p:cNvSpPr" } else { "" }, g.prst()));
                    // Fix the self-closing tag when locks were added.
                    if sh.placeholder.is_some() {
                        let fixed = tree.replace("<p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr/>", "<p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr>");
                        tree = fixed;
                    }
                }
            }
        }
        if !anim_ids.is_empty() {
            let mut pars = String::new();
            for (k, (spid, a)) in anim_ids.iter().enumerate() {
                let (preset, filter) = match a {
                    Anim::Fade => (10, "<p:animEffect transition=\"in\" filter=\"fade\"><p:cBhvr><p:cTn id=\"{X}\" dur=\"500\"/><p:tgtEl><p:spTgt spid=\"{S}\"/></p:tgtEl></p:cBhvr></p:animEffect>"),
                    Anim::FlyIn => (2, ""),
                    Anim::Zoom => (53, ""),
                    Anim::Appear => (1, ""),
                };
                let base = 10 + k as u32 * 10;
                let effect = filter.replace("{X}", &(base + 3).to_string()).replace("{S}", &spid.to_string());
                pars.push_str(&format!(
                    r#"<p:par><p:cTn id="{}" fill="hold"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="{}" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="{}" presetID="{preset}" presetClass="entr" presetSubtype="0" fill="hold" nodeType="clickEffect"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:set><p:cBhvr><p:cTn id="{}" dur="1" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{spid}"/></p:tgtEl><p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="visible"/></p:to></p:set>{effect}</p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par>"#,
                    base, base + 1, base + 2, base + 4
                ));
            }
            timing = format!(r#"<p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst><p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst>{pars}</p:childTnLst></p:cTn><p:prevCondLst><p:cond evt="onPrev" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:prevCondLst><p:nextCondLst><p:cond evt="onNext" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:nextCondLst></p:seq></p:childTnLst></p:cTn></p:par></p:tnLst></p:timing>"#);
        }
        let transition = match s.transition {
            Transition::None => String::new(),
            Transition::Fade => "<p:transition spd=\"med\"><p:fade/></p:transition>".into(),
            Transition::Push => "<p:transition spd=\"med\"><p:push dir=\"u\"/></p:transition>".into(),
            Transition::Wipe => "<p:transition spd=\"med\"><p:wipe/></p:transition>".into(),
        };
        let slide = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld {ns}{}><p:cSld>{bg}<p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>{tree}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>{transition}{timing}</p:sld>"#,
            if s.hidden { " show=\"0\"" } else { "" }
        );
        if !s.notes.trim().is_empty() {
            let notes = format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:notes {ns}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Notes"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>{}</p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#,
                s.notes.lines().map(|l| format!("<a:p><a:r><a:rPr lang=\"ru-RU\"/><a:t>{}</a:t></a:r></a:p>", esc(l))).collect::<String>()
            );
            pkg.add_str(&format!("ppt/notesSlides/notesSlide{n}.xml"), &notes)?;
            pkg.add_str(&format!("ppt/notesSlides/_rels/notesSlide{n}.xml.rels"), &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster" Target="../notesMasters/notesMaster1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="../slides/slide{n}.xml"/></Relationships>"#))?;
            ct.push_str(&format!("<Override PartName=\"/ppt/notesSlides/notesSlide{n}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml\"/>"));
            srels.push_str(&format!("<Relationship Id=\"rIdNotes\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide\" Target=\"../notesSlides/notesSlide{n}.xml\"/>"));
        }
        srels.push_str("</Relationships>");
        pkg.add_str(&format!("ppt/slides/slide{n}.xml"), &slide)?;
        pkg.add_str(&format!("ppt/slides/_rels/slide{n}.xml.rels"), &srels)?;
        ct.push_str(&format!("<Override PartName=\"/ppt/slides/slide{n}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>"));
        sld_ids.push_str(&format!("<p:sldId id=\"{}\" r:id=\"rIdS{n}\"/>", 255 + n));
        pres_rels.push_str(&format!("<Relationship Id=\"rIdS{n}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{n}.xml\"/>"));
    }
    pres_rels.push_str("</Relationships>");
    ct.push_str("</Types>");
    let cx = 12192000i64;
    let cy = (pres.height / pres.width * cx as f32).round() as i64;
    let presentation = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:presentation {ns} saveSubsetFonts="1"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:notesMasterIdLst><p:notesMasterId r:id="rIdN"/></p:notesMasterIdLst><p:sldIdLst>{sld_ids}</p:sldIdLst><p:sldSz cx="{cx}" cy="{cy}"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#
    );
    pkg.add_str("[Content_Types].xml", &ct)?;
    pkg.add_str("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/></Relationships>"#)?;
    pkg.add_str("ppt/presentation.xml", &presentation)?;
    pkg.add_str("ppt/_rels/presentation.xml.rels", &pres_rels)?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    pkg.add_str("docProps/core.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title><dc:creator>{}</dc:creator><dcterms:created xsi:type="dcterms:W3CDTF">{now}</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">{now}</dcterms:modified></cp:coreProperties>"#, esc(&pres.title), esc(&pres.author)))?;
    pkg.add_str("docProps/app.xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>PPT42 for Windows</Application></Properties>"#)?;
    pkg.finish()
}
