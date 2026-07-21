//! FerroMix2 Iced widgets — real interaction: draggable + scroll-wheel faders,
//! draggable DSP knobs, device/app dropdowns, and the send grid.

use crate::theme;
use crate::tokens;
use crate::Message;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input, Space};
use iced::{Alignment, Border, Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use crate::RenameTarget;
use mixer_core::engine::Command;
use mixer_core::model::{pos_to_db, Bus, BusKind, MixerState, RecTarget, Strip, StripDsp};

const FADER_H: f32 = 150.0;
const FADER_W: f32 = 20.0;

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

/// The click-to-rename header used by both strip and bus cards: plain text
/// that becomes a text field on click, committed on Enter.
fn rename_head<'a>(name: String, renaming: Option<&'a str>, target: RenameTarget, size: u16, accent: Color, trailing: Element<'a, Message>) -> Element<'a, Message> {
    if let Some(draft) = renaming {
        text_input("name", draft)
            .on_input(Message::RenameChanged)
            .on_submit(Message::RenameSubmit)
            .size(size)
            .padding(tokens::space::XS)
            .style(move |_t, _s| text_input::Style {
                background: iced::Background::Color(theme::PANEL_HI),
                border: Border { color: accent, width: 1.0, radius: tokens::radius::SM.into() },
                icon: theme::TEXT_DIM,
                placeholder: theme::TEXT_DIM,
                value: theme::TEXT,
                selection: theme::with_alpha(accent, 0.35),
            })
            .into()
    } else {
        let label = button(text(elide(&name, 14)).size(size).color(theme::TEXT))
            .style(|_t, _s| button::Style { background: None, text_color: theme::TEXT, ..Default::default() })
            .padding(0)
            .on_press(Message::RenameStart(target, name));
        row![label, Space::with_width(Length::Fill), trailing].align_y(Alignment::Center).into()
    }
}

/// REC arm toggle — same visual language as MUTE, but red-on when armed.
fn rec_button<'a>(recording: bool, target: RecTarget) -> Element<'a, Message> {
    let msg = if recording {
        Message::Send(Command::StopRecordTarget { target })
    } else {
        Message::Send(Command::StartRecordTarget { target })
    };
    wide_button(if recording { "■ STOP" } else { "● REC" }, recording, theme::REC_RED, msg)
}

fn wide_button<'a>(label: &'a str, on: bool, accent: Color, msg: Message) -> Element<'a, Message> {
    let (bg, fg) = if on { (accent, theme::BG_DEEP) } else { (theme::PANEL_HI, theme::TEXT_DIM) };
    button(text(label).size(10).color(fg).center().width(Length::Fill))
        .style(move |_t, _s| button::Style { background: Some(iced::Background::Color(bg)), border: Border { color: if on { accent } else { theme::EDGE }, width: 1.0, radius: 6.0.into() }, text_color: fg, ..Default::default() })
        .width(Length::Fill).padding([6, 0]).on_press(msg).into()
}

/// A canvas-drawn vertical fader: rounded cap, glow rail below the handle,
/// matching the DSP `Dial`'s visual language. Click/drag to set, scroll to
/// nudge, right-click to reset to unity (0.0 dB) — same interaction model as
/// before, just hand-drawn instead of a restyled built-in slider.
struct FaderCap<F> {
    value: f32,
    accent: Color,
    unity: f32,
    on_change: F,
}

impl<F: Fn(f32) -> Message> FaderCap<F> {
    fn emit(&self, v: f32) -> Message {
        (self.on_change)(v.clamp(0.0, 1.0))
    }
    fn value_at(&self, bounds: Rectangle, y: f32) -> f32 {
        (1.0 - (y - bounds.y) / bounds.height).clamp(0.0, 1.0)
    }
}

