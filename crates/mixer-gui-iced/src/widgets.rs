//! FerroMix Iced widgets — strip/bus cards, sends, fader, meter. Kept in one
//! module for round 1; will split as it grows.

use crate::theme;
use crate::Message;
use iced::widget::{button, column, container, row, text, Space};
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke};
use iced::{Alignment, Border, Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use mixer_core::engine::Command;
use mixer_core::model::{Bus, BusKind, MixerState, Strip};

const STRIP_W: f32 = 138.0;

pub fn tab_button(label: &str, active: bool) -> button::Button<'_, Message> {
    let fg = if active { theme::ACCENT } else { theme::TEXT_DIM };
    button(text(label).size(12).color(fg))
        .style(move |_t, _s| button::Style {
            background: Some(iced::Background::Color(if active {
                theme::with_alpha(theme::ACCENT, 0.12)
            } else {
                Color::TRANSPARENT
            })),
            border: Border {
                color: if active { theme::ACCENT } else { theme::EDGE_SOFT },
                width: 1.0,
                radius: 6.0.into(),
            },
            text_color: fg,
            ..Default::default()
        })
        .padding([5, 12])
}

/// A cyan/violet/coral send pill (A vs B vs blocked).
fn send_pill<'a>(
    label: &'a str,
    on: bool,
    feedback: bool,
    is_b: bool,
    msg: Message,
) -> Element<'a, Message> {
    let (a, _b) = if is_b { (theme::VIOLET, theme::VIOLET_2) } else { (theme::ACCENT, theme::ACCENT_2) };
    let (bg, fg, edge) = if feedback {
        (theme::DANGER, theme::BG_DEEP, theme::DANGER)
    } else if on {
        (a, theme::BG_DEEP, a)
    } else {
        (theme::SEG_OFF, theme::TEXT_DIM, theme::EDGE)
    };
    button(text(label).size(10).color(fg).align_x(Alignment::Center).width(Length::Fill))
        .style(move |_t, _s| button::Style {
            background: Some(iced::Background::Color(bg)),
            border: Border { color: edge, width: 1.0, radius: 5.0.into() },
            text_color: fg,
            ..Default::default()
        })
        .width(34)
        .padding([3, 0])
        .on_press(msg)
        .into()
}

/// Small labelled button (MUTE, SET AS DEFAULT…).
fn wide_button<'a>(label: &'a str, on: bool, accent: Color, msg: Message) -> Element<'a, Message> {
    let (bg, fg) = if on { (accent, theme::BG_DEEP) } else { (theme::PANEL_HI, theme::TEXT_DIM) };
    button(text(label).size(10).color(fg).align_x(Alignment::Center).width(Length::Fill))
        .style(move |_t, _s| button::Style {
            background: Some(iced::Background::Color(bg)),
            border: Border {
                color: if on { accent } else { theme::EDGE },
                width: 1.0,
                radius: 6.0.into(),
            },
            text_color: fg,
            ..Default::default()
        })
        .width(Length::Fill)
        .padding([6, 0])
        .on_press(msg)
        .into()
}

pub fn strip_card<'a>(idx: usize, strip: &'a Strip, state: &'a MixerState) -> Element<'a, Message> {
    let accent = theme::ACCENT;
    let name = strip.display_name(idx);

    let head = row![
        text(elide(&name, 13)).size(11).color(if strip.input_live { theme::TEXT } else { theme::TEXT_DIM }),
        Space::with_width(Length::Fill),
        text(if strip.input_live { "●" } else { "○" })
            .size(8)
            .color(if strip.input_live { accent } else { theme::TEXT_DIM }),
    ]
    .align_y(Alignment::Center);

    // Send pills for each bus.
    let mut sends = column![].spacing(3);
    for (bi, bus) in state.buses.iter().enumerate() {
        let on = strip.assign.get(bi).copied().unwrap_or(false);
        let fb = state.is_feedback(idx, bi);
        let is_b = bus.kind == BusKind::VirtualMic;
        let cmd = Command::ToggleAssign { strip: idx, bus: bi };
        sends = sends.push(send_pill(&bus.label, on, fb, is_b, Message::Send(cmd)));
    }

    let meter = Canvas::new(Meter { level: strip.level.peak(), accent })
        .width(Length::Fixed(22.0))
        .height(Length::Fixed(150.0));

    let fader_row = row![
        meter,
        Space::with_width(6),
        column![
            text(fmt_db(strip.volume)).size(11).color(theme::TEXT),
            Space::with_height(4),
            sends,
        ]
        .spacing(0),
    ]
    .spacing(4);

    let mute = wide_button(
        "MUTE",
        strip.mute,
        theme::REC_RED,
        Message::Send(Command::SetStripMute { strip: idx, mute: !strip.mute }),
    );

    let body = column![
        head,
        Space::with_height(6),
        fader_row,
        Space::with_height(8),
        mute,
    ]
    .spacing(0)
    .width(Length::Fixed(STRIP_W));

    container(body)
        .padding(9)
        .width(Length::Fixed(STRIP_W + 18.0))
        .style(theme::card_accent(if strip.input_live { accent } else { theme::EDGE_SOFT }))
        .into()
}

