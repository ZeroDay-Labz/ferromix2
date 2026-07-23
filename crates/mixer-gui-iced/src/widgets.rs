//! FerroMix2 Iced widgets — real interaction: draggable + scroll-wheel faders,
//! draggable DSP knobs, device/app dropdowns, and the send grid.

use crate::icons;
use crate::theme;
use crate::tokens;
use crate::Message;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use crate::RenameTarget;
use mixer_core::engine::Command;
use mixer_core::model::{pos_to_db, Bus, BusKind, MixerState, RecTarget, SourceKind, Strip, StripDsp};

const FADER_H: f32 = 150.0;
const FADER_W: f32 = 20.0;

/// Every flat-color button in this app was styling itself without ever
/// looking at `button::Status` — no hover or press feedback anywhere, which
/// reads as dead/unresponsive no matter how good the resting-state colors
/// are. This is the shared brightening/darkening curve every button style
/// below runs its base color through, so hovering and pressing always give
/// real, consistent feedback.
pub fn interactive(base: Color, status: button::Status) -> Color {
    mix(base, status_target(status).0, status_target(status).1)
}

fn mix(c: Color, target: f32, amount: f32) -> Color {
    Color { r: c.r + (target - c.r) * amount, g: c.g + (target - c.g) * amount, b: c.b + (target - c.b) * amount, a: c.a }
}

fn status_target(status: button::Status) -> (f32, f32) {
    match status {
        button::Status::Hovered => (1.0, 0.14),
        button::Status::Pressed => (0.0, 0.16),
        button::Status::Active | button::Status::Disabled => (0.0, 0.0),
    }
}

/// A filled "on" surface with real depth instead of a flat color — a subtle
/// top-lit vertical gradient (mirrors `theme::card_gradient`'s reasoning) at
/// rest, falling back to the flat `interactive()` hover/press mix while
/// actually being interacted with (a gradient sliding under the cursor mid-
/// hover would read as a glitch, not polish). Used for every "lit" pill/
/// button — send pills, MUTE/REC when armed, the active tab — so the same
/// glassy language established for cards carries through to controls too.
pub fn accent_fill(base: Color, status: button::Status) -> Background {
    match status {
        button::Status::Hovered | button::Status::Pressed => Background::Color(interactive(base, status)),
        button::Status::Active | button::Status::Disabled => Background::Gradient(
            iced::gradient::Linear::new(iced::Radians(std::f32::consts::FRAC_PI_2))
                .add_stop(0.0, mix(base, 1.0, 0.22))
                .add_stop(1.0, base)
                .into(),
        ),
    }
}

pub fn tab_button(label: &str, active: bool) -> button::Button<'_, Message> {
    let fg = if active { theme::ACCENT } else { theme::TEXT_DIM };
    button(text(label).size(tokens::type_scale::SUBTITLE).color(fg))
        .style(move |_t, s| {
            let bg = if active {
                accent_fill(theme::ACCENT.scale_alpha(0.16), s)
            } else if matches!(s, button::Status::Hovered) {
                Background::Color(theme::EDGE_SOFT.scale_alpha(0.5))
            } else {
                Background::Color(Color::TRANSPARENT)
            };
            button::Style {
                background: Some(bg),
                border: Border { color: if active { theme::ACCENT } else { theme::EDGE_SOFT }, width: 1.0, radius: tokens::radius::MD.into() },
                text_color: fg, ..Default::default()
            }
        })
        .padding([6, 14])
}