impl<F: Fn(f32) -> Message> canvas::Program<Message> for FaderCap<F> {
    type State = bool; // dragging?

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        use canvas::event::{self, Event};
        use iced::mouse::{self, Button};
        let inside = cursor.is_over(bounds);
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) if inside => {
                *state = true;
                let Some(pos) = cursor.position() else { return (event::Status::Ignored, None) };
                (event::Status::Captured, Some(self.emit(self.value_at(bounds, pos.y))))
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if *state => {
                let Some(pos) = cursor.position() else { return (event::Status::Ignored, None) };
                (event::Status::Captured, Some(self.emit(self.value_at(bounds, pos.y))))
            }
            Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) => {
                *state = false;
                (event::Status::Ignored, None)
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if inside => {
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                (event::Status::Captured, Some(self.emit(self.value + dy * 0.02)))
            }
            Event::Mouse(mouse::Event::ButtonPressed(Button::Right)) if inside => {
                (event::Status::Captured, Some(self.emit(self.unity)))
            }
            _ => (event::Status::Ignored, None),
        }
    }

    fn draw(&self, _s: &Self::State, r: &Renderer, _t: &Theme, b: Rectangle, _c: iced::mouse::Cursor) -> Vec<Geometry> {
        let mut f = Frame::new(r, b.size());
        let rail_w = 5.0;
        let cx = b.width / 2.0;
        let rail_x = cx - rail_w / 2.0;

        // Track.
        f.fill(&Path::rectangle(Point::new(rail_x, 0.0), Size::new(rail_w, b.height)), theme::BG_DEEP);
        f.stroke(
            &Path::rounded_rectangle(Point::new(rail_x, 0.0), Size::new(rail_w, b.height), 3.0.into()),
            Stroke::default().with_color(theme::EDGE).with_width(1.0),
        );

        let v = self.value.clamp(0.0, 1.0);
        let handle_y = b.height * (1.0 - v);

        // Glow fill from the handle down to the bottom (louder = more lit rail).
        let fill_h = b.height - handle_y;
        if fill_h > 0.5 {
            f.fill(
                &Path::rectangle(Point::new(rail_x, handle_y), Size::new(rail_w, fill_h)),
                theme::with_alpha(self.accent, 0.85),
            );
        }

        // Handle: rounded cap with a subtle top highlight, bordered in accent.
        let handle_h = 22.0_f32.min(b.height);
        let handle_w = b.width.min(26.0);
        let handle_top = (handle_y - handle_h / 2.0).clamp(0.0, (b.height - handle_h).max(0.0));
        let handle_rect = Path::rounded_rectangle(
            Point::new(cx - handle_w / 2.0, handle_top),
            Size::new(handle_w, handle_h),
            4.0.into(),
        );
        f.fill(&handle_rect, theme::PANEL_HI);
        f.stroke(&handle_rect, Stroke::default().with_color(self.accent).with_width(1.5));
        f.fill(
            &Path::rectangle(Point::new(cx - handle_w / 2.0 + 3.0, handle_top + 3.0), Size::new(handle_w - 6.0, 2.5)),
            theme::with_alpha(theme::TEXT, 0.3),
        );

        vec![f.into_geometry()]
    }
}

fn fader<'a>(value: f32, accent: Color, unity: f32, on_change: impl Fn(f32) -> Message + 'a) -> Element<'a, Message> {
    Canvas::new(FaderCap { value, accent, unity, on_change })
        .width(Length::Fixed(FADER_W))
        .height(Length::Fixed(FADER_H))
        .into()
}

pub fn fmt_db(pos: f32) -> String {
    if pos <= 0.002 { "-∞".into() } else { let db = pos_to_db(pos); if db.abs() < 0.05 { "0.0".into() } else { format!("{db:+.1}") } }
}
fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { let t: String = s.chars().take(max.saturating_sub(1)).collect(); format!("{t}…") }
}

fn knob<'a>(label: &'a str, value: f32, on: bool, accent: Color, strip: usize, dsp: StripDsp, is_gate: bool) -> Element<'a, Message> {
    let dial = Canvas::new(Dial { value, on, accent, strip, dsp, is_gate })
        .width(Length::Fixed(48.0))
        .height(Length::Fixed(48.0));
    let toggle = { let ndsp = if is_gate { StripDsp { gate_on: !dsp.gate_on, ..dsp } } else { StripDsp { comp_on: !dsp.comp_on, ..dsp } }; Message::Send(Command::SetStripDsp { strip, dsp: ndsp }) };
    let lbl = button(text(label).size(9).color(if on { accent } else { theme::TEXT_DIM }).center().width(Length::Fill))
        .style(move |_t, _s| button::Style { background: Some(iced::Background::Color(if on { theme::with_alpha(accent, 0.15) } else { theme::SEG_OFF })), border: Border { color: if on { accent } else { theme::EDGE }, width: 1.0, radius: 4.0.into() }, text_color: if on { accent } else { theme::TEXT_DIM }, ..Default::default() })
        .width(Length::Fixed(48.0)).padding([2, 0]).on_press(toggle);
    column![dial, lbl].spacing(3).align_x(Alignment::Center).into()
}

