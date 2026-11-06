//! Page view (continuous pages rendered by Windows.Data.Pdf, markup interaction) and sidebar.

use crate::markup::{self, Kind, Mark, Tool};
use crate::{Action, App, Drag, Fit, SideTab};
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use kit42::icons as ic;
use kit42::t;
use kit42::ui as kui;
use std::sync::Arc;

const GAP: f32 = 18.0;

/// Screen points per PDF point at zoom 1 (96 / 72).
pub const PT: f32 = 96.0 / 72.0;

impl App {
    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.18);
            let (r, _) = ui.allocate_exact_size(Vec2::splat(96.0), Sense::hover());
            if let Some(tex) = ui.ctx().data(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("appicon"))) {
                ui.painter().image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            } else {
                let tex = kui::texture(ui.ctx(), "appicon", &kit42::icons::app_icon_image("pdf42", 192));
                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("appicon"), tex));
            }
            ui.add_space(8.0);
            ui.heading("PDF42");
            ui.label(t("Read, edit, sign and organise PDF documents.", "Просмотр, правка, подпись и упорядочивание PDF."));
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                let w = 3.0 * 190.0 + 20.0;
                ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
                if ui.add_sized([190.0, 40.0], egui::Button::new(format!("{}  {}", ic::OPEN, t("Open PDF…", "Открыть PDF…")))).clicked() {
                    self.act(Action::Open);
                }
                if ui.add_sized([190.0, 40.0], egui::Button::new(format!("{}  {}", ic::PICTURE, t("PDF from Images…", "PDF из изображений…")))).clicked() {
                    self.act(Action::NewFromImages);
                }
                if ui.add_sized([190.0, 40.0], egui::Button::new(format!("{}  {}", ic::MERGE, t("Merge Files…", "Объединить файлы…")))).clicked() {
                    self.act(Action::Dialog(crate::dialogs::DialogKind::Merge));
                }
            });
            ui.add_space(20.0);
            let recent = kit42::settings::recent("pdf42");
            if !recent.is_empty() {
                ui.label(egui::RichText::new(t("Recent", "Недавние")).strong());
                for p in recent.into_iter().take(8) {
                    if ui.link(kit42::file_name(&p)).on_hover_text(p.display().to_string()).clicked() {
                        self.act(Action::OpenPath(p));
                    }
                }
            }
        });
    }

    pub fn central(&mut self, ui: &mut egui::Ui) {
        if self.doc.is_none() {
            self.welcome(ui);
            return;
        }
        let avail = ui.available_size();
        let ctx = ui.ctx().clone();
        // Zoom from fit modes.
        {
            let doc = self.doc.as_ref().unwrap();
            let maxw = doc.sizes.iter().map(|s| s.0).fold(1.0, f32::max);
            let curh = doc.sizes.get(self.current).map(|s| s.1).unwrap_or(792.0);
            let curw = doc.sizes.get(self.current).map(|s| s.0).unwrap_or(612.0);
            match self.fit {
                Fit::Width => self.zoom = ((avail.x - GAP * 2.0 - 16.0) / (maxw * PT)).clamp(0.1, 8.0),
                Fit::Page => {
                    self.zoom = ((avail.y - GAP * 2.0) / (curh * PT)).min((avail.x - GAP * 2.0 - 16.0) / (curw * PT)).clamp(0.1, 8.0)
                }
                Fit::None => {}
            }
        }
        // Ctrl + wheel zoom.
        let (ctrl_wheel, pointer) = ctx.input(|i| (if i.modifiers.command { i.raw_scroll_delta.y } else { 0.0 }, i.pointer.hover_pos()));
        if ctrl_wheel != 0.0 && pointer.map(|p| ui.max_rect().contains(p)).unwrap_or(false) {
            self.fit = Fit::None;
            self.zoom = (self.zoom * if ctrl_wheel > 0.0 { 1.1 } else { 1.0 / 1.1 }).clamp(0.1, 8.0);
        }
        let k = self.zoom * PT;
        let (layout, total_h, max_w) = {
            let doc = self.doc.as_ref().unwrap();
            let mut y = GAP;
            let mut v = Vec::with_capacity(doc.sizes.len());
            let mut mw: f32 = 0.0;
            let only = if self.continuous { None } else { Some(self.current) };
            for (i, (w, h)) in doc.sizes.iter().enumerate() {
                if only.is_some_and(|c| c != i) {
                    v.push(None);
                    continue;
                }
                v.push(Some((y, w * k, h * k)));
                y += h * k + GAP;
                mw = mw.max(w * k);
            }
            (v, y, mw)
        };
        let mut scroll = egui::ScrollArea::both().auto_shrink(false).drag_to_scroll(false).enable_scrolling(ctrl_wheel == 0.0);
        if let Some(p) = self.scroll_to.take() {
            if let Some(Some((y, _, _))) = layout.get(p) {
                scroll = scroll.vertical_scroll_offset((y - GAP).max(0.0));
            }
        }
        let out = scroll.show_viewport(ui, |ui, viewport| {
            let content_w = (max_w + GAP * 2.0).max(ui.available_width());
            ui.set_min_size(Vec2::new(content_w, total_h));
            let origin = ui.max_rect().min;
            let mut best = (f32::MAX, self.current);
            let center_y = viewport.center().y;
            for (i, l) in layout.iter().enumerate() {
                let Some((y, w, h)) = *l else { continue };
                let rect = Rect::from_min_size(origin + Vec2::new((content_w - w) / 2.0, y), Vec2::new(w, h));
                let dist = if center_y >= y && center_y <= y + h { 0.0 } else { (y + h / 2.0 - center_y).abs() };
                if dist < best.0 {
                    best = (dist, i);
                }
                let vis = Rect::from_min_size(origin + viewport.min.to_vec2(), viewport.size());
                if !rect.intersects(vis.expand(200.0)) {
                    continue;
                }
                self.draw_page(ui, i, rect, k);
            }
            best.1
        });
        if self.drag.is_none() && self.continuous {
            self.current = out.inner;
        }
    }

    fn request_page(&mut self, ctx: &egui::Context, i: usize, want: u32) -> Option<egui::TextureHandle> {
        let doc = self.doc.as_mut()?;
        // Bucket widths so small zoom changes reuse textures.
        let bucket = ((want as f32).log(1.25).ceil()) as i32;
        let want_w = 1.25f32.powi(bucket).round().min(6000.0) as u32;
        if let Some((_, rx)) = doc.pending.get(&i) {
            if let Ok(res) = rx.try_recv() {
                let w = doc.pending.remove(&i).unwrap().0;
                if let Ok(img) = res {
                    let tex = kui::texture(ctx, &format!("p{}-{}", doc.generation, i), &img);
                    doc.tex.insert(i, (w, tex));
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(30));
            }
        }
        let have = doc.tex.get(&i).map(|(w, t)| (*w, t.clone()));
        let need = match &have {
            Some((w, _)) => *w != want_w,
            None => true,
        };
        if need && !doc.pending.contains_key(&i) && doc.pending.len() < 4 {
            if let Some(r) = &doc.renderer {
                doc.pending.insert(i, (want_w, r.request(i as u32, want_w)));
            }
        }
        have.map(|(_, t)| t)
    }

    fn draw_page(&mut self, ui: &mut egui::Ui, i: usize, rect: Rect, k: f32) {
        let ctx = ui.ctx().clone();
        let painter = ui.painter().clone();
        painter.rect_filled(rect.translate(Vec2::new(2.0, 3.0)), 2.0, Color32::from_black_alpha(40));
        painter.rect_filled(rect, 0.0, Color32::WHITE);
        let want = (rect.width() * ctx.pixels_per_point()).round() as u32;
        if let Some(tex) = self.request_page(&ctx, i, want) {
            painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        } else {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, format!("{}", i + 1), egui::FontId::proportional(28.0), Color32::from_gray(200));
        }
        let to_screen = |p: [f32; 2]| rect.min + Vec2::new(p[0] * k, p[1] * k);
        let to_page = |p: Pos2| [(p.x - rect.min.x) / k, (p.y - rect.min.y) / k];
        // Search hits / page selection outline.
        if self.pages_sel.contains(&i) && self.pages_sel.len() > 1 {
            painter.rect_stroke(rect.expand(3.0), 2.0, Stroke::new(2.0, kui::accent(&ctx)), egui::StrokeKind::Outside);
        }
        // Marks.
        let marks: Vec<Mark> = self.doc.as_ref().unwrap().marks.iter().filter(|m| m.page == i).cloned().collect();
        for m in &marks {
            let tex = if let Kind::Image(img) = &m.kind {
                Some(self.mark_tex.entry(m.id).or_insert_with(|| kui::texture(&ctx, &format!("m{}", m.id), img)).clone())
            } else {
                None
            };
            if self.editing == Some(m.id) {
                continue;
            }
            markup::paint(&painter, m, &to_screen, k, tex.as_ref(), self.selected == Some(m.id));
        }
        // Crop rectangle preview.
        if let Some((p, r)) = self.crop_rect {
            if p == i {
                let rr = Rect::from_two_pos(to_screen([r[0], r[1]]), to_screen([r[2], r[3]]));
                painter.rect_stroke(rr, 0.0, Stroke::new(1.5, Color32::from_rgb(0, 120, 215)), egui::StrokeKind::Middle);
            }
        }
        // In-place text editing.
        if let Some(eid) = self.editing {
            if let Some(m) = marks.iter().find(|m| m.id == eid) {
                let r = Rect::from_two_pos(to_screen([m.rect[0], m.rect[1]]), to_screen([m.rect[2], m.rect[3]]));
                let mut text = match &m.kind {
                    Kind::Text(s) => s.clone(),
                    _ => String::new(),
                };
                let id = egui::Id::new(("textedit", eid));
                let font = egui::FontId::new((m.font_size * k).max(4.0), markup::text_family(&ctx, m));
                let resp = if let Some(c) = m.cover {
                    // Edited page text: the original is hidden and the new text is typed in place,
                    // in the matching font, over the page background.
                    let fill = markup::c32(m.fill.unwrap_or([255, 255, 255]));
                    let wide = ctx.fonts(|f| f.layout_no_wrap(text.clone(), font.clone(), Color32::BLACK)).size().x + 6.0;
                    let r = Rect::from_min_size(r.min, Vec2::new(r.width().max(wide), r.height()));
                    painter.rect_filled(Rect::from_two_pos(to_screen([c[0], c[1]]), to_screen([c[2], c[3]])), 0.0, fill);
                    painter.rect_filled(r, 0.0, fill);
                    painter.rect_stroke(r.expand(2.0), 2.0, Stroke::new(1.0, kui::accent(&ctx)), egui::StrokeKind::Outside);
                    ui.put(
                        r,
                        egui::TextEdit::singleline(&mut text)
                            .id(id)
                            .font(font)
                            .text_color(markup::c32(m.color))
                            .frame(false)
                            .margin(egui::Margin::ZERO)
                            .desired_width(f32::INFINITY),
                    )
                } else {
                    ui.put(
                        r,
                        egui::TextEdit::multiline(&mut text)
                            .id(id)
                            .font(font)
                            .text_color(markup::c32(m.color))
                            .frame(true)
                            .desired_width(r.width()),
                    )
                };
                if resp.changed() {
                    // Resolve the family first: ctx.fonts holds egui's context lock, and resolving a
                    // family may lock it again (deadlock).
                    let fid = egui::FontId::new((m.font_size * k).max(4.0), markup::text_family(&ctx, m));
                    let wide = ctx.fonts(|f| f.layout_no_wrap(text.clone(), fid, Color32::BLACK)).size().x / k;
                    if let Some(mm) = self.doc.as_mut().unwrap().marks.iter_mut().find(|x| x.id == eid) {
                        mm.kind = Kind::Text(text.clone());
                        if let Some(c) = mm.cover {
                            // One line: the box follows the text, never narrower than the original.
                            mm.rect[2] = (mm.rect[0] + wide + 2.0).max(c[2]);
                        } else {
                            // Grow the box to fit the text.
                            let lines = text.lines().count().max(1) as f32;
                            let need = lines * mm.font_size * 1.25 + 4.0;
                            if mm.rect[3] - mm.rect[1] < need {
                                mm.rect[3] = mm.rect[1] + need;
                            }
                        }
                    }
                }
                if !resp.has_focus() && ctx.memory(|mm| mm.focused().is_none()) {
                    if ctx.data(|d| d.get_temp::<bool>(id.with("focused"))).unwrap_or(false) {
                        self.editing = None;
                    } else {
                        resp.request_focus();
                        ctx.data_mut(|d| d.insert_temp(id.with("focused"), true));
                    }
                }
            }
        }
        // Interaction.
        let resp = ui.interact(rect, egui::Id::new(("page", i)), Sense::click_and_drag());
        let hover = resp.hover_pos();
        if let Some(h) = hover {
            let icon = match self.tool {
                Tool::Select if self.placing.is_some() => egui::CursorIcon::Copy,
                Tool::Select => egui::CursorIcon::Default,
                Tool::Text | Tool::Note => egui::CursorIcon::Text,
                _ => egui::CursorIcon::Crosshair,
            };
            ctx.set_cursor_icon(icon);
            let _ = h;
        }
        if let Some(img) = self.placing.clone() {
            if let Some(h) = hover {
                let w = 160.0 * k;
                let hgt = w * img.height() as f32 / img.width().max(1) as f32;
                painter.rect_stroke(Rect::from_center_size(h, Vec2::new(w, hgt)), 0.0, Stroke::new(1.0, kui::accent(&ctx)), egui::StrokeKind::Middle);
            }
            if resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let pp = to_page(p);
                    let w = 160.0;
                    let h = w * img.height() as f32 / img.width().max(1) as f32;
                    let doc = self.doc.as_mut().unwrap();
                    doc.checkpoint(t("Insert Signature", "Вставка подписи"));
                    let m = Mark::new(i, [pp[0] - w / 2.0, pp[1] - h / 2.0, pp[0] + w / 2.0, pp[1] + h / 2.0], Kind::Image(img), [0, 0, 0], 1.0);
                    self.selected = Some(m.id);
                    doc.marks.push(m);
                    self.placing = None;
                    self.status.clear();
                }
            }
            return;
        }
        if resp.clicked() || resp.drag_started() {
            self.current = i;
            if !ctx.input(|inp| inp.modifiers.command || inp.modifiers.shift) {
                self.pages_sel.clear();
            }
        }
        let pos = resp.interact_pointer_pos().map(to_page);
        if self.tool == Tool::Select {
            // Images of the page content are objects too, as in a drawing program.
            let imgs = self.doc.as_mut().unwrap().page_images(i);
            let generation = self.doc.as_ref().unwrap().generation;
            if let Some(s) = &mut self.img_sel {
                if s.page == i && s.generation != generation && !matches!(self.drag, Some(Drag::Image { .. })) {
                    // After an edit or undo, find the image again by its box.
                    let near = |a: &[f32; 4], b: &[f32; 4]| (0..4).all(|k| (a[k] - b[k]).abs() < 1.0);
                    match imgs.iter().find(|im| near(&im.rect, &s.rect)) {
                        Some(im) => {
                            s.img = im.clone();
                            s.rect = im.rect;
                            s.generation = generation;
                        }
                        None => self.img_sel = None,
                    }
                }
            }
            let img_hit = |p: [f32; 2]| imgs.iter().rev().find(|im| inside(&im.rect, p, 0.0)).cloned();
            let acc = kui::accent(&ctx);
            if let (Some(h), None) = (hover.map(to_page), &self.drag) {
                if hit(&marks, h).is_none() {
                    if let Some(im) = img_hit(h) {
                        if self.img_sel.as_ref().map(|s| s.page != i || s.img.op != im.op).unwrap_or(true) {
                            let r = Rect::from_two_pos(to_screen([im.rect[0], im.rect[1]]), to_screen([im.rect[2], im.rect[3]]));
                            painter.rect_stroke(r, 0.0, Stroke::new(1.0, acc.gamma_multiply(0.6)), egui::StrokeKind::Outside);
                        }
                        ctx.set_cursor_icon(egui::CursorIcon::Move);
                    }
                }
            }
            if let Some(s) = self.img_sel.as_ref().filter(|s| s.page == i) {
                let r = Rect::from_two_pos(to_screen([s.rect[0], s.rect[1]]), to_screen([s.rect[2], s.rect[3]]));
                if s.rect != s.img.rect {
                    // Live preview: hide the original and draw its pixels at the new place.
                    let o = Rect::from_two_pos(to_screen([s.img.rect[0], s.img.rect[1]]), to_screen([s.img.rect[2], s.img.rect[3]]));
                    painter.rect_filled(o, 0.0, Color32::WHITE);
                    if let Some((_, tex)) = self.doc.as_ref().unwrap().tex.get(&i) {
                        let (pw, ph) = (rect.width() / k, rect.height() / k);
                        let uv = Rect::from_min_max(Pos2::new(s.img.rect[0] / pw, s.img.rect[1] / ph), Pos2::new(s.img.rect[2] / pw, s.img.rect[3] / ph));
                        painter.image(tex.id(), r, uv, Color32::WHITE);
                    }
                }
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, acc), egui::StrokeKind::Outside);
                if s.img.op.is_some() {
                    for h in markup::handles(r) {
                        painter.rect_filled(Rect::from_center_size(h, Vec2::splat(8.0)), 1.0, Color32::WHITE);
                        painter.rect_stroke(Rect::from_center_size(h, Vec2::splat(8.0)), 1.0, Stroke::new(1.0, acc), egui::StrokeKind::Middle);
                    }
                }
            }
            if self.img_sel.as_ref().map(|s| s.page == i).unwrap_or(false) {
                let movable = self.img_sel.as_ref().map(|s| s.img.op.is_some()).unwrap_or(false);
                resp.context_menu(|ui| {
                    if ui.button(t("Save Image As…", "Сохранить изображение как…")).clicked() {
                        ui.close_menu();
                        self.image_cmd(ImageCmd::SaveAs);
                    }
                    if ui.button(t("Copy Image", "Копировать изображение")).clicked() {
                        ui.close_menu();
                        self.image_cmd(ImageCmd::Copy);
                    }
                    if ui.add_enabled(movable, egui::Button::new(t("Replace Image…", "Заменить изображение…"))).clicked() {
                        ui.close_menu();
                        self.image_cmd(ImageCmd::Replace);
                    }
                    ui.separator();
                    if ui.add_enabled(movable, egui::Button::new(t("Delete Image", "Удалить изображение"))).clicked() {
                        ui.close_menu();
                        self.image_cmd(ImageCmd::Delete);
                    }
                });
            }
            if resp.secondary_clicked() {
                if let Some(p) = pos {
                    if hit(&marks, p).is_none() {
                        if let Some(im) = img_hit(p) {
                            self.selected = None;
                            self.img_sel = Some(crate::ImgSel { page: i, rect: im.rect, img: im, generation });
                        }
                    }
                }
            }
            if resp.double_clicked() {
                if let Some(p) = pos {
                    if let Some(m) = hit(&marks, p) {
                        match &m.kind {
                            Kind::Text(_) => {
                                self.doc.as_mut().unwrap().checkpoint(t("Edit Text", "Правка текста"));
                                self.editing = Some(m.id);
                            }
                            Kind::Note(s) => self.dialog = Some(crate::dialogs::Dialog::note(m.id, s.clone())),
                            Kind::Link(s) => self.dialog = Some(crate::dialogs::Dialog::link(m.id, s.clone())),
                            _ => {}
                        }
                    }
                }
            } else if resp.clicked() {
                if let Some(p) = pos {
                    self.selected = hit(&marks, p).map(|m| m.id);
                    if self.editing.is_some() && self.editing != self.selected {
                        self.editing = None;
                    }
                    self.img_sel = match (self.selected, img_hit(p)) {
                        (None, Some(im)) => {
                            if im.op.is_none() {
                                self.status = t(
                                    "This image is inside a form object: it can be saved or copied, not moved.",
                                    "Изображение находится внутри объекта формы: его можно сохранить или скопировать, но не переместить.",
                                )
                                .into();
                            } else {
                                self.status = t("Image selected: drag to move, handles to resize, Del to delete, right-click for more.", "Изображение выделено: перетаскивайте, меняйте размер за маркеры, Del — удалить, правая кнопка — ещё.").into();
                            }
                            Some(crate::ImgSel { page: i, rect: im.rect, img: im, generation })
                        }
                        _ => None,
                    };
                }
            }
            if resp.drag_started() {
                // Where the button went down: egui reports the start after the pointer has moved.
                if let Some(p) = ctx.input(|inp| inp.pointer.press_origin()).map(to_page).or(pos) {
                    // Handles of the selected mark first.
                    let sel = self.selected.and_then(|id| marks.iter().find(|m| m.id == id));
                    let mut started = false;
                    if let Some(m) = sel {
                        let r = Rect::from_two_pos(to_screen([m.rect[0], m.rect[1]]), to_screen([m.rect[2], m.rect[3]]));
                        let sp = to_screen(p);
                        if let Some(h) = markup::handles(r).iter().position(|h| h.distance(sp) < 7.0) {
                            self.doc.as_mut().unwrap().checkpoint(t("Resize", "Изменение размера"));
                            self.drag = Some(Drag::Resize { id: m.id, handle: h, last: p });
                            started = true;
                        }
                    }
                    if !started && sel.is_none() {
                        // The selected image: a handle resizes, the inside moves.
                        if let Some(s) = self.img_sel.as_ref().filter(|s| s.page == i && s.img.op.is_some()) {
                            let r = Rect::from_two_pos(to_screen([s.rect[0], s.rect[1]]), to_screen([s.rect[2], s.rect[3]]));
                            let sp = to_screen(p);
                            if let Some(h) = markup::handles(r).iter().position(|h| h.distance(sp) < 9.0) {
                                self.drag = Some(Drag::Image { handle: Some(h), last: p });
                                started = true;
                            } else if inside(&s.rect, p, 0.0) && hit(&marks, p).is_none() {
                                self.drag = Some(Drag::Image { handle: None, last: p });
                                started = true;
                            }
                        }
                    }
                    if !started && hit(&marks, p).is_none() {
                        if let Some(im) = img_hit(p).filter(|im| im.op.is_some()) {
                            self.selected = None;
                            self.img_sel = Some(crate::ImgSel { page: i, rect: im.rect, img: im, generation });
                            self.drag = Some(Drag::Image { handle: None, last: p });
                            started = true;
                        }
                    }
                    if !started {
                        if let Some(m) = hit(&marks, p) {
                            self.selected = Some(m.id);
                            self.doc.as_mut().unwrap().checkpoint(t("Move", "Перемещение"));
                            self.drag = Some(Drag::Move { id: m.id, last: p });
                        } else {
                            self.selected = None;
                        }
                    }
                }
            }
            if resp.dragged() {
                if let (Some(p), Some(Drag::Image { handle, last }), Some(s)) = (pos, &mut self.drag, &mut self.img_sel) {
                    let (dx, dy) = (p[0] - last[0], p[1] - last[1]);
                    match handle {
                        None => s.rect = [s.rect[0] + dx, s.rect[1] + dy, s.rect[2] + dx, s.rect[3] + dy],
                        Some(h) => {
                            let mut r = markup::drag_handle(s.rect, *h, dx, dy);
                            // Corners keep the proportions (Shift: free).
                            if [0, 2, 4, 6].contains(h) && !ctx.input(|inp| inp.modifiers.shift) {
                                let ratio = (s.img.rect[3] - s.img.rect[1]) / (s.img.rect[2] - s.img.rect[0]).max(0.01);
                                let w = (r[2] - r[0]).abs();
                                if *h == 0 || *h == 2 {
                                    r[1] = r[3] - w * ratio;
                                } else {
                                    r[3] = r[1] + w * ratio;
                                }
                            }
                            if (r[2] - r[0]).abs() > 3.0 && (r[3] - r[1]).abs() > 3.0 {
                                s.rect = markup::norm(r);
                            }
                        }
                    }
                    *last = p;
                }
                if let (Some(p), Some(d)) = (pos, &mut self.drag) {
                    let doc = self.doc.as_mut().unwrap();
                    match d {
                        Drag::Move { id, last } => {
                            if let Some(m) = doc.marks.iter_mut().find(|m| m.id == *id) {
                                m.translate(p[0] - last[0], p[1] - last[1]);
                            }
                            *last = p;
                        }
                        Drag::Resize { id, handle, last } => {
                            if let Some(m) = doc.marks.iter_mut().find(|m| m.id == *id) {
                                let keep_ratio = matches!(m.kind, Kind::Image(_)) && [0, 2, 4, 6].contains(handle);
                                let mut r = markup::drag_handle(m.rect, *handle, p[0] - last[0], p[1] - last[1]);
                                if keep_ratio {
                                    let ratio = (m.rect[3] - m.rect[1]) / (m.rect[2] - m.rect[0]).max(0.01);
                                    let w = (r[2] - r[0]).abs();
                                    if *handle == 0 || *handle == 2 {
                                        r[1] = r[3] - w * ratio;
                                    } else {
                                        r[3] = r[1] + w * ratio;
                                    }
                                }
                                if (r[2] - r[0]).abs() > 2.0 && (r[3] - r[1]).abs() > 2.0 {
                                    m.set_rect(r);
                                }
                            }
                            *last = p;
                        }
                        _ => {}
                    }
                }
            }
            if resp.drag_stopped() {
                if matches!(self.drag, Some(Drag::Image { .. })) {
                    self.drag = None;
                    self.image_cmd(ImageCmd::Commit);
                }
                self.drag = None;
            }
            return;
        }
        if self.tool == Tool::EditText {
            let acc = kui::accent(&ctx);
            let lines = self.doc.as_mut().unwrap().page_lines(i).cloned();
            let edited: Vec<[f32; 4]> = marks.iter().filter_map(|m| m.cover).collect();
            let free = |l: &crate::textlayer::Line| !edited.iter().any(|c| overlaps(c, &l.rect));
            match &lines {
                None => {
                    painter.text(rect.left_top() + Vec2::new(10.0, 8.0), egui::Align2::LEFT_TOP, t("Recognising text…", "Распознавание текста…"), egui::FontId::proportional(13.0), acc);
                    ctx.request_repaint_after(std::time::Duration::from_millis(150));
                }
                Some(ls) if ls.is_empty() => {
                    painter.text(rect.left_top() + Vec2::new(10.0, 8.0), egui::Align2::LEFT_TOP, t("No text found on this page", "На странице не найден текст"), egui::FontId::proportional(13.0), Color32::GRAY);
                }
                Some(ls) => {
                    let hp = hover.map(to_page);
                    for l in ls.iter().filter(|l| free(l)) {
                        let r = Rect::from_two_pos(to_screen([l.rect[0], l.rect[1]]), to_screen([l.rect[2], l.rect[3]])).expand(1.5);
                        let over = hp.map(|p| inside(&l.rect, p, 1.5)).unwrap_or(false);
                        if over {
                            painter.rect_filled(r, 2.0, acc.gamma_multiply(0.12));
                        }
                        painter.rect_stroke(r, 2.0, Stroke::new(if over { 1.5 } else { 0.7 }, if over { acc } else { acc.gamma_multiply(0.4) }), egui::StrokeKind::Outside);
                    }
                }
            }
            if resp.clicked() {
                if let Some(p) = pos {
                    let existing = marks.iter().rev().find(|m| matches!(m.kind, Kind::Text(_)) && (inside(&m.rect, p, 0.0) || m.cover.map(|c| inside(&c, p, 0.0)).unwrap_or(false)));
                    let line = lines.as_ref().and_then(|ls| ls.iter().find(|l| free(l) && inside(&l.rect, p, 1.5)));
                    let doc = self.doc.as_mut().unwrap();
                    if let Some(m) = existing {
                        doc.checkpoint(t("Edit Text", "Правка текста"));
                        self.selected = Some(m.id);
                        self.editing = Some(m.id);
                    } else if let Some(l) = line {
                        doc.checkpoint(t("Edit Text", "Правка текста"));
                        let m = markup::line_edit(i, l, &l.text);
                        self.selected = Some(m.id);
                        self.editing = Some(m.id);
                        doc.marks.push(m);
                    } else {
                        self.editing = None;
                        self.selected = None;
                    }
                }
            }
            return;
        }
        // Drawing tools.
        if resp.clicked() {
            if let Some(p) = pos {
                let doc = self.doc.as_mut().unwrap();
                match self.tool {
                    Tool::Text => {
                        doc.checkpoint(t("Text Box", "Текстовое поле"));
                        let mut m = Mark::new(i, [p[0], p[1], p[0] + 200.0, p[1] + self.font_size * 1.6], Kind::Text(String::new()), self.color, 1.0);
                        m.font_size = self.font_size;
                        m.fill = self.fill;
                        self.selected = Some(m.id);
                        self.editing = Some(m.id);
                        doc.marks.push(m);
                        self.tool = Tool::Select;
                    }
                    Tool::Note => {
                        doc.checkpoint(t("Note", "Заметка"));
                        let m = Mark::new(i, [p[0], p[1], p[0] + 22.0, p[1] + 22.0], Kind::Note(String::new()), self.color, 1.0);
                        self.dialog = Some(crate::dialogs::Dialog::note(m.id, String::new()));
                        self.selected = Some(m.id);
                        doc.marks.push(m);
                        self.tool = Tool::Select;
                    }
                    _ => {}
                }
            }
        }
        if resp.drag_started() {
            if let Some(p) = pos {
                self.drag = Some(Drag::Create { page: i, start: p, cur: p, points: vec![p] });
            }
        }
        if resp.dragged() {
            if let (Some(p), Some(Drag::Create { cur, points, .. })) = (pos, &mut self.drag) {
                let p = [p[0].clamp(0.0, rect.width() / k), p[1].clamp(0.0, rect.height() / k)];
                *cur = p;
                if points.last().map(|l| (l[0] - p[0]).abs() + (l[1] - p[1]).abs() > 0.8).unwrap_or(true) {
                    points.push(p);
                }
            }
        }
        if let Some(Drag::Create { page, start, cur, points }) = &self.drag {
            if *page == i {
                let r = Rect::from_two_pos(to_screen(*start), to_screen(*cur));
                let col = markup::c32(self.color);
                match self.tool {
                    Tool::Pen => {
                        let pts: Vec<Pos2> = points.iter().map(|p| to_screen(*p)).collect();
                        painter.add(egui::Shape::line(pts, Stroke::new(self.width * k, col)));
                    }
                    Tool::Line | Tool::Arrow => {
                        painter.line_segment([to_screen(*start), to_screen(*cur)], Stroke::new(self.width * k, col));
                    }
                    Tool::Ellipse => {
                        painter.add(egui::epaint::EllipseShape { center: r.center(), radius: r.size() / 2.0, fill: Color32::TRANSPARENT, stroke: Stroke::new(self.width * k, col) });
                    }
                    Tool::Highlight => {
                        painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(self.color[0], self.color[1], self.color[2], 110));
                    }
                    _ => {
                        painter.rect_stroke(r, 0.0, Stroke::new(1.0, col), egui::StrokeKind::Middle);
                    }
                }
            }
        }
        if resp.drag_stopped() {
            if let Some(Drag::Create { page, start, cur, points }) = self.drag.take() {
                let r = markup::norm([start[0], start[1], cur[0], cur[1]]);
                let big = (r[2] - r[0]) > 3.0 || (r[3] - r[1]) > 3.0;
                let doc = self.doc.as_mut().unwrap();
                let kind = match self.tool {
                    Tool::Highlight => Some(Kind::Highlight),
                    Tool::Underline => Some(Kind::Underline),
                    Tool::Strike => Some(Kind::Strike),
                    Tool::Pen if points.len() > 1 => Some(Kind::Ink(vec![points.clone()])),
                    Tool::Rect => Some(Kind::Rect),
                    Tool::Ellipse => Some(Kind::Ellipse),
                    Tool::Line => Some(Kind::Line(start, cur)),
                    Tool::Arrow => Some(Kind::Arrow(start, cur)),
                    Tool::Whiteout => Some(Kind::Whiteout),
                    Tool::Redact => Some(Kind::Redact),
                    Tool::Link => Some(Kind::Link(String::new())),
                    Tool::Text => Some(Kind::Text(String::new())),
                    Tool::FieldText => Some(Kind::FieldText(format!("Text{}", doc.marks.len() + doc.fields.len() + 1))),
                    Tool::FieldCheck => Some(Kind::FieldCheck(format!("Check{}", doc.marks.len() + doc.fields.len() + 1))),
                    Tool::Crop => {
                        self.crop_rect = Some((page, r));
                        self.dialog = Some(crate::dialogs::Dialog::crop_from_rect(page, r, doc.sizes[page]));
                        None
                    }
                    _ => None,
                };
                if let Some(kind) = kind {
                    if big || matches!(kind, Kind::Ink(_)) {
                        let rect = match &kind {
                            Kind::Ink(p) => bbox(p.iter().flatten(), self.width),
                            Kind::Line(a, b) | Kind::Arrow(a, b) => bbox([a, b].into_iter(), self.width * 3.0),
                            _ => r,
                        };
                        doc.checkpoint(self.tool.name());
                        let mut m = Mark::new(page, rect, kind, self.color, self.width);
                        m.fill = if matches!(self.tool, Tool::Rect | Tool::Ellipse | Tool::Text) { self.fill } else { None };
                        m.font_size = self.font_size;
                        if matches!(self.tool, Tool::FieldCheck) {
                            let s = (r[2] - r[0]).min(r[3] - r[1]).max(10.0);
                            m.rect = [r[0], r[1], r[0] + s, r[1] + s];
                        }
                        let id = m.id;
                        let is_link = matches!(m.kind, Kind::Link(_));
                        let is_text = matches!(m.kind, Kind::Text(_));
                        doc.marks.push(m);
                        if is_link {
                            self.dialog = Some(crate::dialogs::Dialog::link(id, String::new()));
                        }
                        if is_text {
                            self.editing = Some(id);
                            self.selected = Some(id);
                            self.tool = Tool::Select;
                        }
                    }
                }
            }
        }
    }

    pub fn sidebar_ui(&mut self, ui: &mut egui::Ui, tab: SideTab) {
        ui.horizontal(|ui| {
            for (t_, icon, tip) in [
                (SideTab::Thumbnails, ic::PAGE, t("Thumbnails", "Миниатюры")),
                (SideTab::Bookmarks, ic::BOOKMARK, t("Bookmarks", "Закладки")),
                (SideTab::Attachments, "\u{E723}", t("Attachments", "Вложения")),
                (SideTab::Fields, "\u{E8D3}", t("Form Fields", "Поля формы")),
                (SideTab::Marks, ic::EDIT, t("Markup", "Пометки")),
                (SideTab::Search, ic::SEARCH, t("Search", "Поиск")),
            ] {
                if kui::tool_toggle(ui, tab == t_, icon, tip).clicked() {
                    self.sidebar = Some(t_);
                }
            }
        });
        ui.separator();
        match tab {
            SideTab::Thumbnails => self.thumbnails(ui),
            SideTab::Bookmarks => self.bookmarks(ui),
            SideTab::Attachments => self.attachments_ui(ui),
            SideTab::Fields => self.fields_ui(ui),
            SideTab::Search => self.search_ui(ui),
            SideTab::Marks => self.marks_ui(ui),
        }
    }

    fn thumbnails(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(doc) = &mut self.doc else { return };
        let n = doc.page_count();
        let w = (ui.available_width() - 30.0).clamp(60.0, 260.0);
        let mut clicked = None;
        let mut rects = Vec::with_capacity(n);
        let mut drop_target = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for i in 0..n {
                let (pw, ph) = doc.sizes[i];
                let h = w * ph / pw.max(1.0);
                ui.vertical_centered(|ui| {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click_and_drag());
                    rects.push(rect);
                    if ui.is_rect_visible(rect) {
                        if let Some((_, rx)) = doc.thumb_pending.get(&i).map(|r| ((), r)) {
                            if let Ok(res) = rx.try_recv() {
                                doc.thumb_pending.remove(&i);
                                if let Ok(img) = res {
                                    doc.thumbs.insert(i, kui::texture(&ctx, &format!("t{}-{}", doc.generation, i), &img));
                                }
                            } else {
                                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                            }
                        } else if !doc.thumbs.contains_key(&i) && doc.thumb_pending.len() < 3 {
                            if let Some(r) = &doc.renderer {
                                doc.thumb_pending.insert(i, r.request(i as u32, (w * 1.5) as u32));
                            }
                        }
                        let p = ui.painter();
                        p.rect_filled(rect, 0.0, Color32::WHITE);
                        if let Some(tex) = doc.thumbs.get(&i) {
                            p.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                        }
                        let selected = self.pages_sel.contains(&i) || (self.pages_sel.is_empty() && self.current == i);
                        let stroke = if selected { Stroke::new(3.0, kui::accent(&ctx)) } else { Stroke::new(1.0, Color32::from_gray(170)) };
                        p.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Outside);
                        if doc.marks.iter().any(|m| m.page == i) {
                            p.circle_filled(rect.right_top() + Vec2::new(-8.0, 8.0), 4.0, Color32::from_rgb(230, 120, 0));
                        }
                    }
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                    if resp.drag_started() {
                        self.drag = Some(Drag::Thumb { from: i });
                    }
                    resp.context_menu(|ui| {
                        if ui.button(t("Rotate Right", "Повернуть вправо")).clicked() {
                            self.actions.push(Action::Rotate(90));
                            ui.close_menu();
                        }
                        if ui.button(t("Rotate Left", "Повернуть влево")).clicked() {
                            self.actions.push(Action::Rotate(-90));
                            ui.close_menu();
                        }
                        if ui.button(t("Insert Blank Page After", "Вставить пустую страницу после")).clicked() {
                            self.actions.push(Action::InsertBlank);
                            ui.close_menu();
                        }
                        if ui.button(t("Duplicate", "Дублировать")).clicked() {
                            self.actions.push(Action::DuplicatePages);
                            ui.close_menu();
                        }
                        if ui.button(t("Extract…", "Извлечь…")).clicked() {
                            self.actions.push(Action::ExtractPages);
                            ui.close_menu();
                        }
                        if ui.button(t("Delete", "Удалить")).clicked() {
                            self.actions.push(Action::DeletePages);
                            ui.close_menu();
                        }
                    });
                    ui.label(egui::RichText::new(format!("{}", i + 1)).small());
                });
            }
            if let Some(Drag::Thumb { from }) = &self.drag {
                if let Some(pos) = ctx.pointer_interact_pos() {
                    let target = rects.iter().position(|r| pos.y < r.center().y).unwrap_or(n);
                    let y = if target < n { rects[target].top() - 4.0 } else { rects.last().map(|r| r.bottom() + 4.0).unwrap_or(0.0) };
                    let x0 = rects.first().map(|r| r.left()).unwrap_or(0.0);
                    ui.painter().line_segment([Pos2::new(x0 - 6.0, y), Pos2::new(x0 + w + 6.0, y)], Stroke::new(3.0, kui::accent(&ctx)));
                    if ctx.input(|i| i.pointer.any_released()) {
                        drop_target = Some((*from, target));
                    }
                }
            }
        });
        if let Some(i) = clicked {
            let (ctrl, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
            if ctrl {
                if !self.pages_sel.insert(i) {
                    self.pages_sel.remove(&i);
                }
            } else if shift {
                let (a, b) = (self.current.min(i), self.current.max(i));
                self.pages_sel = (a..=b).collect();
            } else {
                self.pages_sel.clear();
            }
            self.current = i;
            self.scroll_to = Some(i);
        }
        if let Some((from, to)) = drop_target {
            self.drag = None;
            let moving: Vec<usize> = if self.pages_sel.contains(&from) { self.pages_sel.iter().cloned().collect() } else { vec![from] };
            if !(moving.len() == 1 && (to == from || to == from + 1)) {
                let mut order: Vec<usize> = (0..n).filter(|p| !moving.contains(p)).collect();
                let insert_at = order.iter().position(|&p| p >= to).unwrap_or(order.len());
                for (k2, m) in moving.iter().enumerate() {
                    order.insert(insert_at + k2, *m);
                }
                self.reorder(order);
                self.pages_sel.clear();
                self.current = insert_at;
            }
        }
    }

    fn bookmarks(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &mut self.doc else { return };
        let mut list = doc.outline.clone();
        let mut changed = false;
        let mut goto = None;
        ui.horizontal(|ui| {
            if kui::tool_button(ui, ic::ADD, t("Add Bookmark for This Page", "Добавить закладку на эту страницу"), true).clicked() {
                self.actions.push(Action::AddBookmark);
            }
        });
        let sel_key = egui::Id::new("bmsel");
        let mut sel: Option<usize> = ui.ctx().data(|d| d.get_temp(sel_key));
        if list.is_empty() {
            ui.label(egui::RichText::new(t("No bookmarks. Ctrl+D adds one for the current page.", "Закладок нет. Ctrl+D добавляет закладку на текущую страницу.")).weak());
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for i in 0..list.len() {
                ui.horizontal(|ui| {
                    ui.add_space(list[i].level as f32 * 14.0);
                    if sel == Some(i) {
                        let r = ui.add(egui::TextEdit::singleline(&mut list[i].title).desired_width(ui.available_width() - 30.0));
                        if r.changed() {
                            changed = true;
                        }
                        ui.label(format!("{}", list[i].page + 1));
                    } else {
                        let r = ui.selectable_label(false, format!("{}   {}", list[i].title, list[i].page + 1));
                        if r.clicked() {
                            goto = Some(list[i].page);
                            sel = Some(i);
                        }
                    }
                });
            }
        });
        if let Some(i) = sel.filter(|&i| i < list.len()) {
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if kui::tool_button(ui, ic::UP, t("Move Up", "Выше"), i > 0).clicked() {
                    list.swap(i, i - 1);
                    sel = Some(i - 1);
                    changed = true;
                }
                if kui::tool_button(ui, ic::DOWN, t("Move Down", "Ниже"), i + 1 < list.len()).clicked() {
                    list.swap(i, i + 1);
                    sel = Some(i + 1);
                    changed = true;
                }
                if kui::tool_button(ui, ic::BACK, t("Outdent", "Уровень выше"), list[i].level > 0).clicked() {
                    list[i].level -= 1;
                    changed = true;
                }
                if kui::tool_button(ui, ic::FORWARD, t("Indent", "Уровень ниже"), i > 0 && list[i].level <= list[i - 1].level).clicked() {
                    list[i].level += 1;
                    changed = true;
                }
                if kui::tool_button(ui, ic::PAGE, t("Set to Current Page", "Привязать к текущей странице"), true).clicked() {
                    list[i].page = self.current;
                    changed = true;
                }
                if kui::tool_button(ui, ic::DELETE, t("Delete", "Удалить"), true).clicked() {
                    list.remove(i);
                    sel = None;
                    changed = true;
                }
            });
        }
        ui.ctx().data_mut(|d| match sel {
            Some(s) => d.insert_temp(sel_key, s),
            None => d.remove::<usize>(sel_key),
        });
        if changed {
            let l2 = list.clone();
            if let Err(e) = doc.edit(t("Edit Bookmarks", "Правка закладок"), |d| crate::pdfops::write_outline(d, &l2)) {
                kui::error(crate::APP, &e);
            }
        }
        if let Some(p) = goto {
            self.current = p;
            self.scroll_to = Some(p);
        }
    }

    fn attachments_ui(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &mut self.doc else { return };
        if ui.button(format!("{}  {}", ic::ADD, t("Attach File…", "Вложить файл…"))).clicked() {
            self.actions.push(Action::Dialog(crate::dialogs::DialogKind::Attach));
        }
        if doc.attachments.is_empty() {
            ui.label(egui::RichText::new(t("No attached files.", "Вложенных файлов нет.")).weak());
        }
        let mut remove = None;
        for (i, a) in doc.attachments.iter().enumerate() {
            ui.group(|ui| {
                ui.label(egui::RichText::new(&a.name).strong());
                ui.label(egui::RichText::new(format!("{:.1} KB  {}", a.size as f32 / 1024.0, a.description)).small());
                ui.horizontal(|ui| {
                    if ui.small_button(t("Save…", "Сохранить…")).clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_file_name(&a.name).save_file() {
                            let _ = kit42::write_atomic(&p, &a.data);
                        }
                    }
                    if ui.small_button(t("Open", "Открыть")).clicked() {
                        let tmp = std::env::temp_dir().join("Suite42").join(a.name.replace(['\\', '/', ':'], "_"));
                        std::fs::create_dir_all(tmp.parent().unwrap()).ok();
                        if std::fs::write(&tmp, &a.data).is_ok() {
                            kit42::shell::open_path(&tmp);
                        }
                    }
                    if ui.small_button(t("Delete", "Удалить")).clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove {
            let mut list = doc.attachments.clone();
            list.remove(i);
            if let Err(e) = doc.edit(t("Delete Attachment", "Удаление вложения"), |d| crate::pdfops::write_attachments(d, &list)) {
                kui::error(crate::APP, &e);
            }
        }
    }

    fn fields_ui(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &mut self.doc else { return };
        if doc.fields.is_empty() {
            ui.label(egui::RichText::new(t("This document has no form fields. Use Form › Add Text Field to create one.", "В документе нет полей формы. Создайте поле через меню «Форма»."))
                .weak());
            return;
        }
        let mut fields = doc.fields.clone();
        let mut changed = false;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for f in fields.iter_mut() {
                ui.label(egui::RichText::new(&f.name).small());
                match &f.kind {
                    crate::pdfops::FieldKind::Text => {
                        changed |= ui.text_edit_singleline(&mut f.value).lost_focus();
                    }
                    crate::pdfops::FieldKind::Check | crate::pdfops::FieldKind::Radio => {
                        let mut on = !f.value.is_empty() && f.value != "Off";
                        if ui.checkbox(&mut on, "").changed() {
                            f.value = if on { if f.on_state.is_empty() { "Yes".into() } else { f.on_state.clone() } } else { "Off".into() };
                            changed = true;
                        }
                    }
                    crate::pdfops::FieldKind::Choice(opts) => {
                        egui::ComboBox::from_id_salt(&f.name).selected_text(&f.value).show_ui(ui, |ui| {
                            for o in opts {
                                if ui.selectable_label(f.value == *o, o).clicked() {
                                    f.value = o.clone();
                                    changed = true;
                                }
                            }
                        });
                    }
                    crate::pdfops::FieldKind::Signature => {
                        ui.label(t("Signature field", "Поле подписи"));
                    }
                    crate::pdfops::FieldKind::Button => {
                        ui.label(t("Button", "Кнопка"));
                    }
                }
                if let Some((p, _)) = f.widgets.first() {
                    if *p != usize::MAX && ui.small_button(format!("→ {}", p + 1)).clicked() {
                        self.current = *p;
                        self.scroll_to = Some(*p);
                    }
                }
                ui.add_space(4.0);
            }
        });
        if changed {
            let list = fields.clone();
            if let Err(e) = doc.edit(t("Fill Form", "Заполнение формы"), |d| crate::pdfops::write_field_values(d, &list)) {
                kui::error(crate::APP, &e);
            }
        } else {
            doc.fields = fields;
        }
    }

    fn search_ui(&mut self, ui: &mut egui::Ui) {
        let r = ui.add(egui::TextEdit::singleline(&mut self.search).id(egui::Id::new("searchbox")).hint_text(t("Find in document", "Найти в документе")));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.search.trim().is_empty() {
            let q = self.search.to_lowercase();
            let texts = self.doc.as_mut().map(|d| d.texts().clone()).unwrap_or_default();
            self.results.clear();
            for (p, text) in texts.iter().enumerate() {
                let lower = text.to_lowercase();
                let mut start = 0;
                while let Some(pos) = lower[start..].find(&q) {
                    let at = start + pos;
                    let ctx_start = lower[..at].char_indices().rev().nth(30).map(|(i, _)| i).unwrap_or(0);
                    let ctx_end = text[at..].char_indices().nth(q.chars().count() + 30).map(|(i, _)| at + i).unwrap_or(text.len());
                    let snippet: String = text.get(ctx_start..ctx_end).unwrap_or("").replace('\n', " ");
                    self.results.push((p, snippet));
                    start = at + q.len().max(1);
                    if self.results.len() > 2000 {
                        break;
                    }
                }
            }
            if let Some((p, _)) = self.results.first() {
                self.current = *p;
                self.scroll_to = Some(*p);
            }
        }
        ui.label(egui::RichText::new(format!("{}: {}", t("Found", "Найдено"), self.results.len())).small());
        if self.results.is_empty() && !self.search.is_empty() {
            ui.label(egui::RichText::new(t("Scanned pages have no text until you run Tools › Recognize Text.", "У отсканированных страниц нет текста, пока не выполнено распознавание (Инструменты › Распознать текст).")).small().weak());
        }
        let mut goto = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (p, s) in &self.results {
                if ui.selectable_label(*p == self.current, format!("{} {}: {}", t("p.", "с."), p + 1, s)).clicked() {
                    goto = Some(*p);
                }
            }
        });
        if let Some(p) = goto {
            self.current = p;
            self.scroll_to = Some(p);
        }
    }

    fn marks_ui(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &self.doc else { return };
        if doc.marks.is_empty() {
            ui.label(egui::RichText::new(t("No unsaved markup. Saved markup becomes PDF annotations.", "Несохранённых пометок нет. Сохранённые пометки становятся аннотациями PDF.")).weak());
        }
        let mut goto = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for m in &doc.marks {
                let label = match &m.kind {
                    Kind::Text(s) | Kind::Note(s) => format!("{}: {}", kind_name(&m.kind), s.chars().take(40).collect::<String>()),
                    k => kind_name(k).to_string(),
                };
                if ui.selectable_label(self.selected == Some(m.id), format!("{} {} · {}", t("p.", "с."), m.page + 1, label)).clicked() {
                    goto = Some((m.page, m.id));
                }
            }
        });
        if let Some((p, id)) = goto {
            self.current = p;
            self.scroll_to = Some(p);
            self.selected = Some(id);
        }
    }
}