fn send_pill<'a>(label: impl Into<String>, on: bool, fb: bool, is_b: bool, msg: Message) -> Element<'a, Message> {
    let label = label.into();
    let accent = if is_b { theme::VIOLET } else { theme::ACCENT };
    let (bg, fg, edge) = if fb { (theme::DANGER, theme::BG_DEEP, theme::DANGER) }
        else if on { (accent, theme::BG_DEEP, accent) }
        else { (theme::SEG_OFF, theme::TEXT_DIM, theme::EDGE) };
    button(text(label).size(tokens::type_scale::LABEL).color(fg).center().width(Length::Fill))
        .style(move |_t, s| button::Style { background: Some(accent_fill(bg, s)), border: Border { color: edge, width: 1.0, radius: tokens::radius::SM.into() }, text_color: fg, ..Default::default() })
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
                selection: accent.scale_alpha(0.35),
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
    button(text(label).size(tokens::type_scale::LABEL).color(fg).center().width(Length::Fill))
        .style(move |_t, s| button::Style { background: Some(accent_fill(bg, s)), border: Border { color: if on { accent } else { theme::EDGE }, width: 1.0, radius: tokens::radius::MD.into() }, text_color: fg, ..Default::default() })
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

/// `drag` is `Some((anchor_y, anchor_value))` for the whole gesture, fixed at
/// press time and never re-slid (same drift-immune pattern as `Dial` —
/// see its doc comment). `last_click` feeds double-click detection.
/// `shift_held` tracks the Shift modifier globally (canvas widgets receive
/// keyboard events regardless of cursor position — confirmed against Iced's
/// own `canvas.rs::on_event`, which forwards `core::Event::Keyboard`
/// unconditionally) so holding Shift mid-drag divides further movement by
/// 10 for fine adjustment, without needing the cursor to stay in any
/// particular place. `track_cache` holds the static rail (only depends on
/// widget size, never on `value`); `dynamic_cache`/`last_value` hold the
/// value-dependent handle+fill, invalidated only when the value actually
/// changes — same reasoning as `MeterState`.
struct FaderState {
    drag: Option<(f32, f32)>,
    last_click: Option<iced::advanced::mouse::click::Click>,
    shift_held: bool,
    track_cache: canvas::Cache,
    dynamic_cache: canvas::Cache,
    last_value: std::cell::Cell<f32>,
}
impl Default for FaderState {
    fn default() -> Self {
        Self {
            drag: None,
            last_click: None,
            shift_held: false,
            track_cache: canvas::Cache::default(),
            dynamic_cache: canvas::Cache::default(),
            last_value: std::cell::Cell::new(-1.0),
        }
    }
}

impl<F: Fn(f32) -> Message> canvas::Program<Message> for FaderCap<F> {
    type State = FaderState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        use canvas::event::{self, Event};
        use iced::{keyboard, mouse::{self, Button}};
        let inside = cursor.is_over(bounds);
        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                state.shift_held = m.shift();
                (event::Status::Ignored, None)
            }
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) if inside => {
                let Some(pos) = cursor.position() else { return (event::Status::Ignored, None) };
                let click = iced::advanced::mouse::click::Click::new(pos, Button::Left, state.last_click);
                state.last_click = Some(click);
                if matches!(click.kind(), iced::advanced::mouse::click::Kind::Double) {
                    // Double-click: snap back to unity, same as right-click,
                    // just a more discoverable gesture for some users. Don't
                    // start a drag on top of it.
                    state.drag = None;
                    return (event::Status::Captured, Some(self.emit(self.unity)));
                }
                // Click jumps the fader to the clicked position (standard
                // fader UX), then anchors THAT position/value as the
                // reference for the rest of the drag.
                let v = self.value_at(bounds, pos.y);
                state.drag = Some((pos.y, v));
                (event::Status::Captured, Some(self.emit(v)))
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let (Some((anchor_y, anchor_value)), Some(pos)) = (state.drag, cursor.position()) {
                    let mut delta = (anchor_y - pos.y) / bounds.height;
                    if state.shift_held {
                        delta *= 0.1;
                    }
                    return (event::Status::Captured, Some(self.emit((anchor_value + delta).clamp(0.0, 1.0))));
                }
                (event::Status::Ignored, None)
            }
            Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) => {
                state.drag = None;
                (event::Status::Ignored, None)
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if inside => {
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                let step = if state.shift_held { 0.002 } else { 0.02 };
                (event::Status::Captured, Some(self.emit(self.value + dy * step)))
            }
            Event::Mouse(mouse::Event::ButtonPressed(Button::Right)) if inside => {
                (event::Status::Captured, Some(self.emit(self.unity)))
            }
            _ => (event::Status::Ignored, None),
        }
    }

    fn draw(&self, s: &Self::State, r: &Renderer, _t: &Theme, b: Rectangle, _c: iced::mouse::Cursor) -> Vec<Geometry> {
        let rail_w = 5.0;
        let cx = b.width / 2.0;
        let rail_x = cx - rail_w / 2.0;

        let track = s.track_cache.draw(r, b.size(), |f| {
            f.fill(&Path::rectangle(Point::new(rail_x, 0.0), Size::new(rail_w, b.height)), theme::BG_DEEP);
            f.stroke(
                &Path::rounded_rectangle(Point::new(rail_x, 0.0), Size::new(rail_w, b.height), 3.0.into()),
                Stroke::default().with_color(theme::EDGE).with_width(1.0),
            );
        });

        const EPS: f32 = 0.0005;
        if (self.value - s.last_value.get()).abs() > EPS {
            s.dynamic_cache.clear();
            s.last_value.set(self.value);
        }
        let dynamic = s.dynamic_cache.draw(r, b.size(), |f| {
            let v = self.value.clamp(0.0, 1.0);
            let handle_y = b.height * (1.0 - v);

            // Glow fill from the handle down to the bottom (louder = more lit rail).
            let fill_h = b.height - handle_y;
            if fill_h > 0.5 {
                f.fill(
                    &Path::rectangle(Point::new(rail_x, handle_y), Size::new(rail_w, fill_h)),
                    self.accent.scale_alpha(0.85),
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
                theme::TEXT.scale_alpha(0.3),
            );
        });

        vec![track, dynamic]
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
    let lbl = button(text(label).size(tokens::type_scale::CAPTION).color(if on { accent } else { theme::TEXT_DIM }).center().width(Length::Fill))
        .style(move |_t, s| button::Style { background: Some(accent_fill(if on { accent.scale_alpha(0.15) } else { theme::SEG_OFF }, s)), border: Border { color: if on { accent } else { theme::EDGE }, width: 1.0, radius: tokens::radius::SM.into() }, text_color: if on { accent } else { theme::TEXT_DIM }, ..Default::default() })
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

/// (anchor_y_at_press, anchor_value_at_press) for the drag-distance math (see
/// the big comment on `update()`), plus `live` — the value the knob is
/// CURRENTLY showing while a drag is in progress, updated on every
/// `CursorMoved` for a smooth-looking drag, but deliberately NOT sent to the
/// backend until release. `Command::SetStripDsp` triggers a full PipeWire
/// filter-chain module destroy+reload (see `dsp.rs`) — genuinely disruptive,
/// audibly cutting that strip for a moment. Emitting it on every mouse-move
/// sample during a drag (tens of times a second) turned a single knob drag
/// into a rapid-fire storm of real module teardown/rebuild cycles, which is
/// what "touching the GUI kills all audio" traced back to. Committing once,
/// on release, keeps the knob visually responsive (via `live`) without ever
/// re-loading the module more than once per gesture.
struct DialState {
    drag: Option<(f32, f32)>,
    live: Option<f32>,
    tick_cache: canvas::Cache,
    dynamic_cache: canvas::Cache,
    last_dynamic: std::cell::Cell<Option<(f32, bool)>>,
    /// Rate-limits `WheelScrolled` commits — that handler used to call
    /// `emit()` directly on every scroll tick, completely bypassing the
    /// commit-on-release mechanism above. A continuous scroll (trackpad, or
    /// a fast wheel) reproduced the exact reload storm the drag fix
    /// eliminated. `state.live` still updates on every tick (so the knob
    /// visually tracks the scroll immediately), but the actual backend
    /// commit — the expensive one — is capped to roughly once per 150ms.
    last_wheel_emit: Option<std::time::Instant>,
}
impl Default for DialState {
    fn default() -> Self {
        Self {
            drag: None,
            live: None,
            tick_cache: canvas::Cache::default(),
            dynamic_cache: canvas::Cache::default(),
            last_dynamic: std::cell::Cell::new(None),
            last_wheel_emit: None,
        }
    }
}

impl canvas::Program<Message> for Dial {
    type State = DialState;

    fn update(&self, state: &mut Self::State, event: canvas::Event, bounds: Rectangle, cursor: iced::mouse::Cursor) -> (canvas::event::Status, Option<Message>) {
        use canvas::event::{self, Event};
        use iced::mouse::{self, Button};
        let inside = cursor.is_over(bounds);
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) if inside => {
                state.drag = cursor.position().map(|p| (p.y, self.value));
                state.live = Some(self.value);
                (event::Status::Captured, None)
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let (Some((anchor_y, anchor_value)), Some(pos)) = (state.drag, cursor.position()) {
                    // Drag up = increase. 120px of travel = full range,
                    // measured from the press point, not the last event.
                    let delta = (anchor_y - pos.y) / 120.0;
                    state.live = Some((anchor_value + delta).clamp(0.0, 1.0));
                    // No message here — see DialState's doc comment. The
                    // ~60Hz UI redraw tick already picks up `state.live` on
                    // the next frame, so the knob still tracks the cursor
                    // smoothly; only the expensive backend commit waits.
                    return (event::Status::Captured, None);
                }
                (event::Status::Ignored, None)
            }
            Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) => {
                let msg = state.live.take().map(|nv| self.emit(nv));
                state.drag = None;
                (event::Status::Captured, msg)
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if inside => {
                let dy = match delta { mouse::ScrollDelta::Lines { y, .. } => y, mouse::ScrollDelta::Pixels { y, .. } => y / 40.0 };
                let nv = (self.value + dy * 0.05).clamp(0.0, 1.0);
                state.live = Some(nv);
                const MIN_WHEEL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);
                let should_emit = state.last_wheel_emit.map_or(true, |t| t.elapsed() > MIN_WHEEL_INTERVAL);
                if should_emit {
                    state.last_wheel_emit = Some(std::time::Instant::now());
                    (event::Status::Captured, Some(self.emit(nv)))
                } else {
                    // Visual already updated via `state.live` above; skip
                    // the expensive backend commit for this tick.
                    (event::Status::Captured, None)
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(Button::Right)) if inside => {
                // Right-click resets to default amount.
                (event::Status::Captured, Some(self.emit(0.4)))
            }
            _ => (event::Status::Ignored, None),
        }
    }

    fn draw(&self, s: &Self::State, r: &Renderer, _t: &Theme, b: Rectangle, _c: iced::mouse::Cursor) -> Vec<Geometry> {
        use std::f32::consts::PI;
        let c = Point::new(b.width / 2.0, b.height / 2.0);
        let rad = b.width / 2.0 - 6.0;
        let start = PI * 0.75;
        let sweep = PI * 1.5;
        let display_value = s.live.unwrap_or(self.value);

        // Tick marks: purely a function of the widget's size, never of value
        // or on/off state — cached once, redrawn only if the size changes.
        let ticks = s.tick_cache.draw(r, b.size(), |f| {
            for i in 0..=10 {
                let a = start + sweep * (i as f32 / 10.0);
                let (i0, i1) = (rad + 1.0, rad + 4.0);
                f.stroke(
                    &Path::line(Point::new(c.x + i0 * a.cos(), c.y + i0 * a.sin()), Point::new(c.x + i1 * a.cos(), c.y + i1 * a.sin())),
                    Stroke::default().with_color(theme::EDGE.scale_alpha(0.8)).with_width(1.0),
                );
            }
        });

        // Everything else depends on `display_value` and/or `on` — one cache,
        // invalidated only when either actually changes.
        let key = (display_value, self.on);
        if s.last_dynamic.get() != Some(key) {
            s.dynamic_cache.clear();
            s.last_dynamic.set(Some(key));
        }
        let dynamic = s.dynamic_cache.draw(r, b.size(), |f| {
            // Track.
            let dim = self.accent.scale_alpha(if self.on { 0.22 } else { 0.10 });
            f.stroke(
                &Path::new(|p| { p.arc(canvas::path::Arc { center: c, radius: rad, start_angle: iced::Radians(start), end_angle: iced::Radians(start + sweep) }); }),
                Stroke::default().with_color(dim).with_width(5.0),
            );

            // Value arc always renders — dimmer when off — so a drag is
            // visibly responding immediately, even before the GATE/COMP
            // label is clicked on.
            let v = display_value.clamp(0.0, 1.0);
            let (glow_a, bright_a) = if self.on { (0.35, 1.0) } else { (0.15, 0.35) };
            // Glow underlay.
            f.stroke(
                &Path::new(|p| { p.arc(canvas::path::Arc { center: c, radius: rad, start_angle: iced::Radians(start), end_angle: iced::Radians(start + sweep * v) }); }),
                Stroke::default().with_color(self.accent.scale_alpha(glow_a)).with_width(9.0),
            );
            // Value arc.
            f.stroke(
                &Path::new(|p| { p.arc(canvas::path::Arc { center: c, radius: rad, start_angle: iced::Radians(start), end_angle: iced::Radians(start + sweep * v) }); }),
                Stroke::default().with_color(self.accent.scale_alpha(bright_a)).with_width(5.0),
            );
            // Pointer dot.
            let ang = start + sweep * v;
            f.fill(&Path::circle(Point::new(c.x + rad * ang.cos(), c.y + rad * ang.sin()), 3.5), theme::TEXT.scale_alpha(if self.on { 1.0 } else { 0.5 }));

            // Hub with a subtle bevel.
            f.fill(&Path::circle(c, rad * 0.5), theme::PANEL_HI);
            f.stroke(&Path::circle(c, rad * 0.5), Stroke::default().with_color(self.accent.scale_alpha(if self.on { 0.5 } else { 0.2 })).with_width(1.0));
        });

        vec![ticks, dynamic]
    }
}

