//! The sheet grid: virtualised drawing with sticky headers and frozen panes, cell text with
//! formats/overflow/wrap, merged cells, selection and fill handle, in-cell editor, column/row
//! resizing, formula reference picking, AutoFilter buttons and charts.

use crate::model::*;
use crate::{App, Sel};
use egui::{Align2, Color32, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use kit42::t;

const HW: f32 = 46.0; // row header width
const HH: f32 = 22.0; // column header height

pub enum Drag {
    Select,
    Col { c: u32, start_w: f32, start_x: f32 },
    Row { r: u32, start_h: f32, start_y: f32 },
    Fill { src: (u32, u32, u32, u32), to: (u32, u32) },
    HeaderCols(u32),
    HeaderRows(u32),
    ChartMove { id: u64, last: Pos2, acc: Vec2 },
    ChartSize { id: u64, last: Pos2 },
    RefPick { start: (u32, u32), at: usize, len: usize },
}

fn is_ref_insert_point(text: &str) -> bool {
    text.starts_with('=') && text.trim_end().ends_with(['=', '(', ',', ';', '+', '-', '*', '/', '^', '&', '<', '>', ':'])
}

impl App {
    pub fn grid(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let z = self.zoom;
        let (cw_scroll, hover) = ctx.input(|i| (if i.modifiers.command { i.raw_scroll_delta.y } else { 0.0 }, i.pointer.hover_pos()));
        if cw_scroll != 0.0 && hover.map(|p| ui.max_rect().contains(p)).unwrap_or(false) {
            self.zoom = (self.zoom * if cw_scroll > 0.0 { 1.1 } else { 1.0 / 1.1 }).clamp(0.4, 3.0);
        }
        let (ur, uc) = self.sh().used().unwrap_or((0, 0));
        let view = ui.available_size();
        let max_r = (ur + 1 + (view.y / (DEF_ROW_H * z)) as u32 * 2).max(self.sel.other.0 + 30).max(100).min(MAX_ROWS - 1);
        let max_c = (uc + 1 + (view.x / (DEF_COL_W * z)) as u32 * 2).max(self.sel.other.1 + 10).max(26).min(MAX_COLS - 1);
        let total = Vec2::new(HW + self.sh().col_x(max_c) * z + 200.0, HH + self.sh().row_y(max_r) * z + 200.0);
        let (fr, fc) = self.sh().freeze;
        let fx = self.sh().col_x(fc) * z;
        let fy = self.sh().row_y(fr) * z;
        let mut scroll = egui::ScrollArea::both().auto_shrink(false).drag_to_scroll(false).enable_scrolling(cw_scroll == 0.0);
        if let Some((r, c)) = self.scroll_to.take() {
            // Only scroll when the target is outside the visible (unfrozen) area.
            let st = egui::scroll_area::State::load(&ctx, ui.id().with("grid_scroll"));
            let off = st.map(|s| s.offset).unwrap_or_default();
            let x0 = self.sh().col_x(c) * z;
            let x1 = x0 + self.sh().col_width(c) * z;
            let y0 = self.sh().row_y(r) * z;
            let y1 = y0 + self.sh().row_height(r) * z;
            let vw = view.x - HW - fx - 16.0;
            let vh = view.y - HH - fy - 16.0;
            let mut ox = off.x;
            let mut oy = off.y;
            if c >= fc {
                if x0 - fx < off.x {
                    ox = x0 - fx;
                } else if x1 - fx > off.x + vw {
                    ox = x1 - fx - vw;
                }
            }
            if r >= fr {
                if y0 - fy < off.y {
                    oy = y0 - fy;
                } else if y1 - fy > off.y + vh {
                    oy = y1 - fy - vh;
                }
            }
            scroll = scroll.scroll_offset(Vec2::new(ox.max(0.0), oy.max(0.0)));
        }
        scroll.id_salt("grid_scroll").show_viewport(ui, |ui, vp| {
            ui.set_min_size(total);
            let origin = ui.max_rect().min;
            let clip = Rect::from_min_size(origin + vp.min.to_vec2(), vp.size());
            let sheet_ix = self.sheet;
            // Screen position of a cell edge, honouring frozen rows/columns.
            let sx = |c_x: f32, frozen: bool| if frozen { clip.left() + HW + c_x } else { origin.x + HW + c_x - fx + fx };
            let _ = sx;
            let col_screen = |sh: &Sheet, c: u32| -> f32 {
                let x = sh.col_x(c) * z;
                if c < fc {
                    clip.left() + HW + x
                } else {
                    origin.x + HW + x
                }
            };
            let row_screen = |sh: &Sheet, r: u32| -> f32 {
                let y = sh.row_y(r) * z;
                if r < fr {
                    clip.top() + HH + y
                } else {
                    origin.y + HH + y
                }
            };
            // Visible ranges in the scrolled region.
            let (first_c, last_c, first_r, last_r) = {
                let sh = self.sh();
                let x0 = (vp.min.x + fx).max(0.0) / z;
                let y0 = (vp.min.y + fy).max(0.0) / z;
                (sh.col_at(x0).max(fc), sh.col_at((vp.max.x) / z).min(max_c), sh.row_at(y0).max(fr), sh.row_at((vp.max.y) / z).min(max_r))
            };
            let cols: Vec<u32> = (0..fc).chain(first_c..=last_c).collect();
            let rows: Vec<u32> = (0..fr).chain(first_r..=last_r).collect();
            let body = Rect::from_min_max(Pos2::new(clip.left() + HW, clip.top() + HH), clip.max);
            let painter_all = ui.painter().with_clip_rect(body);
            let dark = ctx.style().visuals.dark_mode;
            let bg = if dark { Color32::from_gray(30) } else { Color32::WHITE };
            let grid_c = if dark { Color32::from_gray(60) } else { Color32::from_gray(218) };
            let text_def = if dark { Color32::from_gray(225) } else { Color32::BLACK };
            painter_all.rect_filled(body, 0.0, bg);
            // Region clip for a cell: frozen cells may draw over the scrolled area only within their band.
            let region = |r: u32, c: u32| -> Rect {
                let left = if c < fc { clip.left() + HW } else { clip.left() + HW + fx };
                let top = if r < fr { clip.top() + HH } else { clip.top() + HH + fy };
                let right = if c < fc { clip.left() + HW + fx } else { clip.right() };
                let bottom = if r < fr { clip.top() + HH + fy } else { clip.bottom() };
                Rect::from_min_max(Pos2::new(left, top), Pos2::new(right, bottom))
            };
            let sh = &self.wb.sheets[sheet_ix];
            let show_grid = sh.gridlines;
            // Cells.
            for &r in &rows {
                let rh = sh.row_height(r) * z;
                if rh <= 0.0 {
                    continue;
                }
                let y = row_screen(sh, r);
                for &c in &cols {
                    let cw = sh.col_width(c) * z;
                    if cw <= 0.0 {
                        continue;
                    }
                    let x = col_screen(sh, c);
                    let mut rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(cw, rh));
                    let reg = region(r, c);
                    let p = ui.painter().with_clip_rect(reg.intersect(body));
                    if let Some((mr0, mc0, mr1, mc1)) = sh.merge_at(r, c) {
                        if (mr0, mc0) != (r, c) && !(r == first_r && mr0 < r) && !(c == first_c && mc0 < c) {
                            continue;
                        }
                        let x0 = col_screen(sh, mc0);
                        let y0 = row_screen(sh, mr0);
                        rect = Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(col_screen(sh, mc1) + sh.col_width(mc1) * z, row_screen(sh, mr1) + sh.row_height(mr1) * z));
                        p.rect_filled(rect, 0.0, bg);
                        let (r, c) = (mr0, mc0);
                        self_draw_cell(&p, &self.wb, sh, r, c, rect, z, text_def, true, &cols);
                        if show_grid {
                            p.rect_stroke(rect, 0.0, Stroke::new(1.0, grid_c), egui::StrokeKind::Inside);
                        }
                        continue;
                    }
                    if show_grid {
                        p.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, grid_c));
                        p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, grid_c));
                    }
                    self_draw_cell(&p, &self.wb, sh, r, c, rect, z, text_def, false, &cols);
                }
            }
            // Frozen separators.
            if fr > 0 {
                ui.painter().hline(body.x_range(), clip.top() + HH + fy, Stroke::new(1.5, Color32::from_gray(140)));
            }
            if fc > 0 {
                ui.painter().vline(clip.left() + HW + fx, body.y_range(), Stroke::new(1.5, Color32::from_gray(140)));
            }
            // Selection.
            let acc = kit42::ui::accent(&ctx);
            let (r0, c0, r1, c1) = self.sel.rect();
            let sel_rect = {
                let (er1, ec1) = (r1, c1);
                let (mut rr1, mut cc1) = (er1, ec1);
                if let Some(m) = sh.merge_at(r1, c1) {
                    rr1 = m.2;
                    cc1 = m.3;
                }
                Rect::from_min_max(Pos2::new(col_screen(sh, c0), row_screen(sh, r0)), Pos2::new(col_screen(sh, cc1) + sh.col_width(cc1) * z, row_screen(sh, rr1) + sh.row_height(rr1) * z))
            };
            let sp = ui.painter().with_clip_rect(body);
            sp.rect_filled(sel_rect, 0.0, Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), 28));
            sp.rect_stroke(sel_rect, 0.0, Stroke::new(2.0, acc), egui::StrokeKind::Middle);
            let (ar, ac) = self.sel.active;
            let mut active_rect = Rect::from_min_size(Pos2::new(col_screen(sh, ac), row_screen(sh, ar)), Vec2::new(sh.col_width(ac) * z, sh.row_height(ar) * z));
            if let Some(m) = sh.merge_at(ar, ac) {
                active_rect = Rect::from_min_max(Pos2::new(col_screen(sh, m.1), row_screen(sh, m.0)), Pos2::new(col_screen(sh, m.3) + sh.col_width(m.3) * z, row_screen(sh, m.2) + sh.row_height(m.2) * z));
            }
            if !self.sel.single() {
                sp.rect_filled(active_rect.shrink(1.0), 0.0, bg);
                self_draw_cell(&sp, &self.wb, sh, ar, ac, active_rect, z, text_def, true, &cols);
            }
            let handle = Rect::from_center_size(sel_rect.right_bottom(), Vec2::splat(7.0));
            sp.rect_filled(handle, 0.0, acc);
            sp.rect_stroke(handle, 0.0, Stroke::new(1.0, bg), egui::StrokeKind::Outside);
            // Fill preview.
            if let Some(Drag::Fill { src, to }) = &self.drag {
                let (a0, b0, a1, b1) = fill_target(*src, *to);
                let rr = Rect::from_min_max(Pos2::new(col_screen(sh, b0), row_screen(sh, a0)), Pos2::new(col_screen(sh, b1) + sh.col_width(b1) * z, row_screen(sh, a1) + sh.row_height(a1) * z));
                sp.rect_stroke(rr, 0.0, Stroke::new(1.5, Color32::from_gray(90)), egui::StrokeKind::Middle);
            }
            // Cut marquee.
            if let Some((s, a0, b0, a1, b1)) = self.clip.as_ref().and_then(|c| c.cut) {
                if s == sheet_ix {
                    let rr = Rect::from_min_max(Pos2::new(col_screen(sh, b0), row_screen(sh, a0)), Pos2::new(col_screen(sh, b1) + sh.col_width(b1) * z, row_screen(sh, a1) + sh.row_height(a1) * z));
                    sp.rect_stroke(rr, 0.0, Stroke::new(1.5, acc), egui::StrokeKind::Middle);
                }
            }
            // References of the formula being edited.
            if let Some(text) = &self.editing {
                if text.starts_with('=') {
                    for (k, (sheet, a0, b0, a1, b1)) in crate::formula::references(text).into_iter().enumerate() {
                        if sheet.as_ref().map(|s| !s.eq_ignore_ascii_case(&sh.name)).unwrap_or(false) {
                            continue;
                        }
                        let (a1, b1) = (a1.min(max_r), b1.min(max_c));
                        let col = crate::charts::PALETTE[k % crate::charts::PALETTE.len()];
                        let rr = Rect::from_min_max(Pos2::new(col_screen(sh, b0), row_screen(sh, a0)), Pos2::new(col_screen(sh, b1) + sh.col_width(b1) * z, row_screen(sh, a1) + sh.row_height(a1) * z));
                        sp.rect_stroke(rr, 0.0, Stroke::new(1.5, Color32::from_rgb(col[0], col[1], col[2])), egui::StrokeKind::Middle);
                    }
                }
            }
            // Charts.
            let mut chart_rects = vec![];
            for ch in &sh.charts {
                let pos = Pos2::new(col_screen(sh, ch.anchor.1), row_screen(sh, ch.anchor.0));
                let rect = Rect::from_min_size(pos, Vec2::new(ch.size.0 * z, ch.size.1 * z));
                if rect.intersects(body) {
                    crate::charts::draw(&sp, rect, sh, ch, z);
                    if self.selected_chart == Some(ch.id) {
                        sp.rect_stroke(rect, 0.0, Stroke::new(2.0, acc), egui::StrokeKind::Outside);
                        sp.rect_filled(Rect::from_center_size(rect.right_bottom(), Vec2::splat(9.0)), 1.0, acc);
                    }
                }
                chart_rects.push((ch.id, rect));
            }
            // Headers (sticky).
            let hp = ui.painter().with_clip_rect(clip);
            let hbg = if dark { Color32::from_gray(45) } else { Color32::from_gray(240) };
            let htext = if dark { Color32::from_gray(200) } else { Color32::from_gray(70) };
            hp.rect_filled(Rect::from_min_size(clip.min, Vec2::new(clip.width(), HH)), 0.0, hbg);
            hp.rect_filled(Rect::from_min_size(clip.min, Vec2::new(HW, clip.height())), 0.0, hbg);
            let hfont = FontId::proportional((11.5 * z.max(0.7)).min(15.0));
            for &c in &cols {
                let w = sh.col_width(c) * z;
                if w <= 0.0 {
                    continue;
                }
                let x = col_screen(sh, c);
                let r = Rect::from_min_size(Pos2::new(x, clip.top()), Vec2::new(w, HH));
                let hcp = hp.with_clip_rect(Rect::from_min_max(Pos2::new(if c < fc { clip.left() + HW } else { clip.left() + HW + fx }, clip.top()), Pos2::new(if c < fc { clip.left() + HW + fx } else { clip.right() }, clip.top() + HH)));
                let selc = c >= c0 && c <= c1;
                if selc {
                    hcp.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), 50));
                }
                hcp.text(r.center(), Align2::CENTER_CENTER, col_name(c), hfont.clone(), if selc { acc } else { htext });
                hcp.vline(r.right(), r.y_range(), Stroke::new(1.0, grid_c));
                if let Some((fr0, fc0, _, fc1)) = sh.filter {
                    if c >= fc0 && c <= fc1 {
                        let _ = fr0;
                    }
                }
            }
            for &r in &rows {
                let h = sh.row_height(r) * z;
                if h <= 0.0 {
                    continue;
                }
                let y = row_screen(sh, r);
                let rr = Rect::from_min_size(Pos2::new(clip.left(), y), Vec2::new(HW, h));
                let hrp = hp.with_clip_rect(Rect::from_min_max(Pos2::new(clip.left(), if r < fr { clip.top() + HH } else { clip.top() + HH + fy }), Pos2::new(clip.left() + HW, if r < fr { clip.top() + HH + fy } else { clip.bottom() })));
                let selr = r >= r0 && r <= r1;
                if selr {
                    hrp.rect_filled(rr, 0.0, Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), 50));
                }
                hrp.text(rr.center(), Align2::CENTER_CENTER, (r + 1).to_string(), hfont.clone(), if selr { acc } else { htext });
                hrp.hline(rr.x_range(), rr.bottom(), Stroke::new(1.0, grid_c));
            }
            hp.rect_filled(Rect::from_min_size(clip.min, Vec2::new(HW, HH)), 0.0, hbg);
            // AutoFilter buttons on the header row of the filter range.
            let mut filter_buttons = vec![];
            if let Some((fr0, fc0, _, fc1)) = sh.filter {
                for c in fc0..=fc1 {
                    let w = sh.col_width(c) * z;
                    let x = col_screen(sh, c);
                    let y = row_screen(sh, fr0);
                    let b = Rect::from_min_size(Pos2::new(x + w - 17.0, y + 2.0), Vec2::new(15.0, (sh.row_height(fr0) * z - 4.0).max(10.0)));
                    if b.intersects(body) {
                        sp.rect_filled(b, 2.0, if dark { Color32::from_gray(70) } else { Color32::from_gray(235) });
                        sp.rect_stroke(b, 2.0, Stroke::new(1.0, Color32::from_gray(150)), egui::StrokeKind::Inside);
                        sp.text(b.center(), Align2::CENTER_CENTER, "▾", FontId::proportional(11.0), htext);
                        filter_buttons.push((c, b));
                    }
                }
            }
            // ───── interaction ─────
            let resp = ui.interact(clip, ui.id().with("grid_area"), Sense::click_and_drag());
            let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
            let shared = |pos: Pos2, sh: &Sheet| -> (u32, u32) {
                let rel_x = pos.x - clip.left() - HW;
                let rel_y = pos.y - clip.top() - HH;
                let c = if rel_x < fx { sh.col_at(rel_x.max(0.0) / z) } else { sh.col_at((pos.x - origin.x - HW).max(0.0) / z) };
                let r = if rel_y < fy { sh.row_at(rel_y.max(0.0) / z) } else { sh.row_at((pos.y - origin.y - HH).max(0.0) / z) };
                (r, c)
            };
            // Column/row border hover for resizing.
            let mut border_col = None;
            let mut border_row = None;
            if let Some(p) = pointer {
                if p.y < clip.top() + HH && p.x > clip.left() + HW {
                    for &c in &cols {
                        let right = col_screen(sh, c) + sh.col_width(c) * z;
                        if (p.x - right).abs() < 4.0 {
                            border_col = Some(c);
                        }
                    }
                } else if p.x < clip.left() + HW && p.y > clip.top() + HH {
                    for &r in &rows {
                        let bottom = row_screen(sh, r) + sh.row_height(r) * z;
                        if (p.y - bottom).abs() < 3.0 {
                            border_row = Some(r);
                        }
                    }
                }
                if border_col.is_some() {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeColumn);
                } else if border_row.is_some() {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeRow);
                } else if handle.expand(3.0).contains(p) {
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                } else if p.x > clip.left() + HW && p.y > clip.top() + HH && !chart_rects.iter().any(|(_, r)| r.contains(p)) {
                    ctx.set_cursor_icon(egui::CursorIcon::Cell);
                }
            }
            let shift = ctx.input(|i| i.modifiers.shift);
            if resp.double_clicked() {
                if let Some(p) = pointer {
                    if let Some(c) = border_col {
                        self.checkpoint(t("Column Width", "Ширина столбца"));
                        self.autofit(&ctx, c);
                    } else if let Some((id, _)) = chart_rects.iter().find(|(_, r)| r.contains(p)) {
                        let ch = self.sh().charts.iter().find(|c| c.id == *id).cloned();
                        if let Some(ch) = ch {
                            self.dialog = Some(crate::Dialog::Chart { kind: ch.kind, title: ch.title, legend: ch.legend, edit: Some(ch.id) });
                        }
                    } else if p.x > clip.left() + HW && p.y > clip.top() + HH {
                        self.start_edit(None);
                    }
                }
            } else if resp.drag_started() || resp.clicked() {
                if let Some(p) = pointer {
                    let (r, c) = shared(p, self.sh());
                    if let Some((fc_, _)) = filter_buttons.iter().find(|(_, b)| b.contains(p)) {
                        self.open_filter(*fc_);
                    } else if let Some(c) = border_col {
                        self.drag = Some(Drag::Col { c, start_w: self.sh().col_width(c), start_x: p.x });
                    } else if let Some(r) = border_row {
                        self.drag = Some(Drag::Row { r, start_h: self.sh().row_height(r), start_y: p.y });
                    } else if let Some((id, rect)) = chart_rects.iter().find(|(_, r)| r.expand(5.0).contains(p)).cloned() {
                        self.selected_chart = Some(id);
                        self.checkpoint(t("Move Chart", "Перемещение диаграммы"));
                        if p.distance(rect.right_bottom()) < 10.0 {
                            self.drag = Some(Drag::ChartSize { id, last: p });
                        } else {
                            self.drag = Some(Drag::ChartMove { id, last: p, acc: Vec2::ZERO });
                        }
                    } else if handle.expand(3.0).contains(p) {
                        self.drag = Some(Drag::Fill { src: self.sel.rect(), to: self.sel.other });
                    } else if p.y < clip.top() + HH && p.x > clip.left() + HW {
                        self.commit_edit();
                        if shift {
                            self.sel.other = (MAX_ROWS - 1, c);
                            self.sel.active = (0, self.sel.active.1);
                        } else {
                            self.sel = Sel { active: (0, c), other: (MAX_ROWS - 1, c) };
                        }
                        self.drag = Some(Drag::HeaderCols(c));
                        self.selected_chart = None;
                    } else if p.x < clip.left() + HW && p.y > clip.top() + HH {
                        self.commit_edit();
                        self.sel = Sel { active: (r, 0), other: (r, MAX_COLS - 1) };
                        self.drag = Some(Drag::HeaderRows(r));
                        self.selected_chart = None;
                    } else if p.x > clip.left() + HW && p.y > clip.top() + HH {
                        self.selected_chart = None;
                        let picking = self.editing.as_deref().map(is_ref_insert_point).unwrap_or(false);
                        if picking {
                            let text = self.editing.clone().unwrap();
                            let ins = addr(r, c);
                            let at = text.len();
                            self.editing = Some(format!("{text}{ins}"));
                            self.drag = Some(Drag::RefPick { start: (r, c), at, len: ins.len() });
                            self.request_editor_focus = true;
                        } else {
                            if !self.commit_edit() {
                                return;
                            }
                            if shift {
                                self.sel.other = (r, c);
                            } else {
                                self.sel = Sel { active: (r, c), other: (r, c) };
                            }
                            self.drag = Some(Drag::Select);
                        }
                    }
                }
            }
            if resp.dragged() {
                if let Some(p) = pointer {
                    let (r, c) = shared(p, self.sh());
                    // Auto-scroll near the edges.
                    if p.y > clip.bottom() - 10.0 || p.x > clip.right() - 10.0 {
                        self.scroll_to = Some((r + 1, c + 1));
                    }
                    match &mut self.drag {
                        Some(Drag::Select) => self.sel.other = (r, c),
                        Some(Drag::HeaderCols(_)) => self.sel.other = (MAX_ROWS - 1, c),
                        Some(Drag::HeaderRows(_)) => self.sel.other = (r, MAX_COLS - 1),
                        Some(Drag::Col { c, start_w, start_x }) => {
                            let w = (*start_w + (p.x - *start_x) / z).max(4.0);
                            let c = *c;
                            self.sh_mut().col_w.insert(c, w);
                        }
                        Some(Drag::Row { r, start_h, start_y }) => {
                            let h = (*start_h + (p.y - *start_y) / z).max(4.0);
                            let r = *r;
                            self.sh_mut().row_h.insert(r, h);
                        }
                        Some(Drag::Fill { to, .. }) => *to = (r, c),
                        Some(Drag::RefPick { start, at, len }) => {
                            if let Some(text) = &mut self.editing {
                                let refs = if *start == (r, c) { addr(r, c) } else { format!("{}:{}", addr(start.0.min(r), start.1.min(c)), addr(start.0.max(r), start.1.max(c))) };
                                text.replace_range(*at..*at + *len, &refs);
                                *len = refs.len();
                            }
                        }
                        Some(Drag::ChartMove { id, last, acc }) => {
                            *acc += (p - *last) / z;
                            *last = p;
                            let (id, d) = (*id, *acc);
                            let sheet = self.sheet;
                            let sh = &mut self.wb.sheets[sheet];
                            if let Some(ch) = sh.charts.iter().find(|x| x.id == id).cloned() {
                                let x = sh.col_x(ch.anchor.1) + d.x;
                                let y = sh.row_y(ch.anchor.0) + d.y;
                                let nc = sh.col_at(x.max(0.0));
                                let nr = sh.row_at(y.max(0.0));
                                if (nr, nc) != ch.anchor {
                                    let dx = sh.col_x(nc) - sh.col_x(ch.anchor.1);
                                    let dy = sh.row_y(nr) - sh.row_y(ch.anchor.0);
                                    if let Some(chm) = sh.charts.iter_mut().find(|x| x.id == id) {
                                        chm.anchor = (nr, nc);
                                    }
                                    if let Some(Drag::ChartMove { acc, .. }) = &mut self.drag {
                                        acc.x -= dx;
                                        acc.y -= dy;
                                    }
                                }
                            }
                        }
                        Some(Drag::ChartSize { id, last }) => {
                            let d = (p - *last) / z;
                            *last = p;
                            let id = *id;
                            if let Some(ch) = self.sh_mut().charts.iter_mut().find(|x| x.id == id) {
                                ch.size.0 = (ch.size.0 + d.x).max(160.0);
                                ch.size.1 = (ch.size.1 + d.y).max(120.0);
                            }
                        }
                        None => {}
                    }
                }
            }
            if resp.drag_stopped() {
                match self.drag.take() {
                    Some(Drag::Fill { src, to }) => {
                        let dst = fill_target(src, to);
                        if dst != src {
                            self.fill(src, dst);
                            self.sel = Sel { active: (dst.0, dst.1), other: (dst.2, dst.3) };
                        }
                    }
                    Some(Drag::Col { c, start_w, .. }) => {
                        let w = self.sh().col_width(c);
                        self.sh_mut().col_w.insert(c, start_w);
                        self.checkpoint(t("Column Width", "Ширина столбца"));
                        self.sh_mut().col_w.insert(c, w);
                    }
                    Some(Drag::Row { r, start_h, .. }) => {
                        let h = self.sh().row_height(r);
                        self.sh_mut().row_h.insert(r, start_h);
                        self.checkpoint(t("Row Height", "Высота строки"));
                        self.sh_mut().row_h.insert(r, h);
                    }
                    _ => {}
                }
            }
            // Context menu.
            resp.context_menu(|ui| {
                if ui.button(t("Cut", "Вырезать")).clicked() {
                    self.copy(ui.ctx(), true);
                    ui.close_menu();
                }
                if ui.button(t("Copy", "Копировать")).clicked() {
                    self.copy(ui.ctx(), false);
                    ui.close_menu();
                }
                if ui.add_enabled(self.clip.is_some(), egui::Button::new(t("Paste", "Вставить"))).clicked() {
                    let text = self.clip.as_ref().map(|c| c.text.clone()).unwrap_or_default();
                    self.paste(&text, false, false);
                    ui.close_menu();
                }
                if ui.add_enabled(self.clip.is_some(), egui::Button::new(t("Paste Values", "Вставить значения"))).clicked() {
                    let text = self.clip.as_ref().map(|c| c.text.clone()).unwrap_or_default();
                    self.paste(&text, true, false);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(t("Insert Rows", "Вставить строки")).clicked() {
                    self.insert_delete(true, true);
                    ui.close_menu();
                }
                if ui.button(t("Insert Columns", "Вставить столбцы")).clicked() {
                    self.insert_delete(false, true);
                    ui.close_menu();
                }
                if ui.button(t("Delete Rows", "Удалить строки")).clicked() {
                    self.insert_delete(true, false);
                    ui.close_menu();
                }
                if ui.button(t("Delete Columns", "Удалить столбцы")).clicked() {
                    self.insert_delete(false, false);
                    ui.close_menu();
                }
                if ui.button(t("Clear Contents", "Очистить содержимое")).clicked() {
                    self.clear_selection(true, false);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(t("Format Cells…", "Формат ячеек…")).clicked() {
                    let st = self.active_style();
                    self.dialog = Some(crate::Dialog::Format { tab: 0, custom: st.numfmt.clone(), style: st });
                    ui.close_menu();
                }
                if ui.button(t("Sort Ascending", "Сортировать по возрастанию")).clicked() {
                    let h = self.guess_header();
                    self.sort(self.sel.active.1, false, h);
                    ui.close_menu();
                }
                if ui.button(t("Insert Chart…", "Вставить диаграмму…")).clicked() {
                    self.dialog = Some(crate::Dialog::Chart { kind: ChartKind::Column, title: String::new(), legend: true, edit: None });
                    ui.close_menu();
                }
            });
            // In-cell editor.
            if self.editing.is_some() && self.edit_focus_cell && self.edit_cell.map(|e| e.0) == Some(self.sheet) {
                let (_, er, ec) = self.edit_cell.unwrap();
                let sh = self.sh();
                let st = sh.cells.get(&(er, ec)).map(|c| self.wb.style(c.style).clone()).unwrap_or_default();
                let mut rect = Rect::from_min_size(Pos2::new(col_screen(sh, ec), row_screen(sh, er)), Vec2::new(sh.col_width(ec) * z, sh.row_height(er) * z));
                let need = ctx.fonts(|f| f.layout_no_wrap(self.editing.clone().unwrap_or_default(), FontId::proportional(st.size() * 96.0 / 72.0 * z), Color32::BLACK).size().x) + 16.0;
                rect.max.x = rect.max.x.max(rect.min.x + need).min(clip.right() - 2.0);
                let id = egui::Id::new("celledit");
                let mut text = self.editing.clone().unwrap_or_default();
                ui.painter().rect_filled(rect, 0.0, bg);
                let r = ui.put(
                    rect,
                    egui::TextEdit::singleline(&mut text)
                        .id(id)
                        .frame(false)
                        .font(FontId::proportional(st.size() * 96.0 / 72.0 * z))
                        .text_color(kit42::ui::c32(st.color))
                        .margin(egui::Margin::symmetric(3, 2)),
                );
                ui.painter().rect_stroke(rect, 0.0, Stroke::new(2.0, acc), egui::StrokeKind::Outside);
                if self.request_editor_focus {
                    r.request_focus();
                    if let Some(mut state) = egui::TextEdit::load_state(&ctx, id) {
                        let n = text.chars().count();
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(n))));
                        state.store(&ctx, id);
                    }
                    self.request_editor_focus = false;
                }
                if r.changed() {
                    self.editing = Some(text.clone());
                }
                let (enter, tab, esc, shift) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Tab), i.key_pressed(egui::Key::Escape), i.modifiers.shift));
                if esc {
                    self.cancel_edit();
                } else if (r.lost_focus() && enter) || (r.has_focus() && enter) {
                    if self.commit_edit() {
                        self.move_sel(if shift { -1 } else { 1 }, 0, false);
                    }
                } else if tab && (r.has_focus() || r.lost_focus()) {
                    if self.commit_edit() {
                        self.move_sel(0, if shift { -1 } else { 1 }, false);
                    }
                }
            }
        });
    }
}