pub fn bus_card<'a>(idx: usize, bus: &'a Bus, state: &'a MixerState) -> Element<'a, Message> {
    let accent = theme::VIOLET;
    let name = if bus.name.is_empty() { bus.label.clone() } else { bus.name.clone() };

    let head = row![
        text(name).size(14).color(accent),
        Space::with_width(Length::Fill),
        text("MIC").size(8).color(theme::TEXT_DIM),
    ]
    .align_y(Alignment::Center);

    let listener = if bus.listeners.is_empty() {
        text("no app assigned").size(9).color(theme::TEXT_DIM)
    } else {
        text(format!("◂ {} listening", elide(&bus.listeners[0], 12))).size(9).color(accent)
    };

    let meter = Canvas::new(Meter { level: bus.level.peak(), accent })
        .width(Length::Fixed(22.0))
        .height(Length::Fixed(150.0));

    let mute = wide_button(
        "MUTE",
        bus.mute,
        theme::REC_RED,
        Message::Send(Command::SetBusMute { bus: idx, mute: !bus.mute }),
    );

    let body = column![
        head,
        Space::with_height(4),
        listener,
        Space::with_height(6),
        row![meter, Space::with_width(6), text(fmt_db(bus.volume)).size(11).color(theme::TEXT)],
        Space::with_height(8),
        mute,
    ]
    .spacing(0)
    .width(Length::Fixed(STRIP_W));

    container(body)
        .padding(9)
        .width(Length::Fixed(STRIP_W + 18.0))
        .style(theme::card_accent(accent))
        .into()
}

fn fmt_db(pos: f32) -> String {
    use mixer_core::model::pos_to_db;
    if pos <= 0.002 {
        "-∞".into()
    } else {
        let db = pos_to_db(pos);
        if db.abs() < 0.05 { "0.0".into() } else { format!("{db:+.1}") }
    }
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

/// A vertical peak meter drawn on a canvas.
struct Meter {
    level: f32,
    accent: Color,
}

impl<Message> canvas::Program<Message> for Meter {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;

        // Track.
        frame.fill(&Path::rectangle(Point::new(0.0, 0.0), Size::new(w, h)), theme::SEG_OFF);

        // Segmented fill from the bottom.
        let segs = 20;
        let lit = (self.level.clamp(0.0, 1.0) * segs as f32).round() as i32;
        let seg_h = h / segs as f32;
        for i in 0..segs {
            if (segs - 1 - i) as i32 >= segs as i32 - lit {
                let frac = i as f32 / segs as f32;
                let color = if frac > 0.85 {
                    theme::METER_HI
                } else if frac > 0.6 {
                    theme::METER_MID
                } else {
                    theme::METER_LO
                };
                let y = h - (i as f32 + 1.0) * seg_h;
                frame.fill(
                    &Path::rectangle(Point::new(1.0, y + 0.5), Size::new(w - 2.0, seg_h - 1.0)),
                    color,
                );
            }
        }

        // Accent border.
        frame.stroke(
            &Path::rectangle(Point::new(0.0, 0.0), Size::new(w, h)),
            Stroke::default().with_color(theme::with_alpha(self.accent, 0.4)).with_width(1.0),
        );

        vec![frame.into_geometry()]
    }
}