/// Stereo VU meter: two independent segmented bars (L/R) side by side, each
/// driven by its own channel's peak — a mono source lights both identically,
/// a genuinely stereo one visibly doesn't. `level` is pre-fader/source-only
/// for strips (see `sync_prefader_tap` in the daemon) — it reflects what the
/// strip's assigned source is producing, not whatever's downstream of the
/// fader or where the strip is routed to.
struct Meter { level: mixer_core::model::Level, accent: Color }
impl Meter {
    /// Draw one channel's segmented bar into `x..x+w` of the frame.
    fn draw_bar(f: &mut Frame, x: f32, w: f32, height: f32, level: f32, accent: Color) {
        f.fill(&Path::rounded_rectangle(Point::new(x, 0.0), Size::new(w, height), 3.0.into()), theme::SEG_OFF);
        let segs = 20;
        let lit = (level.clamp(0.0, 1.0) * segs as f32).round() as i32;
        let sh = height / segs as f32;
        // Topmost lit segment = the loudest one; give it a soft bloom (a wider,
        // dimmer underlay) so the peak reads at a glance instead of just being
        // "one more solid block".
        let peak_i = segs - lit;
        for i in 0..segs {
            if i as i32 >= segs as i32 - lit {
                let frac = (segs - 1 - i) as f32 / segs as f32;
                let col = if frac > 0.85 { theme::METER_HI } else if frac > 0.6 { theme::METER_MID } else { theme::METER_LO };
                let seg = Path::rounded_rectangle(
                    Point::new(x + 1.0, i as f32 * sh + 0.75),
                    Size::new(w - 2.0, sh - 1.5),
                    1.5.into(),
                );
                if i as i32 == peak_i {
                    f.fill(
                        &Path::rounded_rectangle(Point::new(x, i as f32 * sh - 1.0), Size::new(w, sh + 2.0), 2.5.into()),
                        col.scale_alpha(0.35),
                    );
                }
                f.fill(&seg, col);
            }
        }
        f.stroke(
            &Path::rounded_rectangle(Point::new(x, 0.0), Size::new(w, height), 3.0.into()),
            Stroke::default().with_color(accent.scale_alpha(0.4)).with_width(1.0),
        );
    }
}
/// A meter redraws at the ~60Hz UI tick regardless of whether the level
/// actually moved — without caching, that's full geometry re-tessellation
/// for every segment/gradient on every strip and bus, 60 times a second,
/// even during silence. `canvas::Cache` holds the last-tessellated geometry;
/// we only clear it (forcing a real redraw) when the level changed enough to
/// matter, tracked via `Cell` since `Program::draw` only gets `&State`.
pub struct MeterState {
    cache: canvas::Cache,
    last_l: std::cell::Cell<f32>,
    last_r: std::cell::Cell<f32>,
}
impl Default for MeterState {
    fn default() -> Self {
        Self { cache: canvas::Cache::default(), last_l: std::cell::Cell::new(-1.0), last_r: std::cell::Cell::new(-1.0) }
    }
}
impl<M> canvas::Program<M> for Meter {
    type State = MeterState;
    fn draw(&self, state: &Self::State, r: &Renderer, _t: &Theme, b: Rectangle, _c: iced::mouse::Cursor) -> Vec<Geometry> {
        const EPS: f32 = 0.001;
        if (self.level.l - state.last_l.get()).abs() > EPS || (self.level.r - state.last_r.get()).abs() > EPS {
            state.cache.clear();
            state.last_l.set(self.level.l);
            state.last_r.set(self.level.r);
        }
        let geometry = state.cache.draw(r, b.size(), |f| {
            let gap = 2.0;
            let bar_w = (b.width - gap) / 2.0;
            Self::draw_bar(f, 0.0, bar_w, b.height, self.level.l, self.accent);
            Self::draw_bar(f, bar_w + gap, bar_w, b.height, self.level.r, self.accent);
        });
        vec![geometry]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opt { pub key: String, pub label: String }
impl std::fmt::Display for Opt { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.label) } }

