//! FerroMix visual identity for Iced — the same deep-indigo glass palette and
//! cyan→violet accent language as the egui build, so the routing state stays
//! legible: A (hardware) reads cyan, B (virtual mic) reads violet, danger reads
//! coral, and everything else stays quiet.

use crate::tokens;
use iced::{gradient, Background, Border, Color, Font, Radians, Shadow, Vector};

/// The bundled UI typeface (see `assets/fonts/`, embedded in `main.rs`).
/// Everything except the pw-metadata code snippet in Settings uses this.
pub const FONT_UI: Font = Font::with_name("Inter");
/// Semibold weight, for headers/wordmark where a touch more presence helps.
pub const FONT_UI_SEMIBOLD: Font = Font { weight: iced::font::Weight::Semibold, ..FONT_UI };

pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

pub const BG: Color = rgb(0x0b, 0x0e, 0x1a);
pub const BG_DEEP: Color = rgb(0x05, 0x06, 0x0d);
/// Top-of-card stop for the card gradient — a touch lighter than `CARD`
/// itself, reads as light catching the top edge of a glass panel rather
/// than a flat fill. See `card_gradient()`.
pub const CARD_HI: Color = rgb(0x1c, 0x22, 0x3a);
pub const CARD: Color = rgb(0x16, 0x1b, 0x2e);
pub const CARD_LO: Color = rgb(0x11, 0x15, 0x24);
pub const PANEL_HI: Color = rgb(0x22, 0x29, 0x44);
pub const EDGE: Color = rgb(0x33, 0x3c, 0x5e);
pub const EDGE_SOFT: Color = rgb(0x1c, 0x22, 0x38);
pub const TEXT: Color = rgb(0xe8, 0xed, 0xf9);
pub const TEXT_DIM: Color = rgb(0x74, 0x80, 0xa3);

// Slightly more saturated/luminous than the original flat palette — the
// same hues, pushed enough to actually read as "sleek" against the deep
// background instead of blending into a uniformly muted dark-mode look.
pub const ACCENT: Color = rgb(0x3a, 0xf0, 0xe4); // cyan (A / hardware)
pub const ACCENT_2: Color = rgb(0x8b, 0x6c, 0xff);
pub const VIOLET: Color = rgb(0xbd, 0x7a, 0xff); // B / virtual mic
pub const VIOLET_2: Color = rgb(0xff, 0x6c, 0xc7);
pub const MIC_AMBER: Color = rgb(0xff, 0xb2, 0x5c);

pub const METER_LO: Color = rgb(0x2d, 0xe0, 0x9a);
pub const METER_MID: Color = rgb(0xff, 0xc7, 0x4d);
pub const METER_HI: Color = rgb(0xff, 0x5c, 0x72);
/// Unlit meter segment color — deliberately a good deal lighter than `CARD`
/// (was nearly the same darkness, which made the whole meter housing all
/// but disappear against the card at rest, only a thin sliver visible at
/// all — reading as "barely there" rather than as an instrument). Now the
/// meter's segmented housing itself is a clearly visible groove even with
/// no signal.
pub const SEG_OFF: Color = rgb(0x24, 0x2c, 0x48);
pub const REC_RED: Color = rgb(0xff, 0x4d, 0x5e);
pub const DANGER: Color = rgb(0xff, 0x4d, 0x5e);

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

/// The shadow every card shares — tinted toward the deep background blue
/// rather than flat black, which reads as a richer glass effect than a
/// muddy generic drop-shadow. Deeper/softer than the original flat-card
/// pass: more blur radius and a larger offset actually separates a card
/// from the page behind it instead of reading as a 1px outline with a
/// barely-visible smudge under it.
fn card_shadow() -> Shadow {
    Shadow { color: BG_DEEP.scale_alpha(0.75), offset: Vector::new(0.0, 6.0), blur_radius: 22.0 }
}

/// Top-to-bottom panel gradient: `CARD_HI` at the top edge fading to `CARD`
/// by the bottom — light catching the top of a glass panel, rather than a
/// single flat fill. This one change is what most makes cards read as
/// "elevated" instead of "flat rectangle" at a glance.
fn card_gradient() -> Background {
    Background::Gradient(
        gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_2))
            .add_stop(0.0, CARD_HI)
            .add_stop(1.0, CARD)
            .into(),
    )
}

/// A glass card container style.
pub fn card(_t: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(card_gradient()),
        border: Border { color: EDGE_SOFT, width: 1.0, radius: tokens::radius::LG.into() },
        shadow: card_shadow(),
        text_color: Some(TEXT),
    }
}

/// A card with an accent-tinted edge (live strip / B bus). `active` overrides
/// the edge with a bright amber "phosphor" outline — the strip/bus you just
/// touched (fader drag, dropdown, mute, any control), fading back to the
/// normal accent edge a moment after you stop interacting with it.
pub fn card_accent(accent: Color, active: bool) -> impl Fn(&iced::Theme) -> iced::widget::container::Style {
    let (edge_color, edge_width) = if active { (MIC_AMBER, 2.0) } else { (accent.scale_alpha(0.55), 1.2) };
    move |_t| iced::widget::container::Style {
        background: Some(card_gradient()),
        border: Border { color: edge_color, width: edge_width, radius: tokens::radius::LG.into() },
        // A touch of accent bleeding into the shadow reads as the card's
        // edge glowing faintly rather than just having a colored outline —
        // still subtle (low blur/offset), not the "bloom" the original
        // design note deliberately ruled out for the `active` state.
        shadow: Shadow { color: accent.scale_alpha(if active { 0.0 } else { 0.16 }), offset: Vector::new(0.0, 6.0), blur_radius: if active { 22.0 } else { 26.0 } },
        text_color: Some(TEXT),
    }
}

/// A hairline divider — used inside cards to separate ROUTING/SEND/DSP
/// sections with a visible seam instead of blank space alone, and under the
/// header to give the chrome bar a defined edge against the console below.
pub fn divider(_t: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Color(EDGE_SOFT.scale_alpha(0.6))),
        ..Default::default()
    }
}

/// The app's own background: a very subtle top-to-bottom darkening (`BG` at
/// the top, `BG_DEEP` at the bottom) instead of one flat color — gives the
/// whole window a hint of depth behind the cards sitting on top of it,
/// noticeable mainly as the page scrolls rather than as an obvious effect.
pub fn panel(_t: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Gradient(
            gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_2))
                .add_stop(0.0, BG)
                .add_stop(1.0, BG_DEEP)
                .into(),
        )),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

/// The header chrome bar's background — a faint accent-tinted gradient
/// (barely-there cyan wash left-to-right) instead of flat `BG`, so the
/// header reads as a distinct lit surface rather than a plain strip of the
/// same background color with a line under it.
pub fn header_bar(_t: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Gradient(
            gradient::Linear::new(Radians(0.0))
                .add_stop(0.0, PANEL_HI.scale_alpha(0.55))
                .add_stop(1.0, BG)
                .into(),
        )),
        text_color: Some(TEXT),
        ..Default::default()
    }
}
