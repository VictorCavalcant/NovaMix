//! Side panel with the effect chain of the selected channel.

use egui::{RichText, Ui, vec2};
use novamix_control::ControlMsg;
use novamix_dsp::noise::NoiseReducer;
use novamix_dsp::params::*;

use crate::app::NovaMixApp;
use crate::widgets::{eq_curve, gr_bar, spectrum_plot, transfer_curve};

fn slider(ui: &mut Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>, label: &str, suffix: &str) -> bool {
    ui.add(egui::Slider::new(v, range).text(label).suffix(suffix)).changed()
}

fn log_slider(ui: &mut Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>, label: &str, suffix: &str) -> bool {
    ui.add(egui::Slider::new(v, range).logarithmic(true).text(label).suffix(suffix)).changed()
}

pub fn panel(app: &mut NovaMixApp, ui: &mut Ui) {
    let i = app.ui.selected_channel.min(app.profile.channels.len().saturating_sub(1));
    egui::Panel::right("effects_panel").resizable(true).default_size(380.0).min_size(320.0).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let name = app.profile.channels[i].name.clone();
            ui.heading(format!("{} {name}", app.t("effects_of")));
            header(app, ui, i);
            ui.separator();
            let advanced = app.config.general.advanced_mode;
            if !advanced {
                ui.label(RichText::new(app.t("simple_mode_hint")).small().color(app.pal.text_dim));
            }
            let order = app.profile.channels[i].chain.order;
            for (pos, kind) in order.iter().enumerate() {
                stage_section(app, ui, i, *kind, pos, advanced);
            }
            if app.config.general.show_spectrum && !app.config.general.power_saving {
                ui.separator();
                ui.label(RichText::new(app.t("spectrum")).strong());
                let w = ui.available_width();
                spectrum_plot(ui, &app.pal, &app.spectrum_bands, vec2(w, 90.0));
            }
        });
    });
}

fn header(app: &mut NovaMixApp, ui: &mut Ui, i: usize) {
    let chain = app.profile.channels[i].chain;
    ui.horizontal(|ui| {
        ui.label(app.t("preset"));
        let current = VoicePreset::detect(&chain);
        let text = current.map(|p| app.t(p.key())).unwrap_or(app.t("preset_custom"));
        egui::ComboBox::from_id_salt(("preset", i)).selected_text(text).show_ui(ui, |ui| {
            for p in VoicePreset::ALL {
                if ui.selectable_label(current == Some(p), app.t(p.key())).clicked() {
                    let bypass = app.profile.channels[i].chain.bypass;
                    app.profile.channels[i].chain = p.chain();
                    app.profile.channels[i].chain.bypass = bypass;
                }
            }
        });
        ui.menu_button(app.t("copy_chain_to"), |ui| {
            for j in 0..app.profile.channels.len() {
                if j != i && ui.button(app.profile.channels[j].name.clone()).clicked() {
                    app.profile.copy_chain(i, j);
                    ui.close();
                }
            }
        });
    });
    ui.horizontal(|ui| {
        let mut bypass = app.profile.channels[i].chain.bypass;
        if ui.toggle_value(&mut bypass, app.t("bypass_all")).on_hover_text(app.t("bypass_tip")).changed() {
            app.profile.channels[i].chain.bypass = bypass;
        }
        let ab_id = egui::Id::new(("ab", i));
        let (mut dry, mut lm) = ui.data(|d| d.get_temp::<(bool, bool)>(ab_id)).unwrap_or((false, true));
        let mut changed = ui.toggle_value(&mut dry, app.t("listen_original")).on_hover_text(app.t("ab_tip")).changed();
        changed |= ui.checkbox(&mut lm, app.t("loudness_match")).changed();
        if changed {
            ui.data_mut(|d| d.insert_temp(ab_id, (dry, lm)));
            app.controller.send(ControlMsg::AbListen { ch: i as u8, dry, loudness_match: lm });
        }
    });
    if let Some(m) = app.controller.shared().meters() {
        let lat = m.channels[i].latency_samples.load(std::sync::atomic::Ordering::Relaxed);
        let sr = app.status.sample_rate.max(1) as f32;
        let load = m.channels[i].dsp_load_pct.load();
        ui.label(
            RichText::new(format!("{}: {:.1} ms · {}: {:.1}%", app.t("latency"), lat as f32 * 1000.0 / sr, app.t("dsp_load"), load))
                .small()
                .color(app.pal.text_dim),
        );
    }
}

