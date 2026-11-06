//! The page editor: lays blocks out on pages at the current zoom and edits paragraphs and
//! table cells in place with keyboard navigation across paragraph boundaries.

use crate::model::*;
use crate::App;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use kit42::richtext::{self, list_numbers, Para};
use kit42::t;

pub const PT: f32 = 96.0 / 72.0;
const GAP: f32 = 24.0;
pub const CELL_PAD: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loc {
    Para(usize),
    Cell(usize, usize, usize),
}

impl Loc {
    pub fn block(self) -> usize {
        match self {
            Loc::Para(b) | Loc::Cell(b, _, _) => b,
        }
    }
    pub fn id(self) -> egui::Id {
        match self {
            Loc::Para(b) => egui::Id::new(("doc-p", b)),
            Loc::Cell(b, r, c) => egui::Id::new(("doc-c", b, r, c)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub block: usize,
    pub row: Option<usize>,
    pub page: usize,
    pub y: f32,
    pub h: f32,
}

enum Ev {
    Changed(Loc, Para),
    Split(Loc, Para, Vec<Para>, usize),
    MergePrev(Loc),
    MergeNext(Loc),
    MovePrev(Loc),
    MoveNext(Loc),
    Focus(Loc, Option<(usize, usize)>),
    SelectPicture(usize),
    ResizePicture(usize, f32),
}

pub fn list_indent(p: &Para) -> f32 {
    p.list.as_ref().map(|l| 18.0 * (l.level as f32 + 1.0)).unwrap_or(0.0)
}

pub fn para_width(p: &Para, tw: f32) -> f32 {
    (tw - p.indent_left - p.indent_right - list_indent(p)).max(24.0)
}

fn para_height(ctx: &egui::Context, p: &Para, width_pt: f32, z: f32) -> f32 {
    let g = richtext::galley(ctx, p, width_pt * z, z);
    p.space_before + g.size().y / z + p.space_after + if p.border_bottom { 4.0 } else { 0.0 }
}

fn row_height(ctx: &egui::Context, t: &Table, r: usize, z: f32, scale: f32) -> f32 {
    let mut h: f32 = 0.0;
    for (c, cell) in t.rows[r].iter().enumerate() {
        let w = (t.widths.get(c).cloned().unwrap_or(72.0) * scale - 2.0 * CELL_PAD).max(10.0);
        let g = richtext::galley(ctx, &cell.para, w * z, z);
        h = h.max(g.size().y / z);
    }
    h + 2.0 * CELL_PAD
}

pub fn table_scale(t: &Table, tw: f32) -> f32 {
    let total: f32 = t.widths.iter().sum();
    (tw / total.max(1.0)).min(1.0)
}

impl App {
    /// Pagination at the current zoom.
    pub fn layout(&self, ctx: &egui::Context) -> (Vec<Item>, usize) {
        let s = &self.doc.setup;
        let z = self.zoom * PT;
        let (top, bottom) = (s.margins[1], s.height - s.margins[3]);
        let tw = s.text_width();
        let mut items = Vec::with_capacity(self.doc.blocks.len());
        let (mut page, mut y) = (0usize, top);
        for (i, b) in self.doc.blocks.iter().enumerate() {
            match b {
                Block::Para(p) => {
                    if p.page_break_before && y > top + 0.1 {
                        page += 1;
                        y = top;
                    }
                    let h = para_height(ctx, p, para_width(p, tw), z);
                    if y + h > bottom && y > top + 0.1 {
                        page += 1;
                        y = top;
                    }
                    items.push(Item { block: i, row: None, page, y, h });
                    y += h;
                }
                Block::Table(t) => {
                    let sc = table_scale(t, tw);
                    for r in 0..t.rows.len() {
                        let h = row_height(ctx, t, r, z, sc);
                        if y + h > bottom && y > top + 0.1 {
                            page += 1;
                            y = top;
                        }
                        items.push(Item { block: i, row: Some(r), page, y, h });
                        y += h;
                    }
                    y += 8.0;
                }
                Block::Picture(pic) => {
                    let h = pic.height.min(bottom - top) + 6.0;
                    if y + h > bottom && y > top + 0.1 {
                        page += 1;
                        y = top;
                    }
                    items.push(Item { block: i, row: None, page, y, h });
                    y += h;
                }
                Block::PageBreak => {
                    items.push(Item { block: i, row: None, page, y, h: 0.0 });
                    page += 1;
                    y = top;
                }
            }
        }
        (items, page + 1)
    }

    pub fn editor(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Ctrl + wheel zooms.
        let (cw, hover) = ctx.input(|i| (if i.modifiers.command { i.raw_scroll_delta.y } else { 0.0 }, i.pointer.hover_pos()));
        if cw != 0.0 && hover.map(|p| ui.max_rect().contains(p)).unwrap_or(false) {
            self.zoom = (self.zoom * if cw > 0.0 { 1.1 } else { 1.0 / 1.1 }).clamp(0.3, 4.0);
        }
        let z = self.zoom * PT;
        let (items, pages) = self.layout(&ctx);
        self.page_count = pages;
        let s = self.doc.setup.clone();
        let (pw, ph) = (s.width * z, s.height * z);
        let total_h = pages as f32 * (ph + GAP) + GAP;
        let mut scroll = egui::ScrollArea::both().auto_shrink(false).drag_to_scroll(false).enable_scrolling(cw == 0.0);
        if let Some(b) = self.scroll_to_block.take() {
            if let Some(it) = items.iter().find(|it| it.block == b) {
                let y = GAP + it.page as f32 * (ph + GAP) + it.y * z - 80.0;
                scroll = scroll.vertical_scroll_offset(y.max(0.0));
            }
        }
        let markers = list_numbers(&self.doc.blocks.iter().map(|b| b.para().cloned().unwrap_or_default()).collect::<Vec<_>>());
        let mut events: Vec<Ev> = vec![];
        let focus_req = self.pending_focus.take();
        let mut cur_page = self.cur_page;
        scroll.show_viewport(ui, |ui, vp| {
            let content_w = (pw + GAP * 2.0).max(ui.available_width());
            ui.set_min_size(Vec2::new(content_w, total_h));
            let origin = ui.max_rect().min;
            let page_rect = |p: usize| Rect::from_min_size(origin + Vec2::new((content_w - pw) / 2.0, GAP + p as f32 * (ph + GAP)), Vec2::new(pw, ph));
            let vis = Rect::from_min_size(origin + vp.min.to_vec2(), vp.size()).expand(100.0);
            let center = origin.y + vp.center().y;
            for p in 0..pages {
                let r = page_rect(p);
                if r.top() <= center && r.bottom() + GAP >= center {
                    cur_page = p;
                }
                if !r.intersects(vis) {
                    continue;
                }
                let painter = ui.painter();
                painter.rect_filled(r.translate(Vec2::new(2.0, 3.0)), 0.0, Color32::from_black_alpha(45));
                painter.rect_filled(r, 0.0, Color32::WHITE);
                // Margin guides at the corners, like Word.
                let m = Rect::from_min_max(r.min + Vec2::new(s.margins[0] * z, s.margins[1] * z), r.max - Vec2::new(s.margins[2] * z, s.margins[3] * z));
                let guide = Stroke::new(1.0, Color32::from_gray(205));
                for (c, dx, dy) in [(m.left_top(), -1.0, -1.0), (m.right_top(), 1.0, -1.0), (m.left_bottom(), -1.0, 1.0), (m.right_bottom(), 1.0, 1.0)] {
                    painter.line_segment([c, c + Vec2::new(dx * 14.0, 0.0)], guide);
                    painter.line_segment([c, c + Vec2::new(0.0, dy * 14.0)], guide);
                }
                for (text, y) in [(&s.header, s.margins[1] * 0.5), (&s.footer, s.height - s.margins[3] * 0.45)] {
                    if text.trim().is_empty() || (s.first_page_different && p == 0) {
                        continue;
                    }
                    let label = text.replace("{page}", &(p + 1).to_string()).replace("{pages}", &pages.to_string());
                    painter.text(Pos2::new(r.center().x, r.top() + y * z), egui::Align2::CENTER_BOTTOM, label, egui::FontId::proportional(9.0 * z), Color32::from_gray(110));
                }
            }
            for it in &items {
                let pr = page_rect(it.page);
                let ir = Rect::from_min_size(pr.min + Vec2::new(s.margins[0] * z, it.y * z), Vec2::new(s.text_width() * z, it.h.max(1.0) * z));
                if !ir.intersects(vis) && focus_req.map(|f| f.0.block() != it.block).unwrap_or(true) {
                    continue;
                }
                self.render_item(ui, it, ir, z, &markers, focus_req, &mut events);
            }
        });
        self.cur_page = cur_page;
        self.apply_events(&ctx, events);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_item(
        &mut self,
        ui: &mut egui::Ui,
        it: &Item,
        ir: Rect,
        z: f32,
        markers: &[Option<String>],
        focus_req: Option<(Loc, usize, usize)>,
        events: &mut Vec<Ev>,
    ) {
        let ctx = ui.ctx().clone();
        let tw = self.doc.setup.text_width();
        let typing_before = self.typing.clone();
        match &mut self.doc.blocks[it.block] {
            Block::Para(p) => {
                let loc = Loc::Para(it.block);
                let li = list_indent(p);
                let x = ir.left() + (p.indent_left + li) * z;
                let w = para_width(p, tw) * z;
                let top = ir.top() + p.space_before * z;
                let rect = Rect::from_min_size(Pos2::new(x, top), Vec2::new(w, (ir.height() - (p.space_before + p.space_after) * z).max(4.0)));
                if let Some(sh) = p.shading {
                    ui.painter().rect_filled(rect.expand2(Vec2::new(3.0, 0.0)), 0.0, richtext::c32(sh));
                }
                if let Some(m) = &markers[it.block] {
                    let f = &p.runs[0].fmt;
                    ui.painter().text(
                        Pos2::new(x - 6.0, top),
                        egui::Align2::RIGHT_TOP,
                        m,
                        egui::FontId::new(f.size * z, egui::FontFamily::Proportional),
                        richtext::c32(f.color),
                    );
                    if p.list.as_ref().map(|l| l.kind == richtext::ListKind::Check).unwrap_or(false) {
                        let mr = Rect::from_min_size(Pos2::new(x - 6.0 - f.size * z, top), Vec2::splat(f.size * z * 1.2));
                        if ui.interact(mr, loc.id().with("chk"), Sense::click()).clicked() {
                            let old = p.clone();
                            if let Some(l) = &mut p.list {
                                l.checked = !l.checked;
                            }
                            events.push(Ev::Changed(loc, old));
                        }
                    }
                }
                if p.border_bottom {
                    let y = ir.bottom() - (p.space_after + 2.0) * z;
                    ui.painter().hline(ir.x_range(), y, Stroke::new(1.0, Color32::from_gray(150)));
                }
                if let Some((l, a, b)) = focus_req {
                    if l == loc {
                        richtext::request_cursor(&ctx, loc.id(), a, b);
                    }
                }
                let old = p.clone();
                let out = ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| richtext::edit_para(ui, loc.id(), p, w, z, &mut self.typing, false)).inner;
                handle_out(loc, old, out, events);
            }
            Block::Table(t) => {
                let r = it.row.unwrap_or(0);
                let sc = table_scale(t, tw);
                let mut x = ir.left();
                let border = Stroke::new(1.0, Color32::from_gray(150));
                for c in 0..t.cols() {
                    let cw = t.widths[c] * sc * z;
                    let cell_rect = Rect::from_min_size(Pos2::new(x, ir.top()), Vec2::new(cw, ir.height()));
                    if let Some(sh) = t.rows[r][c].shading {
                        ui.painter().rect_filled(cell_rect, 0.0, richtext::c32(sh));
                    } else if r == 0 && t.header {
                        ui.painter().rect_filled(cell_rect, 0.0, Color32::from_gray(235));
                    }
                    if t.borders {
                        ui.painter().rect_stroke(cell_rect, 0.0, border, egui::StrokeKind::Middle);
                    } else {
                        ui.painter().rect_stroke(cell_rect, 0.0, Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 160, 220, 60)), egui::StrokeKind::Middle);
                    }
                    let loc = Loc::Cell(it.block, r, c);
                    if let Some((l, a, b)) = focus_req {
                        if l == loc {
                            richtext::request_cursor(&ctx, loc.id(), a, b);
                        }
                    }
                    let inner = cell_rect.shrink(CELL_PAD * z);
                    let p = &mut t.rows[r][c].para;
                    let old = p.clone();
                    let out = ui.allocate_new_ui(egui::UiBuilder::new().max_rect(inner), |ui| richtext::edit_para(ui, loc.id(), p, inner.width(), z, &mut self.typing, false)).inner;
                    handle_out(loc, old, out, events);
                    x += cw;
                }
            }
            Block::Picture(pic) => {
                let (w, h) = (pic.width.min(tw) * z, pic.height.min(self.doc.setup.text_height()) * z * pic.width.min(tw) / pic.width.max(1.0));
                let x = match pic.align {
                    richtext::Align2::Center => ir.left() + (ir.width() - w) / 2.0,
                    richtext::Align2::Right => ir.right() - w,
                    _ => ir.left(),
                };
                let rect = Rect::from_min_size(Pos2::new(x, ir.top()), Vec2::new(w, h));
                let id = pic.id;
                let tex = self.pic_tex.entry(id).or_insert_with(|| {
                    let img = image::load_from_memory(&pic.data).map(|i| i.to_rgba8()).unwrap_or_else(|_| image::RgbaImage::new(1, 1));
                    let img = kit42::raster::fit_within(&img, 1800);
                    kit42::ui::texture(&ctx, &format!("pic{id}"), &img)
                });
                ui.painter().image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                let resp = ui.interact(rect, egui::Id::new(("pic", id)), Sense::click());
                if resp.clicked() {
                    events.push(Ev::SelectPicture(it.block));
                }
                if self.selected_picture == Some(it.block) {
                    let acc = kit42::ui::accent(&ctx);
                    ui.painter().rect_stroke(rect, 0.0, Stroke::new(2.0, acc), egui::StrokeKind::Outside);
                    let handle = Rect::from_center_size(rect.right_bottom(), Vec2::splat(10.0));
                    ui.painter().rect_filled(handle, 2.0, acc);
                    let hr = ui.interact(handle.expand(4.0), egui::Id::new(("pich", id)), Sense::drag());
                    if hr.dragged() {
                        let d = hr.drag_delta().x / z;
                        events.push(Ev::ResizePicture(it.block, d));
                    }
                    if hr.hovered() || hr.dragged() {
                        ctx.set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                    }
                }
            }
            Block::PageBreak => {
                let y = ir.top() + 6.0;
                let p = ui.painter();
                let mut x = ir.left();
                while x < ir.right() {
                    p.hline(x..=(x + 6.0).min(ir.right()), y, Stroke::new(1.0, Color32::from_gray(150)));
                    x += 10.0;
                }
                p.text(Pos2::new(ir.center().x, y), egui::Align2::CENTER_CENTER, t(" Page Break ", " Разрыв страницы "), egui::FontId::proportional(10.0), Color32::from_gray(120));
            }
        }
        if self.typing != typing_before {
            // A format chosen for typing was consumed by an edit.
        }
    }

    fn apply_events(&mut self, ctx: &egui::Context, events: Vec<Ev>) {
        for ev in events {
            match ev {
                Ev::Focus(loc, sel) => {
                    if self.focus != Some(loc) {
                        self.select_all = false;
                    }
                    self.focus = Some(loc);
                    self.sel = sel;
                    self.selected_picture = None;
                }
                Ev::Changed(loc, old) => {
                    let coalesce = self.last_edit.map(|(l, tm)| l == loc && tm.elapsed().as_secs_f32() < 1.5).unwrap_or(false);
                    if !coalesce {
                        let mut prev = self.doc.clone();
                        set_para(&mut prev, loc, old);
                        self.history.push(prev, t("Typing", "Ввод"));
                    }
                    self.last_edit = Some((loc, std::time::Instant::now()));
                    self.select_all = false;
                }
                Ev::Split(loc, old, mut new_paras, cursor) => {
                    let mut prev = self.doc.clone();
                    set_para(&mut prev, loc, old);
                    self.history.push(prev, t("New Paragraph", "Новый абзац"));
                    self.last_edit = None;
                    match loc {
                        Loc::Para(b) => {
                            let n = new_paras.len();
                            for (k, p) in new_paras.drain(..).enumerate() {
                                self.doc.blocks.insert(b + 1 + k, Block::Para(p));
                            }
                            self.pending_focus = Some((Loc::Para(b + n), cursor, cursor));
                            self.scroll_if_needed(b + n);
                        }
                        Loc::Cell(..) => {
                            // Inside a cell Enter keeps a line break in the same cell.
                            if let Some(p) = get_para_mut(&mut self.doc, loc) {
                                let mut pos = p.char_len();
                                for q in new_paras {
                                    let f = p.fmt_at(pos);
                                    p.splice(pos, 0, "\n", &f);
                                    let qn = q.char_len();
                                    p.append(q);
                                    pos += 1 + qn;
                                }
                                let _ = pos;
                            }
                            let len = get_para(&self.doc, loc).map(|p| p.char_len()).unwrap_or(0);
                            self.pending_focus = Some((loc, len.min(self.sel.map(|s| s.1 + 1).unwrap_or(len)), len.min(self.sel.map(|s| s.1 + 1).unwrap_or(len))));
                        }
                    }
                }
                Ev::MergePrev(Loc::Para(b)) => {
                    // Backspace at the start: join with the previous paragraph (or remove a break/picture).
                    if b == 0 {
                        if let Block::Para(p) = &mut self.doc.blocks[0] {
                            if p.list.is_some() {
                                self.checkpoint(t("Remove List", "Удаление списка"));
                                if let Block::Para(p) = &mut self.doc.blocks[0] {
                                    p.list = None;
                                }
                            }
                        }
                        continue;
                    }
                    if let Block::Para(p) = &self.doc.blocks[b] {
                        if p.list.is_some() {
                            self.checkpoint(t("Remove List", "Удаление списка"));
                            if let Block::Para(p) = &mut self.doc.blocks[b] {
                                p.list = None;
                            }
                            continue;
                        }
                    }
                    self.checkpoint(t("Delete", "Удаление"));
                    match self.doc.blocks[b - 1].clone() {
                        Block::Para(mut prev) => {
                            let pos = prev.char_len();
                            if let Block::Para(cur) = self.doc.blocks.remove(b) {
                                prev.append(cur);
                            }
                            self.doc.blocks[b - 1] = Block::Para(prev);
                            self.pending_focus = Some((Loc::Para(b - 1), pos, pos));
                        }
                        Block::PageBreak | Block::Picture(_) => {
                            self.doc.blocks.remove(b - 1);
                            self.pending_focus = Some((Loc::Para(b - 1), 0, 0));
                        }
                        Block::Table(_) => {
                            self.pending_focus = None;
                        }
                    }
                }
                Ev::MergeNext(Loc::Para(b)) => {
                    if b + 1 >= self.doc.blocks.len() {
                        continue;
                    }
                    self.checkpoint(t("Delete", "Удаление"));
                    let pos = get_para(&self.doc, Loc::Para(b)).map(|p| p.char_len()).unwrap_or(0);
                    match self.doc.blocks[b + 1].clone() {
                        Block::Para(next) => {
                            self.doc.blocks.remove(b + 1);
                            if let Block::Para(p) = &mut self.doc.blocks[b] {
                                p.append(next);
                            }
                        }
                        Block::PageBreak | Block::Picture(_) => {
                            self.doc.blocks.remove(b + 1);
                        }
                        Block::Table(_) => {}
                    }
                    self.pending_focus = Some((Loc::Para(b), pos, pos));
                }
                Ev::MergePrev(loc @ Loc::Cell(..)) | Ev::MovePrev(loc) => {
                    if let Some((l, p)) = self.neighbour(loc, false) {
                        self.pending_focus = Some((l, p, p));
                        self.scroll_if_needed(l.block());
                    }
                }
                Ev::MergeNext(loc @ Loc::Cell(..)) | Ev::MoveNext(loc) => {
                    if let Some((l, _)) = self.neighbour(loc, true) {
                        self.pending_focus = Some((l, 0, 0));
                        self.scroll_if_needed(l.block());
                    }
                }
                Ev::SelectPicture(b) => {
                    self.selected_picture = Some(b);
                    ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
                }
                Ev::ResizePicture(b, dx) => {
                    if !self.resizing {
                        self.checkpoint(t("Resize Picture", "Размер рисунка"));
                        self.resizing = true;
                    }
                    let tw = self.doc.setup.text_width();
                    if let Block::Picture(p) = &mut self.doc.blocks[b] {
                        let ratio = p.height / p.width.max(1.0);
                        p.width = (p.width + dx).clamp(16.0, tw);
                        p.height = p.width * ratio;
                    }
                }
            }
        }
        if !ctx.input(|i| i.pointer.any_down()) {
            self.resizing = false;
        }
    }

    fn scroll_if_needed(&mut self, b: usize) {
        self.scroll_target_hint = Some(b);
    }

    /// The next/previous editable location.
    fn neighbour(&self, loc: Loc, forward: bool) -> Option<(Loc, usize)> {
        let len_of = |l: Loc| get_para(&self.doc, l).map(|p| p.char_len()).unwrap_or(0);
        let mut seq: Vec<Loc> = vec![];
        for (i, b) in self.doc.blocks.iter().enumerate() {
            match b {
                Block::Para(_) => seq.push(Loc::Para(i)),
                Block::Table(t) => {
                    for r in 0..t.rows.len() {
                        for c in 0..t.cols() {
                            seq.push(Loc::Cell(i, r, c));
                        }
                    }
                }
                _ => {}
            }
        }
        let pos = seq.iter().position(|l| *l == loc)?;
        let next = if forward { seq.get(pos + 1) } else { pos.checked_sub(1).and_then(|p| seq.get(p)) }?;
        Some((*next, len_of(*next)))
    }

    pub fn checkpoint(&mut self, label: &str) {
        self.history.push(self.doc.clone(), label);
        self.last_edit = None;
    }
}

fn handle_out(loc: Loc, old: Para, out: richtext::EditOutput, events: &mut Vec<Ev>) {
    if out.focused {
        events.push(Ev::Focus(loc, out.selection));
    }
    if !out.new_paras.is_empty() {
        events.push(Ev::Split(loc, old, out.new_paras, out.cursor_after_split));
    } else if out.changed {
        events.push(Ev::Changed(loc, old));
    }
    if out.merge_with_prev {
        events.push(Ev::MergePrev(loc));
    }
    if out.merge_with_next {
        events.push(Ev::MergeNext(loc));
    }
    if out.move_prev {
        events.push(Ev::MovePrev(loc));
    }
    if out.move_next {
        events.push(Ev::MoveNext(loc));
    }
}

pub fn get_para(doc: &Document, loc: Loc) -> Option<&Para> {
    match loc {
        Loc::Para(b) => doc.blocks.get(b).and_then(|x| x.para()),
        Loc::Cell(b, r, c) => match doc.blocks.get(b)? {
            Block::Table(t) => t.rows.get(r)?.get(c).map(|x| &x.para),
            _ => None,
        },
    }
}

pub fn get_para_mut(doc: &mut Document, loc: Loc) -> Option<&mut Para> {
    match loc {
        Loc::Para(b) => match doc.blocks.get_mut(b)? {
            Block::Para(p) => Some(p),
            _ => None,
        },
        Loc::Cell(b, r, c) => match doc.blocks.get_mut(b)? {
            Block::Table(t) => t.rows.get_mut(r)?.get_mut(c).map(|x| &mut x.para),
            _ => None,
        },
    }
}

fn set_para(doc: &mut Document, loc: Loc, p: Para) {
    if let Some(x) = get_para_mut(doc, loc) {
        *x = p;
    }
}

/// All editable paragraphs (body and cells) in reading order.
pub fn all_locs(doc: &Document) -> Vec<Loc> {
    let mut v = vec![];
    for (i, b) in doc.blocks.iter().enumerate() {
        match b {
            Block::Para(_) => v.push(Loc::Para(i)),
            Block::Table(t) => {
                for r in 0..t.rows.len() {
                    for c in 0..t.cols() {
                        v.push(Loc::Cell(i, r, c));
                    }
                }
            }
            _ => {}
        }
    }
    v
}
