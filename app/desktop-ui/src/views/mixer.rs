//! The mixer: channel strips and bus strips.

use egui::{Align, Layout, RichText, Sense, Ui, UiBuilder, vec2};
use novamix_config::BusRole;
use novamix_dsp::params::VoicePreset;
use novamix_mixer_graph::{MAX_BUSES, MAX_CHANNELS};

use crate::app::{Confirm, NovaMixApp};
use crate::views::{output_picker, routing, source_picker};
use crate::widgets::{fader, fmt_db, gr_bar, level_meter, toggle};

const STRIP_W: f32 = 122.0;
const METER_H: f32 = 200.0;

pub fn central(app: &mut NovaMixApp, ui: &mut Ui) {
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for i in 0..app.profile.channels.len() {
                    channel_strip(app, ui, i);
                }
                if app.profile.channels.len() < MAX_CHANNELS && ui.button(app.t("add_channel")).clicked() {
                    let n = app.profile.channels.len() + 1;
                    if let Some(i) = app.profile.add_channel(&format!("Aux {}", n.saturating_sub(2).max(1))) {
                        app.ui.selected_channel = i;
                    }
                }
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);
                for i in 0..app.profile.buses.len() {
                    bus_strip(app, ui, i);
                }
                if app.profile.buses.len() < MAX_BUSES && ui.button(app.t("add_bus")).clicked() {
                    let n = app.profile.buses.len() + 1;
                    app.profile.add_bus(&format!("Bus {n}"), BusRole::Generic);
                }
            });
        });
    });
}

fn strip_frame(app: &NovaMixApp, selected: bool) -> egui::Frame {
    egui::Frame::new()
        .fill(if selected { app.pal.strip_selected } else { app.pal.strip })
        .stroke(egui::Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { app.pal.accent } else { app.pal.strip_border }))
        .corner_radius(6)
        .inner_margin(6)
}

fn short_name(name: &str) -> String {
    name.chars().take(4).collect::<String>().to_uppercase()
}

