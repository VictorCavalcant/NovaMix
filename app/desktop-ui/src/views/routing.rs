//! Routing matrix, feedback protection and signal path preview.

use egui::{RichText, Ui};
use novamix_control::{graph_input, resolve_output, resolve_source};
use novamix_mixer_graph::{
    BusId, ChannelId, GraphInput, RouteCycle, RouteWarning, SoloMode, analyze, describe_paths, suggest_fixes, would_create_cycle,
};

use crate::app::{Confirm, NovaMixApp};
use crate::views::{output_picker, source_picker};

/// Routing graph of the edited profile resolved against current devices.
pub fn current_graph(app: &NovaMixApp) -> GraphInput {
    let st = &app.status;
    let sources: Vec<_> = app
        .profile
        .channels
        .iter()
        .map(|c| if c.enabled { resolve_source(&c.source, &st.devices, &st.virtual_status) } else { novamix_control::ResolvedSource::None })
        .collect();
    let outputs: Vec<_> =
        app.profile.buses.iter().map(|b| resolve_output(&b.output, b.role, &st.devices, &st.virtual_status, &app.config.audio)).collect();
    graph_input(&app.profile, &sources, &outputs, &st.devices, &st.virtual_status)
}

/// Previews whether enabling a send would close a feedback loop.
pub fn send_would_cycle(app: &NovaMixApp, ch: usize, bus: usize) -> Option<RouteCycle> {
    let mut probe = app.profile.clone();
    if let Some(s) = probe.channels.get_mut(ch).and_then(|c| c.sends.get_mut(bus)) {
        s.enabled = false;
    }
    let st = &app.status;
    let sources: Vec<_> = probe
        .channels
        .iter()
        .map(|c| if c.enabled { resolve_source(&c.source, &st.devices, &st.virtual_status) } else { novamix_control::ResolvedSource::None })
        .collect();
    let outputs: Vec<_> = probe.buses.iter().map(|b| resolve_output(&b.output, b.role, &st.devices, &st.virtual_status, &app.config.audio)).collect();
    let g = graph_input(&probe, &sources, &outputs, &st.devices, &st.virtual_status);
    would_create_cycle(&g, (ChannelId(ch as u8), BusId(bus as u8)))
}

pub fn window(app: &mut NovaMixApp, ctx: &egui::Context) {
    let mut open = app.ui.show_routing;
    egui::Window::new(format!("🔀 {}", app.t("routing_matrix"))).open(&mut open).default_width(720.0).resizable(true).show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| content(app, ui));
    });
    app.ui.show_routing = open;
}

fn warning_text(app: &NovaMixApp, w: &RouteWarning) -> String {
    let ch = |c: &ChannelId| app.profile.channels.get(c.index()).map(|x| x.name.clone()).unwrap_or_default();
    let bus = |b: &BusId| app.profile.buses.get(b.index()).map(|x| x.name.clone()).unwrap_or_default();
    match w {
        RouteWarning::MicToSpeakers { channel, device, .. } => format!("{} — {} → {device}", app.t("warn_mic_speakers"), ch(channel)),
        RouteWarning::MicMonitoring { channel, bus: b } => format!("{} ({} → {})", app.t("warn_mic_monitor"), ch(channel), bus(b)),
        RouteWarning::BusWithoutOutput { bus: b } => format!("{}: {}", app.t("warn_bus_no_output"), bus(b)),
        RouteWarning::ChannelNotRouted { channel } => format!("{}: {}", app.t("warn_channel_not_routed"), ch(channel)),
    }
}

fn content(app: &mut NovaMixApp, ui: &mut Ui) {
    let graph = current_graph(app);
    let analysis = analyze(&graph);

    if !analysis.cycles.is_empty() {
        egui::Frame::group(ui.style()).stroke(egui::Stroke::new(1.0, app.pal.danger)).show(ui, |ui| {
            ui.label(RichText::new(format!("⚠ {}", app.t("route_cycle"))).strong().color(app.pal.danger));
            ui.label(app.t("route_cycle_desc"));
            for c in &analysis.cycles {
                ui.label(RichText::new(c.describe()).monospace());
            }
            if ui.button(app.t("fix_route")).clicked() {
                let fixes = suggest_fixes(&graph);
                app.ui.confirm = Some(Confirm::FixRoute(fixes));
            }
        });
    }
    for w in &analysis.warnings {
        let color = match w {
            RouteWarning::MicToSpeakers { .. } => app.pal.danger,
            RouteWarning::MicMonitoring { .. } => app.pal.warn,
            _ => app.pal.text_dim,
        };
        ui.label(RichText::new(format!("• {}", warning_text(app, w))).color(color));
    }
    ui.separator();
    ui.label(RichText::new(app.t("routing_hint")).small().color(app.pal.text_dim));

    let nb = app.profile.buses.len();
    egui::Grid::new("routing_grid").striped(true).num_columns(nb + 2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("");
        ui.label(RichText::new(app.t("source")).strong());
        for b in 0..nb {
            ui.vertical(|ui| {
                ui.label(RichText::new(&app.profile.buses[b].name).strong());
                let mut out = app.profile.buses[b].output.clone();
                if output_picker(app, ui, ("rout", b), &mut out, 120.0) {
                    app.profile.buses[b].output = out;
                }
            });
        }
        ui.end_row();
        for c in 0..app.profile.channels.len() {
            ui.label(RichText::new(&app.profile.channels[c].name).strong());
            let mut src = app.profile.channels[c].source.clone();
            if source_picker(app, ui, ("rsrc", c), &mut src, 150.0) {
                app.profile.channels[c].source = src;
            }
            for b in 0..nb {
                ui.vertical(|ui| {
                    let send = app.profile.channels[c].sends.get(b).copied().unwrap_or_default();
                    let mut on = send.enabled;
                    let blocked = analysis.cycles.iter().any(|cy| cy.sends.contains(&(ChannelId(c as u8), BusId(b as u8))));
                    let label = if blocked {
                        "⚠"
                    } else if on {
                        "●"
                    } else {
                        "○"
                    };
                    let r = ui.toggle_value(&mut on, label);
                    if r.changed() {
                        crate::views::mixer::request_send(app, c, b, on && !send.enabled, on);
                    }
                    if blocked {
                        r.on_hover_text(app.t("route_cycle_desc"));
                    }
                    ui.horizontal(|ui| {
                        let mut s = app.profile.channels[c].sends[b];
                        let mut changed =
                            ui.add(egui::DragValue::new(&mut s.level_db).range(-90.0..=12.0).speed(0.2).suffix(" dB").fixed_decimals(1)).changed();
                        changed |= ui.toggle_value(&mut s.pre_fader, app.tr.t("pre_fader")).on_hover_text(app.tr.t("pre_fader_tip")).changed();
                        if changed {
                            app.profile.channels[c].sends[b] = s;
                        }
                    });
                });
            }
            ui.end_row();
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        ui.label(app.t("solo_mode"));
        ui.selectable_value(&mut app.profile.solo_mode, SoloMode::MonitorOnly, app.tr.t("solo_monitor_only"));
        ui.selectable_value(&mut app.profile.solo_mode, SoloMode::AllBuses, app.tr.t("solo_all_buses"));
    });
    ui.separator();
    ui.label(RichText::new(app.t("signal_paths")).strong());
    for c in 0..app.profile.channels.len() {
        for p in describe_paths(&graph, ChannelId(c as u8)) {
            ui.label(RichText::new(p).monospace().small());
        }
    }
}
