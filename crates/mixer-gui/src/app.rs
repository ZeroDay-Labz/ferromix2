//! Fixed-layout console shell. Everything fits one non-resizable window: top
//! bar, a single row of input strips + a divider + bus strips (no scroll), and
//! a status ticker. MATRIX tab shows the full patch grid.

use crate::controller::Controller;
use crate::theme;
use crate::ui;
use eframe::egui::{self, Margin, RichText, ScrollArea, Sense, Stroke};
use mixer_core::engine::Command;
use mixer_core::model::MixerState;
use std::time::Duration;

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Console,
    Matrix,
    Settings,
}

pub struct App {
    controller: Box<dyn Controller>,
    state: MixerState,
    tab: Tab,
    show_log: bool,
    rec_dir_edit: String,
    rec_dir_synced: bool,
    ui_scale: f32,
    /// Tracks armed for the next take.
    rec_armed: std::collections::HashSet<mixer_core::model::RecTarget>,
    /// Strip whose name is being edited, and the buffer.
    renaming: Option<(usize, String)>,
}

impl App {
    pub fn new(controller: Box<dyn Controller>, ui_scale: f32) -> Self {
        let tab = if std::env::var("FERROMIX_TAB").as_deref() == Ok("matrix") { Tab::Matrix } else { Tab::Console };
        App {
            controller,
            state: MixerState::default(),
            tab,
            show_log: false,
            rec_dir_edit: String::new(),
            rec_dir_synced: false,
            ui_scale,
            rec_armed: Default::default(),
            renaming: None,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(33));
        self.state = self.controller.snapshot();
        if !self.rec_dir_synced && !self.state.recordings_dir.is_empty() {
            self.rec_dir_edit = self.state.recordings_dir.clone();
            self.rec_dir_synced = true;
        }
        let mut cmds: Vec<Command> = Vec::new();

        egui::TopBottomPanel::top("bar")
            .frame(egui::Frame::none().fill(theme::BG_DEEP).inner_margin(Margin::symmetric(14.0, 8.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("FERRO").size(17.0).strong().color(theme::TEXT));
                    ui.add_space(-6.0);
                    ui.label(RichText::new("MIX").size(17.0).strong().color(theme::ACCENT));
                    ui.label(RichText::new("v1.5").size(9.0).color(theme::TEXT_DIM));
                    ui.add_space(20.0);
                    for (tab, label) in
                        [(Tab::Console, "CONSOLE"), (Tab::Matrix, "MATRIX"), (Tab::Settings, "SETTINGS")]
                    {
                        let sel = self.tab == tab;
                        if ui.selectable_label(sel, RichText::new(label).size(11.5).color(if sel { theme::ACCENT } else { theme::TEXT_DIM })).clicked() {
                            self.tab = tab;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mode = self.controller.mode();
                        let col = match mode { "LIVE" => theme::ACCENT, "MOCK" => theme::METER_MID, _ => theme::MUTE_RED };
                        ui.label(RichText::new(format!("● {mode}")).size(10.0).color(col));
                        ui.add_space(12.0);
                        if ui.button(RichText::new("SAVE").size(11.0)).clicked() {
                            cmds.push(Command::Save);
                        }
                    });
                });
            });

        egui::TopBottomPanel::bottom("ticker")
            .frame(egui::Frame::none().fill(theme::BG_DEEP).inner_margin(Margin::symmetric(14.0, 5.0)))
            .show(ctx, |ui| {
                let line = self.state.log.last().cloned().unwrap_or_default();
                ui.horizontal(|ui| {
                    if !self.state.feedback.is_empty() {
                        ui.label(RichText::new("⚠ FEEDBACK BLOCKED").size(10.0).strong().color(theme::DANGER));
                        ui.separator();
                    }
                    let resp = ui.add(
                        egui::Label::new(RichText::new(format!("» {line}")).size(10.0).color(theme::TEXT_DIM))
                            .truncate()
                            .sense(Sense::click()),
                    );
                    if resp.on_hover_text("open event log").clicked() {
                        self.show_log = !self.show_log;
                    }
                });
            });

        if self.show_log {
            egui::Window::new(RichText::new("EVENT LOG").size(12.0)).default_size([560.0, 320.0]).show(ctx, |ui| {
                ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                    for line in &self.state.log {
                        ui.label(RichText::new(line).size(10.0).color(theme::TEXT_DIM));
                    }
                });
            });
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme::BG).inner_margin(Margin::same(12.0)))
            .show(ctx, |ui| match self.tab {
                Tab::Console => {
                    ScrollArea::both().show(ui, |ui| self.console_view(ui, &mut cmds));
                }
                Tab::Matrix => {
                    ScrollArea::both().show(ui, |ui| ui::matrix::matrix_view(ui, &self.state, &mut cmds));
                }
                Tab::Settings => {
                    let mut scale = self.ui_scale;
                    ScrollArea::vertical().show(ui, |ui| {
                        ui::settings::settings_view(
                            ui,
                            &self.state,
                            &mut self.rec_dir_edit,
                            &mut scale,
                            &mut cmds,
                        )
                    });
                    if (scale - self.ui_scale).abs() > 0.001 {
                        self.ui_scale = scale;
                        crate::theme::apply(ctx, scale);
                    }
                }
            });


        // --- headless screenshot hook (dev tool; inert unless FERROMIX_SHOT set) ---
        if let Ok(path) = std::env::var("FERROMIX_SHOT") {
            let n = ctx.data_mut(|d| {
                let c = d.get_temp::<u32>(egui::Id::new("shotn")).unwrap_or(0) + 1;
                d.insert_temp(egui::Id::new("shotn"), c);
                c
            });
            let delay: u32 = std::env::var("FERROMIX_SHOT_FRAME").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
            if n == delay {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            if n > delay {
                let img = ctx.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Screenshot { image, .. } => Some(image.clone()),
                        _ => None,
                    })
                });
                if let Some(img) = img {
                    let (w, h) = (img.width() as u32, img.height() as u32);
                    let mut buf = Vec::with_capacity(8 + (w * h * 4) as usize);
                    buf.extend_from_slice(&w.to_le_bytes());
                    buf.extend_from_slice(&h.to_le_bytes());
                    for px in &img.pixels {
                        buf.extend_from_slice(&[px.r(), px.g(), px.b(), 255]);
                    }
                    let _ = std::fs::write(&path, buf);
                    std::process::exit(0);
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            ctx.request_repaint();
        }
        for cmd in cmds {
            self.controller.send(cmd);
        }
    }
}