/// Interactive DSP knob. Click+drag vertically to set the amount, scroll to
/// fine-tune. Renders a glowing gradient ring with tick marks.
struct Dial { value: f32, on: bool, accent: Color, strip: usize, dsp: StripDsp, is_gate: bool }

impl Dial {
    fn emit(&self, nv: f32) -> Message {
        let nv = nv.clamp(0.0, 1.0);
        let ndsp = if self.is_gate { StripDsp { gate: nv, ..self.dsp } } else { StripDsp { comp: nv, ..self.dsp } };
        Message::Send(Command::SetStripDsp { strip: self.strip, dsp: ndsp })
    }
}

impl canvas::Program<Message> for Dial {
    type State = Option<f32>; // drag anchor: cursor-y at press

    fn update(&self, state: &mut Self::State, event: canvas::Event, bounds: Rectangle, cursor: iced::mouse::Cursor) -> (canvas::event::Status, Option<Message>) {
        use canvas::event::{self, Event};
        use iced::mouse::{self, Button};
        let inside = cursor.is_over(bounds);
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) if inside => {
                *state = cursor.position().map(|p| p.y);
                (event::Status::Captured, None)
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let (Some(anchor), Some(pos)) = (*state, cursor.position()) {
                    // Drag up = increase. 120px of travel = full range.
                    let delta = (anchor - pos.y) / 120.0;
                    *state = Some(pos.y);
                    return (event::Status::Captured, Some(self.emit(self.value + delta)));
                }
                (event::Status::Ignored, None)
            }
            Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) => {
                *state = None;
                (event::Status::Ignored, None)
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if inside => {
                let dy = match delta { mouse::ScrollDelta::Lines { y, .. } => y, mouse::ScrollDelta::Pixels { y, .. } => y / 40.0 };
                (event::Status::Captured, Some(self.emit(self.value + dy * 0.05)))
            }
            Event::Mouse(mouse::Event::ButtonPressed(Button::Right)) if inside => {
                // Right-click resets to default amount.
                (event::Status::Captured, Some(self.emit(0.4)))
            }
            _ => (event::Status::Ignored, None),
        }
    }

    fn draw(&self, _s: &Self::State, r: &Renderer, _t: &Theme, b: Rectangle, _c: iced::mouse::Cursor) -> Vec<Geometry> {
        use std::f32::consts::PI;
        let mut f = Frame::new(r, b.size());
        let c = Point::new(b.width / 2.0, b.height / 2.0);
        let rad = b.width / 2.0 - 6.0;
        let start = PI * 0.75;
        let sweep = PI * 1.5;

        // Tick marks around the dial.
        for i in 0..=10 {
            let a = start + sweep * (i as f32 / 10.0);
            let (i0, i1) = (rad + 1.0, rad + 4.0);
            f.stroke(
                &Path::line(Point::new(c.x + i0 * a.cos(), c.y + i0 * a.sin()), Point::new(c.x + i1 * a.cos(), c.y + i1 * a.sin())),
                Stroke::default().with_color(theme::with_alpha(theme::EDGE, 0.8)).with_width(1.0),
            );
        }

        // Track.
        let dim = theme::with_alpha(self.accent, if self.on { 0.22 } else { 0.10 });
        f.stroke(
            &Path::new(|p| { p.arc(canvas::path::Arc { center: c, radius: rad, start_angle: iced::Radians(start), end_angle: iced::Radians(start + sweep) }); }),
            Stroke::default().with_color(dim).with_width(5.0),
        );

        if self.on {
            let v = self.value.clamp(0.0, 1.0);
            // Glow underlay.
            f.stroke(
                &Path::new(|p| { p.arc(canvas::path::Arc { center: c, radius: rad, start_angle: iced::Radians(start), end_angle: iced::Radians(start + sweep * v) }); }),
                Stroke::default().with_color(theme::with_alpha(self.accent, 0.35)).with_width(9.0),
            );
            // Bright value arc.
            f.stroke(
                &Path::new(|p| { p.arc(canvas::path::Arc { center: c, radius: rad, start_angle: iced::Radians(start), end_angle: iced::Radians(start + sweep * v) }); }),
                Stroke::default().with_color(self.accent).with_width(5.0),
            );
            // Pointer dot.
            let ang = start + sweep * v;
            f.fill(&Path::circle(Point::new(c.x + rad * ang.cos(), c.y + rad * ang.sin()), 3.5), theme::TEXT);
        }

        // Hub with a subtle bevel.
        f.fill(&Path::circle(c, rad * 0.5), theme::PANEL_HI);
        f.stroke(&Path::circle(c, rad * 0.5), Stroke::default().with_color(theme::with_alpha(self.accent, if self.on { 0.5 } else { 0.2 })).with_width(1.0));
        vec![f.into_geometry()]
    }
}

