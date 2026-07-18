//! A bus strip. A-buses (hardware out) get a device dropdown; B-buses are
//! virtual mics. Meter + fader, mute, record.

use super::{db_text, fader, meters, state_button, FADER_H, STRIP_W};
use crate::theme;
use eframe::egui::{self, Color32, Margin, RichText, Stroke};
use mixer_core::engine::Command;
use mixer_core::model::{Bus, BusKind, Device, MixerState, RecTarget};

pub fn bus_strip(
    ui: &mut egui::Ui,
    idx: usize,
    bus: &Bus,
    devices: &[Device],
    state: &MixerState,
    cmds: &mut Vec<Command>,
) {
    let is_hw = bus.kind == BusKind::HwOutput;
    let accent = if is_hw { theme::ACCENT } else { theme::VIOLET };

    egui::Frame::none()
        .fill(theme::CARD)
        .stroke(Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 110),
        ))
        .rounding(8.0)
        .shadow(eframe::egui::epaint::Shadow {
            offset: egui::vec2(0.0, 3.0),
            blur: 12.0,
            spread: 0.0,
            color: Color32::from_black_alpha(90),
        })
        .inner_margin(Margin::symmetric(9.0, 9.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
            ui.set_width(STRIP_W);

            ui.horizontal(|ui| {
                ui.label(RichText::new(&bus.label).size(14.0).strong().color(accent));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(if is_hw { "OUT" } else { "MIC" }).size(8.0).color(theme::TEXT_DIM));
                });
            });

            if is_hw {
                let current = bus
                    .device
                    .as_ref()
                    .and_then(|d| devices.iter().find(|dev| dev.key.contains(d.as_str()) || dev.label.contains(d.as_str())))
                    .map(|d| d.label.clone())
                    .or_else(|| bus.device.clone())
                    .unwrap_or_else(|| "default out".into());
                egui::ComboBox::from_id_salt(("busdev", idx))
                    .selected_text(RichText::new(elide(&current, 15)).size(9.0))
                    .width(STRIP_W - 2.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(bus.device.is_none(), "default out").clicked() {
                            cmds.push(Command::SetBusDevice { bus: idx, device: None });
                        }
                        for d in devices {
                            let sel = bus.device.as_deref() == Some(d.key.as_str());
                            if ui.selectable_label(sel, &d.label).clicked() {
                                cmds.push(Command::SetBusDevice { bus: idx, device: Some(d.key.clone()) });
                            }
                        }
                    });
            } else {
                // WHO LISTENS: pick the app whose microphone this bus feeds.
                // FerroMix retargets the app's capture stream itself, so you
                // never have to go digging through Discord's audio settings.
                let current = bus
                    .listener
                    .as_ref()
                    .and_then(|k| state.capture_apps.iter().find(|a| &a.key == k))
                    .map(|a| a.label.clone())
                    .or_else(|| bus.listener.clone())
                    .unwrap_or_else(|| "◇ SEND TO APP".into());
                let live = !bus.listeners.is_empty();
                egui::ComboBox::from_id_salt(("blisten", idx))
                    .selected_text(
                        RichText::new(elide(&current, 15))
                            .size(9.5)
                            .color(if live { accent } else { theme::TEXT_DIM }),
                    )
                    .width(STRIP_W)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(bus.listener.is_none(), "— none —").clicked() {
                            cmds.push(Command::SetBusListener { bus: idx, app: None });
                        }
                        if state.capture_apps.is_empty() {
                            ui.label(
                                RichText::new("no app has a mic open")
                                    .size(9.0)
                                    .italics()
                                    .color(theme::TEXT_DIM),
                            );
                        }
                        for app in &state.capture_apps {
                            let sel = bus.listener.as_deref() == Some(app.key.as_str());
                            if ui.selectable_label(sel, &app.label).clicked() {
                                cmds.push(Command::SetBusListener {
                                    bus: idx,
                                    app: Some(app.key.clone()),
                                });
                            }
                        }
                    });
                if live {
                    ui.label(
                        RichText::new(format!("◂ {} listening", elide(&bus.listeners[0], 12)))
                            .size(8.0)
                            .color(accent),
                    );
                } else if bus.listener.is_some() {
                    ui.label(
                        RichText::new("waiting for mic…").size(8.0).italics().color(theme::TEXT_DIM),
                    )
                    .on_hover_text("The app has no microphone stream open yet — start a call.");
                } else {
                    ui.label(RichText::new("no app assigned").size(8.0).italics().color(theme::TEXT_DIM));
                }
            }
            ui.add_space(5.0);

            ui.horizontal(|ui| {
                ui.add_space(16.0);
                meters::stereo_meter(ui, ui.id().with(("bm", idx)), bus.level, FADER_H);
                let mut v = bus.volume;
                if fader::fader(ui, &mut v, FADER_H, accent).changed() {
                    cmds.push(Command::SetBusVolume { bus: idx, volume: v });
                }
            });

            ui.vertical_centered(|ui| {
                ui.label(RichText::new(format!("{} dB", db_text(bus.volume))).size(8.5).color(theme::TEXT_DIM));
            });
            ui.add_space(3.0);

            if state_button(ui, bus.mute, "MUTE", theme::MUTE_RED, STRIP_W).clicked() {
                cmds.push(Command::SetBusMute { bus: idx, mute: !bus.mute });
            }
            ui.add_space(4.0);
            if !is_hw {
                // MONITOR: send this virtual mic to a hardware out so you can
                // hear exactly what the far end is getting.
                ui.label(RichText::new("MONITOR ON").size(7.5).color(theme::TEXT_DIM));
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                    let a_buses: Vec<(usize, &Bus)> = state
                        .buses
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| b.kind == BusKind::HwOutput)
                        .collect();
                    for (ai, (_, ab)) in a_buses.iter().enumerate() {
                        let on = bus.monitor.get(ai).copied().unwrap_or(false);
                        if crate::ui::assign_button(ui, on, false, false, &ab.label)
                            .on_hover_text(format!("hear {} on {}", bus.label, ab.label))
                            .clicked()
                        {
                            cmds.push(Command::ToggleBusMonitor { bus: idx, a_bus: ai });
                        }
                    }
                });
                ui.add_space(3.0);
            }
            if !is_hw {
                ui.add_space(2.0);
                let is_def = state.default_input == Some(idx);
                let label = if is_def { "★ DEFAULT MIC" } else { "SET AS DEF MIC" };
                if state_button(ui, is_def, label, theme::MIC_AMBER, STRIP_W)
                    .on_hover_text("Make this virtual mic the system default INPUT — for apps that can't pick a microphone.")
                    .clicked()
                    && !is_def
                {
                    cmds.push(Command::SetDefaultInput { bus: idx });
                }
            }
            });
        });
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut o: String = s.chars().take(max.saturating_sub(1)).collect();
        o.push('…');
        o
    }
}


