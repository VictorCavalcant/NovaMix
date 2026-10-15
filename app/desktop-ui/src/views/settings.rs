//! Settings window.

use egui::{RichText, Ui};
use novamix_config::{
    BUFFER_SIZES, DeviceRef, HotkeyAction, HotkeyBinding, Language, LatencyMode, LogLevel, ResumeBehavior, SAMPLE_RATES, ThemePref,
};
use novamix_control::ControlMsg;
use novamix_device_api::Direction;
use novamix_diagnostics::Level;

use crate::app::{Confirm, NovaMixApp, SettingsTab};

pub fn window(app: &mut NovaMixApp, ctx: &egui::Context) {
    let mut open = app.ui.show_settings;
    egui::Window::new(format!("⚙ {}", app.t("settings"))).open(&mut open).default_width(560.0).resizable(true).show(ctx, |ui| {
        ui.horizontal(|ui| {
            for (tab, key) in [
                (SettingsTab::General, "general"),
                (SettingsTab::Devices, "devices_latency"),
                (SettingsTab::Voice, "voice_effects"),
                (SettingsTab::Hotkeys, "hotkeys"),
                (SettingsTab::Diagnostics, "diagnostics"),
            ] {
                ui.selectable_value(&mut app.ui.settings_tab, tab, app.tr.t(key));
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| match app.ui.settings_tab {
            SettingsTab::General => general(app, ui),
            SettingsTab::Devices => devices(app, ui),
            SettingsTab::Voice => voice(app, ui),
            SettingsTab::Hotkeys => hotkeys(app, ui),
            SettingsTab::Diagnostics => diagnostics(app, ui),
        });
    });
    app.ui.show_settings = open;
}

fn general(app: &mut NovaMixApp, ui: &mut Ui) {
    let tr = app.tr;
    let g = &mut app.config.general;
    egui::Grid::new("general_grid").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
        ui.label(tr.t("language"));
        ui.horizontal(|ui| {
            ui.selectable_value(&mut g.language, Language::PtBr, "Português (Brasil)");
            ui.selectable_value(&mut g.language, Language::En, "English");
        });
        ui.end_row();
        ui.label(tr.t("theme"));
        ui.horizontal(|ui| {
            ui.selectable_value(&mut g.theme, ThemePref::Dark, tr.t("theme_dark"));
            ui.selectable_value(&mut g.theme, ThemePref::Light, tr.t("theme_light"));
            ui.selectable_value(&mut g.theme, ThemePref::System, tr.t("theme_system"));
        });
        ui.end_row();
        ui.label(tr.t("ui_scale"));
        // Discrete steps: changing zoom while dragging a slider would move
        // the slider under the pointer.
        ui.horizontal_wrapped(|ui| {
            for s in [0.75f32, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0] {
                if ui.selectable_label((g.ui_scale - s).abs() < 0.01, format!("{:.0}%", s * 100.0)).clicked() {
                    g.ui_scale = s;
                }
            }
        });
        ui.end_row();
        ui.label(tr.t("resume"));
        egui::ComboBox::from_id_salt("resume")
            .selected_text(match g.resume {
                ResumeBehavior::RestoreProfile => tr.t("resume_restore"),
                ResumeBehavior::DefaultProfile => tr.t("resume_default"),
                ResumeBehavior::SafeMode => tr.t("resume_safe"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut g.resume, ResumeBehavior::RestoreProfile, tr.t("resume_restore"));
                ui.selectable_value(&mut g.resume, ResumeBehavior::DefaultProfile, tr.t("resume_default"));
                ui.selectable_value(&mut g.resume, ResumeBehavior::SafeMode, tr.t("resume_safe"));
            });
        ui.end_row();
    });
    ui.checkbox(&mut g.start_with_system, tr.t("start_with_system"));
    ui.checkbox(&mut g.start_minimized, tr.t("start_minimized"));
    ui.checkbox(&mut g.minimize_to_tray, tr.t("minimize_to_tray"));
    ui.checkbox(&mut g.close_to_tray, tr.t("close_to_tray"));
    ui.checkbox(&mut g.confirm_destructive, tr.t("confirm_destructive"));
    ui.checkbox(&mut g.show_tooltips, tr.t("show_tooltips"));
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button(tr.t("restore_defaults")).clicked() {
            app.ui.confirm = Some(Confirm::RestoreDefaults);
        }
        if ui.button(tr.t("create_backup")).clicked() {
            app.save_profile_now();
            match app.store.create_backup() {
                Ok(p) => app.toast(Level::Info, format!("{}: {}", tr.t("backup_created"), p.display())),
                Err(e) => app.toast(Level::Error, e.to_string()),
            }
            app.ui.backups = app.store.list_backups();
        }
        if ui.button(tr.t("open_data_folder")).clicked() {
            let _ = novamix_device_windows::reveal_in_file_manager(app.store.root());
        }
    });
    ui.collapsing(tr.t("backups"), |ui| {
        if app.ui.backups.is_empty() {
            app.ui.backups = app.store.list_backups();
        }
        for b in app.ui.backups.clone() {
            ui.horizontal(|ui| {
                ui.label(&b);
                if ui.button(tr.t("restore_backup")).clicked() {
                    app.ui.confirm = Some(Confirm::RestoreBackup(b.clone()));
                }
            });
        }
    });
}

