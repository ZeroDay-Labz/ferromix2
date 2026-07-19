//! FerroMix2 Iced widgets — real interaction: draggable + scroll-wheel faders,
//! draggable DSP knobs, device/app dropdowns, and the send grid.

use crate::theme;
use crate::Message;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke};
use iced::widget::{button, column, container, mouse_area, pick_list, row, text, vertical_slider, Space};
use iced::{Alignment, Border, Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use mixer_core::engine::Command;
use mixer_core::model::{pos_to_db, Bus, BusKind, MixerState, Strip, StripDsp};

pub const STRIP_W: f32 = 150.0;
const FADER_H: f32 = 150.0;

pub fn tab_button(label: &str, active: bool) -> button::Button<'_, Message> {
    let fg = if active { theme::ACCENT } else { theme::TEXT_DIM };
    button(text(label).size(12).color(fg))
        .style(move |_t, _s| button::Style {
            background: Some(iced::Background::Color(if active { theme::with_alpha(theme::ACCENT, 0.12) } else { Color::TRANSPARENT })),
            border: Border { color: if active { theme::ACCENT } else { theme::EDGE_SOFT }, width: 1.0, radius: 6.0.into() },
            text_color: fg, ..Default::default()
        })
        .padding([6, 14])
}

fn send_pill<'a>(label: &'a str, on: bool, fb: bool, is_b: bool, msg: Message) -> Element<'a, Message> {
    let accent = if is_b { theme::VIOLET } else { theme::ACCENT };
    let (bg, fg, edge) = if fb { (theme::DANGER, theme::BG_DEEP, theme::DANGER) }
        else if on { (accent, theme::BG_DEEP, accent) }
        else { (theme::SEG_OFF, theme::TEXT_DIM, theme::EDGE) };
    button(text(label).size(10).color(fg).center().width(Length::Fill))
        .style(move |_t, _s| button::Style { background: Some(iced::Background::Color(bg)), border: Border { color: edge, width: 1.0, radius: 5.0.into() }, text_color: fg, ..Default::default() })
        .width(Length::Fixed(38.0)).padding([4, 0]).on_press(msg).into()
}

fn wide_button<'a>(label: &'a str, on: bool, accent: Color, msg: Message) -> Element<'a, Message> {
    let (bg, fg) = if on { (accent, theme::BG_DEEP) } else { (theme::PANEL_HI, theme::TEXT_DIM) };
    button(text(label).size(10).color(fg).center().width(Length::Fill))
        .style(move |_t, _s| button::Style { background: Some(iced::Background::Color(bg)), border: Border { color: if on { accent } else { theme::EDGE }, width: 1.0, radius: 6.0.into() }, text_color: fg, ..Default::default() })
        .width(Length::Fill).padding([6, 0]).on_press(msg).into()
}

fn fader<'a>(value: f32, accent: Color, on_change: impl Fn(f32) -> Message + 'a + Copy) -> Element<'a, Message> {
    let slider = vertical_slider(0.0..=1.0, value, move |v| on_change(v))
        .step(0.001).height(Length::Fixed(FADER_H))
        .style(move |_t, _s| slider_style(accent));
    mouse_area(slider)
        .on_scroll(move |delta| {
            let dy = match delta { iced::mouse::ScrollDelta::Lines { y, .. } => y, iced::mouse::ScrollDelta::Pixels { y, .. } => y / 40.0 };
            on_change((value + dy * 0.02).clamp(0.0, 1.0))
        })
        .into()
}

fn slider_style(accent: Color) -> vertical_slider::Style {
    use iced::widget::slider::{Handle, HandleShape, Rail};
    vertical_slider::Style {
        rail: Rail {
            backgrounds: (iced::Background::Color(accent), iced::Background::Color(theme::BG_DEEP)),
            width: 5.0, border: Border { color: theme::EDGE, width: 1.0, radius: 3.0.into() },
        },
        handle: Handle {
            shape: HandleShape::Rectangle { width: 26, border_radius: 4.0.into() },
            background: iced::Background::Color(theme::PANEL_HI), border_color: accent, border_width: 1.5,
        },
    }
}