fn kind_name(k: &Kind) -> &'static str {
    match k {
        Kind::Highlight => t("Highlight", "Выделение"),
        Kind::Underline => t("Underline", "Подчёркивание"),
        Kind::Strike => t("Strikethrough", "Зачёркивание"),
        Kind::Ink(_) => t("Drawing", "Рисунок"),
        Kind::Rect => t("Rectangle", "Прямоугольник"),
        Kind::Ellipse => t("Oval", "Овал"),
        Kind::Line(..) => t("Line", "Линия"),
        Kind::Arrow(..) => t("Arrow", "Стрелка"),
        Kind::Text(_) => t("Text", "Текст"),
        Kind::Note(_) => t("Note", "Заметка"),
        Kind::Whiteout => t("Whiteout", "Замазка"),
        Kind::Redact => t("Redaction", "Скрытие"),
        Kind::Image(_) => t("Signature/stamp", "Подпись/печать"),
        Kind::Link(_) => t("Link", "Ссылка"),
        Kind::FieldText(_) => t("Text field", "Текстовое поле"),
        Kind::FieldCheck(_) => t("Checkbox", "Флажок"),
    }
}

fn hit(marks: &[Mark], p: [f32; 2]) -> Option<&Mark> {
    marks.iter().rev().find(|m| {
        let pad = 3.0;
        p[0] >= m.rect[0] - pad && p[0] <= m.rect[2] + pad && p[1] >= m.rect[1] - pad && p[1] <= m.rect[3] + pad
    })
}