/// Sentinel key for the synthetic "clear" entry `clearable_opts` prepends.
/// Not a real source/app key, so it can never collide with one.
const NONE_KEY: &str = "\u{0}__ferromix_none__";

/// Prepend a "— none / clear —" entry so a strip/bus that's already assigned
/// can be unassigned again, not just reassigned to a different live source —
/// `SetStripInput`/`SetBusListener` already accept `None` end-to-end, the
/// dropdown just never offered a way to pick it once something was selected.
fn clearable_opts(mut opts: Vec<Opt>) -> Vec<Opt> {
    opts.insert(0, Opt { key: NONE_KEY.to_string(), label: "— none / clear —".to_string() });
    opts
}

fn dropdown<'a>(placeholder: &'a str, options: Vec<Opt>, selected: Option<Opt>, on_select: impl Fn(Opt) -> Message + 'a) -> Element<'a, Message> {
    pick_list(options, selected, on_select)
        .placeholder(placeholder).text_size(tokens::type_scale::LABEL).padding([4, 6]).width(Length::Fill)
        .style(|_t, _s| pick_list::Style { text_color: theme::TEXT, placeholder_color: theme::TEXT_DIM, handle_color: theme::TEXT_DIM, background: iced::Background::Color(theme::PANEL_HI), border: Border { color: theme::EDGE, width: 1.0, radius: tokens::radius::MD.into() } })
        .into()
}

/// A small dim-caps section label ("ROUTING", "SEND", "DSP"...) used inside
/// strip/bus cards to group related controls — the same idiom "MONITOR ON"/
/// "FEED →" already used ad hoc, pulled out into one consistent look so
/// every section header in a card reads the same way.
fn section_label<'a>(label: &'a str) -> Element<'a, Message> {
    text(label).size(tokens::type_scale::MICRO).color(theme::TEXT_DIM).font(theme::FONT_UI_SEMIBOLD).into()
}

/// A hairline rule, full width of whatever it's placed in — see
/// `theme::divider`'s doc comment. Used between a card's ROUTING/SEND/DSP
/// sections in place of blank space alone, so the structure reads as
/// deliberately designed rather than just gaps.
pub fn hr<'a>() -> Element<'a, Message> {
    container(Space::with_height(1)).width(Length::Fill).style(theme::divider).into()
}

fn strip_routing_section<'a>(idx: usize, strip: &'a Strip, state: &'a MixerState, accent: Color) -> Element<'a, Message> {
    let opts: Vec<Opt> = state.inputs.iter().map(|i| Opt { key: i.key.clone(), label: i.label.clone() }).collect();
    let sel = strip.input.as_ref().and_then(|k| opts.iter().find(|o| &o.key == k).cloned());
    let input_dd = dropdown("— select source —", clearable_opts(opts), sel, move |o: Opt| {
        let input = if o.key == NONE_KEY { None } else { Some(o.key) };
        Message::Send(Command::SetStripInput { strip: idx, input })
    });
    // A strip can also SEND to an app's microphone, same as a B-bus — full
    // strip/bus symmetry, so audio can be routed app-to-app (e.g. an app fed
    // by one strip, sent straight into another app's mic) not just app-to-
    // hardware, the same way a hardware mixer routes across multiple devices.
    let cap_opts: Vec<Opt> = state.capture_apps.iter().map(|a| Opt { key: a.key.clone(), label: a.label.clone() }).collect();
    let listener_sel = strip.listener.as_ref().and_then(|k| cap_opts.iter().find(|o| &o.key == k).cloned());
    let listener_dd = dropdown("◇ SEND TO APP", clearable_opts(cap_opts), listener_sel, move |o: Opt| {
        let app = if o.key == NONE_KEY { None } else { Some(o.key) };
        Message::Send(Command::SetStripListener { strip: idx, app })
    });
    let listening = if strip.listeners.is_empty() { text("no app assigned").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM) } else { text(format!("◂ {} listening", elide(&strip.listeners[0], 12))).size(tokens::type_scale::CAPTION).color(accent) };
    // If the same app is both this strip's input AND its listener, sending
    // the strip back to that app would let it hear itself — flag it instead
    // of leaving a silent footgun (this is the strip-level version of the
    // exact bug class the bus meter-tap fix addressed for buses).
    let echo_warn: Element<Message> = match (&strip.input, &strip.listener) {
        (Some(i), Some(l)) if i == l => text("⚠ same app as input — would echo").size(tokens::type_scale::CAPTION).color(theme::MIC_AMBER).into(),
        _ => Space::with_height(0).into(),
    };
    column![section_label("ROUTING"), Space::with_height(3), input_dd, Space::with_height(4), listener_dd, Space::with_height(3), listening, echo_warn]
        .spacing(0).into()
}

fn strip_fader_section<'a>(idx: usize, strip: &'a Strip, accent: Color) -> Element<'a, Message> {
    let meter = Canvas::new(Meter { level: strip.level, accent }).width(Length::Fixed(34.0)).height(Length::Fixed(FADER_H));
    let fad = fader(strip.volume, accent, mixer_core::model::UNITY_POS, move |v| Message::Send(Command::SetStripVolume { strip: idx, volume: v }));
    row![meter, Space::with_width(4), fad, Space::with_width(8), text(fmt_db(strip.volume)).size(tokens::type_scale::BODY).color(theme::TEXT)].align_y(Alignment::Center).into()
}

fn strip_send_matrix<'a>(idx: usize, strip: &'a Strip, state: &'a MixerState) -> Element<'a, Message> {
    let mut a_row = row![].spacing(3);
    let mut b_row = row![].spacing(3);
    for (bi, bus) in state.buses.iter().enumerate() {
        let on = strip.assign.get(bi).copied().unwrap_or(false);
        let fb = state.is_feedback(idx, bi);
        let is_b = bus.kind == BusKind::VirtualMic;
        let pill = send_pill(&bus.label, on, fb, is_b, Message::Send(Command::ToggleAssign { strip: idx, bus: bi }));
        if is_b { b_row = b_row.push(pill) } else { a_row = a_row.push(pill) }
    }
    column![section_label("SEND"), Space::with_height(3), a_row, Space::with_height(3), b_row].spacing(0).into()
}

fn strip_footer<'a>(idx: usize, strip: &'a Strip) -> Element<'a, Message> {
    let dsp = strip.dsp;
    let knobs = row![knob("GATE", dsp.gate, dsp.gate_on, theme::ACCENT, idx, dsp, true), Space::with_width(6), knob("COMP", dsp.comp, dsp.comp_on, theme::VIOLET, idx, dsp, false)];
    let mute = wide_button("MUTE", strip.mute, theme::REC_RED, Message::Send(Command::SetStripMute { strip: idx, mute: !strip.mute }));
    let rec = rec_button(strip.recording, RecTarget::Strip(idx));
    // Fixes a source that presents real stereo ports but only ever writes
    // audio into one of them (e.g. a SIP phone call heard in one ear only)
    // — see `Strip.force_mono`'s doc comment for why this can't be
    // auto-detected and needs an explicit switch.
    let mono_btn = wide_button("MONO", strip.force_mono, theme::MIC_AMBER, Message::Send(Command::SetStripForceMono { strip: idx, on: !strip.force_mono }));
    column![section_label("DSP"), Space::with_height(3), knobs, Space::with_height(8), row![mute, Space::with_width(4), mono_btn, Space::with_width(4), rec].spacing(0)]
        .spacing(0).into()
}

