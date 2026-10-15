//! Views: top bar, status bar, shared pickers and modal dialogs.

pub mod compact;
pub mod diagnostics;
pub mod effects;
pub mod mixer;
pub mod profiles;
pub mod routing;
pub mod settings;
pub mod voice_tune;
pub mod wizard;

use egui::{Align, Layout, RichText, Ui};
use novamix_config::{DeviceRef, OutputSpec, SourceSpec};
use novamix_control::ClockSource;
use novamix_device_api::{DeviceInfo, Direction};
use novamix_diagnostics::Level;
use novamix_dsp::signal::ToneKind;
use novamix_mixer_graph::RouteWarning;

use crate::app::{Confirm, NovaMixApp};

pub fn source_label(app: &NovaMixApp, s: &SourceSpec) -> String {
    match s {
        SourceSpec::None => app.t("src_none").into(),
        SourceSpec::DefaultInput => app.t("src_default_input").into(),
        SourceSpec::Input { device } => device.name.clone(),
        SourceSpec::Loopback { device } => format!("{} {}", app.t("loopback_of"), device.name),
        SourceSpec::DefaultOutputLoopback => app.t("src_default_loopback").into(),
        SourceSpec::VirtualPlayback => app.t("src_virtual_playback").into(),
        SourceSpec::TestTone { kind, .. } => format!("{} ({:?})", app.t("src_test_tone"), kind),
    }
}

pub fn output_label(app: &NovaMixApp, o: &OutputSpec) -> String {
    match o {
        OutputSpec::None => app.t("out_none").into(),
        OutputSpec::DefaultOutput => app.t("out_default").into(),
        OutputSpec::Device { device } => device.name.clone(),
        OutputSpec::VirtualMic => app.t("out_virtual_mic").into(),
    }
}

fn devices(app: &NovaMixApp, dir: Direction) -> Vec<DeviceInfo> {
    app.status.devices.iter().filter(|d| d.direction == dir).cloned().collect()
}

/// Source picker. Returns true when changed.
pub fn source_picker(app: &NovaMixApp, ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, source: &mut SourceSpec, width: f32) -> bool {
    let before = source.clone();
    let label = source_label(app, source);
    egui::ComboBox::from_id_salt(id).width(width).selected_text(RichText::new(label).small()).show_ui(ui, |ui| {
        ui.selectable_value(source, SourceSpec::None, app.t("src_none"));
        ui.selectable_value(source, SourceSpec::DefaultInput, app.t("src_default_input"));
        ui.selectable_value(source, SourceSpec::VirtualPlayback, app.t("src_virtual_playback"));
        ui.separator();
        ui.label(RichText::new(app.t("inputs")).small().strong());
        for d in devices(app, Direction::Input) {
            let v = SourceSpec::Input { device: DeviceRef::new(&d.id, &d.name) };
            let sel = matches!(source, SourceSpec::Input { device } if device.id == d.id);
            if ui.selectable_label(sel, &d.name).clicked() {
                *source = v;
            }
        }
        ui.separator();
        ui.label(RichText::new(app.t("src_loopback")).small().strong());
        ui.selectable_value(source, SourceSpec::DefaultOutputLoopback, app.t("src_default_loopback"));
        for d in devices(app, Direction::Output) {
            let sel = matches!(source, SourceSpec::Loopback { device } if device.id == d.id);
            if ui.selectable_label(sel, format!("{} {}", app.t("loopback_of"), d.name)).clicked() {
                *source = SourceSpec::Loopback { device: DeviceRef::new(&d.id, &d.name) };
            }
        }
        ui.separator();
        ui.label(RichText::new(app.t("src_test_tone")).small().strong());
        for (kind, key, f) in
            [(ToneKind::Beep, "test_tone_beep", 880.0), (ToneKind::Sine, "test_tone_sine", 440.0), (ToneKind::PinkNoise, "test_tone_pink", 440.0)]
        {
            let sel = matches!(source, SourceSpec::TestTone { kind: k, .. } if *k == kind);
            if ui.selectable_label(sel, app.t(key)).clicked() {
                *source = SourceSpec::TestTone { kind, frequency_hz: f, level_db: -20.0 };
            }
        }
    });
    *source != before
}

/// Output picker. Returns true when changed.
pub fn output_picker(app: &NovaMixApp, ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, output: &mut OutputSpec, width: f32) -> bool {
    let before = output.clone();
    egui::ComboBox::from_id_salt(id).width(width).selected_text(RichText::new(output_label(app, output)).small()).show_ui(ui, |ui| {
        ui.selectable_value(output, OutputSpec::None, app.t("out_none"));
        ui.selectable_value(output, OutputSpec::DefaultOutput, app.t("out_default"));
        ui.selectable_value(output, OutputSpec::VirtualMic, app.t("out_virtual_mic"));
        ui.separator();
        for d in devices(app, Direction::Output) {
            let sel = matches!(output, OutputSpec::Device { device } if device.id == d.id);
            if ui.selectable_label(sel, &d.name).clicked() {
                *output = OutputSpec::Device { device: DeviceRef::new(&d.id, &d.name) };
            }
        }
    });
    *output != before
}