fn stage_section(app: &mut NovaMixApp, ui: &mut Ui, i: usize, kind: StageKind, pos: usize, advanced: bool) {
    let title = app.t(kind.key());
    let tip = match kind {
        StageKind::Hpf => app.t("hpf_tip"),
        StageKind::NoiseReduction => app.t("noise_reduction_tip"),
        StageKind::Gate => app.t("gate_tip"),
        StageKind::Expander => app.t("expander_tip"),
        StageKind::Agc => app.t("agc_tip"),
        StageKind::Eq => app.t("eq_tip"),
        StageKind::DeEsser => app.t("deesser_tip"),
        StageKind::Compressor => app.t("compressor_tip"),
        StageKind::OutputGain => app.t("output_gain_tip"),
        StageKind::Limiter => app.t("limiter_tip"),
    };
    let frame = egui::Frame::group(ui.style()).inner_margin(6);
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let mut on = app.profile.channels[i].chain.is_enabled(kind);
            if ui.checkbox(&mut on, RichText::new(title).strong()).on_hover_text(tip).changed() {
                app.profile.channels[i].chain.set_enabled(kind, on);
            }
            if advanced && !kind.is_pinned() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let last_movable = pos + 2 >= STAGE_COUNT;
                    ui.add_enabled_ui(!last_movable, |ui| {
                        if ui.small_button("▼").on_hover_text(app.t("move_down")).clicked() {
                            app.profile.channels[i].chain.move_stage(kind, 1);
                        }
                    });
                    ui.add_enabled_ui(pos > 0, |ui| {
                        if ui.small_button("▲").on_hover_text(app.t("move_up")).clicked() {
                            app.profile.channels[i].chain.move_stage(kind, -1);
                        }
                    });
                });
            }
        });
        let enabled = app.profile.channels[i].chain.is_enabled(kind);
        ui.add_enabled_ui(enabled, |ui| match kind {
            StageKind::Hpf => hpf(app, ui, i, advanced),
            StageKind::NoiseReduction => nr(app, ui, i, advanced),
            StageKind::Gate => gate(app, ui, i, advanced),
            StageKind::Expander => expander(app, ui, i, advanced),
            StageKind::Agc => agc(app, ui, i, advanced),
            StageKind::Eq => eq(app, ui, i, advanced),
            StageKind::DeEsser => deesser(app, ui, i, advanced),
            StageKind::Compressor => compressor(app, ui, i, advanced),
            StageKind::OutputGain => {
                let mut p = app.profile.channels[i].chain.output_gain;
                if slider(ui, &mut p.gain_db, -24.0..=24.0, app.t("gain"), " dB") {
                    app.profile.channels[i].chain.output_gain = p;
                }
            }
            StageKind::Limiter => limiter(app, ui, i, advanced),
        });
    });
}

fn hpf(app: &mut NovaMixApp, ui: &mut Ui, i: usize, advanced: bool) {
    let mut p = app.profile.channels[i].chain.hpf;
    let mut ch = log_slider(ui, &mut p.frequency_hz, 20.0..=500.0, app.t("frequency"), " Hz");
    if advanced {
        egui::ComboBox::from_id_salt(("hpf_slope", i)).selected_text(format!("{} dB/oct", p.slope.db_per_octave())).show_ui(ui, |ui| {
            for s in HpfSlope::ALL {
                ch |= ui.selectable_value(&mut p.slope, s, format!("{} dB/oct", s.db_per_octave())).changed();
            }
        });
    }
    if ch {
        app.profile.channels[i].chain.hpf = p;
    }
}

