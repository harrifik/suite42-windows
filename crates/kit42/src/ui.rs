//! Windows 11 look for egui and small widgets every app uses: icon buttons, colour
//! swatches, native message boxes, the language menu and the unsaved-changes guard.

use crate::t;
use egui::{Color32, CornerRadius, FontFamily, FontId, Response, RichText, Stroke, Ui, Vec2};

pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(0x00, 0x67, 0xC0);
pub const ACCENT_DARK: Color32 = Color32::from_rgb(0x4C, 0xC2, 0xFF);

pub fn accent(ctx: &egui::Context) -> Color32 {
    if ctx.style().visuals.dark_mode {
        ACCENT_DARK
    } else {
        ACCENT_LIGHT
    }
}

fn visuals(dark: bool) -> egui::Visuals {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let accent = if dark { ACCENT_DARK } else { ACCENT_LIGHT };
    let r = CornerRadius::same(4);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.selection.bg_fill = if dark { Color32::from_rgb(0x1E, 0x4F, 0x7A) } else { Color32::from_rgb(0xCC, 0xE4, 0xF7) };
    v.selection.stroke = Stroke::new(1.0, accent);
    v.hyperlink_color = accent;
    if dark {
        v.panel_fill = Color32::from_rgb(0x20, 0x20, 0x20);
        v.window_fill = Color32::from_rgb(0x2B, 0x2B, 0x2B);
        v.extreme_bg_color = Color32::from_rgb(0x1A, 0x1A, 0x1A);
        v.faint_bg_color = Color32::from_rgb(0x27, 0x27, 0x27);
    } else {
        v.panel_fill = Color32::from_rgb(0xF3, 0xF3, 0xF3);
        v.window_fill = Color32::from_rgb(0xFB, 0xFB, 0xFB);
        v.extreme_bg_color = Color32::WHITE;
        v.faint_bg_color = Color32::from_rgb(0xEE, 0xEE, 0xEE);
    }
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    if !dark {
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0xFB, 0xFB, 0xFB);
        v.widgets.inactive.bg_fill = Color32::from_rgb(0xFB, 0xFB, 0xFB);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xE0, 0xE0, 0xE0));
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0xF0, 0xF0, 0xF0);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xD0, 0xD0, 0xD0));
    }
    v
}

pub fn install_style(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Light, visuals(false));
    ctx.set_visuals_of(egui::Theme::Dark, visuals(true));
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(6.0, 5.0);
        s.spacing.button_padding = Vec2::new(8.0, 3.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.menu_margin = egui::Margin::same(6);
        use egui::TextStyle::*;
        s.text_styles.insert(Body, FontId::proportional(14.0));
        s.text_styles.insert(Button, FontId::proportional(14.0));
        s.text_styles.insert(Small, FontId::proportional(11.5));
        s.text_styles.insert(Heading, FontId::new(20.0, FontFamily::Name(crate::fonts::UI_BOLD.into())));
        s.text_styles.insert(Monospace, FontId::monospace(13.0));
    });
    ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::System);
}

/// Text in the Fluent icon font.
pub fn icon_text(icon: &str, size: f32) -> RichText {
    RichText::new(icon).font(FontId::new(size, FontFamily::Name(crate::fonts::ICON_FAMILY.into())))
}

/// A flat toolbar button showing an icon, with a tooltip.
pub fn tool_button(ui: &mut Ui, icon: &str, tip: &str, enabled: bool) -> Response {
    let b = egui::Button::new(icon_text(icon, 16.0)).frame(false).min_size(Vec2::new(30.0, 28.0));
    ui.add_enabled(enabled, b).on_hover_text(tip)
}

/// A toolbar toggle: highlighted when `on`.
pub fn tool_toggle(ui: &mut Ui, on: bool, icon: &str, tip: &str) -> Response {
    let b = egui::Button::new(icon_text(icon, 16.0))
        .frame(on)
        .selected(on)
        .min_size(Vec2::new(30.0, 28.0));
    ui.add(b).on_hover_text(tip)
}

/// Icon + label button for prominent toolbar actions.
pub fn tool_label_button(ui: &mut Ui, icon: &str, label: &str, enabled: bool) -> Response {
    let job = {
        let mut job = egui::text::LayoutJob::default();
        let color = ui.visuals().text_color();
        job.append(
            icon,
            0.0,
            egui::TextFormat::simple(FontId::new(15.0, FontFamily::Name(crate::fonts::ICON_FAMILY.into())), color),
        );
        job.append(label, 6.0, egui::TextFormat::simple(FontId::proportional(13.5), color));
        job
    };
    ui.add_enabled(enabled, egui::Button::new(job).frame(false).min_size(Vec2::new(0.0, 28.0)))
}

