//! MATRIX — strips (rows) × buses (columns) at a glance. Click a cell to
//! toggle a send. Red cells are blocked feedback loops.

use crate::theme;
use eframe::egui::{self, RichText, Sense, Stroke, Vec2};
use mixer_core::engine::Command;
use mixer_core::model::{BusKind, MixerState, SourceKind};

pub fn matrix_view(ui: &mut egui::Ui, state: &MixerState, cmds: &mut Vec<Command>) {
    ui.add_space(4.0);
    ui.label(RichText::new("PATCH MATRIX").size(12.0).strong().color(theme::TEXT));
    ui.label(
        RichText::new("rows send into columns · cyan = hardware out · violet = virtual mic · red = feedback blocked")
            .size(9.0)
            .color(theme::TEXT_DIM),
    );
    ui.add_space(14.0);

    let cell = Vec2::new(52.0, 30.0);

    // A Grid keeps headers and cells on the same columns — hand-rolled
    // horizontal layouts drifted apart as soon as label widths changed.
    egui::Grid::new("patch-matrix")
        .spacing([6.0, 6.0])
        .min_col_width(cell.x)
        .show(ui, |ui| {
            ui.label(RichText::new("STRIP").size(9.0).color(theme::TEXT_DIM));
            for bus in &state.buses {
                let (tag, col) = match bus.kind {
                    BusKind::HwOutput => ("OUT", theme::ACCENT),
                    BusKind::VirtualMic => ("MIC", theme::VIOLET),
                };
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(&bus.label).size(11.5).strong().color(col));
                    ui.label(RichText::new(tag).size(7.0).color(theme::TEXT_DIM));
                });
            }
            ui.end_row();

            for (si, strip) in state.strips.iter().enumerate() {
                let is_mic = strip.kind == Some(SourceKind::HwInput);
                let label = if strip.input.is_some() {
                    strip.input_label.clone()
                } else {
                    format!("Input {}", si + 1)
                };
                let col = if !strip.input_live {
                    theme::TEXT_DIM
                } else if is_mic {
                    theme::MIC_AMBER
                } else {
                    theme::TEXT
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{:02}", si + 1)).size(9.0).color(theme::TEXT_DIM));
                    ui.label(RichText::new(elide(&label, 18)).size(10.5).color(col));
                });
                for (bi, bus) in state.buses.iter().enumerate() {
                    let on = strip.assign.get(bi).copied().unwrap_or(false);
                    let fb = state.is_feedback(si, bi);
                    let is_b = bus.kind == BusKind::VirtualMic;
                    ui.vertical_centered(|ui| {
                        draw_cell(ui, cell, on, fb, is_b, si, bi, cmds);
                    });
                }
                ui.end_row();
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn draw_cell(ui: &mut egui::Ui, size: Vec2, on: bool, feedback: bool, is_b: bool, si: usize, bi: usize, cmds: &mut Vec<Command>) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let inner = rect.shrink(4.0);
    let (a, b) = if is_b { (theme::VIOLET, theme::VIOLET_2) } else { (theme::ACCENT, theme::ACCENT_2) };
    let p = ui.painter();

    if feedback && on {
        theme::gradient_h(p, inner, theme::DANGER, theme::MUTE_RED, 6.0);
    } else if on {
        p.rect_filled(inner.expand(2.0), 8.0, egui::Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 30));
        theme::gradient_h(p, inner, a, b, 6.0);
        p.circle_filled(inner.center(), 3.5, theme::BG_DEEP);
    } else {
        p.rect_filled(inner, 6.0, theme::SEG_OFF);
        p.rect_stroke(inner, 6.0, Stroke::new(1.0_f32, if resp.hovered() { a } else { theme::EDGE }));
        p.circle_stroke(inner.center(), 3.0, Stroke::new(1.0_f32, theme::TEXT_DIM));
    }
    if resp.clicked() {
        cmds.push(Command::ToggleAssign { strip: si, bus: bi });
    }
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