fn nr(app: &mut NovaMixApp, ui: &mut Ui, i: usize, advanced: bool) {
    let mut p = app.profile.channels[i].chain.noise_reduction;
    let mut ch = slider(ui, &mut p.strength, 0.0..=1.0, app.t("strength"), "");
    ui.horizontal(|ui| {
        ui.label(app.t("mode"));
        ch |= ui.selectable_value(&mut p.mode, NoiseReductionMode::Spectral, app.t("nr_spectral")).changed();
        if NoiseReducer::neural_compiled() {
            ch |= ui.selectable_value(&mut p.mode, NoiseReductionMode::Neural, app.t("nr_neural")).changed();
        }
    });
    if p.mode == NoiseReductionMode::Neural && app.status.sample_rate != 48_000 && app.status.sample_rate != 0 {
        ui.label(RichText::new(app.t("nr_neural_unavailable")).small().color(app.pal.warn));
    }
    if advanced {
        ch |= slider(ui, &mut p.max_reduction_db, 0.0..=40.0, app.t("max_reduction"), " dB");
        if p.mode == NoiseReductionMode::Spectral {
            ui.horizontal(|ui| {
                ui.label(app.t("quality"));
                for (q, k) in
                    [(NrQuality::LowLatency, "nr_low_latency"), (NrQuality::Balanced, "nr_balanced"), (NrQuality::HighQuality, "nr_high_quality")]
                {
                    ch |= ui.selectable_value(&mut p.quality, q, app.t(k)).changed();
                }
            });
        }
    }
    if let Some(m) = app.controller.shared().meters() {
        let c = &m.channels[i];
        if c.neural_active.load(std::sync::atomic::Ordering::Relaxed) {
            let vad = c.vad.load();
            ui.add(egui::ProgressBar::new(vad).text(format!("VAD {:.0}%", vad * 100.0)).desired_height(10.0));
        } else if p.enabled {
            gr_bar(ui, &app.pal, vec2(ui.available_width(), 10.0), c.nr_reduction_db.load(), "NR");
        }
    }
    if ch {
        app.profile.channels[i].chain.noise_reduction = p;
    }
}

fn gate(app: &mut NovaMixApp, ui: &mut Ui, i: usize, advanced: bool) {
    let mut p = app.profile.channels[i].chain.gate;
    let mut ch = slider(ui, &mut p.threshold_db, -90.0..=0.0, app.t("threshold"), " dB");
    if advanced {
        ch |= slider(ui, &mut p.hysteresis_db, 0.0..=20.0, app.t("hysteresis"), " dB");
        ch |= log_slider(ui, &mut p.attack_ms, 0.05..=50.0, app.t("attack"), " ms");
        ch |= slider(ui, &mut p.hold_ms, 0.0..=1000.0, app.t("hold"), " ms");
        ch |= log_slider(ui, &mut p.release_ms, 5.0..=2000.0, app.t("release"), " ms");
        ch |= slider(ui, &mut p.range_db, -90.0..=0.0, app.t("range"), " dB");
    }
    if let Some(m) = app.controller.shared().meters() {
        let c = &m.channels[i];
        let level = novamix_dsp::util::gain_to_db(c.input_peak[0].load().max(c.input_rms[0].load() * 1.414));
        let open = c.gate_open.load(std::sync::atomic::Ordering::Relaxed);
        ui.label(RichText::new(format!("{} · {:.0} dB", if open { app.t("gate_open") } else { app.t("gate_closed") }, level.max(-90.0))).small());
    }
    if ch {
        app.profile.channels[i].chain.gate = p;
    }
}

fn expander(app: &mut NovaMixApp, ui: &mut Ui, i: usize, advanced: bool) {
    let mut p = app.profile.channels[i].chain.expander;
    let mut ch = slider(ui, &mut p.threshold_db, -90.0..=0.0, app.t("threshold"), " dB");
    if advanced {
        ch |= slider(ui, &mut p.ratio, 1.0..=10.0, app.t("ratio"), ":1");
        ch |= log_slider(ui, &mut p.attack_ms, 0.1..=100.0, app.t("attack"), " ms");
        ch |= log_slider(ui, &mut p.release_ms, 5.0..=2000.0, app.t("release"), " ms");
        ch |= slider(ui, &mut p.range_db, -60.0..=0.0, app.t("range"), " dB");
        ch |= slider(ui, &mut p.knee_db, 0.0..=12.0, app.t("knee"), " dB");
    }
    if let Some(m) = app.controller.shared().meters() {
        gr_bar(ui, &app.pal, vec2(ui.available_width(), 10.0), m.channels[i].expander_gr_db.load(), "EXP");
    }
    if ch {
        app.profile.channels[i].chain.expander = p;
    }
}

