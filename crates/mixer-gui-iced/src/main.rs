//! FerroMix — Iced console. Owns the audio engine directly (`link::start`
//! boots `mixer_core::Engine` on the real PipeWire backend in-process) but
//! only ever talks to it through `EngineHandle`'s `Command`/`MixerState`
//! seam, same as if it were still a separate process — no PipeWire calls in
//! this crate itself, so the UI can keep evolving without risking the audio
//! engine.

mod icons;
mod link;
mod theme;
mod tokens;
mod widgets;

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Element, Length, Subscription, Task};
use mixer_core::engine::{Command, EngineHandle};
use mixer_core::model::{BusKind, MixerState};
use std::time::{Duration, Instant};

/// How long to wait after the last change before autosaving.
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(1500);

/// How long to show "reconnecting…" before actually rebuilding the backend —
/// gives a dropped PipeWire connection (e.g. mid-restart from RESET AUDIO or
/// a sample-rate change) a moment to settle before we reconnect into it.
const RECONNECT_DELAY: Duration = Duration::from_millis(1200);

/// How long the amber "you're interacting with this stack" outline stays lit
/// after the last command sent for that strip/bus — a drag holds it on
/// continuously (every `CursorMoved` re-sends a volume command, refreshing
/// this), then it fades a beat after you let go, like a phosphor decay.
const ACTIVE_HIGHLIGHT: Duration = Duration::from_millis(1100);

/// Default window size — comfortable for the fixed-ish strip-card layout at
/// startup. The window is resizable and has a real `min_size`; below the
/// comfortable width, cards shrink (see `App::strip_card_width`) and the
/// console row scrolls horizontally rather than clipping.
const DEFAULT_WINDOW: (f32, f32) = (1620.0, 780.0);
const MIN_WINDOW: (f32, f32) = (960.0, 600.0);

fn main() -> iced::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("FerroMix Iced console starting");
    iced::application("FerroMix2", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_| theme::base())
        .scale_factor(|app: &App| {
            app.state
                .as_ref()
                .map(|s| s.ui_scale as f64)
                .filter(|s| *s > 0.0)
                .unwrap_or(1.0)
        })
        .font(include_bytes!("../../../assets/fonts/Inter-Regular.ttf").as_slice())
        .font(include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf").as_slice())
        .font(include_bytes!("../../../assets/fonts/Inter-Bold.ttf").as_slice())
        .default_font(theme::FONT_UI)
        .window(iced::window::Settings {
            size: DEFAULT_WINDOW.into(),
            min_size: Some(MIN_WINDOW.into()),
            resizable: true,
            ..Default::default()
        })
        .run_with(App::new)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Console,
    Matrix,
    Settings,
    Log,
}

/// What's currently being renamed via the click-to-edit header field on a
/// strip/bus card. Only one at a time — starting a new rename discards any
/// uncommitted draft for the previous target (same as clicking away in most
/// apps' inline-rename UIs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameTarget {
    Strip(usize),
    Bus(usize),
}

struct App {
    /// `None` only if the initial PipeWire connection failed outright (see
    /// `link::start`'s error path) — otherwise always `Some`, rebuilt in place
    /// by `Message::Tick` if the backend ever reports itself disconnected.
    engine: Option<EngineHandle>,
    state: Option<MixerState>,
    status: String,
    /// Set the moment the backend is first noticed to be down (engine absent,
    /// or `state.backend_alive == false`); cleared the moment it's healthy
    /// again. `Message::Tick` uses this to wait `RECONNECT_DELAY` before
    /// actually rebuilding the backend, and to retry on that same cadence if
    /// the rebuild itself fails.
    disconnected_since: Option<Instant>,
    tab: Tab,
    /// True when a command has been sent since the last successful save.
    dirty: bool,
    last_change: Option<Instant>,
    /// Current window width, used to shrink strip/bus cards responsively
    /// instead of clipping them when the window is resized down.
    window_width: f32,
    renaming: Option<(RenameTarget, String)>,
    /// Recordings-dir edit-in-progress text, if the user has started typing
    /// in the Settings field. `None` means "show `state.recordings_dir`".
    recdir_draft: Option<String>,
    /// The strip/bus most recently touched by a command, and when — drives
    /// the amber "active stack" outline. Cleared by `Tick` once it's decayed
    /// past `ACTIVE_HIGHLIGHT`.
    active: Option<(RenameTarget, Instant)>,
}