struct Meter { level: f32, accent: Color }
impl<M> canvas::Program<M> for Meter {
    type State = ();
    fn draw(&self, _s: &(), r: &Renderer, _t: &Theme, b: Rectangle, _c: iced::mouse::Cursor) -> Vec<Geometry> {
        let mut f = Frame::new(r, b.size());
        f.fill(&Path::rounded_rectangle(Point::ORIGIN, b.size(), 3.0.into()), theme::SEG_OFF);
        let segs = 20;
        let lit = (self.level.clamp(0.0, 1.0) * segs as f32).round() as i32;
        let sh = b.height / segs as f32;
        // Topmost lit segment = the loudest one; give it a soft bloom (a wider,
        // dimmer underlay) so the peak reads at a glance instead of just being
        // "one more solid block".
        let peak_i = segs - lit;
        for i in 0..segs {
            if i as i32 >= segs as i32 - lit {
                let frac = (segs - 1 - i) as f32 / segs as f32;
                let col = if frac > 0.85 { theme::METER_HI } else if frac > 0.6 { theme::METER_MID } else { theme::METER_LO };
                let seg = Path::rounded_rectangle(
                    Point::new(1.5, i as f32 * sh + 0.75),
                    Size::new(b.width - 3.0, sh - 1.5),
                    1.5.into(),
                );
                if i as i32 == peak_i {
                    f.fill(
                        &Path::rounded_rectangle(
                            Point::new(0.0, i as f32 * sh - 1.0),
                            Size::new(b.width, sh + 2.0),
                            2.5.into(),
                        ),
                        theme::with_alpha(col, 0.35),
                    );
                }
                f.fill(&seg, col);
            }
        }
        f.stroke(
            &Path::rounded_rectangle(Point::ORIGIN, b.size(), 3.0.into()),
            Stroke::default().with_color(theme::with_alpha(self.accent, 0.4)).with_width(1.0),
        );
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

pub fn strip_card<'a>(idx: usize, strip: &'a Strip, state: &'a MixerState, width: f32, renaming: Option<&'a str>) -> Element<'a, Message> {
    let accent = theme::ACCENT;
    let live_dot = text(if strip.input_live { "●" } else { "○" }).size(8).color(if strip.input_live { accent } else { theme::TEXT_DIM });
    let head = rename_head(strip.display_name(idx), renaming, RenameTarget::Strip(idx), 11, accent, live_dot.into());
    let opts: Vec<Opt> = state.inputs.iter().map(|i| Opt { key: i.key.clone(), label: i.label.clone() }).collect();
    let sel = strip.input.as_ref().and_then(|k| opts.iter().find(|o| &o.key == k).cloned());
    let input_dd = dropdown("— select source —", opts, sel, move |o: Opt| Message::Send(Command::SetStripInput { strip: idx, input: Some(o.key) }));
    let meter = Canvas::new(Meter { level: strip.level.peak(), accent }).width(Length::Fixed(20.0)).height(Length::Fixed(FADER_H));
    let fad = fader(strip.volume, accent, mixer_core::model::UNITY_POS, move |v| Message::Send(Command::SetStripVolume { strip: idx, volume: v }));
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
    let rec = rec_button(strip.recording, RecTarget::Strip(idx));
    // SET AS DEFAULT: make this strip the system default output, so any app on
    // "default" (e.g. Spotify) flows into it automatically. This is how you get
    // "all my desktop audio through one strip" without configuring each app.
    let is_default = state.default_output == Some(idx);
    let default_btn = wide_button(
        if is_default { "★ SYSTEM DEFAULT" } else { "SET AS DEFAULT" },
        is_default,
        theme::MIC_AMBER,
        Message::Send(Command::SetDefaultOutput { strip: idx }),
    );
    let body = column![head, Space::with_height(5), input_dd, Space::with_height(8), fader_row, Space::with_height(8), column![a_row, b_row].spacing(3), Space::with_height(8), knobs, Space::with_height(8), row![mute, Space::with_width(4), rec].spacing(0), Space::with_height(4), default_btn]
        .spacing(0).width(Length::Fixed(width));
    container(body).padding(10).width(Length::Fixed(width + 20.0)).style(theme::card_accent(if strip.input_live { accent } else { theme::EDGE_SOFT })).into()
}