/// A HARDWARE OUT slot (A1/A2/A3) — Voicemeeter Potato's top-right corner.
/// An A bus is just "which physical device does this letter mean"; it doesn't
/// need a fader stack of its own, so it gets a compact row instead.
pub fn hw_out_slot(
    ui: &mut egui::Ui,
    idx: usize,
    bus: &Bus,
    devices: &[Device],
    cmds: &mut Vec<Command>,
) {
    egui::Frame::none()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0_f32, theme::EDGE))
        .rounding(3.0)
        .inner_margin(Margin::symmetric(7.0, 5.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&bus.label).size(12.0).strong().color(theme::ACCENT));
                let current = bus
                    .device
                    .as_ref()
                    .and_then(|d| devices.iter().find(|dev| dev.key == *d))
                    .map(|d| d.label.clone())
                    .unwrap_or_else(|| "— select device —".into());
                egui::ComboBox::from_id_salt(("hwout", idx))
                    .selected_text(RichText::new(elide(&current, 30)).size(9.5))
                    .width(250.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(bus.device.is_none(), "— none —").clicked() {
                            cmds.push(Command::SetBusDevice { bus: idx, device: None });
                        }
                        for d in devices {
                            let sel = bus.device.as_deref() == Some(d.key.as_str());
                            if ui.selectable_label(sel, &d.label).clicked() {
                                cmds.push(Command::SetBusDevice { bus: idx, device: Some(d.key.clone()) });
                            }
                        }
                    });
                let mute_col = if bus.mute { theme::MUTE_RED } else { theme::TEXT_DIM };
                if ui
                    .add(
                        egui::Button::new(RichText::new("M").size(9.5).strong().color(
                            if bus.mute { theme::BG_DEEP } else { mute_col },
                        ))
                        .fill(if bus.mute { theme::MUTE_RED } else { theme::PANEL_HI })
                        .stroke(Stroke::new(1.0_f32, theme::EDGE))
                        .rounding(2.0)
                        .min_size(egui::vec2(20.0, 18.0)),
                    )
                    .on_hover_text("mute this hardware output")
                    .clicked()
                {
                    cmds.push(Command::SetBusMute { bus: idx, mute: !bus.mute });
                }
                let rec = if bus.recording { "●" } else { "○" };
                if ui
                    .add(
                        egui::Button::new(RichText::new(rec).size(9.5).color(theme::REC_RED))
                            .fill(theme::PANEL_HI)
                            .stroke(Stroke::new(1.0_f32, theme::EDGE))
                            .rounding(2.0)
                            .min_size(egui::vec2(20.0, 18.0)),
                    )
                    .on_hover_text("record everything you hear on this output")
                    .clicked()
                {
                    if bus.recording {
                        cmds.push(Command::StopRecordTarget { target: RecTarget::Bus(idx) });
                    } else {
                        cmds.push(Command::StartRecordTarget { target: RecTarget::Bus(idx) });
                    }
                }
            });
        });
}
