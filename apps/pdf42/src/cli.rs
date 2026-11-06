//! Headless command line, as in the macOS PDF42:
//! `--info file`, `--merge out a.pdf b.png …`, `--split in dir 1-2,3|every:N|each`,
//! `--watermark in out TEXT [pages]`, `--number-pages in out [format]`, `--compress in out [smallest|balanced|high]`,
//! `--images-to-pdf out images…`, `--ocr in out [lang]`, `--export in out.txt|out.docx`, `--protect in out password`,
//! `--rotate in out 90 [pages]`, `--make-sample out.pdf`.

use crate::pdfops;
use kit42::pdfgen::{self, TextStyle};
use std::path::PathBuf;

fn read(p: &str) -> Result<lopdf::Document, String> {
    pdfops::load(&kit42::ooxml::read_file(std::path::Path::new(p))?, None)
}

fn write(p: &str, doc: &mut lopdf::Document) -> Result<(), String> {
    let b = pdfops::save(doc)?;
    kit42::write_atomic(std::path::Path::new(p), &b).map_err(|e| e.to_string())
}

fn overlay_all(doc: &mut lopdf::Document, pages: &[usize], draw: &dyn Fn(&mut pdfgen::Doc, &mut pdfgen::Page, usize)) -> Result<(), String> {
    let ids = pdfops::page_ids(doc);
    for &i in pages {
        let (w, h) = pdfops::geom(doc, ids[i]).display();
        let mut d = pdfgen::Doc::new();
        let mut p = pdfgen::Page::new(w, h);
        draw(&mut d, &mut p, i);
        d.add_page(p);
        pdfops::stamp(doc, ids[i], &d.finish(), false)?;
    }
    Ok(())
}