pub fn top_bar(app: &mut NovaMixApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    egui::Panel::top("top_bar").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("NovaMix").size(18.0).strong().color(app.pal.accent));
            ui.separator();
            let st = &app.status;
            let (color, text) = if st.running {
                let clock = match &st.clock {
                    ClockSource::Device(d) => format!("{}: {}", app.t("clock"), d),
                    ClockSource::Internal => app.t("clock_internal").to_string(),
                    ClockSource::None => String::new(),
                };
                let block_ms = if st.sample_rate > 0 { st.block_frames as f32 * 1000.0 / st.sample_rate as f32 } else { 0.0 };
                (app.pal.ok, format!("{} · {:.1} kHz · {:.1} ms · {}", app.t("engine_running"), st.sample_rate as f32 / 1000.0, block_ms, clock))
            } else {
                (app.pal.danger, app.t("engine_stopped").to_string())
            };
            crate::widgets::status_dot(ui, color, &text);
            ui.separator();

            ui.label(app.t("profile"));
            let mut chosen: Option<String> = None;
            let dirty = app.profile != app.saved_profile;
            egui::ComboBox::from_id_salt("profile_combo")
                .selected_text(format!("{}{}", app.profile.name, if dirty { " •" } else { "" }))
                .width(150.0)
                .show_ui(ui, |ui| {
                    for name in &app.profile_names {
                        if ui.selectable_label(*name == app.profile.name, name).clicked() && *name != app.profile.name {
                            chosen = Some(name.clone());
                        }
                    }
                });
            if let Some(name) = chosen {
                app.save_profile_now();
                match app.store.load_profile(&name) {
                    Ok(l) => {
                        let changes = novamix_config::diff_profiles(&app.profile, &l.value);
                        app.ui.profile_preview = Some((l.value, changes));
                    }
                    Err(e) => app.toast(Level::Error, e.to_string()),
                }
            }
            if app.previous_profile.is_some() && ui.button(app.t("previous_profile")).on_hover_text(app.t("previous_profile_tip")).clicked() {
                app.return_to_previous_profile();
            }
            ui.add_enabled_ui(app.undo.can_undo(), |ui| {
                if ui.button(app.t("undo")).on_hover_text("Ctrl+Z").clicked()
                    && let Some(p) = app.undo.undo(&app.profile)
                {
                    app.profile = p;
                }
            });
            ui.add_enabled_ui(app.undo.can_redo(), |ui| {
                if ui.button(app.t("redo")).on_hover_text("Ctrl+Y").clicked()
                    && let Some(p) = app.undo.redo(&app.profile)
                {
                    app.profile = p;
                }
            });
            ui.separator();
            let safe_label = if app.safe_mode { app.t("safe_mode_on") } else { app.t("safe_mode") };
            let safe_btn = egui::Button::new(RichText::new(safe_label).strong().color(if app.safe_mode {
                egui::Color32::WHITE
            } else {
                ui.visuals().text_color()
            }))
            .fill(if app.safe_mode { app.pal.warn } else { ui.visuals().widgets.inactive.weak_bg_fill });
            if ui.add(safe_btn).on_hover_text(app.t("safe_mode_tip")).clicked() {
                app.toggle_safe_mode();
            }
            if app.meters.any_clip {
                let clip = egui::Button::new(RichText::new(app.t("clip_any")).strong().color(egui::Color32::WHITE)).fill(app.pal.danger);
                if ui.add(clip).on_hover_text(app.t("clip_tip")).clicked() {
                    app.meters.clip_until_ch = [0.0; novamix_mixer_graph::MAX_CHANNELS];
                    app.meters.clip_until_bus = [0.0; novamix_mixer_graph::MAX_BUSES];
                }
            }
        });
        // Second row: navigation.
        ui.horizontal_wrapped(|ui| {
            let warn = !app.status.route.cycles.is_empty();
            let routing_text = if warn {
                RichText::new(format!("⚠ {}", app.t("routing"))).color(app.pal.danger)
            } else {
                RichText::new(format!("🔀 {}", app.t("routing")))
            };
            if ui.selectable_label(app.ui.show_routing, routing_text).clicked() {
                app.ui.show_routing = !app.ui.show_routing;
            }
            if ui.selectable_label(app.ui.show_profiles, format!("📁 {}", app.t("profiles"))).clicked() {
                app.ui.show_profiles = !app.ui.show_profiles;
            }
            if ui.selectable_label(app.ui.show_voice_tune, format!("🎤 {}", app.t("voice_tuning"))).clicked() {
                app.ui.show_voice_tune = !app.ui.show_voice_tune;
            }
            if ui.selectable_label(app.ui.show_wizard, format!("✨ {}", app.t("setup_wizard"))).clicked() {
                app.ui.show_wizard = !app.ui.show_wizard;
            }
            if ui.selectable_label(app.ui.show_diagnostics, format!("🔍 {}", app.t("diagnostics"))).clicked() {
                app.ui.show_diagnostics = !app.ui.show_diagnostics;
            }
            if ui.selectable_label(app.ui.show_settings, format!("⚙ {}", app.t("settings"))).clicked() {
                app.ui.show_settings = !app.ui.show_settings;
            }
            if ui.button(format!("🗗 {}", app.t("compact_mode"))).clicked() {
                app.set_compact(&ctx, true);
            }
        });
        ui.add_space(2.0);
    });
}

