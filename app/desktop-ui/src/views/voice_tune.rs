//! Assisted voice tuning: measures room noise and speech level (levels
//! only, nothing is recorded) and suggests gain and thresholds. Changes are
//! applied only after confirmation.

use std::time::{Duration, Instant};

use egui::RichText;

use crate::app::NovaMixApp;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Suggestion {
    pub noise_db: f32,
    pub speech_db: f32,
    pub peak_db: f32,
    pub gain_db: f32,
    pub gate_db: f32,
    pub expander_db: f32,
    pub compressor_db: f32,
}

#[derive(Default)]
pub enum Phase {
    #[default]
    Idle,
    Silence(Instant),
    Speech(Instant),
    Done(Suggestion),
}

#[derive(Default)]
pub struct VoiceTuneState {
    pub phase: Phase,
    pub channel: usize,
    noise: Vec<f32>,
    speech: Vec<f32>,
    peak: f32,
}

const SILENCE: Duration = Duration::from_secs(5);
const SPEECH: Duration = Duration::from_secs(8);

fn percentile(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return -90.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((p / 100.0) * (v.len() - 1) as f32) as usize]
}

/// Pure suggestion logic (unit tested).
pub fn suggest(current_gain: f32, noise_db: f32, speech_db: f32, peak_db: f32) -> Suggestion {
    // Aim for speech RMS around -20 dBFS while keeping peaks below -3 dBFS.
    let mut delta = -20.0 - speech_db;
    delta = delta.min(-3.0 - peak_db);
    let gain = (current_gain + delta).clamp(-24.0, 24.0);
    let applied = gain - current_gain;
    let noise = noise_db + applied;
    let speech = speech_db + applied;
    let gap = (speech - noise).max(0.0);
    let gate = (noise + (gap * 0.35).max(6.0)).min(speech - 6.0).clamp(-80.0, -10.0);
    let expander = (gate - 3.0).clamp(-80.0, -10.0);
    let comp = (speech + 2.0).clamp(-40.0, -6.0);
    Suggestion { noise_db, speech_db, peak_db, gain_db: gain, gate_db: gate, expander_db: expander, compressor_db: comp }
}

pub fn window(app: &mut NovaMixApp, ctx: &egui::Context) {
    if !app.ui.show_voice_tune {
        return;
    }
    // Sampling happens every frame while measuring.
    let now = Instant::now();
    let ch = app.voice_tune.channel.min(app.profile.channels.len().saturating_sub(1));
    let m = app.meters.ch_in[ch];
    let rms = m.rms[0].max(m.rms[1]);
    match app.voice_tune.phase {
        Phase::Silence(start) => {
            app.voice_tune.noise.push(rms);
            if now.duration_since(start) > SILENCE {
                app.voice_tune.phase = Phase::Speech(now);
                app.voice_tune.peak = -120.0;
            }
        }
        Phase::Speech(start) => {
            app.voice_tune.speech.push(rms);
            app.voice_tune.peak = app.voice_tune.peak.max(m.max_peak());
            if now.duration_since(start) > SPEECH {
                let noise = percentile(&mut app.voice_tune.noise, 50.0);
                let speech = percentile(&mut app.voice_tune.speech, 90.0);
                let s = suggest(app.profile.channels[ch].input_gain_db, noise, speech, app.voice_tune.peak);
                app.voice_tune.phase = Phase::Done(s);
            }
        }
        _ => {}
    }

    let mut open = true;
    egui::Window::new(format!("🎤 {}", app.t("vt_title"))).open(&mut open).default_width(440.0).show(ctx, |ui| {
        ui.label(app.t("vt_intro"));
        let mics = app.mic_channels();
        if mics.is_empty() {
            ui.label(RichText::new(app.t("vt_select_mic")).color(app.pal.warn));
            return;
        }
        ui.horizontal(|ui| {
            ui.label(app.t("mic_channel"));
            for &i in &mics {
                ui.selectable_value(&mut app.voice_tune.channel, i, app.profile.channels[i].name.clone());
            }
        });
        ui.separator();
        match &app.voice_tune.phase {
            Phase::Idle => {
                if ui.button(app.t("vt_start")).clicked() {
                    app.voice_tune.noise.clear();
                    app.voice_tune.speech.clear();
                    app.voice_tune.phase = Phase::Silence(Instant::now());
                }
            }
            Phase::Silence(start) => {
                let p = now.duration_since(*start).as_secs_f32() / SILENCE.as_secs_f32();
                ui.label(RichText::new(app.t("vt_silence")).strong().size(16.0));
                ui.add(egui::ProgressBar::new(p.min(1.0)));
            }
            Phase::Speech(start) => {
                let p = now.duration_since(*start).as_secs_f32() / SPEECH.as_secs_f32();
                ui.label(RichText::new(app.t("vt_speak")).strong().size(16.0).color(app.pal.accent));
                ui.add(egui::ProgressBar::new(p.min(1.0)));
            }
            Phase::Done(s) => {
                let s = *s;
                ui.label(RichText::new(app.t("vt_results")).strong());
                ui.label(format!(
                    "{}: {:.0} dBFS · {}: {:.0} dBFS · {}: {:.0} dBFS",
                    app.t("vt_noise"),
                    s.noise_db,
                    app.t("vt_voice"),
                    s.speech_db,
                    app.t("vt_peak"),
                    s.peak_db
                ));
                let c = &app.profile.channels[ch];
                egui::Grid::new("vt_grid").num_columns(3).striped(true).show(ui, |ui| {
                    ui.label("");
                    ui.label(RichText::new(app.t("vt_current")).strong());
                    ui.label(RichText::new(app.t("vt_suggested")).strong());
                    ui.end_row();
                    for (label, cur, sug) in [
                        (app.t("input_gain"), c.input_gain_db, s.gain_db),
                        (app.t("gate"), c.chain.gate.threshold_db, s.gate_db),
                        (app.t("expander"), c.chain.expander.threshold_db, s.expander_db),
                        (app.t("compressor"), c.chain.compressor.threshold_db, s.compressor_db),
                    ] {
                        ui.label(label);
                        ui.label(format!("{cur:+.1} dB"));
                        ui.label(RichText::new(format!("{sug:+.1} dB")).color(app.pal.accent));
                        ui.end_row();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(app.t("vt_apply")).clicked() {
                        let c = &mut app.profile.channels[ch];
                        c.input_gain_db = s.gain_db;
                        c.chain.gate.threshold_db = s.gate_db;
                        c.chain.expander.threshold_db = s.expander_db;
                        c.chain.compressor.threshold_db = s.compressor_db;
                        app.voice_tune.phase = Phase::Idle;
                    }
                    if ui.button(app.t("cancel")).clicked() {
                        app.voice_tune.phase = Phase::Idle;
                    }
                });
            }
        }
    });
    if !open {
        app.ui.show_voice_tune = false;
        app.voice_tune.phase = Phase::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_gain_towards_minus_20() {
        let s = suggest(0.0, -60.0, -30.0, -12.0);
        assert!((s.gain_db - 9.0).abs() < 1e-3, "peak limited: {}", s.gain_db);
        assert!(s.gate_db > -51.0 && s.gate_db < -21.0 - 6.0 + 1e-3, "{}", s.gate_db);
        assert!(s.compressor_db <= -6.0);
    }

    #[test]
    fn loud_input_reduces_gain() {
        let s = suggest(6.0, -50.0, -8.0, -1.0);
        assert!(s.gain_db < 0.0);
    }
}