fn devices(app: &mut NovaMixApp, ui: &mut Ui) {
    let tr = app.tr;
    let st = app.status.clone();
    let master = st.devices.iter().find(|d| match &st.clock {
        novamix_control::ClockSource::Device(n) => &d.name == n && d.direction == Direction::Output,
        _ => false,
    });
    let a = &mut app.config.audio;
    egui::Grid::new("devices_grid").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
        ui.label(tr.t("audio_host"));
        let hosts = novamix_device_api::available_hosts();
        egui::ComboBox::from_id_salt("host").selected_text(a.host.clone().unwrap_or_else(|| format!("{} ({})", tr.t("default"), st.host))).show_ui(
            ui,
            |ui| {
                ui.selectable_value(&mut a.host, None, tr.t("default"));
                for h in hosts {
                    ui.selectable_value(&mut a.host, Some(h.clone()), h);
                }
            },
        );
        ui.end_row();

        ui.label(tr.t("sample_rate"));
        ui.horizontal(|ui| {
            for r in SAMPLE_RATES {
                // Show only rates the master device reports (when known).
                let supported = master.map(|m| m.sample_rates.contains(&r)).unwrap_or(true);
                ui.add_enabled_ui(supported, |ui| {
                    ui.selectable_value(&mut a.preferred_sample_rate, r, format!("{:.1} kHz", r as f32 / 1000.0));
                });
            }
        });
        ui.end_row();

        ui.label(tr.t("latency_mode"));
        ui.horizontal(|ui| {
            for (m, k, tip) in [
                (LatencyMode::LowLatency, "lat_low", "lat_low_tip"),
                (LatencyMode::Balanced, "lat_balanced", "lat_balanced_tip"),
                (LatencyMode::Stability, "lat_stability", "lat_stability_tip"),
            ] {
                ui.selectable_value(&mut a.latency_mode, m, tr.t(k)).on_hover_text(tr.t(tip));
            }
        });
        ui.end_row();

        ui.label(tr.t("buffer_size"));
        let range = master.and_then(|m| m.buffer_range);
        egui::ComboBox::from_id_salt("buffer")
            .selected_text(match a.buffer_frames {
                None => format!("{} ({})", tr.t("buffer_auto"), a.latency_mode.buffer_frames()),
                Some(b) => format!("{b}"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut a.buffer_frames, None, tr.t("buffer_auto"));
                for b in BUFFER_SIZES {
                    let ok = range.map(|(lo, hi)| b >= lo && b <= hi).unwrap_or(true);
                    if ok {
                        ui.selectable_value(
                            &mut a.buffer_frames,
                            Some(b),
                            format!("{b} ({:.1} ms)", b as f32 * 1000.0 / a.preferred_sample_rate as f32),
                        );
                    }
                }
            });
        ui.end_row();
        ui.label("");
        ui.label(RichText::new(tr.t("buffer_unavailable")).small().color(app.pal.text_dim));
        ui.end_row();

        ui.label(tr.t("clock_bus")).on_hover_text(tr.t("clock_tip"));
        let bus_names: Vec<String> = app.profile.buses.iter().map(|b| b.name.clone()).collect();
        egui::ComboBox::from_id_salt("clock_bus")
            .selected_text(a.clock_bus.and_then(|b| bus_names.get(b as usize).cloned()).unwrap_or_else(|| tr.t("clock_auto").to_string()))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut a.clock_bus, None, tr.t("clock_auto"));
                for (i, n) in bus_names.iter().enumerate() {
                    ui.selectable_value(&mut a.clock_bus, Some(i as u8), n);
                }
            });
        ui.end_row();

        ui.label(tr.t("fallback_output"));
        let outputs: Vec<_> = st.devices.iter().filter(|d| d.direction == Direction::Output).cloned().collect();
        egui::ComboBox::from_id_salt("fallback")
            .selected_text(a.fallback_output.as_ref().map(|d| d.name.clone()).unwrap_or_else(|| tr.t("out_default").to_string()))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut a.fallback_output, None, tr.t("out_default"));
                for d in &outputs {
                    let v = Some(DeviceRef::new(&d.id, &d.name));
                    ui.selectable_value(&mut a.fallback_output, v, &d.name);
                }
            });
        ui.end_row();

        ui.label(tr.t("smoothing"));
        ui.add(egui::Slider::new(&mut a.smoothing_ms, 5.0..=200.0).suffix(" ms"));
        ui.end_row();
        ui.label(tr.t("reconnect_interval"));
        ui.add(egui::Slider::new(&mut a.reconnect_interval_ms, 500..=30_000).suffix(" ms"));
        ui.end_row();
    });
    ui.checkbox(&mut a.auto_reconnect, tr.t("auto_reconnect"));
    ui.separator();
    let g = &mut app.config.general;
    ui.checkbox(&mut g.power_saving, tr.t("power_saving"));
    ui.add_enabled_ui(!g.power_saving, |ui| {
        ui.add(egui::Slider::new(&mut g.meter_fps, 10..=120).text(tr.t("meter_fps")).suffix(" fps"));
        ui.checkbox(&mut g.show_spectrum, tr.t("show_spectrum"));
    });
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button(tr.t("rescan")).clicked() {
            app.controller.send(ControlMsg::RescanDevices);
        }
        if ui.button(tr.t("restart_engine")).clicked() {
            app.controller.send(ControlMsg::RestartEngine);
        }
        if ui.button(tr.t("open_sound_settings")).clicked() {
            let _ = novamix_device_windows::open_sound_settings();
        }
    });
    ui.collapsing(tr.t("negotiated"), |ui| {
        for (i, c) in st.channels.iter().enumerate() {
            if let Some(info) = &c.info {
                ui.label(format!(
                    "{}: {} — {} Hz, {} ch, {}, buffer {:?}",
                    app.profile.channels.get(i).map(|c| c.name.as_str()).unwrap_or("?"),
                    info.device_name,
                    info.sample_rate,
                    info.channels,
                    info.sample_format,
                    info.buffer_frames
                ));
            }
        }
        for (i, b) in st.buses.iter().enumerate() {
            if let Some(info) = &b.info {
                ui.label(format!(
                    "{}: {} — {} Hz, {} ch, {}, buffer {:?}",
                    app.profile.buses.get(i).map(|b| b.name.as_str()).unwrap_or("?"),
                    info.device_name,
                    info.sample_rate,
                    info.channels,
                    info.sample_format,
                    info.buffer_frames
                ));
            }
        }
    });
}