pub fn status_bar(app: &mut NovaMixApp, ui: &mut Ui) {
    egui::Panel::bottom("status_bar").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let st = &app.status;
            if let Some(l) = st.voice_latency_ms {
                ui.label(format!("{}: ~{:.0} ms", app.t("latency_voice"), l)).on_hover_text("Microfone → efeitos → microfone virtual (estimativa)");
                ui.separator();
            }
            if let Some(m) = app.controller.shared().meters() {
                let t = m.timing.snapshot();
                let color = if t.p99_load_pct > 80.0 {
                    app.pal.danger
                } else if t.p99_load_pct > 50.0 {
                    app.pal.warn
                } else {
                    app.pal.text_dim
                };
                ui.label(RichText::new(format!("{}: {:.1}% (p99 {:.0}%)", app.t("cpu"), t.avg_load_pct, t.p99_load_pct)).color(color))
                    .on_hover_text("Tempo de processamento do motor em relação ao orçamento do bloco");
                ui.separator();
            }
            let underruns: u64 = st.channels.iter().chain(st.buses.iter()).filter_map(|c| c.stats.map(|s| s.underruns)).sum();
            ui.label(format!("{}: {underruns}", app.t("underruns")));
            ui.separator();
            let vs = &st.virtual_status;
            let (c, key) = if vs.driver_installed {
                (app.pal.ok, "virtual_driver_ok")
            } else if vs.mic_feed.is_some() {
                (app.pal.warn, "virtual_dev_fallback")
            } else {
                (app.pal.danger, "virtual_missing")
            };
            crate::widgets::status_dot(ui, c, app.t(key)).on_hover_text(vs.notes.join("\n"));
            if !st.route.cycles.is_empty() {
                ui.separator();
                if ui.link(RichText::new(format!("⚠ {}", app.t("feedback_blocked"))).color(app.pal.danger)).clicked() {
                    app.ui.show_routing = true;
                }
            }
            let warnings = st.route.warnings.iter().filter(|w| matches!(w, RouteWarning::MicToSpeakers { .. })).count();
            if warnings > 0 {
                ui.separator();
                if ui.link(RichText::new(format!("⚠ {}", app.t("warn_mic_speakers"))).color(app.pal.warn)).clicked() {
                    app.ui.show_routing = true;
                }
            }
            if let Some(e) = &st.last_error {
                ui.separator();
                ui.label(RichText::new(format!("⚠ {e}")).color(app.pal.danger).small());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(app.t("shortcuts_help")).small().color(app.pal.text_dim));
            });
        });
    });
}