fn agc(app: &mut NovaMixApp, ui: &mut Ui, i: usize, advanced: bool) {
    let mut p = app.profile.channels[i].chain.agc;
    let mut ch = slider(ui, &mut p.target_db, -36.0..=-6.0, app.t("target"), " dB");
    if advanced {
        ch |= slider(ui, &mut p.max_gain_db, 0.0..=30.0, app.t("max_gain"), " dB");
        ch |= slider(ui, &mut p.noise_floor_db, -80.0..=-30.0, app.t("noise_floor"), " dB");
        ui.horizontal(|ui| {
            ui.label(app.t("speed"));
            for (s, k) in [(AgcSpeed::Slow, "speed_slow"), (AgcSpeed::Normal, "speed_normal"), (AgcSpeed::Fast, "speed_fast")] {
                ch |= ui.selectable_value(&mut p.speed, s, app.t(k)).changed();
            }
        });
    }
    if let Some(m) = app.controller.shared().meters() {
        ui.label(RichText::new(format!("{}: {:+.1} dB", app.t("gain"), m.channels[i].agc_gain_db.load())).small());
    }
    if ch {
        app.profile.channels[i].chain.agc = p;
    }
}

fn band_kind_key(k: EqBandKind) -> &'static str {
    match k {
        EqBandKind::Peak => "band_peak",
        EqBandKind::LowShelf => "band_low_shelf",
        EqBandKind::HighShelf => "band_high_shelf",
        EqBandKind::LowPass => "band_low_pass",
        EqBandKind::HighPass => "band_high_pass",
        EqBandKind::Notch => "band_notch",
    }
}

fn eq(app: &mut NovaMixApp, ui: &mut Ui, i: usize, advanced: bool) {
    let mut p = app.profile.channels[i].chain.eq;
    let sr = if app.status.sample_rate > 0 { app.status.sample_rate as f32 } else { 48_000.0 };
    let w = ui.available_width();
    let mut ch = eq_curve(ui, &app.pal, &mut p, sr, vec2(w, 120.0)).is_some();
    if advanced {
        ui.horizontal(|ui| {
            ui.label(app.t("bands"));
            for n in [5u8, 10] {
                if ui.selectable_label(p.band_count == n, n.to_string()).clicked() {
                    p.set_band_count(n);
                    ch = true;
                }
            }
        });
        egui::Grid::new(("eq_grid", i)).num_columns(5).spacing([4.0, 2.0]).show(ui, |ui| {
            for b in 0..p.band_count as usize {
                let band = &mut p.bands[b];
                ch |= ui.checkbox(&mut band.enabled, format!("{}", b + 1)).changed();
                egui::ComboBox::from_id_salt(("band_kind", i, b)).width(80.0).selected_text(app.tr.t(band_kind_key(band.kind))).show_ui(ui, |ui| {
                    for k in EqBandKind::ALL {
                        ch |= ui.selectable_value(&mut band.kind, k, app.tr.t(band_kind_key(k))).changed();
                    }
                });
                let speed = band.frequency_hz * 0.01;
                ch |= ui
                    .add(egui::DragValue::new(&mut band.frequency_hz).range(20.0..=20_000.0).speed(speed).suffix(" Hz").fixed_decimals(0))
                    .changed();
                ui.add_enabled_ui(band.kind.uses_gain(), |ui| {
                    ch |= ui.add(egui::DragValue::new(&mut band.gain_db).range(-18.0..=18.0).speed(0.1).suffix(" dB").fixed_decimals(1)).changed();
                });
                ch |= ui.add(egui::DragValue::new(&mut band.q).range(0.1..=18.0).speed(0.01).prefix("Q ").fixed_decimals(2)).changed();
                ui.end_row();
            }
        });
    } else {
        // Simple mode: three broad tone controls mapped onto bands 1, 3/4 and 5.
        let mut low = p.bands[0].gain_db;
        let mut presence = p.bands[3].gain_db;
        let mut air = p.bands[4].gain_db;
        if slider(ui, &mut low, -12.0..=12.0, "Graves / Lows", " dB") {
            p.bands[0].gain_db = low;
            ch = true;
        }
        if slider(ui, &mut presence, -12.0..=12.0, "Presença / Presence", " dB") {
            p.bands[3].gain_db = presence;
            ch = true;
        }
        if slider(ui, &mut air, -12.0..=12.0, "Brilho / Air", " dB") {
            p.bands[4].gain_db = air;
            ch = true;
        }
    }
    if ch {
        app.profile.channels[i].chain.eq = p.sanitized();
    }
}

