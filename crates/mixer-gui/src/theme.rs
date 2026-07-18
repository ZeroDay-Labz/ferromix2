//! FerroMix visual identity.
//!
//! Deep indigo base, soft glass cards, and a cyan→violet accent gradient that
//! only ever marks *live signal*. Hardware (A) reads cyan; virtual mics (B)
//! read violet; danger reads coral. Everything else stays quiet, so the routing
//! state is legible at a glance — the console should feel like instrumentation,
//! not decoration.

use eframe::egui::{
    self, epaint, Color32, FontFamily, FontId, Rect, Rounding, Stroke, TextStyle, Visuals,
};

// ------------------------------------------------------------------ palette

pub const BG: Color32 = Color32::from_rgb(0x0b, 0x0e, 0x1a);
pub const BG_DEEP: Color32 = Color32::from_rgb(0x07, 0x09, 0x12);
/// Card surface (top of its gradient).
pub const CARD: Color32 = Color32::from_rgb(0x16, 0x1b, 0x2e);
/// Card surface (bottom of its gradient) — the subtle depth.
#[allow(dead_code)] pub const CARD_LO: Color32 = Color32::from_rgb(0x11, 0x15, 0x24);
pub const PANEL_HI: Color32 = Color32::from_rgb(0x1e, 0x24, 0x3c);
pub const EDGE: Color32 = Color32::from_rgb(0x2a, 0x32, 0x50);
pub const EDGE_SOFT: Color32 = Color32::from_rgb(0x1c, 0x22, 0x38);
pub const TEXT: Color32 = Color32::from_rgb(0xdc, 0xe3, 0xf2);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x6b, 0x76, 0x99);

/// The accent gradient: cyan → violet. Signal, selection, life.
pub const ACCENT: Color32 = Color32::from_rgb(0x35, 0xe2, 0xd8);
pub const ACCENT_2: Color32 = Color32::from_rgb(0x8b, 0x6c, 0xff);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x1b, 0x3f, 0x4e);

/// Virtual mics (B buses) — violet/magenta so an A send never looks like a B send.
pub const VIOLET: Color32 = Color32::from_rgb(0xb4, 0x6c, 0xff);
pub const VIOLET_2: Color32 = Color32::from_rgb(0xff, 0x6c, 0xc7);
/// Hardware inputs (mics).
pub const MIC_AMBER: Color32 = Color32::from_rgb(0xff, 0xb2, 0x5c);

// Meter zones.
pub const METER_LO: Color32 = Color32::from_rgb(0x2d, 0xe0, 0x9a);
pub const METER_MID: Color32 = Color32::from_rgb(0xff, 0xc7, 0x4d);
pub const METER_HI: Color32 = Color32::from_rgb(0xff, 0x5c, 0x72);
pub const SEG_OFF: Color32 = Color32::from_rgb(0x18, 0x1e, 0x33);
pub const MUTE_RED: Color32 = Color32::from_rgb(0xf0, 0x4d, 0x6b);
pub const REC_RED: Color32 = Color32::from_rgb(0xff, 0x4d, 0x5e);
pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x4d, 0x5e);

/// Lighten a colour toward white — used for the far end of accent gradients.
pub fn lighten(c: Color32, t: f32) -> Color32 {
    lerp(c, Color32::WHITE, t)
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

// ------------------------------------------------------------------ helpers

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgba_unmultiplied(
        f(a.r(), b.r()),
        f(a.g(), b.g()),
        f(a.b(), b.b()),
        f(a.a(), b.a()),
    )
}

/// Fill a rect with a vertical gradient (top → bottom).
#[allow(dead_code)]
pub fn gradient_v(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32, rounding: f32) {
    // Approximate the gradient with horizontal bands; cheap and artifact-free
    // at these sizes, and it keeps us on egui's plain shape pipeline.
    let bands = 24;
    let h = rect.height() / bands as f32;
    for i in 0..bands {
        let t = i as f32 / (bands - 1).max(1) as f32;
        let y = rect.top() + i as f32 * h;
        let band = Rect::from_min_max(
            egui::pos2(rect.left(), y),
            egui::pos2(rect.right(), (y + h + 0.5).min(rect.bottom())),
        );
        let r = if i == 0 || i == bands - 1 { rounding } else { 0.0 };
        painter.rect_filled(band, r, lerp(top, bottom, t));
    }
}

/// Horizontal accent gradient (used for active pills / bars).
pub fn gradient_h(painter: &egui::Painter, rect: Rect, left: Color32, right: Color32, rounding: f32) {
    let bands = 16;
    let w = rect.width() / bands as f32;
    for i in 0..bands {
        let t = i as f32 / (bands - 1).max(1) as f32;
        let x = rect.left() + i as f32 * w;
        let band = Rect::from_min_max(
            egui::pos2(x, rect.top()),
            egui::pos2((x + w + 0.5).min(rect.right()), rect.bottom()),
        );
        let r = if i == 0 || i == bands - 1 { rounding } else { 0.0 };
        painter.rect_filled(band, r, lerp(left, right, t));
    }
}

/// A glass card: gradient fill, hairline edge, soft drop shadow.
#[allow(dead_code)]
pub fn card(painter: &egui::Painter, rect: Rect, rounding: f32, glow: Option<Color32>) {
    // Drop shadow.
    painter.add(epaint::Shape::Rect(epaint::RectShape::new(
        rect.translate(egui::vec2(0.0, 2.0)).expand(1.0),
        Rounding::same(rounding + 1.0),
        Color32::from_black_alpha(70),
        Stroke::NONE,
    )));
    gradient_v(painter, rect, CARD, CARD_LO, rounding);
    let edge = glow.unwrap_or(EDGE_SOFT);
    painter.rect_stroke(rect, rounding, Stroke::new(1.0_f32, edge));
    // Top highlight — the "glass" tell.
    painter.hline(
        egui::Rangef::new(rect.left() + rounding, rect.right() - rounding),
        rect.top() + 0.5,
        Stroke::new(1.0_f32, Color32::from_white_alpha(10)),
    );
}

/// Apply the theme. `scale` is the UI scale factor (DPI / user preference).
pub fn apply(ctx: &egui::Context, scale: f32) {
    ctx.set_pixels_per_point(scale);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Heading, mono(15.0)),
        (TextStyle::Body, mono(12.0)),
        (TextStyle::Monospace, mono(12.0)),
        (TextStyle::Button, mono(11.5)),
        (TextStyle::Small, mono(9.5)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);

    let mut v = Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0_f32, EDGE);
    v.extreme_bg_color = BG_DEEP;
    v.faint_bg_color = CARD;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    let r = Rounding::same(6.0);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = r;
    }
    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, EDGE_SOFT);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.bg_fill = PANEL_HI;
    v.widgets.inactive.weak_bg_fill = PANEL_HI;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, EDGE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.hovered.bg_fill = PANEL_HI;
    v.widgets.hovered.weak_bg_fill = PANEL_HI;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.active.bg_fill = ACCENT_DIM;
    v.widgets.active.weak_bg_fill = ACCENT_DIM;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT);

    v.window_rounding = Rounding::same(8.0);
    v.menu_rounding = Rounding::same(8.0);
    v.window_shadow = epaint::Shadow {
        offset: egui::vec2(0.0, 6.0),
        blur: 20.0,
        spread: 0.0,
        color: Color32::from_black_alpha(120),
    };
    v.popup_shadow = v.window_shadow;

    style.visuals = v;
    ctx.set_style(style);
}