pub fn bus_card<'a>(idx: usize, bus: &'a Bus, state: &'a MixerState, width: f32, renaming: Option<&'a str>) -> Element<'a, Message> {
    let accent = theme::VIOLET;
    let name = if bus.name.is_empty() { bus.label.clone() } else { bus.name.clone() };
    let mic_tag = text("MIC").size(8).color(theme::TEXT_DIM);
    let head = rename_head(name, renaming, RenameTarget::Bus(idx), 14, accent, mic_tag.into());
    let opts: Vec<Opt> = state.capture_apps.iter().map(|a| Opt { key: a.key.clone(), label: a.label.clone() }).collect();
    let sel = bus.listener.as_ref().and_then(|k| opts.iter().find(|o| &o.key == k).cloned());
    let app_dd = dropdown("◇ SEND TO APP", opts, sel, move |o: Opt| Message::Send(Command::SetBusListener { bus: idx, app: Some(o.key) }));
    let listening = if bus.listeners.is_empty() { text("no app assigned").size(9).color(theme::TEXT_DIM) } else { text(format!("◂ {} listening", elide(&bus.listeners[0], 12))).size(9).color(accent) };
    let meter = Canvas::new(Meter { level: bus.level.peak(), accent }).width(Length::Fixed(20.0)).height(Length::Fixed(FADER_H));
    let fad = fader(bus.volume, accent, mixer_core::model::UNITY_POS, move |v| Message::Send(Command::SetBusVolume { bus: idx, volume: v }));
    let fader_row = row![meter, Space::with_width(4), fad, Space::with_width(8), text(fmt_db(bus.volume)).size(11).color(theme::TEXT)].align_y(Alignment::Center);
    let mut mon = row![].spacing(3);
    let a_buses: Vec<(usize, &Bus)> = state.buses.iter().enumerate().filter(|(_, b)| b.kind == BusKind::HwOutput).collect();
    for (ai, (_, ab)) in a_buses.iter().enumerate() {
        let on = bus.monitor.get(ai).copied().unwrap_or(false);
        mon = mon.push(send_pill(&ab.label, on, false, false, Message::Send(Command::ToggleBusMonitor { bus: idx, a_bus: ai })));
    }
    let mute = wide_button("MUTE", bus.mute, theme::REC_RED, Message::Send(Command::SetBusMute { bus: idx, mute: !bus.mute }));
    let rec = rec_button(bus.recording, RecTarget::Bus(idx));
    // SET AS DEF MIC: make this virtual mic the system default input, so apps
    // on "default" microphone (e.g. browsers) transmit this bus's mix.
    let is_def_mic = state.default_input == Some(idx);
    let def_mic_btn = wide_button(
        if is_def_mic { "★ DEFAULT MIC" } else { "SET AS DEF MIC" },
        is_def_mic,
        theme::MIC_AMBER,
        Message::Send(Command::SetDefaultInput { bus: idx }),
    );
    let body = column![head, Space::with_height(5), app_dd, Space::with_height(3), listening, Space::with_height(6), fader_row, Space::with_height(8), text("MONITOR ON").size(8).color(theme::TEXT_DIM), Space::with_height(2), mon, Space::with_height(8), row![mute, Space::with_width(4), rec].spacing(0), Space::with_height(4), def_mic_btn]
        .spacing(0).width(Length::Fixed(width));
    container(body).padding(10).width(Length::Fixed(width + 20.0)).style(theme::card_accent(accent)).into()
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

// ─────────────────────────────────────────────────────── MATRIX