/// Returns Some(exit code) when the arguments were a CLI command.
pub fn run(args: &[String]) -> Option<i32> {
    let cmd = args.first()?.as_str();
    if !cmd.starts_with("--") {
        return None;
    }
    kit42::attach_console();
    kit42::settings::load_global();
    let res: Result<String, String> = (|| match cmd {
        "--make-sample" => {
            let out = args.get(1).ok_or("usage: --make-sample out.pdf")?;
            let mut d = pdfgen::Doc::new();
            let f = d.font("Arial", false, false);
            let b = d.font("Arial", true, false);
            for i in 0..4 {
                let mut p = pdfgen::Page::new(pdfgen::A4.0, pdfgen::A4.1);
                d.text(&mut p, TextStyle::new(b, 22.0), 60.0, 90.0, &format!("Suite42 sample — page {} / Страница {}", i + 1, i + 1));
                let body = "The quick brown fox jumps over the lazy dog. Съешь же ещё этих мягких французских булок, да выпей чаю. ";
                let lines = d.wrap(f, 12.0, &body.repeat(6), 470.0);
                for (k, l) in lines.iter().enumerate() {
                    d.text(&mut p, TextStyle::new(f, 12.0), 60.0, 130.0 + k as f32 * 16.0, l);
                }
                p.fill_rect(60.0, 420.0, 200.0, 120.0, [70, 130, 200]);
                p.ellipse(300.0, 420.0, 200.0, 120.0, Some([240, 180, 60]), Some(([40, 40, 40], 2.0)));
                d.add_page(p);
            }
            kit42::write_atomic(std::path::Path::new(out), &d.finish()).map_err(|e| e.to_string())?;
            Ok(format!("wrote {out}"))
        }
        "--info" => {
            let doc = read(args.get(1).ok_or("usage: --info file")?)?;
            let info = pdfops::read_info(&doc);
            let pages = pdfops::page_ids(&doc);
            let mut s = format!("pages: {}\nversion: {}\ntitle: {}\nauthor: {}\nproducer: {}\n", pages.len(), doc.version, info.title, info.author, info.producer);
            for (i, p) in pages.iter().enumerate().take(20) {
                let g = pdfops::geom(&doc, *p);
                let (w, h) = g.display();
                s.push_str(&format!("page {}: {:.1} x {:.1} pt rotate {}\n", i + 1, w, h, g.rot));
            }
            s.push_str(&format!("bookmarks: {}\nfields: {}\nattachments: {}\n", pdfops::read_outline(&doc).len(), pdfops::read_fields(&doc).len(), pdfops::read_attachments(&doc).len()));
            Ok(s)
        }
        "--merge" => {
            let out = args.get(1).ok_or("usage: --merge out files…")?;
            let files: Vec<PathBuf> = args[2..].iter().map(PathBuf::from).collect();
            if files.is_empty() {
                return Err("no input files".into());
            }
            let b = crate::dialogs::merge_files(&files)?;
            kit42::write_atomic(std::path::Path::new(out), &b).map_err(|e| e.to_string())?;
            Ok(format!("merged {} files into {out}", files.len()))
        }
        "--images-to-pdf" => {
            let out = args.get(1).ok_or("usage: --images-to-pdf out images…")?;
            let files: Vec<PathBuf> = args[2..].iter().map(PathBuf::from).collect();
            let b = crate::images_to_pdf(&files)?;
            kit42::write_atomic(std::path::Path::new(out), &b).map_err(|e| e.to_string())?;
            Ok(format!("wrote {out}"))
        }
        "--split" => {
            let (input, dir, spec) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?, args.get(3).map(|s| s.as_str()).unwrap_or("each"));
            let bytes = kit42::ooxml::read_file(std::path::Path::new(input))?;
            let n = pdfops::page_ids(&pdfops::load(&bytes, None)?).len();
            let groups: Vec<Vec<usize>> = if spec == "each" {
                (0..n).map(|i| vec![i]).collect()
            } else if let Some(k) = spec.strip_prefix("every:") {
                let k: usize = k.parse().map_err(|_| "bad N")?;
                (0..n).collect::<Vec<_>>().chunks(k.max(1)).map(|c| c.to_vec()).collect()
            } else {
                pdfops::parse_ranges(spec, n)?
            };
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            for (i, g) in groups.iter().enumerate() {
                let b = pdfops::extract(&bytes, g)?;
                kit42::write_atomic(&PathBuf::from(dir).join(format!("part-{:03}.pdf", i + 1)), &b).map_err(|e| e.to_string())?;
            }
            Ok(format!("{} parts", groups.len()))
        }
        "--watermark" => {
            let (input, out, text) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?, args.get(3).ok_or("usage")?.clone());
            let mut doc = read(input)?;
            let n = pdfops::page_ids(&doc).len();
            let pages = pdfops::page_set(args.get(4).map(|s| s.as_str()).unwrap_or(""), n)?;
            overlay_all(&mut doc, &pages, &|d, p, _| {
                let f = d.font("Arial", true, false);
                let (w, h) = (p.width, p.height);
                p.save();
                p.alpha(0.25);
                p.rotate_about(w / 2.0, h / 2.0, -45.0);
                let tw = d.measure(f, 60.0, &text);
                let mut st = TextStyle::new(f, 60.0);
                st.color = [200, 0, 0];
                d.text(p, st, (w - tw) / 2.0, h / 2.0, &text);
                p.restore();
            })?;
            write(out, &mut doc)?;
            Ok(format!("watermarked {} pages", pages.len()))
        }
        "--number-pages" => {
            let (input, out) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?);
            let fmt = args.get(3).cloned().unwrap_or_else(|| "1".into());
            let mut doc = read(input)?;
            let n = pdfops::page_ids(&doc).len();
            let pages: Vec<usize> = (0..n).collect();
            overlay_all(&mut doc, &pages, &|d, p, i| {
                let f = d.font("Arial", false, false);
                let label = fmt.replace('N', &n.to_string()).replacen('1', &(i + 1).to_string(), 1);
                let w = d.measure(f, 10.0, &label);
                let x = (p.width - w) / 2.0;
                let y = p.height - 28.0;
                d.text(p, TextStyle::new(f, 10.0), x, y, &label);
            })?;
            write(out, &mut doc)?;
            Ok(format!("numbered {n} pages"))
        }
        "--rotate" => {
            let (input, out) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?);
            let deg: i32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(90);
            let mut doc = read(input)?;
            let n = pdfops::page_ids(&doc).len();
            let pages = pdfops::page_set(args.get(4).map(|s| s.as_str()).unwrap_or(""), n)?;
            pdfops::rotate(&mut doc, &pages, deg)?;
            write(out, &mut doc)?;
            Ok(format!("rotated {} pages", pages.len()))
        }
        "--compress" => {
            let (input, out) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?);
            let (side, q) = match args.get(3).map(|s| s.as_str()).unwrap_or("balanced") {
                "smallest" => (1200, 55),
                "high" => (2600, 82),
                _ => (1800, 70),
            };
            let before = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            let mut doc = read(input)?;
            let (n, _) = pdfops::compress_images(&mut doc, side, q);
            doc.compress();
            write(out, &mut doc)?;
            let after = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
            Ok(format!("{n} images re-encoded, {before} -> {after} bytes"))
        }
        "--protect" => {
            let (input, out, pw) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?, args.get(3).ok_or("usage")?);
            let mut doc = read(input)?;
            crate::crypt::encrypt(&mut doc, pw, pw, &crate::crypt::Permissions { print: true, copy: true, modify: false })?;
            write(out, &mut doc)?;
            Ok("encrypted".into())
        }
        "--export" => {
            let (input, out) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?);
            let doc = read(input)?;
            let texts = pdfops::all_text(&doc);
            kit42::write_atomic(std::path::Path::new(out), texts.join("\n\x0C\n").as_bytes()).map_err(|e| e.to_string())?;
            Ok(format!("{} pages of text", texts.len()))
        }
        "--ocr" => {
            let (input, out) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?);
            let bytes = kit42::ooxml::read_file(std::path::Path::new(input))?;
            let (r, sizes) = kit42::winrt::PdfRenderer::open(bytes.clone(), None)?;
            let mut doc = pdfops::load(&bytes, None)?;
            let ids = pdfops::page_ids(&doc);
            let mut words = 0;
            for (i, (w, h)) in sizes.iter().enumerate() {
                let img = r.render(i as u32, (w / 72.0 * 300.0) as u32)?;
                let s = img.width() as f32 / w;
                let res = kit42::winrt::ocr(&img, args.get(3).cloned())?;
                let mut d = pdfgen::Doc::new();
                let f = d.font("Arial", false, false);
                let mut p = pdfgen::Page::new(*w, *h);
                words += res.lines.iter().map(|l| l.words.len()).sum::<usize>();
                res.to_pdf_layer(&mut d, &mut p, f, 1.0 / s);
                d.add_page(p);
                pdfops::stamp(&mut doc, ids[i], &d.finish(), false)?;
            }
            write(out, &mut doc)?;
            Ok(format!("{words} words recognised"))
        }
        "--lines" => {
            let input = args.get(1).ok_or("usage: --lines in.pdf [page]")?;
            let bytes = std::sync::Arc::new(kit42::ooxml::read_file(std::path::Path::new(input))?);
            let doc = pdfops::load(&bytes, None)?;
            let ids = pdfops::page_ids(&doc);
            let page: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
            let size = pdfops::geom(&doc, *ids.get(page - 1).ok_or("no such page")?).display();
            let lines = crate::recognize_page(bytes, None, page - 1, size)?;
            let mut out = String::new();
            for l in &lines {
                out.push_str(&format!(
                    "[{:6.1} {:6.1} {:6.1} {:6.1}] {:5.1}pt {}{}{} rgb{:?} bg{:?}{} | {}\n",
                    l.rect[0], l.rect[1], l.rect[2], l.rect[3], l.size, l.font,
                    if l.bold { " bold" } else { "" }, if l.italic { " italic" } else { "" },
                    l.color, l.bg, if l.hidden { " [image/OCR]" } else { "" }, l.text
                ));
            }
            out.push_str(&format!("{} lines", lines.len()));
            Ok(out)
        }
        "--replace-text" => {
            let usage = "usage: --replace-text in.pdf out.pdf OLD NEW [page]";
            let (input, out, old, new) = (args.get(1).ok_or(usage)?, args.get(2).ok_or(usage)?, args.get(3).ok_or(usage)?, args.get(4).ok_or(usage)?);
            let bytes = std::sync::Arc::new(kit42::ooxml::read_file(std::path::Path::new(input))?);
            let mut doc = pdfops::load(&bytes, None)?;
            let ids = pdfops::page_ids(&doc);
            let pages: Vec<usize> = match args.get(5).and_then(|s| s.parse::<usize>().ok()) {
                Some(p) => vec![p - 1],
                None => (0..ids.len()).collect(),
            };
            let mut marks = Vec::new();
            for p in pages {
                let size = pdfops::geom(&doc, ids[p]).display();
                for l in crate::recognize_page(bytes.clone(), None, p, size)? {
                    if l.text.contains(old.as_str()) {
                        marks.push(crate::markup::line_edit(p, &l, &l.text.replace(old.as_str(), new)));
                    }
                }
            }
            let (r, _) = kit42::winrt::PdfRenderer::open((*bytes).clone(), None)?;
            let render = move |page: usize, w: u32| r.render(page as u32, w);
            crate::markup::apply(&mut doc, &marks, &render, "PDF42")?;
            write(out, &mut doc)?;
            Ok(format!("{} lines replaced", marks.len()))
        }
        "--images" => {
            let input = args.get(1).ok_or("usage: --images in.pdf [page]")?;
            let doc = read(input)?;
            let ids = pdfops::page_ids(&doc);
            let pages: Vec<usize> = match args.get(2).and_then(|s| s.parse::<usize>().ok()) {
                Some(p) => vec![p - 1],
                None => (0..ids.len()).collect(),
            };
            let mut out = String::new();
            for p in pages {
                for (k, im) in crate::textlayer::page_images(&doc, ids[p]).iter().enumerate() {
                    out.push_str(&format!(
                        "page {} #{} [{:.1} {:.1} {:.1} {:.1}] {}x{} px /{} {}\n",
                        p + 1, k + 1, im.rect[0], im.rect[1], im.rect[2], im.rect[3], im.px.0, im.px.1,
                        String::from_utf8_lossy(&im.name), if im.op.is_some() { "" } else { "(inside a form)" }
                    ));
                }
            }
            Ok(out)
        }
        "--image-op" => {
            let usage = "usage: --image-op in.pdf out PAGE N move DX DY | scale K | delete | replace FILE | extract";
            let (input, out) = (args.get(1).ok_or(usage)?, args.get(2).ok_or(usage)?);
            let page: usize = args.get(3).and_then(|s| s.parse().ok()).ok_or(usage)?;
            let n: usize = args.get(4).and_then(|s| s.parse().ok()).ok_or(usage)?;
            let mut doc = read(input)?;
            let pid = *pdfops::page_ids(&doc).get(page - 1).ok_or("no such page")?;
            let im = crate::textlayer::page_images(&doc, pid).get(n - 1).cloned().ok_or("no such image")?;
            let f = |i: usize| args.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let r = im.rect;
            match args.get(5).map(|s| s.as_str()).ok_or(usage)? {
                "extract" => {
                    let (bytes, ext) = crate::pageimg::extract(&doc, &im).ok_or("cannot decode this image")?;
                    let path = format!("{out}.{ext}");
                    kit42::write_atomic(std::path::Path::new(&path), &bytes).map_err(|e| e.to_string())?;
                    return Ok(format!("wrote {path}"));
                }
                "move" => crate::pageimg::transform(&mut doc, pid, im.op.ok_or("inside a form")?, r, [r[0] + f(6), r[1] + f(7), r[2] + f(6), r[3] + f(7)])?,
                "scale" => crate::pageimg::transform(&mut doc, pid, im.op.ok_or("inside a form")?, r, [r[0], r[1], r[0] + (r[2] - r[0]) * f(6), r[1] + (r[3] - r[1]) * f(6)])?,
                "delete" => crate::pageimg::delete(&mut doc, pid, im.op.ok_or("inside a form")?)?,
                "replace" => {
                    let b = std::fs::read(args.get(6).ok_or(usage)?).map_err(|e| e.to_string())?;
                    crate::pageimg::replace(&mut doc, pid, im.op.ok_or("inside a form")?, &b)?
                }
                _ => return Err(usage.into()),
            }
            write(out, &mut doc)?;
            Ok(format!("wrote {out}"))
        }
        "--render" => {
            let (input, out) = (args.get(1).ok_or("usage")?, args.get(2).ok_or("usage")?);
            let page: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
            let bytes = kit42::ooxml::read_file(std::path::Path::new(input))?;
            let img = kit42::winrt::pdf_render_page(bytes, page - 1, 1000, None)?;
            img.save(out).map_err(|e| e.to_string())?;
            Ok(format!("{}x{}", img.width(), img.height()))
        }
        _ => Err(format!("unknown command {cmd}")),
    })();
    match res {
        Ok(msg) => {
            println!("{msg}");
            Some(0)
        }
        Err(e) => {
            eprintln!("error: {e}");
            Some(1)
        }
    }
}