fn channel_strip(app: &mut NovaMixApp, ui: &mut Ui, i: usize) {
    let selected = app.ui.selected_channel == i;
    let frame = strip_frame(app, selected);
    // Background click sense is registered before the contents so the strip's own widgets keep their clicks.
    let resp = ui.scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
        frame.show(ui, |ui| {
            ui.set_width(STRIP_W);
            ui.vertical(|ui| {
                // Name (click selects, double-click renames).
                let renaming = app.ui.renaming_channel.as_ref().map(|(idx, _)| *idx == i).unwrap_or(false);
                if renaming {
                    let mut done = false;
                    if let Some((_, text)) = app.ui.renaming_channel.as_mut() {
                        let r = ui.add(egui::TextEdit::singleline(text).desired_width(STRIP_W - 8.0));
                        r.request_focus();
                        if r.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            done = true;
                        }
                    }
                    if done && let Some((_, text)) = app.ui.renaming_channel.take() {
                        let t = text.trim();
                        if !t.is_empty() {
                            app.profile.channels[i].name = t.chars().take(48).collect();
                        }
                    }
                } else {
                    ui.horizontal(|ui| {
                        let ch = &app.profile.channels[i];
                        let name = RichText::new(&ch.name).strong().color(if ch.enabled { app.pal.text } else { app.pal.text_dim });
                        let r = ui.add(egui::Label::new(name).sense(Sense::click())).on_hover_text(app.t("rename_channel"));
                        if r.clicked() {
                            app.ui.selected_channel = i;
                        }
                        if r.double_clicked() {
                            app.ui.renaming_channel = Some((i, app.profile.channels[i].name.clone()));
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let mut enabled = app.profile.channels[i].enabled;
                            if ui.checkbox(&mut enabled, "").on_hover_text(app.t("channel_enabled")).changed() {
                                app.profile.channels[i].enabled = enabled;
                            }
                            let mut mic = app.profile.channels[i].is_microphone;
                            if ui.selectable_label(mic, "🎤").on_hover_text(app.t("mic_channel")).clicked() {
                                mic = !mic;
                                app.profile.channels[i].is_microphone = mic;
                            }
                        });
                    });
                }
                // Source.
                let mut src = app.profile.channels[i].source.clone();
                if source_picker(app, ui, ("src", i), &mut src, STRIP_W - 6.0) {
                    app.profile.channels[i].source = src;
                }
                let st = app.status.channels.get(i).map(|s| s.state.clone()).unwrap_or(novamix_control::EndpointState::Off);
                let color = app.status_color(&st);
                let text = app.status_text(&st);
                let short: String = text.chars().take(18).collect();
                crate::widgets::status_dot(ui, color, &short).on_hover_text(text);

                // Input gain.
                let mut g = app.profile.channels[i].input_gain_db;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(app.t("input_gain")).small());
                    if ui
                        .add(egui::DragValue::new(&mut g).range(-24.0..=24.0).speed(0.1).suffix(" dB").fixed_decimals(1))
                        .on_hover_text(app.t("input_gain_tip"))
                        .changed()
                    {
                        app.profile.channels[i].input_gain_db = g;
                    }
                });

                // Meters and fader.
                ui.horizontal(|ui| {
                    let stereo = !app.profile.channels[i].mono;
                    level_meter(ui, &app.pal, vec2(10.0, METER_H), &app.meters.ch_in[i], false).on_hover_text("Entrada (antes dos efeitos)");
                    let r = level_meter(ui, &app.pal, vec2(22.0, METER_H), &app.meters.ch_out[i], stereo);
                    if r.clicked() {
                        app.meters.clip_until_ch[i] = 0.0;
                    }
                    let mut v = app.profile.channels[i].fader_db;
                    let accent = if app.profile.channels[i].mute { app.pal.mute } else { app.pal.accent };
                    if fader(ui, &app.pal, &mut v, vec2(56.0, METER_H), accent).on_hover_text(app.t("fader_tip")).changed() {
                        app.profile.channels[i].fader_db = v;
                    }
                });
                ui.label(RichText::new(format!("{} dB", fmt_db(app.profile.channels[i].fader_db))).monospace().small());
                if app.meters.source_clipped[i] && app.meters.ch_out[i].clip {
                    ui.label(RichText::new("⚠ CLIP").color(app.pal.danger).small().strong()).on_hover_text(app.t("source_clipped"));
                }

                // Pan.
                let mut pan = app.profile.channels[i].pan;
                let r = ui.add(egui::Slider::new(&mut pan, -1.0..=1.0).show_value(false).text(app.t("pan"))).on_hover_text(app.t("pan_tip"));
                if r.double_clicked() {
                    pan = 0.0;
                }
                if pan != app.profile.channels[i].pan {
                    app.profile.channels[i].pan = pan;
                }

                // Buttons.
                ui.horizontal(|ui| {
                    let s = vec2(34.0, 20.0);
                    let ch = &mut app.profile.channels[i];
                    toggle(ui, &mut ch.mute, app.tr.t("mute_short"), app.pal.mute, s).on_hover_text(app.tr.t("mute"));
                    toggle(ui, &mut ch.solo, app.tr.t("solo_short"), app.pal.solo, s).on_hover_text(app.tr.t("solo"));
                    toggle(ui, &mut ch.mono, app.tr.t("mono"), app.pal.cue, s).on_hover_text(app.tr.t("mono_tip"));
                });
                ui.horizontal(|ui| {
                    let ch = &mut app.profile.channels[i];
                    let n = ch.chain.enabled_count();
                    let mut fx_on = !ch.chain.bypass;
                    let label = format!("{} {}", app.tr.t("fx"), n);
                    if toggle(ui, &mut fx_on, &label, app.pal.accent, vec2(52.0, 20.0)).on_hover_text(app.tr.t("fx_tip")).changed() {
                        ch.chain.bypass = !fx_on;
                        app.ui.selected_channel = i;
                    }
                    if ui.small_button("⚙").on_hover_text(app.tr.t("effects")).clicked() {
                        app.ui.selected_channel = i;
                    }
                    let preset = VoicePreset::detect(&ch.chain).map(|p| app.tr.t(p.key())).unwrap_or(app.tr.t("preset_custom"));
                    ui.label(RichText::new(preset).small().color(app.pal.text_dim));
                });

                // Sends.
                ui.label(RichText::new(app.t("sends")).small().color(app.pal.text_dim));
                ui.horizontal_wrapped(|ui| {
                    for b in 0..app.profile.buses.len() {
                        let bus = &app.profile.buses[b];
                        let label = short_name(&bus.name);
                        let tip = format!("{} → {}", app.t("send_tip"), bus.name);
                        let mut on = app.profile.channels[i].sends.get(b).map(|s| s.enabled).unwrap_or(false);
                        let was = on;
                        if toggle(ui, &mut on, &label, app.pal.send, vec2(32.0, 18.0)).on_hover_text(tip).changed() {
                            request_send(app, i, b, on && !was, on);
                        }
                    }
                });

                // Dynamics indicators.
                if let Some(m) = app.controller.shared().meters() {
                    let cm = &m.channels[i];
                    let chain = &app.profile.channels[i].chain;
                    if chain.compressor.enabled {
                        gr_bar(ui, &app.pal, vec2(STRIP_W - 6.0, 10.0), cm.compressor_gr_db.load(), "COMP");
                    }
                    if chain.limiter.enabled {
                        gr_bar(ui, &app.pal, vec2(STRIP_W - 6.0, 10.0), cm.limiter_gr_db.load(), "LIM");
                    }
                    if chain.gate.enabled {
                        let open = cm.gate_open.load(std::sync::atomic::Ordering::Relaxed);
                        let (c, t) = if open { (app.pal.ok, app.t("gate_open")) } else { (app.pal.text_dim, app.t("gate_closed")) };
                        crate::widgets::status_dot(ui, c, t);
                    }
                    let load = cm.dsp_load_pct.load();
                    if load > 0.05 {
                        ui.label(RichText::new(format!("{}: {:.1}%", app.t("dsp_load"), load)).small().color(app.pal.text_dim));
                    }
                }
            });
        })
    });
    let r = resp.response;
    if r.clicked() {
        app.ui.selected_channel = i;
    }
    r.context_menu(|ui| {
        if ui.button(app.t("rename_channel")).clicked() {
            app.ui.renaming_channel = Some((i, app.profile.channels[i].name.clone()));
            ui.close();
        }
        ui.menu_button(app.t("copy_chain_to"), |ui| {
            for j in 0..app.profile.channels.len() {
                if j != i && ui.button(app.profile.channels[j].name.clone()).clicked() {
                    app.profile.copy_chain(i, j);
                    ui.close();
                }
            }
        });
        if app.profile.channels.len() > 1 && ui.button(app.t("remove_channel")).clicked() {
            app.ui.confirm = Some(Confirm::RemoveChannel(i));
            ui.close();
        }
    });
}

