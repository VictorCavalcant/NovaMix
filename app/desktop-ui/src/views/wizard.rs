//! Setup assistant: microphone, output, virtual endpoints, voice preset.

use std::time::{Duration, Instant};

use egui::{RichText, vec2};
use novamix_config::{BusRole, Language, Scenario};
use novamix_mixer_graph::{ChannelId, describe_paths};

use crate::app::NovaMixApp;
use crate::views::{output_picker, source_picker};
use crate::widgets::level_meter;

#[derive(Default)]
pub struct WizardState {
    pub step: usize,
    pub beep_until: Option<Instant>,
}

const STEPS: usize = 6;

pub fn window(app: &mut NovaMixApp, ctx: &egui::Context) {
    if let Some(t) = app.wizard.beep_until
        && Instant::now() > t
    {
        app.wizard.beep_until = None;
        app.test_mode = None;
    }
    if !app.ui.show_wizard {
        return;
    }
    let mut open = true;
    egui::Window::new(format!("✨ {}", app.t("wizard_title")))
        .open(&mut open)
        .collapsible(false)
        .default_width(520.0)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(RichText::new(format!("{} / {STEPS}", app.wizard.step + 1)).small().color(app.pal.text_dim));
            ui.add_space(4.0);
            match app.wizard.step {
                0 => welcome(app, ui),
                1 => microphone(app, ui),
                2 => output(app, ui),
                3 => virtual_endpoints(app, ui),
                4 => preset(app, ui),
                _ => done(app, ui),
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if app.wizard.step > 0 && ui.button(app.t("back")).clicked() {
                    app.wizard.step -= 1;
                }
                if app.wizard.step + 1 < STEPS {
                    if ui.button(app.t("next")).clicked() {
                        app.wizard.step += 1;
                    }
                } else if ui.button(app.t("finish")).clicked() {
                    app.config.general.first_run_completed = true;
                    app.ui.show_wizard = false;
                    app.wizard.step = 0;
                    app.save_profile_now();
                }
            });
        });
    if !open {
        app.ui.show_wizard = false;
        app.config.general.first_run_completed = true;
    }
}

fn mic_index(app: &NovaMixApp) -> usize {
    app.mic_channels().first().copied().unwrap_or(0)
}

fn welcome(app: &mut NovaMixApp, ui: &mut egui::Ui) {
    ui.label(app.t("wizard_welcome"));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(app.t("language"));
        ui.selectable_value(&mut app.config.general.language, Language::PtBr, "Português (Brasil)");
        ui.selectable_value(&mut app.config.general.language, Language::En, "English");
    });
}

fn microphone(app: &mut NovaMixApp, ui: &mut egui::Ui) {
    let i = mic_index(app);
    ui.label(app.t("wizard_mic"));
    let mut src = app.profile.channels[i].source.clone();
    if source_picker(app, ui, "wiz_mic", &mut src, 360.0) {
        app.profile.channels[i].source = src;
        app.profile.channels[i].is_microphone = true;
    }
    ui.horizontal(|ui| {
        level_meter(ui, &app.pal, vec2(24.0, 120.0), &app.meters.ch_in[i], false);
        ui.vertical(|ui| {
            let peak = app.meters.ch_in[i].max_peak();
            let (text, color) = if peak < -60.0 {
                (app.t("wizard_level_none"), app.pal.danger)
            } else if peak < -30.0 {
                (app.t("wizard_level_low"), app.pal.warn)
            } else if peak > -3.0 {
                (app.t("wizard_level_high"), app.pal.danger)
            } else {
                (app.t("wizard_level_ok"), app.pal.ok)
            };
            ui.label(RichText::new(text).color(color).strong());
            let mut g = app.profile.channels[i].input_gain_db;
            if ui.add(egui::Slider::new(&mut g, -24.0..=24.0).text(app.t("input_gain")).suffix(" dB")).changed() {
                app.profile.channels[i].input_gain_db = g;
            }
            let st = app.status.channels.get(i).map(|s| s.state.clone()).unwrap_or(novamix_control::EndpointState::Off);
            crate::widgets::status_dot(ui, app.status_color(&st), &app.status_text(&st));
        });
    });
}

fn output(app: &mut NovaMixApp, ui: &mut egui::Ui) {
    ui.label(app.t("wizard_output"));
    if let Some(m) = app.profile.monitor_bus() {
        let mut out = app.profile.buses[m].output.clone();
        if output_picker(app, ui, "wiz_out", &mut out, 360.0) {
            app.profile.buses[m].output = out;
        }
        if ui.button(format!("🔔 {}", app.t("wizard_test_beep"))).clicked() {
            app.test_mode = Some(app.profile.channels.len() - 1);
            app.wizard.beep_until = Some(Instant::now() + Duration::from_secs(3));
        }
        level_meter(ui, &app.pal, vec2(160.0, 16.0), &app.meters.bus[m], true);
    }
}

fn virtual_endpoints(app: &mut NovaMixApp, ui: &mut egui::Ui) {
    ui.heading(app.t("wizard_virtual"));
    let vs = app.status.virtual_status.clone();
    match (&vs.mic_capture, &vs.playback) {
        (None, None) => {
            ui.label(RichText::new(app.t("wizard_virtual_missing")).color(app.pal.warn));
        }
        (mic, play) => {
            if let Some(m) = mic {
                ui.label(app.t("wizard_virtual_ok"));
                ui.label(RichText::new(format!("🎤 {}", m.name)).strong().color(app.pal.accent));
            }
            if let Some(p) = play {
                ui.add_space(6.0);
                ui.label(app.t("wizard_virtual_playback"));
                ui.label(RichText::new(format!("🔊 {}", p.name)).strong().color(app.pal.accent));
            }
        }
    }
    for n in &vs.notes {
        ui.label(RichText::new(format!("• {n}")).small().color(app.pal.text_dim));
    }
}

fn preset(app: &mut NovaMixApp, ui: &mut egui::Ui) {
    ui.label(app.t("wizard_preset"));
    let i = mic_index(app);
    for s in Scenario::ALL {
        let preset = match s {
            Scenario::Default => novamix_dsp::params::VoicePreset::Natural,
            Scenario::Meeting => novamix_dsp::params::VoicePreset::Call,
            Scenario::Gaming => novamix_dsp::params::VoicePreset::NoisyRoom,
            Scenario::Streaming => novamix_dsp::params::VoicePreset::Stream,
        };
        let selected = novamix_dsp::params::VoicePreset::detect(&app.profile.channels[i].chain) == Some(preset);
        if ui.selectable_label(selected, format!("{} — {}", app.t(s.key()), app.t(preset.key()))).clicked() {
            app.profile.channels[i].chain = preset.chain();
        }
    }
}

fn done(app: &mut NovaMixApp, ui: &mut egui::Ui) {
    ui.label(app.t("wizard_done"));
    let g = crate::views::routing::current_graph(app);
    for c in 0..app.profile.channels.len() {
        for p in describe_paths(&g, ChannelId(c as u8)) {
            ui.label(RichText::new(p).monospace().small());
        }
    }
    let monitor_mic = app.profile.microphone_channels().iter().any(|&c| {
        app.profile
            .buses
            .iter()
            .enumerate()
            .any(|(b, bus)| bus.role == BusRole::Monitor && app.profile.channels[c].sends.get(b).map(|s| s.enabled).unwrap_or(false))
    });
    if monitor_mic {
        ui.label(RichText::new(app.t("warn_mic_monitor")).color(app.pal.warn));
    }
}