/// Confirmation and information modals.
pub fn modals(app: &mut NovaMixApp, ctx: &egui::Context) {
    if let Some(confirm) = app.ui.confirm.clone() {
        let mut done = false;
        let modal = egui::Modal::new(egui::Id::new("confirm_modal")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            let message: String = match &confirm {
                Confirm::RestoreDefaults => app.t("restore_defaults_confirm").into(),
                Confirm::DeleteProfile(n) => format!("{}\n\n{n}", app.t("delete_profile_confirm")),
                Confirm::RestoreBackup(n) => format!("{}\n\n{n}", app.t("restore_backup_confirm")),
                Confirm::FixRoute(fixes) => {
                    let mut s = app.t("fix_route_confirm").to_string();
                    for (c, b) in fixes {
                        let cn = app.profile.channels.get(c.index()).map(|x| x.name.as_str()).unwrap_or("?");
                        let bn = app.profile.buses.get(b.index()).map(|x| x.name.as_str()).unwrap_or("?");
                        s.push_str(&format!("\n  • {cn} → {bn}"));
                    }
                    s
                }
                Confirm::EnableMicMonitoring { .. } => app.t("mic_monitoring_warn").into(),
                Confirm::RemoveChannel(i) => {
                    format!("{}: {}", app.t("remove_channel"), app.profile.channels.get(*i).map(|c| c.name.as_str()).unwrap_or("?"))
                }
                Confirm::RemoveBus(i) => format!("{}: {}", app.t("remove_bus"), app.profile.buses.get(*i).map(|c| c.name.as_str()).unwrap_or("?")),
            };
            ui.heading(app.t("confirm"));
            ui.label(message);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(app.t("confirm")).clicked() {
                    match &confirm {
                        Confirm::RestoreDefaults => {
                            let first_run = app.config.general.first_run_completed;
                            let active = app.config.active_profile.clone();
                            app.config = novamix_config::AppConfig::default();
                            app.config.general.first_run_completed = first_run;
                            app.config.active_profile = active;
                        }
                        Confirm::DeleteProfile(n) => {
                            if let Err(e) = app.store.delete_profile(n) {
                                app.toast(Level::Error, e.to_string());
                            }
                            app.profile_names = app.store.list_profiles();
                        }
                        Confirm::RestoreBackup(n) => match app.store.restore_backup(n) {
                            Ok(()) => {
                                let (cfg, _) = app.store.load_config();
                                app.config = cfg;
                                app.saved_config = app.config.clone();
                                app.profile_names = app.store.list_profiles();
                                let name = app.config.active_profile.clone();
                                app.activate_profile_by_name(&name);
                            }
                            Err(e) => app.toast(Level::Error, e.to_string()),
                        },
                        Confirm::FixRoute(fixes) => app.fix_routes(fixes),
                        Confirm::EnableMicMonitoring { ch, bus } => {
                            if let Some(s) = app.profile.channels.get_mut(*ch).and_then(|c| c.sends.get_mut(*bus)) {
                                s.enabled = true;
                            }
                        }
                        Confirm::RemoveChannel(i) => {
                            app.profile.remove_channel(*i);
                            app.ui.selected_channel = app.ui.selected_channel.min(app.profile.channels.len() - 1);
                        }
                        Confirm::RemoveBus(i) => {
                            app.profile.remove_bus(*i);
                        }
                    }
                    done = true;
                }
                if ui.button(app.t("cancel")).clicked() {
                    done = true;
                }
            });
        });
        if done || modal.should_close() {
            app.ui.confirm = None;
        }
    }

    if let Some((target, changes)) = app.ui.profile_preview.clone() {
        let mut done = false;
        let modal = egui::Modal::new(egui::Id::new("profile_preview")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.heading(format!("{} — {}", app.t("profile_changes"), target.name));
            if !target.description.is_empty() {
                ui.label(RichText::new(&target.description).italics());
            }
            ui.separator();
            if changes.is_empty() {
                ui.label(app.t("profile_no_changes"));
            } else {
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    egui::Grid::new("changes").striped(true).num_columns(4).show(ui, |ui| {
                        for c in &changes {
                            let scope = match &c.scope {
                                novamix_config::ChangeScope::Profile => app.t("profile_scope").to_string(),
                                novamix_config::ChangeScope::Channel(_, n) => format!("{} {n}", app.t("ch_scope")),
                                novamix_config::ChangeScope::Bus(_, n) => format!("{} {n}", app.t("bus_scope")),
                            };
                            ui.label(format!("{}{}", if c.affects_devices { "⟳ " } else { "" }, scope));
                            ui.label(RichText::new(c.field).strong());
                            ui.label(RichText::new(&c.from).color(app.pal.text_dim));
                            ui.label(format!("→ {}", c.to));
                            ui.end_row();
                        }
                    });
                });
                if changes.iter().any(|c| c.affects_devices) {
                    ui.label(RichText::new(app.t("profile_device_changes")).small());
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(app.t("load")).clicked() {
                    app.activate_profile(target.clone());
                    done = true;
                }
                if ui.button(app.t("cancel")).clicked() {
                    done = true;
                }
            });
        });
        if done || modal.should_close() {
            app.ui.profile_preview = None;
        }
    }

    if let Some(cycle) = app.ui.cycle_warning.clone() {
        let modal = egui::Modal::new(egui::Id::new("cycle_warning")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading(RichText::new(format!("⚠ {}", app.t("would_cycle"))).color(app.pal.danger));
            ui.label(app.t("would_cycle_desc"));
            ui.add_space(6.0);
            ui.label(RichText::new(cycle.describe()).monospace());
            ui.add_space(8.0);
            ui.button(app.t("ok")).clicked()
        });
        if modal.inner || modal.should_close() {
            app.ui.cycle_warning = None;
        }
    }

    if let Some(note) = app.ui.crash_notice.clone() {
        let modal = egui::Modal::new(egui::Id::new("crash_notice")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.label(app.t("crash_found"));
            ui.label(RichText::new(&note).monospace().small());
            ui.button(app.t("ok")).clicked()
        });
        if modal.inner || modal.should_close() {
            app.ui.crash_notice = None;
        }
    }
}