/// Which strip/bus a `Command` targets, for the active-stack highlight — a
/// command that doesn't target one specific strip/bus (global settings,
/// `Save`, `AddStrip`) yields `None` and leaves the highlight untouched by
/// that message. Renaming isn't included here — it already has its own
/// distinct visual feedback (the inline text field).
fn command_target(c: &Command) -> Option<RenameTarget> {
    use Command::*;
    match *c {
        SetStripInput { strip, .. }
        | SetStripVolume { strip, .. }
        | SetStripMute { strip, .. }
        | SetStripDsp { strip, .. }
        | SetStripForceMono { strip, .. }
        | SetStripListener { strip, .. } => Some(RenameTarget::Strip(strip)),
        ToggleAssign { strip, .. } => Some(RenameTarget::Strip(strip)),
        SetBusVolume { bus, .. }
        | SetBusMute { bus, .. }
        | SetBusDevice { bus, .. }
        | SetBusInput { bus, .. }
        | SetBusListener { bus, .. } => Some(RenameTarget::Bus(bus)),
        ToggleBusMonitor { bus, .. } => Some(RenameTarget::Bus(bus)),
        ToggleBusFeed { from, .. } => Some(RenameTarget::Bus(from)),
        ToggleBusStripFeed { bus, .. } => Some(RenameTarget::Bus(bus)),
        StartRecordTarget { target } | StopRecordTarget { target } => match target {
            mixer_core::model::RecTarget::Strip(s) => Some(RenameTarget::Strip(s)),
            mixer_core::model::RecTarget::Bus(b) => Some(RenameTarget::Bus(b)),
        },
        SetRecordingsDir { .. } | SetUiScale { .. } | SetSampleRate { .. } | SetStripName { .. } | SetBusName { .. }
        | SetFeedbackGuard { .. } | SetEnabled { .. } | AddStrip | RemoveLastStrip | Save => None,
    }
}

/// How many `card_w`-wide cards fit per row in `available` width, given
/// `gap` between cards. `card_w + 20.0` matches `strip_card`/`bus_card`'s own
/// outer container width (`width + 20.0`, their padding/border allowance) —
/// see `widgets.rs`. Always at least 1: an oversized single card still gets
/// its own row rather than the calculation returning 0 and losing cards.
fn cards_per_row(available: f32, card_w: f32, gap: f32) -> usize {
    let footprint = card_w + 20.0 + gap;
    ((available / footprint).floor() as usize).max(1)
}

/// Arrange `cards` into a wrapping grid — as many per row as fit, additional
/// cards flow onto new rows instead of overflowing the window or forcing
/// `strip_card_width` to shrink cards past `STRIP_MIN`. This is what makes
/// the console genuinely fit any window size/aspect ratio: the outer
/// `scrollable` in `console()` is vertical, so extra rows just scroll,
/// instead of the previous behavior of silently running wider than the
/// window with no way to see the rest.
fn wrap_cards<'a>(cards: Vec<Element<'a, Message>>, per_row: usize, gap: f32) -> Element<'a, Message> {
    let mut rows = column![].spacing(gap);
    let mut current = row![].spacing(gap);
    let mut n = 0;
    for card in cards {
        current = current.push(card);
        n += 1;
        if n == per_row {
            rows = rows.push(current);
            current = row![].spacing(gap);
            n = 0;
        }
    }
    if n > 0 {
        rows = rows.push(current);
    }
    rows.into()
}

