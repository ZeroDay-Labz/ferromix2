//! An input strip, Voicemeeter-Potato layout: input selector on top, then
//! meter | fader | assign-stack side by side, then dB + MUTE. Contents are
//! explicitly vertical so strips stay narrow columns inside the console row.

use super::{assign_button, db_text, fader, meters, state_button, FADER_H, STRIP_W};
use crate::theme;
use eframe::egui::{self, Color32, Margin, RichText};
use mixer_core::engine::Command;
use mixer_core::model::{Bus, BusKind, InputOption, MixerState, SourceKind, Strip};

#[allow(clippy::too_many_arguments)]
pub fn input_strip(
    ui: &mut egui::Ui,
    idx: usize,
    strip: &Strip,
    buses: &[Bus],
    inputs: &[InputOption],
    state: &MixerState,
    renaming: &mut Option<(usize, String)>,
    cmds: &mut Vec<Command>,
) {
    let assigned = strip.input.is_some();
    let accent = match strip.kind {
        Some(SourceKind::HwInput) => theme::MIC_AMBER,
        _ => theme::ACCENT,
    };
    // Live strips get a faint accent halo on the card edge; idle ones stay quiet.
    let edge = if strip.input_live {
        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 110)
    } else {
        theme::EDGE_SOFT
    };

    egui::Frame::none()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0_f32, edge))
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

                // Header: the strip's NAME (right-click to rename), not "IN 03".
                ui.horizontal(|ui| {
                    if let Some((ri, buf)) = renaming.as_mut().filter(|(ri, _)| *ri == idx) {
                        let _ = ri;
                        let resp = ui.add(
                            egui::TextEdit::singleline(buf)
                                .desired_width(STRIP_W - 20.0)
                                .font(egui::TextStyle::Monospace),
                        );
                        resp.request_focus();
                        if resp.lost_focus() {
                            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                cmds.push(Command::SetStripName { strip: idx, name: buf.clone() });
                            }
                            *renaming = None;
                        }
                    } else {
                        let title = strip.display_name(idx);
                        let name_col = if strip.input_live { theme::TEXT } else { theme::TEXT_DIM };
                        let resp = ui
                            .label(RichText::new(elide(&title, 13)).size(11.0).strong().color(name_col))
                            .on_hover_text("right-click to rename");
                        resp.context_menu(|ui| {
                            if ui.button("Rename…").clicked() {
                                *renaming = Some((idx, strip.name.clone()));
                                ui.close_menu();
                            }
                            if ui.button("Clear name").clicked() {
                                cmds.push(Command::SetStripName { strip: idx, name: String::new() });
                                ui.close_menu();
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if strip.input_live {
                                ui.label(RichText::new("●").size(8.0).color(accent));
                            } else if assigned {
                                ui.label(RichText::new("○").size(8.0).color(theme::MUTE_RED))
                                    .on_hover_text("source offline — route is held");
                            }
                        });
                    }
                });

                // INPUT SELECTOR — pick a virtual input, any mic, or any app.
                let sel = if assigned {
                    elide(&strip.input_label, 15)
                } else {
                    format!("▸ Input {}", idx + 1) // the strip's own device
                };
                let sel_col = if !assigned {
                    theme::TEXT
                } else if strip.input_live {
                    theme::TEXT
                } else {
                    theme::MUTE_RED
                };
                egui::ComboBox::from_id_salt(("stripinput", idx))
                    .selected_text(RichText::new(sel).size(10.0).color(sel_col))
                    .width(STRIP_W)
                    .show_ui(ui, |ui| {
                        ui.set_min_width(200.0);
                        if ui
                            .selectable_label(
                                strip.input.is_none(),
                                RichText::new("— device only —").size(10.5),
                            )
                            .on_hover_text("The strip is still live: point an app's OUTPUT at \"FerroMix Input\" and it lands here.")
                            .clicked()
                        {
                            cmds.push(Command::SetStripInput { strip: idx, input: None });
                        }
                        section(ui, "MICROPHONES", theme::MIC_AMBER);
                        pick(ui, inputs, SourceKind::HwInput, strip, idx, cmds);
                        section(ui, "APPS PLAYING NOW", theme::TEXT_DIM);
                        if !pick(ui, inputs, SourceKind::App, strip, idx, cmds) {
                            ui.label(RichText::new("  (nothing playing)").size(9.5).italics().color(theme::TEXT_DIM));
                        }
                    });
                ui.add_space(5.0);

                // meter | fader | assign stack — the Potato silhouette.
                ui.horizontal(|ui| {
                    ui.add_space(1.0);
                    meters::stereo_meter(ui, ui.id().with(("sm", idx)), strip.level, FADER_H);
                    let mut v = strip.volume;
                    let resp = fader::fader(ui, &mut v, FADER_H, accent);
                    if resp.changed() {
                        cmds.push(Command::SetStripVolume { strip: idx, volume: v });
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                        for (bi, bus) in buses.iter().enumerate() {
                            let on = strip.assign.get(bi).copied().unwrap_or(false);
                            let fb = state.is_feedback(idx, bi);
                            let is_b = bus.kind == BusKind::VirtualMic;
                            let resp = assign_button(ui, on, fb && on, is_b, &bus.label);
                            let tip = if fb && on {
                                format!("⚠ FEEDBACK: this app listens to {} — link blocked", bus.label)
                            } else if is_b {
                                format!("send to virtual mic {}", bus.label)
                            } else {
                                format!("send to output {}", bus.label)
                            };
                            if resp.on_hover_text(tip).clicked() {
                                cmds.push(Command::ToggleAssign { strip: idx, bus: bi });
                            }
                        }
                    });
                });

                ui.add_space(2.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(format!("{} dB", db_text(strip.volume))).size(8.5).color(theme::TEXT_DIM));
                });
                ui.add_space(2.0);
                if state_button(ui, strip.mute, "MUTE", theme::MUTE_RED, STRIP_W).clicked() {
                    cmds.push(Command::SetStripMute { strip: idx, mute: !strip.mute });
                }
                ui.add_space(2.0);
                // Apps that can't pick a device (Zoiper, and plenty of others)
                // just follow the system default — send it here in one click.
                let is_def = state.default_output == Some(idx);
                let label = if is_def { "★ SYSTEM DEFAULT" } else { "SET AS DEFAULT" };
                if state_button(ui, is_def, label, theme::ACCENT, STRIP_W)
                    .on_hover_text("Make this strip the system default OUTPUT — any app that doesn't let you choose a device will land here.")
                    .clicked()
                    && !is_def
                {
                    cmds.push(Command::SetDefaultOutput { strip: idx });
                }
            });
        });
}

fn section(ui: &mut egui::Ui, label: &str, color: eframe::egui::Color32) {
    ui.add_space(4.0);
    ui.label(RichText::new(label).size(8.0).color(color));
}

fn pick(
    ui: &mut egui::Ui,
    inputs: &[InputOption],
    kind: SourceKind,
    strip: &Strip,
    idx: usize,
    cmds: &mut Vec<Command>,
) -> bool {
    let mut any = false;
    for opt in inputs.iter().filter(|i| i.kind == kind) {
        any = true;
        let on = strip.input.as_deref() == Some(opt.key.as_str());
        if ui.selectable_label(on, RichText::new(&opt.label).size(10.5)).clicked() {
            cmds.push(Command::SetStripInput { strip: idx, input: Some(opt.key.clone()) });
        }
    }
    any
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