/// The patch matrix: strips as rows, buses as columns, a toggle cell at each
/// crossing. Cyan = hardware send, violet = virtual mic, coral = feedback.
pub fn matrix_view<'a>(state: &'a MixerState) -> Element<'a, Message> {
    let mut grid = column![].spacing(6);

    // Header row: bus labels.
    let mut header = row![container(text("").width(Length::Fixed(150.0))).width(Length::Fixed(150.0))].spacing(6);
    for bus in &state.buses {
        let col = if bus.kind == BusKind::VirtualMic { theme::VIOLET } else { theme::ACCENT };
        let tag = if bus.kind == BusKind::VirtualMic { "MIC" } else { "OUT" };
        header = header.push(
            container(column![text(&bus.label).size(12).color(col), text(tag).size(7).color(theme::TEXT_DIM)].align_x(Alignment::Center))
                .width(Length::Fixed(54.0)).center_x(Length::Fixed(54.0)),
        );
    }
    grid = grid.push(header);

    for (si, strip) in state.strips.iter().enumerate() {
        let name = strip.display_name(si);
        let name_col = if !strip.input_live { theme::TEXT_DIM } else { theme::TEXT };
        let mut r = row![
            container(text(format!("{:02}  {}", si + 1, elide(&name, 16))).size(11).color(name_col))
                .width(Length::Fixed(150.0))
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        for (bi, bus) in state.buses.iter().enumerate() {
            let on = strip.assign.get(bi).copied().unwrap_or(false);
            let fb = state.is_feedback(si, bi);
            let is_b = bus.kind == BusKind::VirtualMic;
            let accent = if is_b { theme::VIOLET } else { theme::ACCENT };
            let (bg, mark) = if fb { (theme::DANGER, "✕") } else if on { (accent, "●") } else { (theme::SEG_OFF, "") };
            let cell = button(text(mark).size(12).color(theme::BG_DEEP).center().width(Length::Fill))
                .style(move |_t, _s| button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: Border { color: if on || fb { bg } else { theme::EDGE }, width: 1.0, radius: 6.0.into() },
                    text_color: theme::BG_DEEP, ..Default::default()
                })
                .width(Length::Fixed(54.0)).height(Length::Fixed(30.0))
                .on_press(Message::Send(Command::ToggleAssign { strip: si, bus: bi }));
            r = r.push(cell);
        }
        grid = grid.push(r);
    }

    let head = column![
        text("PATCH MATRIX").size(13).color(theme::TEXT),
        text("rows send into columns · cyan = hardware out · violet = virtual mic · ✕ = feedback blocked").size(9).color(theme::TEXT_DIM),
    ]
    .spacing(4);

    container(column![head, Space::with_height(16), grid].spacing(0).padding(20))
        .width(Length::Fill).into()
}

// ─────────────────────────────────────────────────────── SETTINGS