#[derive(Debug, Clone)]
enum Message {
    Tab(Tab),
    Send(Command),
    WindowResized(f32),
    RenameStart(RenameTarget, String),
    RenameChanged(String),
    RenameSubmit,
    RenameCancel,
    RecDirChanged(String),
    RecDirApply,
    CopyLog,
    /// Restart PipeWire/WirePlumber/pipewire-pulse back to a clean, stock
    /// state — the escape hatch for when something (e.g. a DSP module) has
    /// left the live graph in a bad state that FerroMix itself can't recover
    /// from. Same effect as running `reset_audio.sh`.
    ResetAudio,
    Tick,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let (engine, status, disconnected_since) = match link::start() {
            Ok(e) => (Some(e), "starting…".to_string(), None),
            // Seed `disconnected_since` too, not just `engine: None` — otherwise
            // the first `Message::Tick` (16ms later) immediately overwrites this
            // error text with a generic "reconnecting…" before anyone sees it.
            Err(e) => (None, format!("PipeWire error: {e}"), Some(Instant::now())),
        };
        (
            App {
                engine,
                state: None,
                status,
                disconnected_since,
                tab: Tab::Console,
                dirty: false,
                last_change: None,
                window_width: DEFAULT_WINDOW.0,
                renaming: None,
                recdir_draft: None,
                active: None,
            },
            Task::none(),
        )
    }

    fn send(&self, cmd: Command) {
        if let Some(engine) = &self.engine {
            engine.send(cmd);
        }
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Tab(t) => self.tab = t,
            Message::WindowResized(w) => self.window_width = w,
            Message::RenameStart(target, current) => self.renaming = Some((target, current)),
            Message::RenameChanged(s) => {
                if let Some((_, draft)) = &mut self.renaming {
                    *draft = s;
                }
            }
            Message::RenameSubmit => {
                if let Some((target, draft)) = self.renaming.take() {
                    let name = draft.trim().to_string();
                    if !name.is_empty() {
                        let cmd = match target {
                            RenameTarget::Strip(strip) => Command::SetStripName { strip, name },
                            RenameTarget::Bus(bus) => Command::SetBusName { bus, name },
                        };
                        self.dirty = true;
                        self.last_change = Some(Instant::now());
                        self.send(cmd);
                    }
                }
            }
            Message::RenameCancel => self.renaming = None,
            Message::RecDirChanged(s) => self.recdir_draft = Some(s),
            Message::RecDirApply => {
                if let Some(path) = self.recdir_draft.take() {
                    if !path.trim().is_empty() {
                        self.dirty = true;
                        self.last_change = Some(Instant::now());
                        self.send(Command::SetRecordingsDir { path });
                    }
                }
            }
            Message::Send(c) => {
                // Save itself doesn't dirty the state — everything else does.
                if matches!(c, Command::Save) {
                    self.dirty = false;
                } else {
                    self.dirty = true;
                    self.last_change = Some(Instant::now());
                }
                if let Some(target) = command_target(&c) {
                    self.active = Some((target, Instant::now()));
                }
                // RemoveLastStrip always targets the current last index —
                // if that strip happens to be mid-rename or the active-
                // highlight target, both are keyed by index and that index
                // won't exist anymore after removal, so clear them here
                // rather than leaving stale state pointing at nothing (or
                // worse, silently pointing at whatever strip now occupies
                // a since-reused index in some future add).
                if matches!(c, Command::RemoveLastStrip) {
                    if let Some(state) = &self.state {
                        let doomed = state.strips.len().saturating_sub(1);
                        if matches!(self.renaming, Some((RenameTarget::Strip(i), _)) if i == doomed) {
                            self.renaming = None;
                        }
                        if matches!(self.active, Some((RenameTarget::Strip(i), _)) if i == doomed) {
                            self.active = None;
                        }
                    }
                }
                self.send(c);
            }
            Message::CopyLog => {
                if let Some(state) = &self.state {
                    let text = state.log.join("\n");
                    return iced::clipboard::write(text);
                }
            }
            Message::ResetAudio => {
                self.status = "resetting PipeWire/WirePlumber…".into();
                // Fire-and-forget: restarting these services drops our own
                // PipeWire connection, which `Message::Tick`'s reconnect
                // logic below already handles — no need to wait for or
                // track completion here. `host_command` routes this through
                // `flatpak-spawn --host` under Flatpak, where a sandboxed
                // process can't reach the host's systemd directly.
                let _ = mixer_core::engine::host_command("systemctl")
                    .args(["--user", "restart", "pipewire.socket", "pipewire-pulse.socket"])
                    .spawn();
                let _ = mixer_core::engine::host_command("systemctl")
                    .args(["--user", "restart", "wireplumber.service"])
                    .spawn();
            }
            Message::Tick => {
                // Poll the engine's state directly — in-process now, so this
                // is just a mutex lock + clone, no socket round-trip. `broken`
                // covers both "never connected" (startup failure) and "was
                // connected, backend just died" (e.g. RESET AUDIO / a
                // sample-rate change restarting PipeWire out from under us).
                let broken = match &self.engine {
                    None => true,
                    Some(engine) => {
                        let snapshot = engine.snapshot();
                        let alive = snapshot.backend_alive;
                        self.state = Some(snapshot);
                        !alive
                    }
                };
                if broken {
                    match self.disconnected_since {
                        None => {
                            self.disconnected_since = Some(Instant::now());
                            self.status = "reconnecting…".into();
                        }
                        Some(since) if since.elapsed() > RECONNECT_DELAY => match link::start() {
                            Ok(engine) => {
                                self.state = Some(engine.snapshot());
                                self.engine = Some(engine);
                                self.disconnected_since = None;
                                self.status = "LIVE".into();
                            }
                            Err(e) => {
                                self.status = format!("PipeWire error: {e} — retrying…");
                                self.disconnected_since = Some(Instant::now());
                            }
                        },
                        Some(_) => {}
                    }
                } else {
                    self.disconnected_since = None;
                }

                if self.dirty {
                    if let Some(t) = self.last_change {
                        if t.elapsed() > AUTOSAVE_DEBOUNCE {
                            self.send(Command::Save);
                            self.dirty = false;
                        }
                    }
                }
                if let Some((_, t)) = self.active {
                    if t.elapsed() > ACTIVE_HIGHLIGHT {
                        self.active = None;
                    }
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        // 16ms (~60Hz) tick drives both the state poll (see `Message::Tick`)
        // and the autosave/active-highlight timers — snappier meters than
        // the old 33ms (~30Hz) IPC-poll cadence.
        let poll = iced::time::every(std::time::Duration::from_millis(16)).map(|_| Message::Tick);
        let resize = iced::window::resize_events().map(|(_id, size)| Message::WindowResized(size.width));
        // Escape aborts an in-progress rename without submitting it —
        // `RenameCancel` existed as dead code (a variant with a working
        // `update()` arm but nothing ever constructed it: no click-away or
        // key handler wired it up), so there was no way to back out of a
        // rename short of typing the original text back and hitting Enter.
        let keys = iced::keyboard::on_key_press(|key, _modifiers| match key {
            iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) => Some(Message::RenameCancel),
            _ => None,
        });
        Subscription::batch([poll, resize, keys])
    }

    /// Strip/bus card width for the current window: shrinks smoothly between
    /// `STRIP_MAX` (roomy) and `STRIP_MIN` (compact) as the window narrows.
    /// Deliberately independent of how many strips/buses actually exist —
    /// that used to be divided into this calculation directly, which meant
    /// the row was sized for the strip count alone even though strips AND
    /// buses shared the same row, silently running ~35% wider than the
    /// window whenever buses were also present. Card count no longer needs
    /// to factor in here at all: `console()` wraps any number of cards onto
    /// as many rows as needed at whatever width this returns (see
    /// `wrap_cards`), so sizing and card count are fully decoupled.
    fn strip_card_width(&self) -> f32 {
        // Same fudge factor as `console()`'s wrap calculation — must match
        // or the two disagree about how much width is actually available.
        let available = self.window_width - 40.0;
        // /7.0 is just "how many cards a comfortably-sized row should aim to
        // hold before the window is considered wide" — not derived from
        // anything exact, chosen so the default 1620px window lands near
        // STRIP_MAX and the 960px minimum window lands near STRIP_MIN.
        (available / 7.0 - 20.0).clamp(tokens::layout::STRIP_MIN, tokens::layout::STRIP_MAX)
    }

    fn view(&self) -> Element<Message> {
        let header = self.header();
        let body: Element<Message> = match self.tab {
            Tab::Console => self.console(),
            Tab::Matrix => self.matrix(),
            Tab::Settings => self.settings(),
            Tab::Log => self.log(),
        };

        container(column![header, body].spacing(0))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::panel)
            .into()
    }

    fn header(&self) -> Element<Message> {
        let logo = row![
            text("FERRO").size(tokens::type_scale::DISPLAY).color(theme::TEXT),
            text("MIX").size(tokens::type_scale::DISPLAY).color(theme::ACCENT),
            text(concat!("2  v", env!("CARGO_PKG_VERSION"))).size(tokens::type_scale::BODY).color(theme::TEXT_DIM),
        ]
        .align_y(iced::Alignment::Center);

        let tabs = row![
            widgets::tab_button("CONSOLE", self.tab == Tab::Console).on_press(Message::Tab(Tab::Console)),
            widgets::tab_button("MATRIX", self.tab == Tab::Matrix).on_press(Message::Tab(Tab::Matrix)),
            widgets::tab_button("SETTINGS", self.tab == Tab::Settings).on_press(Message::Tab(Tab::Settings)),
            widgets::tab_button("LOG", self.tab == Tab::Log).on_press(Message::Tab(Tab::Log)),
        ]
        .spacing(8);

        // `enabled` defaults true while state hasn't loaded yet, so the
        // indicator doesn't flash OFF for a moment on every launch.
        let enabled = self.state.as_ref().map(|s| s.enabled).unwrap_or(true);
        // Set by `Message::Tick` the moment the backend is noticed down
        // (never connected, or `MixerState.backend_alive` went false) and
        // cleared the moment a rebuilt backend reports itself alive again —
        // takes priority over the enabled/disabled read since the backend
        // might not even be up to report its enabled state accurately.
        let status = if self.disconnected_since.is_some() {
            row![
                icons::icon(icons::Icon::Dot, 10.0, theme::MIC_AMBER),
                Space::with_width(4),
                text(self.status.clone()).size(tokens::type_scale::BODY).color(theme::TEXT_DIM),
            ]
            .align_y(iced::Alignment::Center)
        } else if enabled {
            row![
                icons::icon(icons::Icon::Dot, 10.0, theme::METER_LO),
                Space::with_width(4),
                text("LIVE").size(tokens::type_scale::SUBTITLE).color(theme::TEXT),
            ]
            .align_y(iced::Alignment::Center)
        } else {
            // Deliberately off (`Command::SetEnabled { on: false }`) reads
            // the same as a real connection failure — red dot — since
            // either way FerroMix isn't routing anything right now; the
            // label is what distinguishes "you turned this off" from
            // "something's actually wrong".
            row![
                icons::icon(icons::Icon::Dot, 10.0, theme::REC_RED),
                Space::with_width(4),
                text("OFF").size(tokens::type_scale::SUBTITLE).color(theme::TEXT_DIM),
            ]
            .align_y(iced::Alignment::Center)
        };
        let power_btn = button(text(if enabled { "ON" } else { "OFF" }).size(tokens::type_scale::LABEL).color(theme::BG_DEEP).center().width(Length::Fill))
            .style(move |_t, s| iced::widget::button::Style {
                background: Some(widgets::accent_fill(if enabled { theme::METER_LO } else { theme::REC_RED }, s)),
                border: iced::Border { color: if enabled { theme::METER_LO } else { theme::REC_RED }, width: 1.0, radius: tokens::radius::SM.into() },
                text_color: theme::BG_DEEP,
                ..Default::default()
            })
            .width(Length::Fixed(40.0))
            .padding([4, 0])
            .on_press(Message::Send(Command::SetEnabled { on: !enabled }));

        let save_label = if self.dirty { "● SAVE" } else { "SAVED" };
        let save_fg = if self.dirty { theme::MIC_AMBER } else { theme::TEXT_DIM };
        let save_btn = button(text(save_label).size(tokens::type_scale::BODY).color(save_fg))
            .style(move |_t, s| iced::widget::button::Style {
                background: Some(iced::Background::Color(widgets::interactive(if self.dirty {
                    theme::MIC_AMBER.scale_alpha(0.12)
                } else {
                    iced::Color::TRANSPARENT
                }, s))),
                border: iced::Border { color: if self.dirty { theme::MIC_AMBER } else { theme::EDGE_SOFT }, width: 1.0, radius: tokens::radius::MD.into() },
                text_color: save_fg,
                ..Default::default()
            })
            .padding([6, 12])
            .on_press(Message::Send(Command::Save));

        // A hairline under the bar (same `theme::divider` used between card
        // sections) gives the header a defined edge as chrome, instead of
        // blending straight into the console content below it.
        column![
            container(
                row![
                    logo,
                    Space::with_width(24),
                    tabs,
                    Space::with_width(Length::Fill),
                    save_btn,
                    Space::with_width(16),
                    status,
                    Space::with_width(8),
                    power_btn,
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            )
            .padding([12, 18])
            .width(Length::Fill)
            .style(theme::header_bar),
            widgets::hr(),
        ]
        .spacing(0)
        .into()
    }

    fn console(&self) -> Element<Message> {
        let Some(state) = &self.state else {
            return container(text("waiting for daemon state…").color(theme::TEXT_DIM))
                .padding(40)
                .center_x(Length::Fill)
                .into();
        };

        // Hardware-out row: A1/A2/A3 device slots across the top. Label sits
        // above (matching the INPUT STRIPS/VIRTUAL MICS label pattern below)
        // and the slot row itself is centered in the available width instead
        // of clumping left — on a wide window 2-3 fixed-width slots left no
        // other cue that they weren't meant to fill the row.
        let hw_label = text("HARDWARE OUT").size(tokens::type_scale::LABEL).color(theme::TEXT_DIM);
        let mut hw_slots = row![].spacing(10);
        for (i, b) in state.buses.iter().enumerate() {
            if b.kind == BusKind::HwOutput {
                hw_slots = hw_slots.push(widgets::hw_out_slot(i, b, state));
            }
        }
        let hw = container(hw_slots).width(Length::Fill).center_x(Length::Fill);

        let card_w = self.strip_card_width();
        let renaming_strip = |idx: usize| match &self.renaming {
            Some((RenameTarget::Strip(i), draft)) if *i == idx => Some(draft.as_str()),
            _ => None,
        };
        let renaming_bus = |idx: usize| match &self.renaming {
            Some((RenameTarget::Bus(i), draft)) if *i == idx => Some(draft.as_str()),
            _ => None,
        };
        let is_active_strip = |idx: usize| matches!(self.active, Some((RenameTarget::Strip(i), _)) if i == idx);
        let is_active_bus = |idx: usize| matches!(self.active, Some((RenameTarget::Bus(i), _)) if i == idx);

        // Strips and buses render as ONE combined row (strip 1..N, then
        // B1/B2/B3 right after) — with a sane strip count (5, not 8) that
        // fits comfortably, so they don't need separate stacked groups.
        // Still wraps onto additional rows via `wrap_cards`/`cards_per_row`
        // if the window is too narrow to hold everything at once, rather
        // than reverting to the old overflow bug.
        //
        // The fudge factor here (and in `strip_card_width`) should be just
        // the console's own side padding (16px × 2) plus a small buffer for
        // the scrollable's vertical scrollbar — NOT a generous guess. A
        // bigger fudge means the wrap threshold triggers before the window
        // border actually reaches the last card, leaving a dead gap on the
        // right that doesn't match the snug left margin. Confirmed live
        // this was too conservative at 80px; tightened to 40.
        let available = self.window_width - 40.0;
        let per_row = cards_per_row(available, card_w, 10.0);
        let mut cards: Vec<Element<Message>> = Vec::new();
        for (i, s) in state.strips.iter().enumerate() {
            cards.push(widgets::strip_card(i, s, state, card_w, renaming_strip(i), is_active_strip(i)));
        }
        for (i, b) in state.buses.iter().enumerate() {
            if b.kind == BusKind::VirtualMic {
                cards.push(widgets::bus_card(i, b, state, card_w, renaming_bus(i), is_active_bus(i)));
            }
        }
        let cards_grid = wrap_cards(cards, per_row, 10.0);

        // Small, secondary controls — rarely used, shouldn't compete for
        // attention with the cards. Sit inline with the "INPUT STRIPS"
        // label instead of their own large row below the grid.
        let add_strip = button(icons::icon(icons::Icon::Plus, 11.0, theme::TEXT_DIM))
            .style(|_t, s| iced::widget::button::Style {
                background: Some(iced::Background::Color(widgets::interactive(theme::CARD_LO, s))),
                border: iced::Border { color: theme::EDGE_SOFT, width: 1.0, radius: tokens::radius::SM.into() },
                text_color: theme::TEXT_DIM,
                ..Default::default()
            })
            .width(Length::Fixed(22.0))
            .height(Length::Fixed(20.0))
            .padding(0)
            .on_press(Message::Send(Command::AddStrip));
        // Mirrors "+" — always removes the LAST strip only (see
        // `Command::RemoveLastStrip`'s doc comment for why never a specific
        // one). Disabled (no `on_press`, dimmed) rather than hidden when
        // only one strip remains, so the control stays in a stable place.
        // Raw `.size(12)`, not a type-scale token: sized to match the "+"
        // icon beside it, not body text — no minus-sign SVG exists in
        // `icons.rs` so it stays Unicode.
        let can_remove = state.strips.len() > 1;
        let mut remove_strip = button(text("−").size(12).color(if can_remove { theme::TEXT_DIM } else { theme::EDGE_SOFT }).center().width(Length::Fill).height(Length::Fill))
            .style(move |_t, s| iced::widget::button::Style {
                background: Some(iced::Background::Color(if can_remove { widgets::interactive(theme::CARD_LO, s) } else { theme::CARD_LO }.scale_alpha(if can_remove { 1.0 } else { 0.5 }))),
                border: iced::Border { color: theme::EDGE_SOFT, width: 1.0, radius: tokens::radius::SM.into() },
                text_color: theme::TEXT_DIM,
                ..Default::default()
            })
            .width(Length::Fixed(22.0))
            .height(Length::Fixed(20.0))
            .padding(0);
        if can_remove {
            remove_strip = remove_strip.on_press(Message::Send(Command::RemoveLastStrip));
        }
        let strip_controls = row![add_strip, Space::with_width(6), remove_strip].align_y(iced::Alignment::Center);

        let labels = row![
            text("INPUT STRIPS").size(tokens::type_scale::LABEL).color(theme::TEXT_DIM),
            Space::with_width(10),
            strip_controls,
            Space::with_width(Length::Fill),
            text("VIRTUAL MICS (apps select these as input)").size(tokens::type_scale::LABEL).color(theme::TEXT_DIM),
        ]
        .align_y(iced::Alignment::Center);

        let console = column![
            hw_label,
            Space::with_height(6),
            hw,
            Space::with_height(10),
            widgets::rec_panel(state),
            Space::with_height(14),
            labels,
            Space::with_height(6),
            cards_grid,
        ]
        .spacing(4)
        .padding(16);

        // Only ever scrolls vertically — `wrap_cards` is what keeps this
        // fitting horizontally at any window width now (extra cards become
        // extra rows), so there's nothing left that needs a horizontal
        // scrollbar (which Iced's `scrollable` can't mix with the
        // `Length::Fill` used elsewhere in this content anyway).
        scrollable(console).width(Length::Fill).height(Length::Fill).into()
    }

    fn matrix(&self) -> Element<Message> {
        let Some(state) = &self.state else {
            return container(text("waiting for daemon…").color(theme::TEXT_DIM)).padding(40).into();
        };
        widgets::matrix_view(state)
    }

    fn settings(&self) -> Element<Message> {
        let Some(state) = &self.state else {
            return container(text("waiting for daemon…").color(theme::TEXT_DIM)).padding(40).into();
        };
        widgets::settings_view(state, self.recdir_draft.as_deref())
    }

    fn log(&self) -> Element<Message> {
        let Some(state) = &self.state else {
            return container(text("waiting for daemon…").color(theme::TEXT_DIM)).padding(40).into();
        };
        widgets::log_view(state)
    }
}
