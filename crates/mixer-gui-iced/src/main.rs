//! FerroMix — Iced console. A pure client of the FerroMix daemon: it renders
//! the mixer state it polls over the Unix socket and sends `Command`s back. No
//! PipeWire here, so the audio engine is never at risk while the UI evolves.

mod link;
mod theme;
mod widgets;

use iced::widget::{column, container, row, scrollable, text, Space};
use iced::{Element, Length, Subscription, Task};
use mixer_core::engine::Command;
use mixer_core::model::{BusKind, MixerState};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Mutex;

fn main() -> iced::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("FerroMix Iced console starting");
    iced::application("FerroMix2", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_| theme::base())
        .window_size((1620.0, 780.0))
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
}

struct App {
    state: Option<MixerState>,
    connected: bool,
    status: String,
    tab: Tab,
}

#[derive(Debug, Clone)]
enum Message {
    Link(link::FromLink),
    Tab(Tab),
    Send(Command),
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
            Message::Send(c) => self.send(c),
            Message::Tick => {}
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        // Drain the link worker's channel on a timer and feed events in.
        iced::time::every(std::time::Duration::from_millis(33)).map(|_| {
            let mut guard = LINK_RX.lock().unwrap();
            if let Some(rx) = guard.as_ref() {
                if let Ok(ev) = rx.try_recv() {
                    return Message::Link(ev);
                }
            }
            Message::Tick
        })
    }

    fn view(&self) -> Element<Message> {
        let header = self.header();
        let body: Element<Message> = match self.tab {
            Tab::Console => self.console(),
            Tab::Matrix => self.matrix(),
            Tab::Settings => self.settings(),
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
            text("2  v2.2").size(11).color(theme::TEXT_DIM),
        ]
        .align_y(iced::Alignment::Center);

        let tabs = row![
            widgets::tab_button("CONSOLE", self.tab == Tab::Console).on_press(Message::Tab(Tab::Console)),
            widgets::tab_button("MATRIX", self.tab == Tab::Matrix).on_press(Message::Tab(Tab::Matrix)),
            widgets::tab_button("SETTINGS", self.tab == Tab::Settings).on_press(Message::Tab(Tab::Settings)),
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

        container(
            row![
                logo,
                Space::with_width(24),
                tabs,
                Space::with_width(Length::Fill),
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

        // Input strips (sources).
        let mut strips = row![].spacing(10);
        for (i, s) in state.strips.iter().enumerate() {
            strips = strips.push(widgets::strip_card(i, s, state));
        }

        // Virtual mic buses (B), on the right.
        let mut buses = row![].spacing(10);
        for (i, b) in state.buses.iter().enumerate() {
            if b.kind == BusKind::VirtualMic {
                buses = buses.push(widgets::bus_card(i, b, state));
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
        widgets::settings_view(state)
    }
}