impl App {
    fn console_view(&mut self, ui: &mut egui::Ui, cmds: &mut Vec<Command>) {
        if self.state.strips.is_empty() {
            ui.add_space(120.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("DAEMON NOT RUNNING")
                        .size(14.0)
                        .strong()
                        .color(theme::MUTE_RED),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("start it with:  ferromix-daemon      (or: systemctl --user start ferromix)")
                        .size(11.0)
                        .color(theme::TEXT_DIM),
                );
            });
            return;
        }
        use mixer_core::model::BusKind;

        // --- HARDWARE OUT (A1/A2/A3): just device slots, no fader stack ---
        ui.horizontal(|ui| {
            ui.label(RichText::new("HARDWARE OUT").size(9.0).strong().color(theme::ACCENT));
            ui.add_space(6.0);
            for (i, bus) in self.state.buses.iter().enumerate() {
                if bus.kind == BusKind::HwOutput {
                    ui::bus::hw_out_slot(ui, i, bus, &self.state.devices, cmds);
                }
            }
        });
        ui.add_space(8.0);

        // --- RECORD RACK: arm any strips and/or buses, then hit REC. ---
        use mixer_core::model::RecTarget;
        ui.horizontal(|ui| {
            let armed: Vec<RecTarget> = self
                .rec_armed
                .iter()
                .copied()
                .collect::<Vec<_>>();
            let rolling = self.state.strips.iter().any(|s| s.recording)
                || self.state.buses.iter().any(|b| b.recording);

            ui.label(
                RichText::new("RECORD")
                    .size(9.0)
                    .strong()
                    .color(if rolling { theme::REC_RED } else { theme::TEXT_DIM }),
            );
            ui.add_space(6.0);

            // Inputs.
            for (i, strip) in self.state.strips.iter().enumerate() {
                let t = RecTarget::Strip(i);
                let on = self.rec_armed.contains(&t) || strip.recording;
                let label = format!("IN{}", i + 1);
                if ui::assign_button(ui, on, false, false, &label)
                    .on_hover_text(format!("arm {}", strip.display_name(i)))
                    .clicked()
                {
                    toggle_arm(&mut self.rec_armed, t);
                }
                ui.add_space(2.0);
            }
            ui.add_space(6.0);
            // Buses.
            for (i, bus) in self.state.buses.iter().enumerate() {
                let t = RecTarget::Bus(i);
                let on = self.rec_armed.contains(&t) || bus.recording;
                let is_b = bus.kind == mixer_core::model::BusKind::VirtualMic;
                if ui::assign_button(ui, on, false, is_b, &bus.label)
                    .on_hover_text(if is_b {
                        format!("arm {} — what the far end hears", bus.label)
                    } else {
                        format!("arm {} — what you hear", bus.label)
                    })
                    .clicked()
                {
                    toggle_arm(&mut self.rec_armed, t);
                }
                ui.add_space(2.0);
            }

            ui.add_space(10.0);
            // The transport.
            let rec_label = if rolling { "■ STOP" } else { "● REC" };
            if ui::state_button(ui, rolling, rec_label, theme::REC_RED, 80.0)
                .on_hover_text("start/stop recording every armed track — each gets its own WAV")
                .clicked()
            {
                if rolling {
                    for (i, s) in self.state.strips.iter().enumerate() {
                        if s.recording {
                            cmds.push(Command::StopRecordTarget { target: RecTarget::Strip(i) });
                        }
                    }
                    for (i, b) in self.state.buses.iter().enumerate() {
                        if b.recording {
                            cmds.push(Command::StopRecordTarget { target: RecTarget::Bus(i) });
                        }
                    }
                } else {
                    for t in armed {
                        cmds.push(Command::StartRecordTarget { target: t });
                    }
                }
            }
            if rolling {
                ui.add_space(8.0);
                let n = self.state.strips.iter().filter(|s| s.recording).count()
                    + self.state.buses.iter().filter(|b| b.recording).count();
                ui.label(
                    RichText::new(format!("● {n} track{}", if n == 1 { "" } else { "s" }))
                        .size(9.5)
                        .strong()
                        .color(theme::REC_RED),
                );
            } else if self.rec_armed.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("arm a track above")
                        .size(9.0)
                        .italics()
                        .color(theme::TEXT_DIM),
                );
            }
        });
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label(RichText::new("INPUT STRIPS").size(9.0).strong().color(theme::TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("VIRTUAL MICS (apps select these as input)").size(8.5).color(theme::TEXT_DIM));
            });
        });
        ui.add_space(4.0);

        ui.horizontal_top(|ui| {
            let inputs = self.state.inputs.clone();
            let buses = self.state.buses.clone();
            for (i, strip) in self.state.strips.iter().enumerate() {
                ui::strip::input_strip(ui, i, strip, &buses, &inputs, &self.state, &mut self.renaming, cmds);
            }

            ui.add_space(5.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 330.0), Sense::hover());
            ui.painter().rect_filled(rect, 0.0, theme::EDGE);
            let _ = Stroke::NONE;
            ui.add_space(5.0);

            // Only B buses get a full strip.
            for (i, bus) in self.state.buses.iter().enumerate() {
                if bus.kind == BusKind::VirtualMic {
                    ui::bus::bus_strip(ui, i, bus, &self.state.devices, &self.state, cmds);
                }
            }
        });
    }
}


fn toggle_arm(
    set: &mut std::collections::HashSet<mixer_core::model::RecTarget>,
    t: mixer_core::model::RecTarget,
) {
    if !set.remove(&t) {
        set.insert(t);
    }
}
