//! Headless commands (nothing scans for real):
//! `--make-test-inputs dir`, `--process in out [--crop --deskew --clean --enhance --gray --bw --dpi n]`,
//! `--pipeline files… out.pdf|tif [--ocr --remove-blank --upright --gray --bw --size smallest|balanced|high|original --pdfa]`,
//! `--list-devices`.

use crate::export;
use crate::process::{self, ColorMode, Opts};
use std::path::PathBuf;

fn flags(args: &[String]) -> (Vec<String>, Vec<String>) {
    args.iter().cloned().partition(|a| !a.starts_with("--"))
}

fn opts_from(fl: &[String]) -> (Opts, f32) {
    let has = |f: &str| fl.iter().any(|x| x == f);
    let mut o = Opts { crop: has("--crop"), deskew: has("--deskew"), cleanup: has("--clean"), enhance: has("--enhance"), color: ColorMode::Color, rotation: 0 };
    if has("--gray") {
        o.color = ColorMode::Gray;
    }
    if has("--bw") {
        o.color = ColorMode::BlackWhite;
    }
    if !(o.crop || o.deskew || o.cleanup || o.enhance) && !has("--raw") {
        o.crop = true;
        o.deskew = true;
        o.cleanup = true;
    }
    let dpi = fl.iter().find_map(|f| f.strip_prefix("--dpi=").and_then(|v| v.parse().ok())).unwrap_or(300.0);
    (o, dpi)
}

pub fn run(args: &[String]) -> Option<i32> {
    let cmd = args.first()?.clone();
    if !cmd.starts_with("--") {
        return None;
    }
    kit42::attach_console();
    kit42::settings::load_global();
    let rest = &args[1..];
    let res: Result<String, String> = (|| match cmd.as_str() {
        "--make-test-inputs" => {
            let dir = rest.first().ok_or("usage: --make-test-inputs dir")?;
            process::make_test_inputs(std::path::Path::new(dir))?;
            Ok(format!("wrote test inputs to {dir}"))
        }
        "--process" => {
            let (pos, fl) = flags(rest);
            let (input, out) = (pos.first().ok_or("usage")?, pos.get(1).ok_or("usage")?);
            let img = image::open(input).map_err(|e| e.to_string())?.to_rgba8();
            let (o, dpi) = opts_from(&fl);
            let crop = process::find_page(&img);
            let angle = if o.deskew { process::skew_angle(&img) } else { 0.0 };
            let res = process::process(&img, &o, dpi);
            res.save(out).map_err(|e| e.to_string())?;
            Ok(format!("crop {:?} skew {:.2}° ink {:.4} blank {} -> {}x{}", crop, angle, process::ink_coverage(&res), process::is_blank(&res), res.width(), res.height()))
        }
        "--pipeline" => {
            let (pos, fl) = flags(rest);
            if pos.len() < 2 {
                return Err("usage: --pipeline inputs… out".into());
            }
            let out = PathBuf::from(pos.last().unwrap());
            let (o, dpi) = opts_from(&fl);
            let has = |f: &str| fl.iter().any(|x| x == f);
            let mut pages = vec![];
            let mut report = String::new();
            let mut inputs: Vec<PathBuf> = vec![];
            for p in &pos[..pos.len() - 1] {
                let pb = PathBuf::from(p);
                if pb.is_dir() {
                    let mut v: Vec<PathBuf> = std::fs::read_dir(&pb).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
                    v.sort();
                    inputs.extend(v);
                } else {
                    inputs.push(pb);
                }
            }
            for f in inputs {
                let img = image::open(&f).map_err(|e| format!("{}: {e}", f.display()))?.to_rgba8();
                let mut opts = o.clone();
                if has("--upright") {
                    opts.rotation = process::upright_turns(&img);
                }
                let processed = process::process(&img, &opts, dpi);
                let blank = process::is_blank(&processed);
                report.push_str(&format!("{}: rotation {} blank {} ink {:.4}\n", kit42::file_name(&f), opts.rotation, blank, process::ink_coverage(&processed)));
                if blank && has("--remove-blank") {
                    continue;
                }
                let mut page = crate::Page::new(img, dpi, opts);
                page.processed = std::sync::Arc::new(processed);
                if has("--ocr") {
                    let r = kit42::winrt::ocr(&page.processed, None)?;
                    report.push_str(&format!("  OCR: {}\n", r.lines.first().map(|l| l.text.clone()).unwrap_or_default()));
                    page.ocr = Some(std::sync::Arc::new(r));
                }
                pages.push(page);
            }
            let size = match fl.iter().find_map(|f| f.strip_prefix("--size=")) {
                Some("smallest") => export::Size::Smallest,
                Some("high") => export::Size::High,
                Some("original") => export::Size::Lossless,
                _ => export::Size::Balanced,
            };
            let fmt = match kit42::ext(&out).as_str() {
                "tif" | "tiff" => export::Format::Tiff,
                "jpg" | "jpeg" => export::Format::Jpeg,
                "png" => export::Format::Png,
                _ if has("--pdfa") => export::Format::PdfA,
                _ => export::Format::Pdf,
            };
            let files = export::save(&pages, fmt, size, &out, false)?;
            for f in files {
                report.push_str(&format!("wrote {} ({} bytes)\n", f.display(), std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0)));
            }
            Ok(report)
        }
        "--list-devices" => {
            let list = kit42::winrt::list_scanners()?;
            Ok(if list.is_empty() { "no scanners".into() } else { list.iter().map(|s| format!("{}  {}", s.name, s.id)).collect::<Vec<_>>().join("\n") })
        }
        _ => Err(format!("unknown command {cmd}")),
    })();
    match res {
        Ok(m) => {
            println!("{m}");
            Some(0)
        }
        Err(e) => {
            eprintln!("error: {e}");
            Some(1)
        }
    }
}