pub fn fmt_db(pos: f32) -> String {
    if pos <= 0.002 { "-∞".into() } else { let db = pos_to_db(pos); if db.abs() < 0.05 { "0.0".into() } else { format!("{db:+.1}") } }
}
fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { let t: String = s.chars().take(max.saturating_sub(1)).collect(); format!("{t}…") }
}

fn knob<'a>(label: &'a str, value: f32, on: bool, accent: Color, strip: usize, dsp: StripDsp, is_gate: bool) -> Element<'a, Message> {
    let dial = Canvas::new(Dial { value, on, accent }).width(Length::Fixed(44.0)).height(Length::Fixed(44.0));
    let dial_area = mouse_area(dial).on_scroll(move |d| {
        let dy = match d { iced::mouse::ScrollDelta::Lines { y, .. } => y, iced::mouse::ScrollDelta::Pixels { y, .. } => y / 40.0 };
        let nv = (value + dy * 0.05).clamp(0.0, 1.0);
        let ndsp = if is_gate { StripDsp { gate: nv, ..dsp } } else { StripDsp { comp: nv, ..dsp } };
        Message::Send(Command::SetStripDsp { strip, dsp: ndsp })
    });
    let toggle = { let ndsp = if is_gate { StripDsp { gate_on: !dsp.gate_on, ..dsp } } else { StripDsp { comp_on: !dsp.comp_on, ..dsp } }; Message::Send(Command::SetStripDsp { strip, dsp: ndsp }) };
    let lbl = button(text(label).size(9).color(if on { accent } else { theme::TEXT_DIM }).center().width(Length::Fill))
        .style(move |_t, _s| button::Style { background: Some(iced::Background::Color(if on { theme::with_alpha(accent, 0.15) } else { theme::SEG_OFF })), border: Border { color: if on { accent } else { theme::EDGE }, width: 1.0, radius: 4.0.into() }, text_color: if on { accent } else { theme::TEXT_DIM }, ..Default::default() })
        .width(Length::Fixed(48.0)).padding([2, 0]).on_press(toggle);
    column![dial_area, lbl].spacing(2).align_x(Alignment::Center).into()
}

struct Dial { value: f32, on: bool, accent: Color }
impl<M> canvas::Program<M> for Dial {
    type State = ();
    fn draw(&self, _s: &(), r: &Renderer, _t: &Theme, b: Rectangle, _c: iced::mouse::Cursor) -> Vec<Geometry> {
        use std::f32::consts::PI;
        let mut f = Frame::new(r, b.size());
        let c = Point::new(b.width / 2.0, b.height / 2.0);
        let rad = b.width / 2.0 - 5.0; let start = PI * 0.75; let sweep = PI * 1.5;
        let dim = theme::with_alpha(self.accent, if self.on { 0.25 } else { 0.12 });
        let track = Path::new(|p| { p.arc(canvas::path::Arc { center: c, radius: rad, start_angle: iced::Radians(start), end_angle: iced::Radians(start + sweep) }); });
        f.stroke(&track, Stroke::default().with_color(dim).with_width(4.0));
        if self.on {
            let v = self.value.clamp(0.0, 1.0);
            let val = Path::new(|p| { p.arc(canvas::path::Arc { center: c, radius: rad, start_angle: iced::Radians(start), end_angle: iced::Radians(start + sweep * v) }); });
            f.stroke(&val, Stroke::default().with_color(self.accent).with_width(4.0));
            let ang = start + sweep * v;
            f.fill(&Path::circle(Point::new(c.x + rad * ang.cos(), c.y + rad * ang.sin()), 3.0), self.accent);
        }
        f.fill(&Path::circle(c, rad * 0.42), theme::PANEL_HI);
        vec![f.into_geometry()]
    }
}