/// A small coloured square that opens a palette; returns true when the colour changed.
pub fn color_button(ui: &mut Ui, color: &mut [u8; 3], tip: &str) -> bool {
    let mut changed = false;
    let c = Color32::from_rgb(color[0], color[1], color[2]);
    let id = ui.make_persistent_id(("k42color", tip));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(26.0, 24.0), egui::Sense::click());
    let painter = ui.painter();
    let inner = rect.shrink(4.0);
    painter.rect_filled(inner, 3.0, c);
    painter.rect_stroke(inner, 3.0, Stroke::new(1.0, Color32::from_gray(120)), egui::StrokeKind::Inside);
    if resp.hovered() {
        painter.rect_stroke(rect, 4.0, ui.visuals().widgets.hovered.bg_stroke, egui::StrokeKind::Inside);
    }
    let resp = resp.on_hover_text(tip);
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    egui::popup::popup_below_widget(ui, id, &resp, egui::popup::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_min_width(200.0);
        if palette(ui, color) {
            changed = true;
            ui.memory_mut(|m| m.close_popup());
        }
        ui.separator();
        let mut rgb = [color[0] as f32 / 255.0, color[1] as f32 / 255.0, color[2] as f32 / 255.0];
        if ui.color_edit_button_rgb(&mut rgb).changed() {
            *color = [(rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8];
            changed = true;
        }
    });
    changed
}

pub const PALETTE: [[u8; 3]; 30] = [
    [0, 0, 0], [64, 64, 64], [128, 128, 128], [191, 191, 191], [217, 217, 217], [255, 255, 255],
    [192, 0, 0], [255, 0, 0], [255, 192, 0], [255, 255, 0], [146, 208, 80], [0, 176, 80],
    [0, 176, 240], [0, 112, 192], [0, 32, 96], [112, 48, 160], [255, 102, 204], [153, 51, 0],
    [242, 220, 219], [253, 233, 217], [255, 242, 204], [226, 239, 218], [221, 235, 247], [228, 223, 236],
    [0, 51, 153], [31, 78, 121], [84, 130, 53], [191, 143, 0], [197, 90, 17], [128, 0, 0],
];

/// A grid of preset colours; returns true when one was picked.
pub fn palette(ui: &mut Ui, color: &mut [u8; 3]) -> bool {
    let mut picked = false;
    egui::Grid::new("k42palette").spacing([3.0, 3.0]).show(ui, |ui| {
        for (i, c) in PALETTE.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
            let stroke = if *c == *color {
                Stroke::new(2.0, accent(ui.ctx()))
            } else {
                Stroke::new(1.0, Color32::from_gray(150))
            };
            ui.painter().rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
            if resp.clicked() {
                *color = *c;
                picked = true;
            }
            if i % 6 == 5 {
                ui.end_row();
            }
        }
    });
    picked
}