pub fn strip_card<'a>(idx: usize, strip: &'a Strip, state: &'a MixerState, width: f32, renaming: Option<&'a str>, active: bool) -> Element<'a, Message> {
    let accent = theme::ACCENT;
    let live_icon = if strip.input_live { icons::Icon::Dot } else { icons::Icon::Ring };
    let live_dot = icons::icon(live_icon, 8.0, if strip.input_live { accent } else { theme::TEXT_DIM });
    let head = rename_head(strip.display_name(idx), renaming, RenameTarget::Strip(idx), tokens::type_scale::BODY, accent, live_dot.into());
    let routing = strip_routing_section(idx, strip, state, accent);
    let fader_section = strip_fader_section(idx, strip, accent);
    let send_matrix = strip_send_matrix(idx, strip, state);
    let footer = strip_footer(idx, strip);
    let body = column![
        head, Space::with_height(6), hr(), Space::with_height(6),
        routing, Space::with_height(6), hr(), Space::with_height(6),
        fader_section, Space::with_height(6), hr(), Space::with_height(6),
        send_matrix, Space::with_height(6), hr(), Space::with_height(6),
        footer,
    ]
    .spacing(0).width(Length::Fixed(width));
    container(body).padding(10).width(Length::Fixed(width + 20.0)).style(theme::card_accent(if strip.input_live { accent } else { theme::EDGE_SOFT }, active)).into()
}

fn bus_routing_section<'a>(idx: usize, bus: &'a Bus, state: &'a MixerState, accent: Color) -> Element<'a, Message> {
    // Direct input: this bus's own source, same freedom a strip's input has.
    // The bus's meter reflects ONLY this — pre-fader, source-only — never
    // the mixed content routed in via the strip send matrix or bus feeds.
    let in_opts: Vec<Opt> = state.inputs.iter().map(|i| Opt { key: i.key.clone(), label: i.label.clone() }).collect();
    let in_sel = bus.input.as_ref().and_then(|k| in_opts.iter().find(|o| &o.key == k).cloned());
    let input_dd = dropdown("◆ INPUT (drives meter)", clearable_opts(in_opts), in_sel, move |o: Opt| {
        let input = if o.key == NONE_KEY { None } else { Some(o.key) };
        Message::Send(Command::SetBusInput { bus: idx, input })
    });
    // A bus's INPUT is metering-only by design (see `Bus.input`'s doc
    // comment — routing it for real risks feeding an app's own voice back
    // into its own mic capture). Picking an app here silently does nothing
    // to what you actually hear from it, which reads as "the fader/mute are
    // broken" — confirmed live confusion, not a hypothetical. Surface it
    // instead of leaving it a silent trap.
    let app_input_warn: Element<Message> = match bus
        .input
        .as_deref()
        .and_then(|k| state.inputs.iter().find(|i| i.key == k))
    {
        Some(i) if i.kind == SourceKind::App => {
            text("⚠ meter only — this app's audio is NOT routed here. To control its volume, assign it as a STRIP's input and send that strip to a hardware bus.")
                .size(tokens::type_scale::CAPTION)
                .color(theme::MIC_AMBER)
                .into()
        }
        _ => Space::with_height(0).into(),
    };
    let opts: Vec<Opt> = state.capture_apps.iter().map(|a| Opt { key: a.key.clone(), label: a.label.clone() }).collect();
    let sel = bus.listener.as_ref().and_then(|k| opts.iter().find(|o| &o.key == k).cloned());
    let app_dd = dropdown("◇ SEND TO APP", clearable_opts(opts), sel, move |o: Opt| {
        let app = if o.key == NONE_KEY { None } else { Some(o.key) };
        Message::Send(Command::SetBusListener { bus: idx, app })
    });
    let listening = if bus.listeners.is_empty() { text("no app assigned").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM) } else { text(format!("◂ {} listening", elide(&bus.listeners[0], 12))).size(tokens::type_scale::CAPTION).color(accent) };
    // If another bus shares this exact listener key, both buses feed the
    // same app's mic at once — legitimate (an app CAN listen to more than
    // one bus), but easy to end up with by accident and easy to misread as
    // "which one is actually active" (it's both, summed). Surface it instead
    // of leaving it invisible — this is what "B1/B2/B3 all show as default
    // mic" turned out to actually be.
    let dup_others: Vec<&str> = bus
        .listener
        .as_deref()
        .map(|key| {
            state
                .buses
                .iter()
                .enumerate()
                .filter(|(oi, ob)| *oi != idx && ob.listener.as_deref() == Some(key))
                .map(|(_, ob)| ob.label.as_str())
                .collect()
        })
        .unwrap_or_default();
    let dup_note: Element<Message> = if dup_others.is_empty() {
        Space::with_height(0).into()
    } else {
        text(format!("⚠ same app also on {}", dup_others.join(", "))).size(tokens::type_scale::CAPTION).color(theme::MIC_AMBER).into()
    };
    column![section_label("ROUTING"), Space::with_height(3), input_dd, app_input_warn, Space::with_height(4), app_dd, Space::with_height(3), listening, dup_note]
        .spacing(0).into()
}

fn bus_fader_section<'a>(idx: usize, bus: &'a Bus, accent: Color) -> Element<'a, Message> {
    let meter = Canvas::new(Meter { level: bus.level, accent }).width(Length::Fixed(34.0)).height(Length::Fixed(FADER_H));
    let fad = fader(bus.volume, accent, mixer_core::model::UNITY_POS, move |v| Message::Send(Command::SetBusVolume { bus: idx, volume: v }));
    row![meter, Space::with_width(4), fad, Space::with_width(8), text(fmt_db(bus.volume)).size(tokens::type_scale::BODY).color(theme::TEXT)].align_y(Alignment::Center).into()
}