pub fn settings_view<'a>(state: &'a MixerState, recdir_draft: Option<&'a str>) -> Element<'a, Message> {
    let section = |title: &'a str| text(title).size(11).color(theme::ACCENT);

    // Feedback guard toggle.
    let guard = state.feedback_guard;
    let guard_btn = wide_button(
        if guard { "FEEDBACK GUARD: ON" } else { "FEEDBACK GUARD: OFF" },
        guard,
        theme::ACCENT,
        Message::Send(Command::SetFeedbackGuard { on: !guard }),
    );

    let card = |content: Element<'a, Message>| {
        container(content).padding(16).width(Length::Fill).max_width(640.0).style(theme::card)
    };

    let routing = card(
        column![
            section("ROUTING"),
            Space::with_height(8),
            container(guard_btn).width(Length::Fixed(260.0)),
            Space::with_height(6),
            text("Blocks a strip from sending into a virtual mic its own app captures — prevents echo loops. Leave ON unless you know what you're doing.").size(9).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    let recdir_value = recdir_draft.unwrap_or(state.recordings_dir.as_str());
    let recdir_input = text_input("~/Music/ferromix2", recdir_value)
        .on_input(Message::RecDirChanged)
        .on_submit(Message::RecDirApply)
        .size(10)
        .padding(tokens::space::SM)
        .style(|_t, _s| text_input::Style {
            background: iced::Background::Color(theme::BG_DEEP),
            border: Border { color: theme::EDGE, width: 1.0, radius: tokens::radius::SM.into() },
            icon: theme::TEXT_DIM,
            placeholder: theme::TEXT_DIM,
            value: theme::TEXT,
            selection: theme::with_alpha(theme::ACCENT, 0.35),
        })
        .width(Length::Fill);
    let apply_btn = button(text("APPLY").size(9).color(theme::BG_DEEP).center())
        .style(|_t, _s| button::Style {
            background: Some(iced::Background::Color(theme::ACCENT)),
            border: Border { color: theme::ACCENT, width: 1.0, radius: tokens::radius::SM.into() },
            text_color: theme::BG_DEEP,
            ..Default::default()
        })
        .padding([6, 10])
        .on_press(Message::RecDirApply);

    let rec = card(
        column![
            section("RECORDING"),
            Space::with_height(8),
            row![recdir_input, Space::with_width(6), apply_btn].align_y(Alignment::Center),
            Space::with_height(4),
            text("Each armed track writes its own WAV. Arm tracks with the REC button on a strip or bus card.").size(9).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    let scale = if state.ui_scale > 0.0 { state.ui_scale } else { 1.0 };
    let scale_pct = (scale * 100.0).round() as i32;
    let step_btn = move |label: &'static str, delta: f32| -> Element<'a, Message> {
        button(text(label).size(12).color(theme::TEXT).center().width(Length::Fill))
            .style(|_t, _s| button::Style {
                background: Some(iced::Background::Color(theme::PANEL_HI)),
                border: Border { color: theme::EDGE, width: 1.0, radius: tokens::radius::SM.into() },
                text_color: theme::TEXT,
                ..Default::default()
            })
            .width(Length::Fixed(32.0))
            .padding([4, 0])
            .on_press(Message::Send(Command::SetUiScale { scale: (scale + delta).clamp(0.5, 3.0) }))
            .into()
    };
    let display = card(
        column![
            section("DISPLAY"),
            Space::with_height(8),
            row![
                step_btn("−", -0.1),
                Space::with_width(10),
                text(format!("{scale_pct}%")).size(12).color(theme::TEXT).width(Length::Fixed(48.0)),
                Space::with_width(10),
                step_btn("+", 0.1),
            ]
            .align_y(Alignment::Center),
            Space::with_height(6),
            text("UI scale. Persists across restarts.").size(9).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    let latency = card(
        column![
            section("LATENCY (the Linux \"ASIO\")"),
            Space::with_height(8),
            text("FerroMix2 runs on PipeWire — no ASIO driver needed. For low-latency, set a small quantum globally:").size(9).color(theme::TEXT_DIM),
            Space::with_height(6),
            container(text("pw-metadata -n settings 0 clock.force-quantum 256").size(10).color(theme::ACCENT))
                .padding(8).style(|_t| iced::widget::container::Style { background: Some(iced::Background::Color(theme::BG_DEEP)), border: Border { color: theme::EDGE, width: 1.0, radius: 4.0.into() }, ..Default::default() }),
            Space::with_height(4),
            text("256 samples @ 48kHz ≈ 5ms. Lower = tighter but more CPU. Reset with clock.force-quantum 0.").size(9).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    let about = card(
        column![
            section("ABOUT"),
            Space::with_height(8),
            text("FerroMix2 — an open-source Voicemeeter-class mixer for Linux / PipeWire.").size(10).color(theme::TEXT),
            text("Every strip receives one source. A-buses you hear; B-buses are virtual mics apps read. MUTE cuts a strip everywhere.").size(9).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    scrollable(
        column![
            text("SETTINGS").size(14).color(theme::TEXT),
            Space::with_height(16),
            routing,
            Space::with_height(12),
            rec,
            Space::with_height(12),
            display,
            Space::with_height(12),
            latency,
            Space::with_height(12),
            about,
        ]
        .spacing(0)
        .padding(20),
    )
    .into()
}

// ─────────────────────────────────────────────────────── LOG

/// Activity log — newest entries first, so the most recent routing/feedback/
/// save events are visible without scrolling.
pub fn log_view<'a>(state: &'a MixerState) -> Element<'a, Message> {
    let mut lines = column![].spacing(4);
    for line in state.log.iter().rev() {
        lines = lines.push(text(line).size(10).color(theme::TEXT_DIM).font(iced::Font::MONOSPACE));
    }
    let head = column![
        text("ACTIVITY LOG").size(14).color(theme::TEXT),
        text("Routing changes, feedback blocks, saves — newest first.").size(9).color(theme::TEXT_DIM),
    ]
    .spacing(4);

    scrollable(
        column![head, Space::with_height(16), container(lines).padding(12).width(Length::Fill).style(theme::card)]
            .spacing(0)
            .padding(20),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