fn deesser(app: &mut NovaMixApp, ui: &mut Ui, i: usize, advanced: bool) {
    let mut p = app.profile.channels[i].chain.deesser;
    let mut ch = slider(ui, &mut p.threshold_db, -60.0..=0.0, app.t("threshold"), " dB");
    if advanced {
        ch |= log_slider(ui, &mut p.frequency_hz, 2000.0..=12_000.0, app.t("frequency"), " Hz");
        ch |= slider(ui, &mut p.q, 0.5..=6.0, app.t("q"), "");
        ch |= slider(ui, &mut p.max_reduction_db, 0.0..=24.0, app.t("max_reduction"), " dB");
    }
    if let Some(m) = app.controller.shared().meters() {
        gr_bar(ui, &app.pal, vec2(ui.available_width(), 10.0), m.channels[i].deesser_gr_db.load(), "DS");
    }
    if ch {
        app.profile.channels[i].chain.deesser = p;
    }
}

fn compressor(app: &mut NovaMixApp, ui: &mut Ui, i: usize, advanced: bool) {
    let mut p = app.profile.channels[i].chain.compressor;
    let mut ch = slider(ui, &mut p.threshold_db, -60.0..=0.0, app.t("threshold"), " dB");
    ch |= slider(ui, &mut p.ratio, 1.0..=20.0, app.t("ratio"), ":1");
    if advanced {
        ch |= log_slider(ui, &mut p.attack_ms, 0.1..=200.0, app.t("attack"), " ms");
        ch |= log_slider(ui, &mut p.release_ms, 5.0..=2000.0, app.t("release"), " ms");
        ch |= slider(ui, &mut p.knee_db, 0.0..=24.0, app.t("knee"), " dB");
        ch |= ui.checkbox(&mut p.auto_makeup, app.t("auto_makeup")).changed();
        ui.add_enabled_ui(!p.auto_makeup, |ui| {
            ch |= slider(ui, &mut p.makeup_db, -12.0..=24.0, app.t("makeup"), " dB");
        });
        ui.horizontal(|ui| {
            ui.label(app.t("detector"));
            ch |= ui.selectable_value(&mut p.detector, DetectorMode::Rms, "RMS").changed();
            ch |= ui.selectable_value(&mut p.detector, DetectorMode::Peak, "Peak").changed();
        });
    }
    let gr = app.controller.shared().meters().map(|m| m.channels[i].compressor_gr_db.load()).unwrap_or(0.0);
    ui.horizontal(|ui| {
        transfer_curve(ui, &app.pal, &p, vec2(110.0, 110.0), gr);
        ui.vertical(|ui| {
            gr_bar(ui, &app.pal, vec2(ui.available_width().min(160.0), 12.0), gr, app.tr.t("gr"));
            ui.label(RichText::new(format!("{} {:.1} dB", app.tr.t("makeup"), p.effective_makeup_db())).small());
        });
    });
    if ch {
        app.profile.channels[i].chain.compressor = p;
    }
}

fn limiter(app: &mut NovaMixApp, ui: &mut Ui, i: usize, advanced: bool) {
    let mut p = app.profile.channels[i].chain.limiter;
    let mut ch = slider(ui, &mut p.ceiling_db, -24.0..=0.0, app.t("ceiling"), " dB");
    if advanced {
        ch |= slider(ui, &mut p.lookahead_ms, 0.0..=10.0, app.t("lookahead"), " ms");
        ch |= log_slider(ui, &mut p.release_ms, 1.0..=1000.0, app.t("release"), " ms");
    }
    if let Some(m) = app.controller.shared().meters() {
        gr_bar(ui, &app.pal, vec2(ui.available_width(), 10.0), m.channels[i].limiter_gr_db.load(), "LIM");
    }
    if ch {
        app.profile.channels[i].chain.limiter = p;
    }
}