fn bus_send_sections<'a>(idx: usize, bus: &'a Bus, state: &'a MixerState) -> Element<'a, Message> {
    let mut mon = row![].spacing(3);
    let a_buses: Vec<(usize, &Bus)> = state.buses.iter().enumerate().filter(|(_, b)| b.kind == BusKind::HwOutput).collect();
    for (ai, (_, ab)) in a_buses.iter().enumerate() {
        let on = bus.monitor.get(ai).copied().unwrap_or(false);
        mon = mon.push(send_pill(&ab.label, on, false, false, Message::Send(Command::ToggleBusMonitor { bus: idx, a_bus: ai })));
    }
    // Bus-to-bus: this bus's output additionally feeding another B-bus's
    // input. Global bus indices on both sides — see `Bus.feeds` doc comment.
    // Excludes self and A-buses: feeding a hardware-out bus isn't this
    // feature, and self-feed would be a trivial 1-cycle.
    let mut feed = row![].spacing(3);
    for (oi, ob) in state.buses.iter().enumerate().filter(|(oi, ob)| *oi != idx && ob.kind == BusKind::VirtualMic) {
        let on = bus.feeds.get(oi).copied().unwrap_or(false);
        feed = feed.push(send_pill(&ob.label, on, false, true, Message::Send(Command::ToggleBusFeed { from: idx, to: oi })));
    }
    // Bus-to-strip: this bus's output additionally feeding one or more
    // strips (the reverse of a strip's own send-to-bus pills) — e.g. B1 as a
    // shared "everything" channel feeding back into the input strips.
    // Wraps onto extra rows (3 per row) rather than overflowing the card —
    // with `STRIP_MAX` tuned down so 8 cards fit a default-width window
    // (see `tokens::layout::STRIP_MAX`'s doc comment), a card is no longer
    // wide enough to fit more than ~3-4 of these pills on one line once the
    // strip count grows past a handful.
    let mut to_strip_pills: Vec<Element<Message>> = Vec::new();
    for si in 0..state.strips.len() {
        let on = bus.strip_feeds.get(si).copied().unwrap_or(false);
        to_strip_pills.push(send_pill(format!("S{}", si + 1), on, false, true, Message::Send(Command::ToggleBusStripFeed { bus: idx, strip: si })));
    }
    let to_strips = pill_grid(to_strip_pills, 3);
    column![
        section_label("MONITOR ON"), Space::with_height(2), mon,
        Space::with_height(6), section_label("FEED →"), Space::with_height(2), feed,
        Space::with_height(6), section_label("FEED → STRIPS"), Space::with_height(2), to_strips,
    ]
    .spacing(0).into()
}

/// Wraps `pills` (fixed-width `send_pill`s) into rows of `per_row`, so a
/// group that doesn't fit on one line grows downward instead of overflowing
/// the card. Same idea as `main.rs`'s `wrap_cards`, just for the smaller
/// fixed-size pill elements used inside a card rather than whole cards.
fn pill_grid<'a>(pills: Vec<Element<'a, Message>>, per_row: usize) -> Element<'a, Message> {
    let mut rows = column![].spacing(3);
    let mut r = row![].spacing(3);
    let mut n = 0;
    for p in pills {
        r = r.push(p);
        n += 1;
        if n == per_row {
            rows = rows.push(r);
            r = row![].spacing(3);
            n = 0;
        }
    }
    if n > 0 {
        rows = rows.push(r);
    }
    rows.into()
}

fn bus_footer<'a>(idx: usize, bus: &'a Bus) -> Element<'a, Message> {
    let mute = wide_button("MUTE", bus.mute, theme::REC_RED, Message::Send(Command::SetBusMute { bus: idx, mute: !bus.mute }));
    let rec = rec_button(bus.recording, RecTarget::Bus(idx));
    row![mute, Space::with_width(4), rec].spacing(0).into()
}

pub fn bus_card<'a>(idx: usize, bus: &'a Bus, state: &'a MixerState, width: f32, renaming: Option<&'a str>, active: bool) -> Element<'a, Message> {
    let accent = theme::VIOLET;
    let name = if bus.name.is_empty() { bus.label.clone() } else { bus.name.clone() };
    let mic_tag = row![icons::icon(icons::Icon::Mic, 10.0, theme::TEXT_DIM), Space::with_width(3), text("MIC").size(tokens::type_scale::MICRO).color(theme::TEXT_DIM)]
        .align_y(Alignment::Center);
    let head = rename_head(name, renaming, RenameTarget::Bus(idx), tokens::type_scale::TITLE, accent, mic_tag.into());
    let routing = bus_routing_section(idx, bus, state, accent);
    let fader_section = bus_fader_section(idx, bus, accent);
    let sends = bus_send_sections(idx, bus, state);
    let footer = bus_footer(idx, bus);
    let body = column![
        head, Space::with_height(6), hr(), Space::with_height(6),
        routing, Space::with_height(6), hr(), Space::with_height(6),
        fader_section, Space::with_height(6), hr(), Space::with_height(6),
        sends, Space::with_height(6), hr(), Space::with_height(6),
        footer,
    ]
    .spacing(0).width(Length::Fixed(width));
    container(body).padding(10).width(Length::Fixed(width + 20.0)).style(theme::card_accent(accent, active)).into()
}

pub fn hw_out_slot<'a>(idx: usize, bus: &'a Bus, state: &'a MixerState) -> Element<'a, Message> {
    let opts: Vec<Opt> = state.devices.iter().map(|d| Opt { key: d.key.clone(), label: d.label.clone() }).collect();
    let sel = bus.device.as_ref().and_then(|k| opts.iter().find(|o| &o.key == k).cloned());
    let dd = dropdown("— select device —", clearable_opts(opts), sel, move |o: Opt| {
        let device = if o.key == NONE_KEY { None } else { Some(o.key) };
        Message::Send(Command::SetBusDevice { bus: idx, device })
    });
    let mute = button(icons::icon(icons::Icon::Mute, 12.0, if bus.mute { theme::BG_DEEP } else { theme::TEXT_DIM }))
        .style(move |_t, s| button::Style { background: Some(accent_fill(if bus.mute { theme::REC_RED } else { theme::PANEL_HI }, s)), border: Border { color: theme::EDGE, width: 1.0, radius: tokens::radius::SM.into() }, text_color: theme::TEXT_DIM, ..Default::default() })
        .padding([4, 8]).on_press(Message::Send(Command::SetBusMute { bus: idx, mute: !bus.mute }));
    let top = row![text(&bus.label).size(tokens::type_scale::TITLE).color(theme::ACCENT), Space::with_width(8), dd, Space::with_width(6), mute].align_y(Alignment::Center);
    // A-buses are hardware outputs, not sources — no direct input/meter
    // here (that's what strips and B-buses are for). This flip-flopped a
    // couple of rounds this session; settled per explicit direction: "A1 A2
    // A3 do not need input because they are not driving meters, that's our
    // hardware outputs."
    container(top).padding(tokens::space::SM).width(Length::Fixed(340.0)).style(theme::card).into()
}

/// A single bus's entry in `rec_panel` — a small fixed-width chip (label over
/// a compact toggle), not the full-size `rec_button` (which is `Length::Fill`
/// and sized for sitting alongside a MUTE button inside a strip/bus card's
/// fixed-width body — dropped straight into a bare row it stretches to fill
/// whatever space is available, which is what made the first version of this
/// panel look oversized).
fn mini_rec_chip<'a>(label: &'a str, accent: Color, recording: bool, target: RecTarget) -> Element<'a, Message> {
    let msg = if recording {
        Message::Send(Command::StopRecordTarget { target })
    } else {
        Message::Send(Command::StartRecordTarget { target })
    };
    let (bg, fg, edge) = if recording {
        (theme::REC_RED, theme::BG_DEEP, theme::REC_RED)
    } else {
        (theme::PANEL_HI, theme::TEXT_DIM, theme::EDGE)
    };
    let btn = button(text(if recording { "■" } else { "●" }).size(tokens::type_scale::CAPTION).color(fg).center().width(Length::Fill))
        .style(move |_t, s| button::Style { background: Some(accent_fill(bg, s)), border: Border { color: edge, width: 1.0, radius: tokens::radius::SM.into() }, text_color: fg, ..Default::default() })
        .width(Length::Fixed(22.0)).padding([3, 0]).on_press(msg);
    column![text(label).size(tokens::type_scale::CAPTION).color(accent), Space::with_height(3), btn]
        .align_x(Alignment::Center)
        .width(Length::Fixed(34.0))
        .into()
}