/// Handles a send toggle: previews feedback loops and confirms mic monitoring.
pub fn request_send(app: &mut NovaMixApp, ch: usize, bus: usize, enabling: bool, on: bool) {
    if !enabling {
        if let Some(s) = app.profile.channels[ch].sends.get_mut(bus) {
            s.enabled = on;
        }
        return;
    }
    if let Some(cycle) = routing::send_would_cycle(app, ch, bus) {
        app.ui.cycle_warning = Some(cycle);
        return;
    }
    let mic = app.profile.channels[ch].is_microphone;
    let monitor = app.profile.buses.get(bus).map(|b| b.role == BusRole::Monitor).unwrap_or(false);
    if mic && monitor && app.config.general.confirm_destructive {
        app.ui.confirm = Some(Confirm::EnableMicMonitoring { ch, bus });
        return;
    }
    if let Some(s) = app.profile.channels[ch].sends.get_mut(bus) {
        s.enabled = true;
    }
}

fn bus_strip(app: &mut NovaMixApp, ui: &mut Ui, i: usize) {
    let frame = strip_frame(app, false);
    let resp = ui.scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
        frame.show(ui, |ui| {
            ui.set_width(STRIP_W);
            ui.vertical(|ui| {
                let renaming = app.ui.renaming_bus.as_ref().map(|(idx, _)| *idx == i).unwrap_or(false);
                if renaming {
                    let mut done = false;
                    if let Some((_, text)) = app.ui.renaming_bus.as_mut() {
                        let r = ui.add(egui::TextEdit::singleline(text).desired_width(STRIP_W - 8.0));
                        r.request_focus();
                        done = r.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                    }
                    if done && let Some((_, text)) = app.ui.renaming_bus.take() {
                        let t = text.trim();
                        if !t.is_empty() {
                            app.profile.buses[i].name = t.chars().take(48).collect();
                        }
                    }
                } else {
                    let bus = &app.profile.buses[i];
                    let r = ui.add(egui::Label::new(RichText::new(&bus.name).strong()).sense(Sense::click()));
                    if r.double_clicked() {
                        app.ui.renaming_bus = Some((i, bus.name.clone()));
                    }
                    ui.label(RichText::new(app.t(bus.role.key())).small().color(app.pal.text_dim));
                }
                let mut out = app.profile.buses[i].output.clone();
                if output_picker(app, ui, ("out", i), &mut out, STRIP_W - 6.0) {
                    app.profile.buses[i].output = out;
                }
                let st = app.status.buses.get(i).map(|s| s.state.clone()).unwrap_or(novamix_control::EndpointState::Off);
                let text = app.status_text(&st);
                let short: String = text.chars().take(18).collect();
                crate::widgets::status_dot(ui, app.status_color(&st), &short).on_hover_text(text);
                ui.horizontal(|ui| {
                    let r = level_meter(ui, &app.pal, vec2(22.0, METER_H), &app.meters.bus[i], !app.profile.buses[i].mono);
                    if r.clicked() {
                        app.meters.clip_until_bus[i] = 0.0;
                    }
                    let mut v = app.profile.buses[i].fader_db;
                    let accent = if app.profile.buses[i].mute { app.pal.mute } else { app.pal.cue };
                    if fader(ui, &app.pal, &mut v, vec2(56.0, METER_H), accent).changed() {
                        app.profile.buses[i].fader_db = v;
                    }
                });
                ui.label(RichText::new(format!("{} dB", fmt_db(app.profile.buses[i].fader_db))).monospace().small());
                ui.horizontal(|ui| {
                    let s = vec2(34.0, 20.0);
                    let b = &mut app.profile.buses[i];
                    toggle(ui, &mut b.mute, app.tr.t("mute_short"), app.pal.mute, s).on_hover_text(app.tr.t("mute"));
                    toggle(ui, &mut b.mono, app.tr.t("mono"), app.pal.cue, s).on_hover_text(app.tr.t("mono_tip"));
                    if b.role != BusRole::Monitor {
                        toggle(ui, &mut b.cue, app.tr.t("cue"), app.pal.cue, s).on_hover_text(app.tr.t("cue_tip"));
                    }
                });
                ui.horizontal(|ui| {
                    let b = &mut app.profile.buses[i];
                    ui.label(RichText::new(app.tr.t("delay")).small());
                    ui.add(egui::DragValue::new(&mut b.delay_ms).range(0.0..=500.0).speed(1.0).suffix(" ms").fixed_decimals(0))
                        .on_hover_text(app.tr.t("delay_tip"));
                });
                ui.horizontal(|ui| {
                    let b = &mut app.profile.buses[i];
                    toggle(ui, &mut b.limiter.enabled, app.tr.t("limiter"), app.pal.accent, vec2(56.0, 18.0)).on_hover_text(app.tr.t("limiter_tip"));
                    ui.add(egui::DragValue::new(&mut b.limiter.ceiling_db).range(-24.0..=0.0).speed(0.1).suffix(" dB").fixed_decimals(1));
                });
                if app.profile.buses[i].limiter.enabled
                    && let Some(m) = app.controller.shared().meters()
                {
                    gr_bar(ui, &app.pal, vec2(STRIP_W - 6.0, 10.0), m.buses[i].limiter_gr_db.load(), "LIM");
                }
            });
        })
    });
    resp.response.context_menu(|ui| {
        ui.menu_button(app.t("role"), |ui| {
            for r in BusRole::ALL {
                if ui.selectable_label(app.profile.buses[i].role == r, app.t(r.key())).clicked() {
                    app.profile.buses[i].role = r;
                    ui.close();
                }
            }
        });
        if ui.button(app.t("rename")).clicked() {
            app.ui.renaming_bus = Some((i, app.profile.buses[i].name.clone()));
            ui.close();
        }
        if app.profile.buses.len() > 1 && ui.button(app.t("remove_bus")).clicked() {
            app.ui.confirm = Some(Confirm::RemoveBus(i));
            ui.close();
        }
    });
}