struct Meter { level: f32, accent: Color }
impl<M> canvas::Program<M> for Meter {
    type State = ();
    fn draw(&self, _s: &(), r: &Renderer, _t: &Theme, b: Rectangle, _c: iced::mouse::Cursor) -> Vec<Geometry> {
        let mut f = Frame::new(r, b.size());
        f.fill(&Path::rectangle(Point::ORIGIN, b.size()), theme::SEG_OFF);
        let segs = 20; let lit = (self.level.clamp(0.0, 1.0) * segs as f32).round() as i32; let sh = b.height / segs as f32;
        for i in 0..segs {
            if i as i32 >= segs as i32 - lit {
                let frac = (segs - 1 - i) as f32 / segs as f32;
                let col = if frac > 0.85 { theme::METER_HI } else if frac > 0.6 { theme::METER_MID } else { theme::METER_LO };
                f.fill(&Path::rectangle(Point::new(1.0, i as f32 * sh + 0.5), Size::new(b.width - 2.0, sh - 1.0)), col);
            }
        }
        f.stroke(&Path::rectangle(Point::ORIGIN, b.size()), Stroke::default().with_color(theme::with_alpha(self.accent, 0.4)).with_width(1.0));
        vec![f.into_geometry()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opt { pub key: String, pub label: String }
impl std::fmt::Display for Opt { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.label) } }

fn dropdown<'a>(placeholder: &'a str, options: Vec<Opt>, selected: Option<Opt>, on_select: impl Fn(Opt) -> Message + 'a) -> Element<'a, Message> {
    pick_list(options, selected, on_select)
        .placeholder(placeholder).text_size(10).padding([4, 6]).width(Length::Fill)
        .style(|_t, _s| pick_list::Style { text_color: theme::TEXT, placeholder_color: theme::TEXT_DIM, handle_color: theme::TEXT_DIM, background: iced::Background::Color(theme::PANEL_HI), border: Border { color: theme::EDGE, width: 1.0, radius: 6.0.into() } })
        .into()
}

pub fn strip_card<'a>(idx: usize, strip: &'a Strip, state: &'a MixerState) -> Element<'a, Message> {
    let accent = theme::ACCENT;
    let head = row![
        text(elide(&strip.display_name(idx), 14)).size(11).color(if strip.input_live { theme::TEXT } else { theme::TEXT_DIM }),
        Space::with_width(Length::Fill),
        text(if strip.input_live { "●" } else { "○" }).size(8).color(if strip.input_live { accent } else { theme::TEXT_DIM }),
    ].align_y(Alignment::Center);
    let opts: Vec<Opt> = state.inputs.iter().map(|i| Opt { key: i.key.clone(), label: i.label.clone() }).collect();
    let sel = strip.input.as_ref().and_then(|k| opts.iter().find(|o| &o.key == k).cloned());
    let input_dd = dropdown("— select source —", opts, sel, move |o: Opt| Message::Send(Command::SetStripInput { strip: idx, input: Some(o.key) }));
    let meter = Canvas::new(Meter { level: strip.level.peak(), accent }).width(Length::Fixed(20.0)).height(Length::Fixed(FADER_H));
    let fad = fader(strip.volume, accent, move |v| Message::Send(Command::SetStripVolume { strip: idx, volume: v }));
    let fader_row = row![meter, Space::with_width(4), fad, Space::with_width(8), text(fmt_db(strip.volume)).size(11).color(theme::TEXT)].align_y(Alignment::Center);
    let mut a_row = row![].spacing(3); let mut b_row = row![].spacing(3);
    for (bi, bus) in state.buses.iter().enumerate() {
        let on = strip.assign.get(bi).copied().unwrap_or(false);
        let fb = state.is_feedback(idx, bi);
        let is_b = bus.kind == BusKind::VirtualMic;
        let pill = send_pill(&bus.label, on, fb, is_b, Message::Send(Command::ToggleAssign { strip: idx, bus: bi }));
        if is_b { b_row = b_row.push(pill) } else { a_row = a_row.push(pill) }
    }
    let dsp = strip.dsp;
    let knobs = row![knob("GATE", dsp.gate, dsp.gate_on, theme::ACCENT, idx, dsp, true), Space::with_width(6), knob("COMP", dsp.comp, dsp.comp_on, theme::VIOLET, idx, dsp, false)];
    let mute = wide_button("MUTE", strip.mute, theme::REC_RED, Message::Send(Command::SetStripMute { strip: idx, mute: !strip.mute }));
    let body = column![head, Space::with_height(5), input_dd, Space::with_height(8), fader_row, Space::with_height(8), column![a_row, b_row].spacing(3), Space::with_height(8), knobs, Space::with_height(8), mute]
        .spacing(0).width(Length::Fixed(STRIP_W));
    container(body).padding(10).width(Length::Fixed(STRIP_W + 20.0)).style(theme::card_accent(if strip.input_live { accent } else { theme::EDGE_SOFT })).into()
}

pub fn bus_card<'a>(idx: usize, bus: &'a Bus, state: &'a MixerState) -> Element<'a, Message> {
    let accent = theme::VIOLET;
    let name = if bus.name.is_empty() { bus.label.clone() } else { bus.name.clone() };
    let head = row![text(name).size(14).color(accent), Space::with_width(Length::Fill), text("MIC").size(8).color(theme::TEXT_DIM)].align_y(Alignment::Center);
    let opts: Vec<Opt> = state.capture_apps.iter().map(|a| Opt { key: a.key.clone(), label: a.label.clone() }).collect();
    let sel = bus.listener.as_ref().and_then(|k| opts.iter().find(|o| &o.key == k).cloned());
    let app_dd = dropdown("◇ SEND TO APP", opts, sel, move |o: Opt| Message::Send(Command::SetBusListener { bus: idx, app: Some(o.key) }));
    let listening = if bus.listeners.is_empty() { text("no app assigned").size(9).color(theme::TEXT_DIM) } else { text(format!("◂ {} listening", elide(&bus.listeners[0], 12))).size(9).color(accent) };
    let meter = Canvas::new(Meter { level: bus.level.peak(), accent }).width(Length::Fixed(20.0)).height(Length::Fixed(FADER_H));
    let fad = fader(bus.volume, accent, move |v| Message::Send(Command::SetBusVolume { bus: idx, volume: v }));
    let fader_row = row![meter, Space::with_width(4), fad, Space::with_width(8), text(fmt_db(bus.volume)).size(11).color(theme::TEXT)].align_y(Alignment::Center);
    let mut mon = row![].spacing(3);
    let a_buses: Vec<(usize, &Bus)> = state.buses.iter().enumerate().filter(|(_, b)| b.kind == BusKind::HwOutput).collect();
    for (ai, (_, ab)) in a_buses.iter().enumerate() {
        let on = bus.monitor.get(ai).copied().unwrap_or(false);
        mon = mon.push(send_pill(&ab.label, on, false, false, Message::Send(Command::ToggleBusMonitor { bus: idx, a_bus: ai })));
    }
    let mute = wide_button("MUTE", bus.mute, theme::REC_RED, Message::Send(Command::SetBusMute { bus: idx, mute: !bus.mute }));
    let body = column![head, Space::with_height(5), app_dd, Space::with_height(3), listening, Space::with_height(6), fader_row, Space::with_height(8), text("MONITOR ON").size(8).color(theme::TEXT_DIM), Space::with_height(2), mon, Space::with_height(8), mute]
        .spacing(0).width(Length::Fixed(STRIP_W));
    container(body).padding(10).width(Length::Fixed(STRIP_W + 20.0)).style(theme::card_accent(accent)).into()
}

pub fn hw_out_slot<'a>(idx: usize, bus: &'a Bus, state: &'a MixerState) -> Element<'a, Message> {
    let opts: Vec<Opt> = state.devices.iter().map(|d| Opt { key: d.key.clone(), label: d.label.clone() }).collect();
    let sel = bus.device.as_ref().and_then(|k| opts.iter().find(|o| &o.key == k).cloned());
    let dd = dropdown("— select device —", opts, sel, move |o: Opt| Message::Send(Command::SetBusDevice { bus: idx, device: Some(o.key) }));
    let mute = button(text("M").size(10).color(if bus.mute { theme::BG_DEEP } else { theme::TEXT_DIM }))
        .style(move |_t, _s| button::Style { background: Some(iced::Background::Color(if bus.mute { theme::REC_RED } else { theme::PANEL_HI })), border: Border { color: theme::EDGE, width: 1.0, radius: 5.0.into() }, text_color: theme::TEXT_DIM, ..Default::default() })
        .padding([4, 8]).on_press(Message::Send(Command::SetBusMute { bus: idx, mute: !bus.mute }));
    container(row![text(&bus.label).size(14).color(theme::ACCENT), Space::with_width(8), dd, Space::with_width(6), mute].align_y(Alignment::Center))
        .padding(8).width(Length::Fixed(340.0)).style(theme::card).into()
}