fn fill_target(src: (u32, u32, u32, u32), to: (u32, u32)) -> (u32, u32, u32, u32) {
    let (r0, c0, r1, c1) = src;
    let (tr, tc) = to;
    let down = tr as i64 - r1 as i64;
    let up = r0 as i64 - tr as i64;
    let right = tc as i64 - c1 as i64;
    let left = c0 as i64 - tc as i64;
    let best = down.max(up).max(right).max(left);
    if best <= 0 {
        return src;
    }
    if best == down {
        (r0, c0, tr, c1)
    } else if best == up {
        (tr, c0, r1, c1)
    } else if best == right {
        (r0, c0, r1, tc)
    } else {
        (r0, tc, r1, c1)
    }
}

#[allow(clippy::too_many_arguments)]
fn self_draw_cell(p: &egui::Painter, wb: &Workbook, sh: &Sheet, r: u32, c: u32, rect: Rect, z: f32, text_def: Color32, merged: bool, cols: &[u32]) {
    let cell = sh.cells.get(&(r, c));
    let style = cell.map(|x| wb.style(x.style)).unwrap_or(&wb.styles[0]);
    if let Some(f) = style.fill {
        p.rect_filled(rect, 0.0, kit42::ui::c32(f));
    }
    let b = style.border;
    let bs = Stroke::new(1.0, Color32::BLACK);
    if b[0] {
        p.vline(rect.left(), rect.y_range(), bs);
    }
    if b[1] {
        p.hline(rect.x_range(), rect.top(), bs);
    }
    if b[2] {
        p.vline(rect.right(), rect.y_range(), bs);
    }
    if b[3] {
        p.hline(rect.x_range(), rect.bottom(), bs);
    }
    let v = sh.value(r, c);
    if v.is_empty() {
        return;
    }
    let (text, color) = crate::numfmt::format(&v, &style.numfmt);
    if text.is_empty() {
        return;
    }
    let size = style.size() * 96.0 / 72.0 * z;
    let family = if style.bold { FontFamily::Name(kit42::fonts::UI_BOLD.into()) } else { FontFamily::Proportional };
    let fid = FontId::new(size.max(3.0), family);
    let default_color = if style.color == [0, 0, 0] { text_def } else { kit42::ui::c32(style.color) };
    let col = color.map(kit42::ui::c32).unwrap_or(default_color);
    let is_num = matches!(v, Value::Num(_));
    let is_err = matches!(v, Value::Err(_));
    let align = match style.halign {
        HAlign::General => {
            if is_num {
                HAlign::Right
            } else if is_err || matches!(v, Value::Bool(_)) {
                HAlign::Center
            } else {
                HAlign::Left
            }
        }
        a => a,
    };
    let fmt = egui::TextFormat {
        font_id: fid.clone(),
        color: col,
        italics: style.italic,
        underline: if style.underline { Stroke::new(1.0, col) } else { Stroke::NONE },
        strikethrough: if style.strike { Stroke::new(1.0, col) } else { Stroke::NONE },
        ..Default::default()
    };
    let pad = 3.0;
    if style.wrap {
        let mut job = egui::text::LayoutJob::single_section(text, fmt);
        job.wrap.max_width = rect.width() - 2.0 * pad;
        job.halign = match align {
            HAlign::Center => egui::Align::Center,
            HAlign::Right => egui::Align::RIGHT,
            _ => egui::Align::LEFT,
        };
        let g = p.layout_job(job);
        let x = match align {
            HAlign::Center => rect.center().x,
            HAlign::Right => rect.right() - pad,
            _ => rect.left() + pad,
        };
        let y = match style.valign {
            VAlign::Top => rect.top() + 1.0,
            VAlign::Center => rect.center().y - g.size().y / 2.0,
            VAlign::Bottom => rect.bottom() - g.size().y - 1.0,
        };
        p.with_clip_rect(rect.intersect(p.clip_rect())).galley(Pos2::new(x, y), g, col);
        return;
    }
    let g = p.layout_job(egui::text::LayoutJob::single_section(text.clone(), fmt.clone()));
    let mut avail = rect.width() - 2.0 * pad;
    let mut clip = rect;
    if is_num && g.size().x > avail {
        // Numbers that do not fit show ###.
        let hashes = "#".repeat(((avail / (size * 0.55)).floor() as usize).max(1));
        let g2 = p.layout_job(egui::text::LayoutJob::single_section(hashes, fmt));
        p.with_clip_rect(rect.intersect(p.clip_rect())).galley(Pos2::new(rect.right() - pad - g2.size().x, rect.center().y - g2.size().y / 2.0), g2, col);
        return;
    }
    // Text overflows into empty neighbours.
    if !is_num && !merged && g.size().x > avail && align == HAlign::Left {
        let mut cc = c + 1;
        let mut extra = 0.0;
        while extra + avail < g.size().x && cc < c + 40 && sh.value(r, cc).is_empty() && sh.merge_at(r, cc).is_none() {
            extra += sh.col_width(cc) * z;
            cc += 1;
        }
        avail += extra;
        clip.max.x += extra;
        let _ = cols;
    }
    let x = match align {
        HAlign::Center => rect.center().x - g.size().x / 2.0,
        HAlign::Right => rect.right() - pad - g.size().x,
        _ => rect.left() + pad,
    };
    let y = match style.valign {
        VAlign::Top => rect.top() + 1.0,
        VAlign::Center => rect.center().y - g.size().y / 2.0,
        VAlign::Bottom => rect.bottom() - g.size().y - 1.0,
    };
    let _ = avail;
    p.with_clip_rect(clip.intersect(p.clip_rect())).galley(Pos2::new(x, y), g, col);
}
