//! Diagnostics window: engine timing, streams, devices, events, export.

use egui::RichText;
use novamix_diagnostics::{Level, time};

use crate::app::NovaMixApp;

pub fn window(app: &mut NovaMixApp, ctx: &egui::Context) {
    let mut open = app.ui.show_diagnostics;
    egui::Window::new(format!("🔍 {}", app.t("diagnostics"))).open(&mut open).default_width(760.0).default_height(560.0).resizable(true).show(
        ctx,
        |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| content(app, ui));
        },
    );
    app.ui.show_diagnostics = open;
}

fn content(app: &mut NovaMixApp, ui: &mut egui::Ui) {
    let st = app.status.clone();
    let tr = app.tr;
    ui.heading(tr.t("engine"));
    let timing = app.controller.shared().meters().map(|m| m.timing.snapshot()).unwrap_or_default();
    egui::Grid::new("diag_engine").num_columns(2).striped(true).show(ui, |ui| {
        ui.label(tr.t("state"));
        ui.label(if st.running { tr.t("engine_running") } else { tr.t("engine_stopped") });
        ui.end_row();
        ui.label(tr.t("audio_host"));
        ui.label(&st.host);
        ui.end_row();
        ui.label(tr.t("sample_rate"));
        ui.label(format!(
            "{} Hz · {} frames ({:.2} ms)",
            st.sample_rate,
            st.block_frames,
            if st.sample_rate > 0 { st.block_frames as f32 * 1000.0 / st.sample_rate as f32 } else { 0.0 }
        ));
        ui.end_row();
        ui.label(tr.t("clock"));
        ui.label(format!("{:?}", st.clock));
        ui.end_row();
        ui.label(tr.t("cpu"));
        ui.label(format!(
            "{} {:.1}% · p50 {:.0}% · p95 {:.0}% · p99 {:.0}% · max {:.0}% · {} {}",
            tr.t("load_avg"),
            timing.avg_load_pct,
            timing.p50_load_pct,
            timing.p95_load_pct,
            timing.p99_load_pct,
            timing.max_load_pct,
            tr.t("over_budget"),
            timing.over_budget
        ));
        ui.end_row();
        ui.label(tr.t("latency_voice"));
        ui.label(st.voice_latency_ms.map(|l| format!("~{l:.1} ms")).unwrap_or_else(|| "—".into()));
        ui.end_row();
        ui.label(tr.t("restarts"));
        ui.label(st.engine_restarts.to_string());
        ui.end_row();
    });
    if ui.button(tr.t("reset_stats")).clicked()
        && let Some(m) = app.controller.shared().meters()
    {
        m.timing.reset();
    }

    ui.separator();
    ui.heading(tr.t("streams"));
    egui::Grid::new("diag_streams").num_columns(9).striped(true).show(ui, |ui| {
        for h in ["", "device", "state", "format", "fill", "drift", "underruns", "overruns", "latency"] {
            ui.label(RichText::new(if h.is_empty() { "" } else { tr.t(h) }).strong());
        }
        ui.end_row();
        let rows = st
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| (app.profile.channels.get(i).map(|x| x.name.clone()).unwrap_or_default(), c))
            .chain(st.buses.iter().enumerate().map(|(i, b)| (app.profile.buses.get(i).map(|x| format!("→ {}", x.name)).unwrap_or_default(), b)));
        for (name, c) in rows {
            ui.label(name);
            ui.label(c.device_name.clone().unwrap_or_else(|| "—".into()));
            ui.label(RichText::new(app.status_text(&c.state)).color(app.status_color(&c.state)));
            ui.label(c.info.as_ref().map(|i| format!("{} Hz {}ch {}", i.sample_rate, i.channels, i.sample_format)).unwrap_or_default());
            ui.label(c.stats.map(|s| format!("{}/{}", s.fill_frames, s.target_frames)).unwrap_or_default());
            ui.label(c.stats.map(|s| format!("{:+} ppm", s.correction_ppm)).unwrap_or_default());
            ui.label(c.stats.map(|s| s.underruns.to_string()).unwrap_or_default());
            ui.label(c.stats.map(|s| s.overruns.to_string()).unwrap_or_default());
            ui.label(if c.latency_ms > 0.0 { format!("{:.1} ms", c.latency_ms) } else { String::new() });
            ui.end_row();
        }
    });

    ui.separator();
    ui.heading(tr.t("virtual_endpoints"));
    let vs = &st.virtual_status;
    ui.label(if vs.driver_installed {
        tr.t("virtual_driver_ok")
    } else if vs.mic_feed.is_some() {
        tr.t("virtual_dev_fallback")
    } else {
        tr.t("virtual_missing")
    });
    for n in &vs.notes {
        ui.label(RichText::new(format!("• {n}")).small());
    }

    ui.collapsing(tr.t("devices"), |ui| {
        egui::Grid::new("diag_devices").num_columns(5).striped(true).show(ui, |ui| {
            for d in &st.devices {
                ui.label(format!("{}{}", if d.is_default { "★ " } else { "" }, d.name));
                ui.label(format!("{:?}", d.direction));
                ui.label(format!("{} Hz", d.default_sample_rate));
                ui.label(format!("{} ch", d.channels));
                ui.label(format!("{:?} / {}", d.form, d.interface));
                ui.end_row();
            }
        });
    });

    ui.separator();
    ui.heading(tr.t("test_mode"));
    ui.label(RichText::new(tr.t("test_mode_desc")).small());
    if app.test_mode.is_some() {
        if ui.button(tr.t("stop_test")).clicked() {
            app.test_mode = None;
        }
    } else if ui.button(tr.t("start_test")).clicked() {
        let ch = app.profile.channels.len().saturating_sub(1);
        app.test_mode = Some(ch);
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.heading(tr.t("event_log"));
        for (lvl, label) in [(None, "*"), (Some(Level::Warning), "⚠"), (Some(Level::Error), "⛔")] {
            ui.selectable_value(&mut app.ui.log_filter, lvl, label);
        }
        if ui.button(tr.t("clear")).clicked()
            && let Ok(mut l) = app.controller.shared().log.lock()
        {
            l.clear();
        }
    });
    let log = app.controller.shared().log_snapshot();
    egui::ScrollArea::vertical().id_salt("log_scroll").max_height(200.0).stick_to_bottom(true).show(ui, |ui| {
        for e in log.iter().filter(|e| app.ui.log_filter.map(|f| e.level >= f).unwrap_or(true)) {
            let color = match e.level {
                Level::Error => app.pal.danger,
                Level::Warning => app.pal.warn,
                Level::Info => app.pal.text_dim,
            };
            let repeat = if e.repeat > 1 { format!(" ×{}", e.repeat) } else { String::new() };
            ui.label(RichText::new(format!("{} [{}] {}{}", time::clock(e.unix_ms), e.category, e.message, repeat)).color(color).small().monospace());
        }
    });

    ui.separator();
    ui.label(RichText::new(tr.t("no_audio_in_report")).small());
    ui.horizontal(|ui| {
        ui.checkbox(&mut app.ui.diag_anonymize, tr.t("anonymize"));
        if ui.button(tr.t("export_report")).clicked() {
            let report =
                novamix_control::build_report(app.controller.shared(), &app.config, &app.profile, env!("CARGO_PKG_VERSION"), app.ui.diag_anonymize);
            let default = format!("novamix-diag-{}.json", novamix_config::store::utc_timestamp());
            if let Some(path) =
                rfd::FileDialog::new().set_directory(app.store.diagnostics_dir()).set_file_name(default).add_filter("JSON", &["json"]).save_file()
            {
                match novamix_config::store::write_atomic(&path, report.to_json().as_bytes()) {
                    Ok(()) => app.toast(Level::Info, format!("{}: {}", tr.t("report_saved"), path.display())),
                    Err(e) => app.toast(Level::Error, e.to_string()),
                }
            }
        }
    });
}
