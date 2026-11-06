//! Chart drawing for the grid (egui) and for PDF export.

use crate::io::chart_series;
use crate::model::*;
use egui::{Color32, Pos2, Rect, Stroke, Vec2};

pub const PALETTE: [[u8; 3]; 8] = [[68, 114, 196], [237, 125, 49], [165, 165, 165], [255, 192, 0], [91, 155, 213], [112, 173, 71], [38, 68, 120], [158, 72, 14]];

fn c(i: usize) -> Color32 {
    let p = PALETTE[i % PALETTE.len()];
    Color32::from_rgb(p[0], p[1], p[2])
}

fn nice_max(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let e = 10f64.powf(v.log10().floor());
    let f = v / e;
    let nf = if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    };
    nf * e
}

pub fn draw(p: &egui::Painter, rect: Rect, sh: &Sheet, ch: &Chart, z: f32) {
    p.rect_filled(rect, 2.0, Color32::WHITE);
    p.rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::from_gray(180)), egui::StrokeKind::Inside);
    let (cats, series, _) = chart_series(sh, ch);
    let font = |s: f32| egui::FontId::proportional((s * z).max(6.0));
    let text_c = Color32::from_gray(60);
    let mut area = rect.shrink(10.0 * z);
    if !ch.title.is_empty() {
        p.text(Pos2::new(rect.center().x, rect.top() + 8.0 * z), egui::Align2::CENTER_TOP, &ch.title, font(13.0), text_c);
        area.min.y += 24.0 * z;
    }
    if ch.legend && !series.is_empty() {
        let lw = 110.0 * z;
        let items: Vec<String> = if ch.kind == ChartKind::Pie { cats.clone() } else { series.iter().map(|s| s.0.clone()).collect() };
        for (i, name) in items.iter().enumerate().take(16) {
            let y = area.top() + 6.0 * z + i as f32 * 16.0 * z;
            p.rect_filled(Rect::from_min_size(Pos2::new(area.right() - lw, y), Vec2::splat(9.0 * z)), 1.0, c(i));
            p.text(Pos2::new(area.right() - lw + 13.0 * z, y - 2.0 * z), egui::Align2::LEFT_TOP, name, font(10.0), text_c);
        }
        area.max.x -= lw + 6.0 * z;
    }
    if series.is_empty() || cats.is_empty() {
        p.text(area.center(), egui::Align2::CENTER_CENTER, kit42::t("No data", "Нет данных"), font(12.0), text_c);
        return;
    }
    if ch.kind == ChartKind::Pie {
        let vals: Vec<f64> = series[0].1.iter().map(|v| v.max(0.0)).collect();
        let total: f64 = vals.iter().sum();
        if total <= 0.0 {
            return;
        }
        let center = area.center();
        let r = area.width().min(area.height()) / 2.0 - 4.0;
        let mut a0 = -std::f64::consts::FRAC_PI_2;
        for (i, v) in vals.iter().enumerate() {
            let sweep = v / total * std::f64::consts::TAU;
            let steps = ((sweep / 0.05).ceil() as usize).max(2);
            let mut pts = vec![center];
            for k in 0..=steps {
                let a = a0 + sweep * k as f64 / steps as f64;
                pts.push(center + Vec2::new(a.cos() as f32 * r, a.sin() as f32 * r));
            }
            p.add(egui::Shape::convex_polygon(pts, c(i), Stroke::new(1.0, Color32::WHITE)));
            if sweep > 0.25 {
                let mid = a0 + sweep / 2.0;
                let lp = center + Vec2::new(mid.cos() as f32 * r * 0.65, mid.sin() as f32 * r * 0.65);
                p.text(lp, egui::Align2::CENTER_CENTER, format!("{:.0}%", v / total * 100.0), font(10.0), Color32::WHITE);
            }
            a0 += sweep;
        }
        return;
    }
    // Axes.
    let all: Vec<f64> = series.iter().flat_map(|s| s.1.iter().cloned()).collect();
    let (mut lo, mut hi) = (all.iter().cloned().fold(0.0, f64::min), all.iter().cloned().fold(0.0, f64::max));
    hi = nice_max(hi);
    if lo < 0.0 {
        lo = -nice_max(-lo);
    }
    let left = 44.0 * z;
    let bottom = 20.0 * z;
    let plot = Rect::from_min_max(Pos2::new(area.left() + left, area.top() + 4.0), Pos2::new(area.right(), area.bottom() - bottom));
    let horizontal = ch.kind == ChartKind::Bar;
    for k in 0..=5 {
        let v = lo + (hi - lo) * k as f64 / 5.0;
        let label = fmt_general(((v * 1e6).round()) / 1e6);
        if horizontal {
            let x = plot.left() + plot.width() * k as f32 / 5.0;
            p.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.0, Color32::from_gray(230)));
            p.text(Pos2::new(x, plot.bottom() + 2.0), egui::Align2::CENTER_TOP, label, font(9.0), text_c);
        } else {
            let y = plot.bottom() - plot.height() * k as f32 / 5.0;
            p.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], Stroke::new(1.0, Color32::from_gray(230)));
            p.text(Pos2::new(plot.left() - 4.0, y), egui::Align2::RIGHT_CENTER, label, font(9.0), text_c);
        }
    }
    let scale = |v: f64| -> f32 { ((v - lo) / (hi - lo)) as f32 };
    let n = cats.len();
    let ns = series.len();
    let band = if horizontal { plot.height() } else { plot.width() } / n as f32;
    match ch.kind {
        ChartKind::Column | ChartKind::Bar => {
            let bw = band * 0.7 / ns as f32;
            for (si, (_, vals)) in series.iter().enumerate() {
                for (i, v) in vals.iter().enumerate() {
                    let off = band * i as f32 + band * 0.15 + bw * si as f32;
                    let (a, b) = (scale(0.0f64.max(lo)), scale(*v));
                    let r = if horizontal {
                        Rect::from_two_pos(Pos2::new(plot.left() + plot.width() * a, plot.top() + off), Pos2::new(plot.left() + plot.width() * b, plot.top() + off + bw))
                    } else {
                        Rect::from_two_pos(Pos2::new(plot.left() + off, plot.bottom() - plot.height() * a), Pos2::new(plot.left() + off + bw, plot.bottom() - plot.height() * b))
                    };
                    p.rect_filled(r, 0.0, c(si));
                }
            }
        }
        ChartKind::Line | ChartKind::Area | ChartKind::Scatter => {
            let xs: Vec<f64> = if ch.kind == ChartKind::Scatter { cats.iter().map(|s| s.parse::<f64>().unwrap_or(0.0)).collect() } else { vec![] };
            let (xmin, xmax) = if xs.is_empty() { (0.0, 1.0) } else { (xs.iter().cloned().fold(f64::MAX, f64::min), xs.iter().cloned().fold(f64::MIN, f64::max).max(xs[0] + 1e-9)) };
            for (si, (_, vals)) in series.iter().enumerate() {
                let pts: Vec<Pos2> = vals
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let x = if ch.kind == ChartKind::Scatter { plot.left() + plot.width() * ((xs[i] - xmin) / (xmax - xmin)) as f32 } else { plot.left() + band * (i as f32 + 0.5) };
                        Pos2::new(x, plot.bottom() - plot.height() * scale(*v))
                    })
                    .collect();
                if ch.kind == ChartKind::Area && pts.len() > 1 {
                    let base = plot.bottom() - plot.height() * scale(0.0f64.max(lo));
                    for w in pts.windows(2) {
                        let quad = vec![w[0], w[1], Pos2::new(w[1].x, base), Pos2::new(w[0].x, base)];
                        let col = c(si);
                        p.add(egui::Shape::convex_polygon(quad, Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 150), Stroke::NONE));
                    }
                }
                if ch.kind != ChartKind::Scatter {
                    p.add(egui::Shape::line(pts.clone(), Stroke::new(2.0 * z, c(si))));
                }
                for pt in &pts {
                    p.circle_filled(*pt, 3.0 * z, c(si));
                }
            }
        }
        ChartKind::Pie => {}
    }
    // Category labels.
    let step = ((n as f32 * 40.0 * z) / if horizontal { plot.height() } else { plot.width() }).ceil().max(1.0) as usize;
    if ch.kind != ChartKind::Scatter {
        for (i, cat) in cats.iter().enumerate().step_by(step) {
            let label: String = cat.chars().take(14).collect();
            if horizontal {
                p.text(Pos2::new(plot.left() - 4.0, plot.top() + band * (i as f32 + 0.5)), egui::Align2::RIGHT_CENTER, label, font(9.0), text_c);
            } else {
                p.text(Pos2::new(plot.left() + band * (i as f32 + 0.5), plot.bottom() + 2.0), egui::Align2::CENTER_TOP, label, font(9.0), text_c);
            }
        }
    }
    p.line_segment([plot.left_bottom(), plot.right_bottom()], Stroke::new(1.0, Color32::from_gray(140)));
    p.line_segment([plot.left_top(), plot.left_bottom()], Stroke::new(1.0, Color32::from_gray(140)));
}