fn voice(app: &mut NovaMixApp, ui: &mut Ui) {
    let tr = app.tr;
    ui.checkbox(&mut app.config.general.advanced_mode, tr.t("advanced_mode"));
    ui.label(RichText::new(tr.t("simple_mode_hint")).small().color(app.pal.text_dim));
    ui.separator();
    ui.label(tr.t("neural_info"));
    if !novamix_dsp::noise::NoiseReducer::neural_compiled() {
        ui.label(RichText::new("(build without the `rnnoise` feature)").small());
    }
    ui.separator();
    if ui.button(format!("🎤 {}", tr.t("voice_tuning"))).clicked() {
        app.ui.show_voice_tune = true;
    }
}

fn hotkeys(app: &mut NovaMixApp, ui: &mut Ui) {
    let tr = app.tr;
    let hk = &mut app.config.hotkeys;
    ui.checkbox(&mut hk.enabled, tr.t("hotkeys_enable"));
    ui.label(RichText::new(tr.t("hotkeys_hint")).small().color(app.pal.text_dim));
    for e in &app.ui.hotkey_errors {
        ui.label(RichText::new(format!("⚠ {e}")).color(app.pal.danger).small());
    }
    let profiles = app.profile_names.clone();
    let buses: Vec<String> = app.profile.buses.iter().map(|b| b.name.clone()).collect();
    let mut remove = None;
    egui::Grid::new("hk_grid").num_columns(5).striped(true).show(ui, |ui| {
        for (i, b) in hk.bindings.iter_mut().enumerate() {
            ui.checkbox(&mut b.enabled, "");
            egui::ComboBox::from_id_salt(("hk_action", i)).width(170.0).selected_text(tr.t(b.action.key())).show_ui(ui, |ui| {
                for a in HotkeyAction::ALL {
                    ui.selectable_value(&mut b.action, a, tr.t(a.key()));
                }
            });
            let mut accel = b.accelerator.clone();
            let valid = accel.parse::<global_hotkey::hotkey::HotKey>().is_ok();
            let r = ui.add(egui::TextEdit::singleline(&mut accel).desired_width(130.0).text_color(if valid {
                ui.visuals().text_color()
            } else {
                app.pal.danger
            }));
            if r.changed() {
                b.accelerator = accel;
            }
            if !valid {
                r.on_hover_text(tr.t("hotkey_invalid"));
            }
            match b.action {
                HotkeyAction::ActivateProfile => {
                    egui::ComboBox::from_id_salt(("hk_profile", i)).width(110.0).selected_text(b.profile.clone().unwrap_or_default()).show_ui(
                        ui,
                        |ui| {
                            for p in &profiles {
                                ui.selectable_value(&mut b.profile, Some(p.clone()), p);
                            }
                        },
                    );
                }
                HotkeyAction::ToggleBusMute => {
                    egui::ComboBox::from_id_salt(("hk_bus", i))
                        .width(110.0)
                        .selected_text(b.bus.and_then(|x| buses.get(x as usize).cloned()).unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for (j, n) in buses.iter().enumerate() {
                                ui.selectable_value(&mut b.bus, Some(j as u8), n);
                            }
                        });
                }
                _ => {
                    ui.label("");
                }
            }
            if ui.small_button("🗑").clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = remove {
        hk.bindings.remove(i);
    }
    if ui.button(tr.t("add_hotkey")).clicked() {
        hk.bindings.push(HotkeyBinding {
            enabled: true,
            action: HotkeyAction::ToggleMicMute,
            accelerator: "Ctrl+Alt+F9".into(),
            bus: None,
            profile: None,
        });
    }
}

fn diagnostics(app: &mut NovaMixApp, ui: &mut Ui) {
    let tr = app.tr;
    let d = &mut app.config.diagnostics;
    ui.checkbox(&mut d.anonymize_reports, tr.t("anonymize_default"));
    ui.horizontal(|ui| {
        ui.label(tr.t("log_level"));
        for l in [LogLevel::Error, LogLevel::Warn, LogLevel::Info, LogLevel::Debug] {
            ui.selectable_value(&mut d.log_level, l, l.as_filter());
        }
    });
    if ui.button(tr.t("open_logs")).clicked() {
        let _ = novamix_device_windows::reveal_in_file_manager(&app.store.logs_dir());
    }
}