/// Consolidated recording dashboard: every bus (A then B, natural array
/// order), each with its own compact REC toggle, so you can see/control
/// exactly what's being recorded without hunting across scattered per-card
/// buttons. Purely additive — the existing per-card REC buttons on
/// `strip_card`/`bus_card` stay too; both read the same `bus.recording`
/// field so there's no risk of the two views disagreeing.
pub fn rec_panel<'a>(state: &'a MixerState) -> Element<'a, Message> {
    let mut items = row![].spacing(6);
    for (idx, bus) in state.buses.iter().enumerate() {
        let accent = if bus.kind == BusKind::HwOutput { theme::ACCENT } else { theme::VIOLET };
        items = items.push(mini_rec_chip(&bus.label, accent, bus.recording, RecTarget::Bus(idx)));
    }
    container(row![text("REC").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM), Space::with_width(10), items].align_y(Alignment::Center))
        .padding([6, 10]).style(theme::card).into()
}

// ─────────────────────────────────────────────────────── MATRIX

/// The patch matrix: strips as rows, buses as columns, a toggle cell at each
/// crossing. Cyan = hardware send, violet = virtual mic, coral = feedback.
pub fn matrix_view<'a>(state: &'a MixerState) -> Element<'a, Message> {
    let mut grid = column![].spacing(6);

    // Header row: bus labels, each with its own REC toggle right in the grid
    // — the matrix already shows every bus at a glance, so recording control
    // belongs here too instead of only on the strip/bus cards.
    let mut header = row![container(text("").width(Length::Fixed(150.0))).width(Length::Fixed(150.0))].spacing(6);
    for (bi, bus) in state.buses.iter().enumerate() {
        let col = if bus.kind == BusKind::VirtualMic { theme::VIOLET } else { theme::ACCENT };
        let tag = if bus.kind == BusKind::VirtualMic { "MIC" } else { "OUT" };
        header = header.push(
            container(
                column![
                    text(&bus.label).size(tokens::type_scale::SUBTITLE).color(col),
                    text(tag).size(tokens::type_scale::MICRO).color(theme::TEXT_DIM),
                    Space::with_height(3),
                    mini_rec_chip("", col, bus.recording, RecTarget::Bus(bi)),
                ]
                .align_x(Alignment::Center),
            )
            .width(Length::Fixed(54.0)).center_x(Length::Fixed(54.0)),
        );
    }
    header = header.push(container(text("REC").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM)).width(Length::Fixed(54.0)).center_x(Length::Fixed(54.0)));
    grid = grid.push(header);

    for (si, strip) in state.strips.iter().enumerate() {
        let name = strip.display_name(si);
        let name_col = if !strip.input_live { theme::TEXT_DIM } else { theme::TEXT };
        let mut r = row![
            container(text(format!("{:02}  {}", si + 1, elide(&name, 16))).size(tokens::type_scale::BODY).color(name_col))
                .width(Length::Fixed(150.0))
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        for (bi, bus) in state.buses.iter().enumerate() {
            let on = strip.assign.get(bi).copied().unwrap_or(false);
            let fb = state.is_feedback(si, bi);
            let is_b = bus.kind == BusKind::VirtualMic;
            let accent = if is_b { theme::VIOLET } else { theme::ACCENT };
            let (bg, mark): (Color, Element<Message>) = if fb {
                (theme::DANGER, container(icons::icon(icons::Icon::X, 12.0, theme::BG_DEEP)).center_x(Length::Fill).center_y(Length::Fill).into())
            } else if on {
                (accent, text("●").size(tokens::type_scale::SUBTITLE).color(theme::BG_DEEP).center().width(Length::Fill).into())
            } else {
                (theme::SEG_OFF, Space::with_width(0).into())
            };
            let cell = button(mark)
                .style(move |_t, s| button::Style {
                    background: Some(accent_fill(bg, s)),
                    border: Border { color: if on || fb { bg } else { theme::EDGE }, width: 1.0, radius: tokens::radius::MD.into() },
                    text_color: theme::BG_DEEP, ..Default::default()
                })
                .width(Length::Fixed(54.0)).height(Length::Fixed(30.0))
                .on_press(Message::Send(Command::ToggleAssign { strip: si, bus: bi }));
            r = r.push(cell);
        }
        r = r.push(
            container(mini_rec_chip("", theme::TEXT_DIM, strip.recording, RecTarget::Strip(si)))
                .width(Length::Fixed(54.0)).center_x(Length::Fixed(54.0)),
        );
        grid = grid.push(r);
    }

    let head = column![
        text("PATCH MATRIX").size(tokens::type_scale::TITLE).color(theme::TEXT),
        text("rows send into columns · cyan = hardware out · violet = virtual mic · ✕ = feedback blocked").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM),
    ]
    .spacing(4);

    // The grid itself is a fixed-width table (name column + one 54px column
    // per bus) — on a wide window that leaves a dead void to its right if
    // left-aligned like the title above it. Center just the grid; the title
    // and legend stay left, reading naturally above it.
    let centered_grid = container(grid).width(Length::Fill).center_x(Length::Fill);

    container(column![head, Space::with_height(16), centered_grid].spacing(0).padding(tokens::space::LG))
        .width(Length::Fill).into()
}

// ─────────────────────────────────────────────────────── SETTINGS