/// Renders a chart to an image (for PDF export) by software rasterisation.
pub fn render_image(sh: &Sheet, ch: &Chart, w: u32, h: u32) -> image::RgbaImage {
    // A simple raster of bars/lines; enough for printing.
    let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
    let (cats, series, _) = chart_series(sh, ch);
    let font = kit42::fonts::load_bytes("Segoe UI", false, false).unwrap_or_default();
    let mut top = 10.0;
    if !ch.title.is_empty() {
        kit42::raster::draw_text_centered(&mut img, &font, &ch.title, 18.0, w as f32 / 2.0, 16.0, [60, 60, 60, 255]);
        top = 34.0;
    }
    if series.is_empty() || cats.is_empty() {
        return img;
    }
    fn put(img: &mut image::RgbaImage, x: i32, y: i32, col: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
            img.put_pixel(x as u32, y as u32, image::Rgba([col[0], col[1], col[2], 255]));
        }
    }
    let (l, r, b) = (50.0f32, w as f32 - 10.0, h as f32 - 20.0);
    if ch.kind == ChartKind::Pie {
        let vals: Vec<f64> = series[0].1.iter().map(|v| v.max(0.0)).collect();
        let total: f64 = vals.iter().sum::<f64>().max(1e-9);
        let (cx, cy) = (w as f32 / 2.0, (top + h as f32) / 2.0);
        let rad = ((h as f32 - top) / 2.0 - 6.0).min(w as f32 / 2.0 - 6.0);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                if dx * dx + dy * dy <= rad * rad {
                    let mut a = (dy.atan2(dx) as f64) + std::f64::consts::FRAC_PI_2;
                    if a < 0.0 {
                        a += std::f64::consts::TAU;
                    }
                    let frac = a / std::f64::consts::TAU * total;
                    let mut acc = 0.0;
                    let mut idx = 0;
                    for (i, v) in vals.iter().enumerate() {
                        acc += v;
                        if frac <= acc {
                            idx = i;
                            break;
                        }
                    }
                    put(&mut img, x, y, PALETTE[idx % PALETTE.len()]);
                }
            }
        }
        return img;
    }
    let all: Vec<f64> = series.iter().flat_map(|s| s.1.iter().cloned()).collect();
    let hi = nice_max(all.iter().cloned().fold(0.0, f64::max));
    let lo = all.iter().cloned().fold(0.0, f64::min).min(0.0);
    let sy = |v: f64| b - ((v - lo) / (hi - lo)) as f32 * (b - top);
    for k in 0..=5 {
        let y = sy(lo + (hi - lo) * k as f64 / 5.0) as i32;
        for x in l as i32..r as i32 {
            put(&mut img, x, y, [225, 225, 225]);
        }
        kit42::raster::draw_text(&mut img, &font, &fmt_general(lo + (hi - lo) * k as f64 / 5.0), 11.0, 4.0, y as f32 + 4.0, [90, 90, 90, 255]);
    }
    let n = cats.len();
    let band = (r - l) / n as f32;
    for (si, (_, vals)) in series.iter().enumerate() {
        let col = PALETTE[si % PALETTE.len()];
        let bw = band * 0.7 / series.len() as f32;
        let mut prev: Option<(f32, f32)> = None;
        for (i, v) in vals.iter().enumerate() {
            let x0 = l + band * i as f32 + band * 0.15 + bw * si as f32;
            match ch.kind {
                ChartKind::Column | ChartKind::Bar => {
                    let (ya, yb) = (sy(0.0f64.max(lo)), sy(*v));
                    for x in x0 as i32..(x0 + bw) as i32 {
                        for y in ya.min(yb) as i32..ya.max(yb) as i32 {
                            put(&mut img, x, y, col);
                        }
                    }
                }
                _ => {
                    let pt = (l + band * (i as f32 + 0.5), sy(*v));
                    if let Some(p0) = prev {
                        let steps = ((pt.0 - p0.0).abs().max((pt.1 - p0.1).abs()) as i32).max(1);
                        for s in 0..=steps {
                            let t = s as f32 / steps as f32;
                            let (x, y) = (p0.0 + (pt.0 - p0.0) * t, p0.1 + (pt.1 - p0.1) * t);
                            for d in -1..=1 {
                                put(&mut img, x as i32, y as i32 + d, col);
                            }
                        }
                    }
                    prev = Some(pt);
                }
            }
        }
    }
    img
}