fn bbox<'a>(pts: impl Iterator<Item = &'a [f32; 2]>, pad: f32) -> [f32; 4] {
    let mut r = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in pts {
        r[0] = r[0].min(p[0]);
        r[1] = r[1].min(p[1]);
        r[2] = r[2].max(p[0]);
        r[3] = r[3].max(p[1]);
    }
    [r[0] - pad, r[1] - pad, r[2] + pad, r[3] + pad]
}

#[allow(dead_code)]
fn arc(img: image::RgbaImage) -> Arc<image::RgbaImage> {
    Arc::new(img)
}

fn inside(r: &[f32; 4], p: [f32; 2], pad: f32) -> bool {
    p[0] >= r[0] - pad && p[0] <= r[2] + pad && p[1] >= r[1] - pad && p[1] <= r[3] + pad
}

fn overlaps(a: &[f32; 4], b: &[f32; 4]) -> bool {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    ix * iy > 0.5 * (b[2] - b[0]) * (b[3] - b[1])
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ImageCmd {
    /// Writes the dragged box of the selected image into the page.
    Commit,
    SaveAs,
    Copy,
    Replace,
    Delete,
}

impl App {
    /// The selected image's pixels: its own data when it can be decoded, else the rendered page.
    fn image_pixels(&mut self) -> Option<(Vec<u8>, &'static str)> {
        let s = self.img_sel.as_ref()?;
        let (page, img) = (s.page, s.img.clone());
        let doc = self.doc.as_mut()?;
        if let Some(out) = doc.parsed().and_then(|d| crate::pageimg::extract(&d, &img)) {
            return Some(out);
        }
        let (w, _) = *doc.sizes.get(page)?;
        let px = (w / 72.0 * 300.0) as u32;
        let full = doc.renderer.as_ref()?.render(page as u32, px).ok()?;
        let k = full.width() as f32 / w;
        let r = img.rect;
        let (x0, y0) = ((r[0] * k).max(0.0) as u32, (r[1] * k).max(0.0) as u32);
        let cw = ((r[2] - r[0]) * k) as u32;
        let ch = ((r[3] - r[1]) * k) as u32;
        let crop = image::imageops::crop_imm(&full, x0, y0, cw.min(full.width().saturating_sub(x0)), ch.min(full.height().saturating_sub(y0))).to_image();
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(crop).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
        Some((out, "png"))
    }

    pub fn image_cmd(&mut self, cmd: ImageCmd) {
        let Some(s) = self.img_sel.as_ref() else { return };
        let (page, op, from, to) = (s.page, s.img.op, s.img.rect, s.rect);
        match cmd {
            ImageCmd::SaveAs => {
                let Some((bytes, ext)) = self.image_pixels() else {
                    kui::error(crate::APP, t("The image could not be read.", "Не удалось прочитать изображение."));
                    return;
                };
                let stem = self.doc.as_ref().and_then(|d| d.path.as_ref()).and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
                let filter = if ext == "jpg" { ("JPEG", &["jpg"][..]) } else { ("PNG", &["png"][..]) };
                if let Some(p) = kui::open_dialog(t("Save Image As", "Сохранить изображение"), &[filter]).set_file_name(format!("{stem} {} .{ext}", page + 1).replace(" .", ".")).save_file() {
                    kui::remember_dir(&p);
                    match kit42::write_atomic(&p, &bytes) {
                        Ok(()) => self.status = format!("{} {}", t("Saved:", "Сохранено:"), kit42::file_name(&p)),
                        Err(e) => kui::error(crate::APP, &e.to_string()),
                    }
                }
            }
            ImageCmd::Copy => {
                let Some((bytes, _)) = self.image_pixels() else { return };
                let Ok(img) = image::load_from_memory(&bytes) else { return };
                let rgba = img.to_rgba8();
                let data = arboard::ImageData { width: rgba.width() as usize, height: rgba.height() as usize, bytes: std::borrow::Cow::Owned(rgba.into_raw()) };
                match arboard::Clipboard::new().and_then(|mut c| c.set_image(data)) {
                    Ok(()) => self.status = t("Image copied.", "Изображение скопировано.").into(),
                    Err(e) => kui::error(crate::APP, &e.to_string()),
                }
            }
            ImageCmd::Commit | ImageCmd::Replace | ImageCmd::Delete => {
                let Some(op) = op else { return };
                let replacement = if cmd == ImageCmd::Replace {
                    let Some(p) = kui::open_dialog(t("Replace Image", "Заменить изображение"), &[(t("Images", "Изображения"), &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff"])]).pick_file() else { return };
                    match std::fs::read(&p) {
                        Ok(b) => Some(b),
                        Err(e) => return kui::error(crate::APP, &e.to_string()),
                    }
                } else {
                    None
                };
                if cmd == ImageCmd::Commit && (0..4).all(|k| (from[k] - to[k]).abs() < 0.05) {
                    return;
                }
                let label = match cmd {
                    ImageCmd::Commit => t("Move Image", "Перемещение изображения"),
                    ImageCmd::Replace => t("Replace Image", "Замена изображения"),
                    _ => t("Delete Image", "Удаление изображения"),
                };
                let Some(doc) = self.doc.as_mut() else { return };
                let res = doc.edit(label, |d| {
                    let pid = *crate::pdfops::page_ids(d).get(page).ok_or("no page")?;
                    match cmd {
                        ImageCmd::Commit => crate::pageimg::transform(d, pid, op, from, to),
                        ImageCmd::Replace => crate::pageimg::replace(d, pid, op, replacement.as_deref().unwrap_or_default()),
                        _ => crate::pageimg::delete(d, pid, op),
                    }
                });
                match res {
                    Ok(()) => {
                        if cmd == ImageCmd::Delete {
                            self.img_sel = None;
                        } else if let Some(s) = &mut self.img_sel {
                            // Found again by its box after the page is re-read.
                            s.img.rect = s.rect;
                            if cmd == ImageCmd::Replace {
                                let r = s.rect;
                                if let Some(im) = self.doc.as_mut().unwrap().page_images(page).into_iter().rev().find(|im| overlaps(&r, &im.rect)) {
                                    s.rect = im.rect;
                                    s.img.rect = im.rect;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(s) = &mut self.img_sel {
                            s.rect = s.img.rect;
                        }
                        kui::error(crate::APP, &e);
                    }
                }
            }
        }
    }
}
