//! Stereo LED VU meter with peak-hold — two columns (L | R), because almost
//! all audio here is stereo and a single bar hides a dead channel.
//! Green → amber → red zones, dim wells for unlit segments, slow peak decay.

use crate::theme;
use eframe::egui::{self, Color32, Rangef, Sense, Stroke, Vec2};
use mixer_core::model::Level;

const SEGMENTS: usize = 22;
const GAP: f32 = 2.0;
const COL_W: f32 = 6.0;
const COL_GAP: f32 = 2.0;

/// Total width a stereo meter occupies.
pub const METER_W: f32 = COL_W * 2.0 + COL_GAP;

fn zone_color(frac: f32) -> Color32 {
    if frac > 0.90 {
        theme::METER_HI
    } else if frac > 0.72 {
        theme::METER_MID
    } else {
        theme::METER_LO
    }
}

fn column(ui: &egui::Ui, rect: egui::Rect, level: f32, peak: f32) {
    let p = ui.painter();
    let seg_h = (rect.height() - GAP * (SEGMENTS as f32 - 1.0)) / SEGMENTS as f32;
    for i in 0..SEGMENTS {
        let frac_top = (i as f32 + 1.0) / SEGMENTS as f32;
        let lit = level > (i as f32) / SEGMENTS as f32 && level > 0.001;
        let y_bottom = rect.bottom() - (i as f32) * (seg_h + GAP);
        let seg = egui::Rect::from_min_max(
            egui::pos2(rect.left(), y_bottom - seg_h),
            egui::pos2(rect.right(), y_bottom),
        );
        p.rect_filled(seg, 1.0, if lit { zone_color(frac_top) } else { theme::SEG_OFF });
    }
    if peak > 0.01 {
        let y = rect.bottom() - peak * rect.height();
        p.hline(Rangef::new(rect.left(), rect.right()), y, Stroke::new(1.5_f32, zone_color(peak)));
    }
}

/// Draw an L/R pair. `height` is the meter height; width is [`METER_W`].
pub fn stereo_meter(ui: &mut egui::Ui, id: egui::Id, level: Level, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(METER_W, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let (l, r) = (level.l.clamp(0.0, 1.0), level.r.clamp(0.0, 1.0));

    // Peak-hold, one per channel, stored in temp memory.
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    let mut held = ui.ctx().data(|d| d.get_temp::<(f32, f32)>(id)).unwrap_or((0.0, 0.0));
    held.0 = if l >= held.0 { l } else { (held.0 - dt * 0.28).max(l) };
    held.1 = if r >= held.1 { r } else { (held.1 - dt * 0.28).max(r) };
    ui.ctx().data_mut(|d| d.insert_temp(id, held));

    let left = egui::Rect::from_min_size(rect.min, Vec2::new(COL_W, height));
    let right = egui::Rect::from_min_size(
        egui::pos2(rect.min.x + COL_W + COL_GAP, rect.min.y),
        Vec2::new(COL_W, height),
    );
    column(ui, left, l, held.0);
    column(ui, right, r, held.1);
}
