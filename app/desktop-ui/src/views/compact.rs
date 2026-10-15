//! Compact mode: small window with mute, volume and device state.

use egui::{CornerRadius, Rect, RichText, Sense, Ui, pos2, vec2};

use crate::app::NovaMixApp;
use crate::theme::level_color;
use crate::widgets::{MeterState, fmt_db};

fn mini_meter(ui: &mut Ui, app: &NovaMixApp, m: &MeterState, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 10.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(2), app.pal.meter_bg);
    let db = m.max_peak();
    let frac = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let r = Rect::from_min_max(rect.min, pos2(rect.min.x + frac * rect.width(), rect.max.y));
    p.rect_filled(r, CornerRadius::same(2), if m.clip { app.pal.danger } else { level_color(&app.pal, db) });
}

pub fn show(app: &mut NovaMixApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("NovaMix").strong().color(app.pal.accent));
            let color = if app.status.running { app.pal.ok } else { app.pal.danger };
            crate::widgets::status_dot(ui, color, &app.profile.name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(app.t("compact_expand")).clicked() {
                    app.set_compact(&ctx, false);
                }
                if ui.selectable_label(app.safe_mode, app.t("safe_mode")).clicked() {
                    app.toggle_safe_mode();
                }
            });
        });
        let muted = app.mic_muted();
        let (label, color) = if muted { (app.t("mic_muted"), app.pal.mute) } else { (app.t("mic_live"), app.pal.ok) };
        if ui
            .add_sized(
                [ui.available_width(), 30.0],
                egui::Button::new(RichText::new(format!("🎤 {label}")).strong().color(egui::Color32::WHITE)).fill(color),
            )
            .clicked()
        {
            app.toggle_mic_mute();
        }
        ui.separator();
        egui::Grid::new("compact_grid").num_columns(5).spacing([6.0, 6.0]).show(ui, |ui| {
            for i in 0..app.profile.channels.len() {
                let st = app.status.channels.get(i).map(|s| s.state.clone()).unwrap_or(novamix_control::EndpointState::Off);
                let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, app.status_color(&st));
                ui.label(&app.profile.channels[i].name).on_hover_text(app.status_text(&st));
                let mut mute = app.profile.channels[i].mute;
                crate::widgets::toggle(ui, &mut mute, app.tr.t("mute_short"), app.pal.mute, vec2(26.0, 20.0));
                app.profile.channels[i].mute = mute;
                let mut v = app.profile.channels[i].fader_db.max(-60.0);
                if ui.add(egui::Slider::new(&mut v, -60.0..=12.0).show_value(false)).changed() {
                    app.profile.channels[i].fader_db = if v <= -60.0 { -90.0 } else { v };
                }
                ui.vertical(|ui| {
                    mini_meter(ui, app, &app.meters.ch_out[i], 90.0);
                    ui.label(RichText::new(fmt_db(app.profile.channels[i].fader_db)).small().monospace());
                });
                ui.end_row();
            }
            for b in 0..app.profile.buses.len() {
                let st = app.status.buses.get(b).map(|s| s.state.clone()).unwrap_or(novamix_control::EndpointState::Off);
                let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, app.status_color(&st));
                ui.label(RichText::new(format!("→ {}", app.profile.buses[b].name)).strong()).on_hover_text(app.status_text(&st));
                let mut mute = app.profile.buses[b].mute;
                crate::widgets::toggle(ui, &mut mute, app.tr.t("mute_short"), app.pal.mute, vec2(26.0, 20.0));
                app.profile.buses[b].mute = mute;
                let mut v = app.profile.buses[b].fader_db.max(-60.0);
                if ui.add(egui::Slider::new(&mut v, -60.0..=12.0).show_value(false)).changed() {
                    app.profile.buses[b].fader_db = if v <= -60.0 { -90.0 } else { v };
                }
                mini_meter(ui, app, &app.meters.bus[b], 90.0);
                ui.end_row();
            }
        });
    });
}
