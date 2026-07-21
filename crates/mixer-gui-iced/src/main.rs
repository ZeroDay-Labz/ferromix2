//! FerroMix — Iced console. A pure client of the FerroMix daemon: it renders
//! the mixer state it polls over the Unix socket and sends `Command`s back. No
//! PipeWire here, so the audio engine is never at risk while the UI evolves.

mod icons;
mod link;
mod theme;
mod tokens;
mod widgets;

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Element, Length, Subscription, Task};
use mixer_core::engine::Command;
use mixer_core::model::{BusKind, MixerState};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long to wait after the last change before autosaving.
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(1500);

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

/// The link worker handles are global because Iced's subscription needs to read
/// the receiver from a plain function; a Mutex keeps it simple and safe.
static LINK_RX: Mutex<Option<Receiver<link::FromLink>>> = Mutex::new(None);
static LINK_TX: Mutex<Option<Sender<link::ToLink>>> = Mutex::new(None);

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
    state: Option<MixerState>,
    connected: bool,
    status: String,
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
}

#[derive(Debug, Clone)]
enum Message {
    Link(link::FromLink),
    Tab(Tab),
    Send(Command),
    WindowResized(f32),
    RenameStart(RenameTarget, String),
    RenameChanged(String),
    RenameSubmit,
    RenameCancel,
    RecDirChanged(String),
    RecDirApply,
    Tick,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let (tx, rx) = link::spawn();
        *LINK_TX.lock().unwrap() = Some(tx);
        *LINK_RX.lock().unwrap() = Some(rx);
        (
            App {
                state: None,
                connected: false,
                status: "connecting to daemon…".into(),
                tab: Tab::Console,
                dirty: false,
                last_change: None,
                window_width: DEFAULT_WINDOW.0,
                renaming: None,
                recdir_draft: None,
            },
            Task::none(),
        )
    }

    fn send(&self, cmd: Command) {
        if let Some(tx) = LINK_TX.lock().unwrap().as_ref() {
            let _ = tx.send(link::ToLink::Cmd(cmd));
        }
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Link(ev) => match ev {
                link::FromLink::Connected => {
                    self.connected = true;
                    self.status = "LIVE".into();
                }
                link::FromLink::State(s) => {
                    self.connected = true;
                    self.state = Some(*s);
                }
                link::FromLink::Disconnected(e) => {
                    self.connected = false;
                    self.status = format!("daemon offline — {e}");
                }
            },
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
                self.send(c);
            }
            Message::Tick => {
                if self.dirty {
                    if let Some(t) = self.last_change {
                        if t.elapsed() > AUTOSAVE_DEBOUNCE {
                            self.send(Command::Save);
                            self.dirty = false;
                        }
                    }
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        // Drain the link worker's channel on a timer and feed events in.
        let poll = iced::time::every(std::time::Duration::from_millis(33)).map(|_| {
            let mut guard = LINK_RX.lock().unwrap();
            if let Some(rx) = guard.as_ref() {
                if let Ok(ev) = rx.try_recv() {
                    return Message::Link(ev);
                }
            }
            Message::Tick
        });
        let resize = iced::window::resize_events().map(|(_id, size)| Message::WindowResized(size.width));
        Subscription::batch([poll, resize])
    }

    /// Strip/bus card width for the current window: shrinks smoothly between
    /// `STRIP_MAX` (roomy) and `STRIP_MIN` (compact) as the window narrows,
    /// rather than clipping — below `STRIP_MIN` the console row scrolls
    /// horizontally instead (see the `scrollable` wrapping it in `console()`).
    fn strip_card_width(&self) -> f32 {
        // Rough available width: window minus side padding/chrome. Doesn't
        // need to be exact — it only picks a card size, the scrollable
        // handles anything left over.
        let available = self.window_width - 80.0;
        let strips = self.state.as_ref().map(|s| s.strips.len()).unwrap_or(5).max(1) as f32;
        let per_card = available / strips - 20.0; // card padding/gap allowance
        per_card.clamp(tokens::layout::STRIP_MIN, tokens::layout::STRIP_MAX)
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
            text("FERRO").size(20).color(theme::TEXT),
            text("MIX").size(20).color(theme::ACCENT),
            text("2  v2.4").size(11).color(theme::TEXT_DIM),
        ]
        .align_y(iced::Alignment::Center);

        let tabs = row![
            widgets::tab_button("CONSOLE", self.tab == Tab::Console).on_press(Message::Tab(Tab::Console)),
            widgets::tab_button("MATRIX", self.tab == Tab::Matrix).on_press(Message::Tab(Tab::Matrix)),
            widgets::tab_button("SETTINGS", self.tab == Tab::Settings).on_press(Message::Tab(Tab::Settings)),
            widgets::tab_button("LOG", self.tab == Tab::Log).on_press(Message::Tab(Tab::Log)),
        ]
        .spacing(8);

        let status = if self.connected {
            row![
                text("●").size(12).color(theme::METER_LO),
                text(" LIVE").size(12).color(theme::TEXT),
            ]
        } else {
            row![text("● ").size(12).color(theme::REC_RED), text(self.status.clone()).size(11).color(theme::TEXT_DIM)]
        };

        let save_label = if self.dirty { "● SAVE" } else { "SAVED" };
        let save_fg = if self.dirty { theme::MIC_AMBER } else { theme::TEXT_DIM };
        let save_btn = button(text(save_label).size(11).color(save_fg))
            .style(move |_t, _s| iced::widget::button::Style {
                background: Some(iced::Background::Color(if self.dirty {
                    theme::with_alpha(theme::MIC_AMBER, 0.12)
                } else {
                    iced::Color::TRANSPARENT
                })),
                border: iced::Border { color: if self.dirty { theme::MIC_AMBER } else { theme::EDGE_SOFT }, width: 1.0, radius: 6.0.into() },
                text_color: save_fg,
                ..Default::default()
            })
            .padding([6, 12])
            .on_press(Message::Send(Command::Save));

        container(
            row![
                logo,
                Space::with_width(24),
                tabs,
                Space::with_width(Length::Fill),
                save_btn,
                Space::with_width(16),
                status,
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        )
        .padding([12, 18])
        .width(Length::Fill)
        .into()
    }

    fn console(&self) -> Element<Message> {
        let Some(state) = &self.state else {
            return container(text("waiting for daemon state…").color(theme::TEXT_DIM))
                .padding(40)
                .center_x(Length::Fill)
                .into();
        };

        // Hardware-out row: A1/A2/A3 device slots across the top.
        let mut hw = row![text("HARDWARE OUT").size(10).color(theme::TEXT_DIM)]
            .spacing(10)
            .align_y(iced::Alignment::Center);
        for (i, b) in state.buses.iter().enumerate() {
            if b.kind == BusKind::HwOutput {
                hw = hw.push(widgets::hw_out_slot(i, b, state));
            }
        }

        let card_w = self.strip_card_width();
        let renaming_strip = |idx: usize| match &self.renaming {
            Some((RenameTarget::Strip(i), draft)) if *i == idx => Some(draft.as_str()),
            _ => None,
        };
        let renaming_bus = |idx: usize| match &self.renaming {
            Some((RenameTarget::Bus(i), draft)) if *i == idx => Some(draft.as_str()),
            _ => None,
        };

        // Input strips (sources).
        let mut strips = row![].spacing(10);
        for (i, s) in state.strips.iter().enumerate() {
            strips = strips.push(widgets::strip_card(i, s, state, card_w, renaming_strip(i)));
        }
        let add_strip = button(text("+").size(16).color(theme::TEXT_DIM).center().width(Length::Fill))
            .style(|_t, _s| iced::widget::button::Style {
                background: Some(iced::Background::Color(theme::CARD_LO)),
                border: iced::Border { color: theme::EDGE_SOFT, width: 1.0, radius: 8.0.into() },
                text_color: theme::TEXT_DIM,
                ..Default::default()
            })
            .width(Length::Fixed(44.0))
            .height(Length::Fixed(300.0))
            .on_press(Message::Send(Command::AddStrip));
        strips = strips.push(add_strip);

        // Virtual mic buses (B), on the right.
        let mut buses = row![].spacing(10);
        for (i, b) in state.buses.iter().enumerate() {
            if b.kind == BusKind::VirtualMic {
                buses = buses.push(widgets::bus_card(i, b, state, card_w, renaming_bus(i)));
            }
        }

        let labels = row![
            text("INPUT STRIPS").size(10).color(theme::TEXT_DIM),
            Space::with_width(Length::Fill),
            text("VIRTUAL MICS (apps select these as input)").size(10).color(theme::TEXT_DIM),
        ];

        let console = column![
            hw,
            Space::with_height(14),
            labels,
            Space::with_height(6),
            row![strips, Space::with_width(20), buses].spacing(0),
        ]
        .spacing(4)
        .padding(16);

        // Horizontal scroll-on-overflow isn't viable here: several rows in
        // this content (e.g. `labels`) intentionally use Length::Fill space
        // to right-align a label, and Iced panics if scrollable content fills
        // the axis it scrolls. Card-shrinking (`strip_card_width`) covers the
        // common case instead; below STRIP_MIN, cards stay at STRIP_MIN and
        // rely on window resize rather than a horizontal scrollbar.
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
