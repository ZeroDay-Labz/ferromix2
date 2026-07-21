//! Bundled SVG icon set — replaces the Unicode glyphs (`●○✕◇◂★`) the console
//! used to render via `text!`. Each icon is a single flat shape; color comes
//! from `svg::Style` at render time (Iced 0.13 supports recoloring symbolic
//! SVGs this way — see `iced_widget::svg::Style::color`), so one file per
//! shape covers every accent/dim/danger use rather than needing baked
//! color variants.

use iced::widget::svg::{self, Svg};
use iced::{Color, Length};

macro_rules! icon_handles {
    ($($name:ident => $file:literal),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Icon { $($name),* }

        fn bytes(icon: Icon) -> &'static [u8] {
            match icon {
                $(Icon::$name => include_bytes!(concat!("../../../assets/icons/", $file)).as_slice()),*
            }
        }
    };
}

icon_handles! {
    Dot => "dot.svg",
    Ring => "ring.svg",
    X => "x.svg",
    Mic => "mic.svg",
    Headphones => "headphones.svg",
    Star => "star.svg",
    Mute => "mute.svg",
    Plus => "plus.svg",
    List => "list.svg",
}

/// A tinted icon at `size` logical pixels, ready to drop into a `row!`/`column!`.
pub fn icon<'a>(which: Icon, size: f32, color: Color) -> Svg<'a> {
    iced::widget::svg(svg::Handle::from_memory(bytes(which)))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_t, _s| svg::Style { color: Some(color) })
}