pub fn c32(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// Native error box.
pub fn error(title: &str, message: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(title)
        .set_description(message)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// Native information box.
pub fn info(title: &str, message: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title(title)
        .set_description(message)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// Native yes/no question.
pub fn confirm(title: &str, message: &str) -> bool {
    matches!(
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title(title)
            .set_description(message)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show(),
        rfd::MessageDialogResult::Yes
    )
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum SaveChoice {
    Save,
    Discard,
    Cancel,
}

/// "Do you want to save the changes?" with Save / Don't Save / Cancel.
pub fn ask_save(doc_name: &str) -> SaveChoice {
    let msg = format!(
        "{} «{}»?\n\n{}",
        t("Do you want to save the changes made to", "Сохранить изменения в документе"),
        doc_name,
        t(
            "Yes — save, No — discard the changes, Cancel — keep editing.",
            "Да — сохранить, Нет — не сохранять, Отмена — продолжить правку."
        )
    );
    match rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Suite42")
        .set_description(msg)
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .show()
    {
        rfd::MessageDialogResult::Yes => SaveChoice::Save,
        rfd::MessageDialogResult::No => SaveChoice::Discard,
        _ => SaveChoice::Cancel,
    }
}

/// Language submenu; changing the language applies immediately in every app on next frame.
pub fn language_menu(ui: &mut Ui) {
    ui.menu_button(t("Language", "Язык"), |ui| {
        for l in crate::Lang::ALL {
            if ui.radio(crate::lang() == l, l.native_name()).clicked() {
                crate::set_lang(l);
                crate::settings::set("language", l);
                ui.close_menu();
            }
        }
    });
}

/// Handles the window close button: asks to save when dirty. Returns true to proceed.
/// `save` is called when the user chooses Save and must return true on success.
pub fn guard_close(ctx: &egui::Context, dirty: bool, doc_name: &str, save: impl FnOnce() -> bool) {
    if ctx.input(|i| i.viewport().close_requested()) && dirty {
        match ask_save(doc_name) {
            SaveChoice::Save => {
                if !save() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
            }
            SaveChoice::Discard => {}
            SaveChoice::Cancel => ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose),
        }
    }
}

/// Window title "name — App" with a bullet when modified.
pub fn set_title(ctx: &egui::Context, app: &str, doc: Option<&str>, dirty: bool) {
    let title = match doc {
        Some(d) => format!("{}{} — {}", if dirty { "• " } else { "" }, d, app),
        None => app.to_string(),
    };
    let key = egui::Id::new("k42title");
    let prev: Option<String> = ctx.data(|d| d.get_temp(key));
    if prev.as_deref() != Some(&title) {
        ctx.data_mut(|d| d.insert_temp(key, title.clone()));
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }
}

/// Consumes a keyboard shortcut (Ctrl = Command on Windows).
pub fn shortcut(ctx: &egui::Context, mods: egui::Modifiers, key: egui::Key) -> bool {
    ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(mods, key)))
}

pub const CTRL: egui::Modifiers = egui::Modifiers::COMMAND;
pub const CTRL_SHIFT: egui::Modifiers = egui::Modifiers { alt: false, ctrl: false, shift: true, mac_cmd: false, command: true };
pub const CTRL_ALT: egui::Modifiers = egui::Modifiers { alt: true, ctrl: false, shift: false, mac_cmd: false, command: true };
pub const NONE: egui::Modifiers = egui::Modifiers::NONE;

/// A menu item with a right-aligned shortcut hint.
pub fn menu_item(ui: &mut Ui, label: &str, shortcut: &str) -> bool {
    let r = ui.add(egui::Button::new(label).shortcut_text(shortcut));
    if r.clicked() {
        ui.close_menu();
        true
    } else {
        false
    }
}

pub fn menu_item_enabled(ui: &mut Ui, enabled: bool, label: &str, shortcut: &str) -> bool {
    let r = ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(shortcut));
    if r.clicked() {
        ui.close_menu();
        true
    } else {
        false
    }
}

/// Standard About box contents.
pub fn about(app: &str, what: &str) {
    info(
        app,
        &format!(
            "{app} — {what}\n\nSuite42 for Windows 11 {}\n{}\n\nMIT License · Kit42",
            env!("CARGO_PKG_VERSION"),
            t(
                "Native Rust port of the Suite42 macOS apps.",
                "Нативный порт приложений Suite42 для macOS на Rust."
            )
        ),
    );
}

/// Labeled numeric field.
pub fn drag_f32(ui: &mut Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, speed: f64) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(v).range(range).speed(speed)).changed()
    })
    .inner
}

/// A status bar strip at the bottom of the window.
pub fn status_bar(ctx: &egui::Context, add: impl FnOnce(&mut Ui)) {
    egui::TopBottomPanel::bottom("k42status")
        .exact_height(24.0)
        .frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(egui::Margin::symmetric(10, 2)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.style_mut().override_text_style = Some(egui::TextStyle::Small);
                add(ui)
            });
        });
}

/// Loads an RgbaImage as an egui texture.
pub fn texture(ctx: &egui::Context, name: &str, img: &image::RgbaImage) -> egui::TextureHandle {
    ctx.load_texture(name, crate::raster::to_color_image(img), egui::TextureOptions::LINEAR)
}

/// Draws a checkerboard behind transparent images.
pub fn checkerboard(painter: &egui::Painter, rect: egui::Rect, cell: f32) {
    painter.rect_filled(rect, 0.0, Color32::from_gray(255));
    let cols = (rect.width() / cell).ceil() as i32;
    let rows = (rect.height() / cell).ceil() as i32;
    if cols * rows > 40_000 {
        return;
    }
    for y in 0..rows {
        for x in 0..cols {
            if (x + y) % 2 == 0 {
                let r = egui::Rect::from_min_size(
                    rect.min + Vec2::new(x as f32 * cell, y as f32 * cell),
                    Vec2::splat(cell),
                )
                .intersect(rect);
                painter.rect_filled(r, 0.0, Color32::from_gray(225));
            }
        }
    }
}

/// File-dialog filters shared across apps.
pub fn open_dialog(title: &str, filters: &[(&str, &[&str])]) -> rfd::FileDialog {
    let mut d = rfd::FileDialog::new().set_title(title);
    for (name, exts) in filters {
        d = d.add_filter(*name, exts);
    }
    if let Some(dir) = crate::settings::get::<std::path::PathBuf>("last_dir") {
        d = d.set_directory(dir);
    }
    d
}

/// Remembers the folder of a chosen file for the next dialog.
pub fn remember_dir(path: &std::path::Path) {
    if let Some(dir) = path.parent() {
        crate::settings::set("last_dir", dir);
    }
}
