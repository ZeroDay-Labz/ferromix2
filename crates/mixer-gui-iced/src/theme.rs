//! FerroMix visual identity for Iced — the same deep-indigo glass palette and
//! cyan→violet accent language as the egui build, so the routing state stays
//! legible: A (hardware) reads cyan, B (virtual mic) reads violet, danger reads
//! coral, and everything else stays quiet.

use iced::{Background, Border, Color, Font, Shadow, Vector};

/// The bundled UI typeface (see `assets/fonts/`, embedded in `main.rs`).
/// Everything except the pw-metadata code snippet in Settings uses this.
pub const FONT_UI: Font = Font::with_name("Inter");
/// Semibold weight, for headers/wordmark where a touch more presence helps.
pub const FONT_UI_SEMIBOLD: Font = Font { weight: iced::font::Weight::Semibold, ..FONT_UI };

pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

pub const BG: Color = rgb(0x0b, 0x0e, 0x1a);
pub const BG_DEEP: Color = rgb(0x07, 0x09, 0x12);
pub const CARD: Color = rgb(0x16, 0x1b, 0x2e);
pub const CARD_LO: Color = rgb(0x11, 0x15, 0x24);
pub const PANEL_HI: Color = rgb(0x1e, 0x24, 0x3c);
pub const EDGE: Color = rgb(0x2a, 0x32, 0x50);
pub const EDGE_SOFT: Color = rgb(0x1c, 0x22, 0x38);
pub const TEXT: Color = rgb(0xdc, 0xe3, 0xf2);
pub const TEXT_DIM: Color = rgb(0x6b, 0x76, 0x99);

pub const ACCENT: Color = rgb(0x35, 0xe2, 0xd8); // cyan (A / hardware)
pub const ACCENT_2: Color = rgb(0x8b, 0x6c, 0xff);
pub const VIOLET: Color = rgb(0xb4, 0x6c, 0xff); // B / virtual mic
pub const VIOLET_2: Color = rgb(0xff, 0x6c, 0xc7);
pub const MIC_AMBER: Color = rgb(0xff, 0xb2, 0x5c);

pub const METER_LO: Color = rgb(0x2d, 0xe0, 0x9a);
pub const METER_MID: Color = rgb(0xff, 0xc7, 0x4d);
pub const METER_HI: Color = rgb(0xff, 0x5c, 0x72);
pub const SEG_OFF: Color = rgb(0x18, 0x1e, 0x33);
pub const REC_RED: Color = rgb(0xff, 0x4d, 0x5e);
pub const DANGER: Color = rgb(0xff, 0x4d, 0x5e);

pub fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

/// The dark base theme.
pub fn base() -> iced::Theme {
    iced::Theme::custom(
        "FerroMix".to_string(),
        iced::theme::Palette {
            background: BG,
            text: TEXT,
            primary: ACCENT,
            success: METER_LO,
            danger: DANGER,
        },
    )
}

/// A glass card container style.
pub fn card(_t: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Color(CARD)),
        border: Border { color: EDGE_SOFT, width: 1.0, radius: 8.0.into() },
        shadow: Shadow {
            color: Color { a: 0.35, ..BG_DEEP },
            offset: Vector::new(0.0, 3.0),
            blur_radius: 12.0,
        },
        text_color: Some(TEXT),
    }
}

/// A card with an accent-tinted edge (live strip / B bus).
pub fn card_accent(accent: Color) -> impl Fn(&iced::Theme) -> iced::widget::container::Style {
    move |_t| iced::widget::container::Style {
        background: Some(Background::Color(CARD)),
        border: Border { color: with_alpha(accent, 0.45), width: 1.0, radius: 8.0.into() },
        shadow: Shadow {
            color: Color { a: 0.35, ..BG_DEEP },
            offset: Vector::new(0.0, 3.0),
            blur_radius: 12.0,
        },
        text_color: Some(TEXT),
    }
}

pub fn panel(_t: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Color(BG)),
        text_color: Some(TEXT),
        ..Default::default()
    }
}