pub fn settings_view<'a>(state: &'a MixerState, recdir_draft: Option<&'a str>) -> Element<'a, Message> {
    let section = |title: &'a str| text(title).size(tokens::type_scale::BODY).color(theme::ACCENT);

    // Feedback guard toggle.
    let guard = state.feedback_guard;
    let guard_btn = wide_button(
        if guard { "FEEDBACK GUARD: ON" } else { "FEEDBACK GUARD: OFF" },
        guard,
        theme::ACCENT,
        Message::Send(Command::SetFeedbackGuard { on: !guard }),
    );

    // Fixed (not Fill+max_width) so the column of cards has a well-defined
    // natural width the outer container can center as a group — see the
    // centering wrapper at the bottom of this function.
    let card = |content: Element<'a, Message>| {
        container(content).padding(16).width(Length::Fixed(640.0)).style(theme::card)
    };

    let routing = card(
        column![
            section("ROUTING"),
            Space::with_height(8),
            container(guard_btn).width(Length::Fixed(260.0)),
            Space::with_height(6),
            text("Blocks a strip from sending into a virtual mic its own app captures — prevents echo loops. Leave ON unless you know what you're doing.").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    let recdir_value = recdir_draft.unwrap_or(state.recordings_dir.as_str());
    let recdir_input = text_input("~/Music/ferromix2", recdir_value)
        .on_input(Message::RecDirChanged)
        .on_submit(Message::RecDirApply)
        .size(tokens::type_scale::LABEL)
        .padding(tokens::space::SM)
        .style(|_t, _s| text_input::Style {
            background: iced::Background::Color(theme::BG_DEEP),
            border: Border { color: theme::EDGE, width: 1.0, radius: tokens::radius::SM.into() },
            icon: theme::TEXT_DIM,
            placeholder: theme::TEXT_DIM,
            value: theme::TEXT,
            selection: theme::ACCENT.scale_alpha(0.35),
        })
        .width(Length::Fill);
    let apply_btn = button(text("APPLY").size(tokens::type_scale::CAPTION).color(theme::BG_DEEP).center())
        .style(|_t, s| button::Style {
            background: Some(accent_fill(theme::ACCENT, s)),
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
            text("Each armed track writes its own WAV. Arm tracks with the REC button on a strip or bus card.").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    let scale = if state.ui_scale > 0.0 { state.ui_scale } else { 1.0 };
    let scale_pct = (scale * 100.0).round() as i32;
    let step_btn = move |label: &'static str, delta: f32| -> Element<'a, Message> {
        button(text(label).size(tokens::type_scale::SUBTITLE).color(theme::TEXT).center().width(Length::Fill))
            .style(|_t, s| button::Style {
                background: Some(accent_fill(theme::PANEL_HI, s)),
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
                text(format!("{scale_pct}%")).size(tokens::type_scale::SUBTITLE).color(theme::TEXT).width(Length::Fixed(48.0)),
                Space::with_width(10),
                step_btn("+", 0.1),
            ]
            .align_y(Alignment::Center),
            Space::with_height(6),
            text("UI scale. Persists across restarts.").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    let active_rate = if state.sample_rate == 0 { 48_000 } else { state.sample_rate };
    let rate_btn = move |rate: u32| -> Element<'a, Message> {
        let on = active_rate == rate;
        let (bg, fg, edge) = if on { (theme::ACCENT, theme::BG_DEEP, theme::ACCENT) } else { (theme::PANEL_HI, theme::TEXT_DIM, theme::EDGE) };
        button(text(format!("{rate}")).size(tokens::type_scale::LABEL).color(fg).center().width(Length::Fill))
            .style(move |_t, s| button::Style {
                background: Some(accent_fill(bg, s)),
                border: Border { color: edge, width: 1.0, radius: tokens::radius::SM.into() },
                text_color: fg,
                ..Default::default()
            })
            .padding([6, 0])
            .on_press(Message::Send(Command::SetSampleRate { rate }))
            .into()
    };
    let sample_rate = card(
        column![
            section("SAMPLE RATE"),
            Space::with_height(8),
            row![rate_btn(44_100), Space::with_width(8), rate_btn(48_000), Space::with_width(8), rate_btn(96_000)],
            Space::with_height(6),
            text("Forces PipeWire's whole graph clock to this rate — every app, not just FerroMix. Fixes audio that sounds smeared/\"underwater\" from apps whose native rate doesn't match. Streams may briefly glitch while the graph renegotiates; new strip/bus nodes are pinned to this rate going forward, existing ones after a FerroMix restart.").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    let latency = card(
        column![
            section("LATENCY (the Linux \"ASIO\")"),
            Space::with_height(8),
            text("FerroMix2 runs on PipeWire — no ASIO driver needed. For low-latency, set a small quantum globally:").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM),
            Space::with_height(6),
            container(text("pw-metadata -n settings 0 clock.force-quantum 256").size(tokens::type_scale::LABEL).color(theme::ACCENT))
                .padding(tokens::space::SM).style(|_t| iced::widget::container::Style { background: Some(iced::Background::Color(theme::BG_DEEP)), border: Border { color: theme::EDGE, width: 1.0, radius: tokens::radius::SM.into() }, ..Default::default() }),
            Space::with_height(4),
            text("256 samples @ 48kHz ≈ 5ms. Lower = tighter but more CPU. Reset with clock.force-quantum 0.").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    let reset_btn = button(text("⟲ RESET AUDIO TO STOCK PIPEWIRE").size(tokens::type_scale::LABEL).color(theme::BG_DEEP).center().width(Length::Fill))
        .style(|_t, s| button::Style { background: Some(accent_fill(theme::MIC_AMBER, s)), border: Border { color: theme::MIC_AMBER, width: 1.0, radius: tokens::radius::SM.into() }, text_color: theme::BG_DEEP, ..Default::default() })
        .padding([8, 0])
        .on_press(Message::ResetAudio);
    let recovery = card(
        column![
            section("RECOVERY"),
            Space::with_height(8),
            container(reset_btn).width(Length::Fixed(320.0)),
            Space::with_height(6),
            text("Restarts PipeWire, PipeWire-Pulse and WirePlumber back to a clean, stock state — the fix if audio ever gets stuck (most often after a DSP module misbehaves). Restart FerroMix itself afterward — its connection to the old PipeWire session won't survive the restart.").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    let about = card(
        column![
            section("ABOUT"),
            Space::with_height(8),
            text("FerroMix2 — an open-source Virtual mixer for Linux / PipeWire.").size(tokens::type_scale::LABEL).color(theme::TEXT),
            text("Every strip receives one source. A-buses you hear; B-buses are virtual mics apps read. MUTE cuts a strip everywhere.").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM),
        ]
        .spacing(0)
        .into(),
    );

    // Cards are a fixed 640px column — center that group in the available
    // width instead of leaving it pinned to the left edge on a wide window.
    // Title stays left, reading naturally above the centered stack (same
    // pattern as the Matrix tab's title-above-centered-grid).
    let cards = container(
        column![routing, Space::with_height(12), rec, Space::with_height(12), display, Space::with_height(12), sample_rate, Space::with_height(12), latency, Space::with_height(12), recovery, Space::with_height(12), about]
            .spacing(0),
    )
    .width(Length::Fill)
    .center_x(Length::Fill);

    scrollable(
        column![text("SETTINGS").size(tokens::type_scale::TITLE).color(theme::TEXT), Space::with_height(16), cards]
            .spacing(0)
            .padding(tokens::space::LG),
    )
    .into()
}

// ─────────────────────────────────────────────────────── LOG

/// Activity log — newest entries first, so the most recent routing/feedback/
/// save events are visible without scrolling.
pub fn log_view<'a>(state: &'a MixerState) -> Element<'a, Message> {
    let mut lines = column![].spacing(4);
    for line in state.log.iter().rev() {
        lines = lines.push(text(line).size(tokens::type_scale::LABEL).color(theme::TEXT_DIM).font(iced::Font::MONOSPACE));
    }
    let copy_btn = button(text("⧉ COPY LOG").size(tokens::type_scale::LABEL).color(theme::TEXT_DIM))
        .style(|_t, s| button::Style { background: Some(accent_fill(theme::PANEL_HI, s)), border: Border { color: theme::EDGE, width: 1.0, radius: tokens::radius::MD.into() }, text_color: theme::TEXT_DIM, ..Default::default() })
        .padding([6, 12])
        .on_press(Message::CopyLog);
    let head = row![
        column![
            text("ACTIVITY LOG").size(tokens::type_scale::TITLE).color(theme::TEXT),
            text("Routing changes, feedback blocks, saves — newest first.").size(tokens::type_scale::CAPTION).color(theme::TEXT_DIM),
        ]
        .spacing(4),
        Space::with_width(Length::Fill),
        copy_btn,
    ]
    .align_y(Alignment::Center);

    scrollable(
        column![head, Space::with_height(16), container(lines).padding(tokens::space::MD).width(Length::Fill).style(theme::card)]
            .spacing(0)
            .padding(tokens::space::LG),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
