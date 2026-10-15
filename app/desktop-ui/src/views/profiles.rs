//! Profile manager: load (with change preview), save, save as, rename,
//! duplicate, delete, import/export, templates.

use egui::RichText;
use novamix_config::{Scenario, diff_profiles, validate::sanitize_name};
use novamix_diagnostics::Level;

use crate::app::{Confirm, NovaMixApp};

pub fn window(app: &mut NovaMixApp, ctx: &egui::Context) {
    let mut open = app.ui.show_profiles;
    egui::Window::new(format!("📁 {}", app.t("profiles"))).open(&mut open).default_width(460.0).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(format!("{}:", app.t("active_profile")));
            ui.label(RichText::new(&app.profile.name).strong().color(app.pal.accent));
            if app.profile != app.saved_profile {
                ui.label(RichText::new("•").color(app.pal.warn));
            }
        });
        let mut desc = app.profile.description.clone();
        if ui.add(egui::TextEdit::multiline(&mut desc).desired_rows(2).desired_width(f32::INFINITY)).changed() {
            app.profile.description = desc.chars().take(500).collect();
        }
        ui.horizontal(|ui| {
            if ui.button(app.t("save")).clicked() {
                app.save_profile_now();
                app.toast(Level::Info, app.t("saved"));
            }
            if app.previous_profile.is_some() && ui.button(app.t("previous_profile")).clicked() {
                app.return_to_previous_profile();
            }
        });
        ui.horizontal(|ui| {
            let hint = app.t("profile_name");
            ui.add(egui::TextEdit::singleline(&mut app.ui.save_as_name).hint_text(hint).desired_width(180.0));
            if ui.button(app.t("save_as")).clicked() {
                let name = sanitize_name(&app.ui.save_as_name);
                if app.store.profile_exists(&name) {
                    app.toast(Level::Warning, app.t("unsaved_name_exists"));
                } else {
                    let mut p = app.profile.clone();
                    p.name = name;
                    match app.store.save_profile(&p) {
                        Ok(()) => {
                            app.profile_names = app.store.list_profiles();
                            app.activate_profile(p);
                            app.ui.save_as_name.clear();
                        }
                        Err(e) => app.toast(Level::Error, e.to_string()),
                    }
                }
            }
        });
        ui.separator();

        let names = app.profile_names.clone();
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            egui::Grid::new("profiles_grid").striped(true).num_columns(2).show(ui, |ui| {
                for name in &names {
                    let active = *name == app.profile.name;
                    ui.label(if active { RichText::new(format!("▶ {name}")).strong().color(app.pal.accent) } else { RichText::new(name) });
                    ui.horizontal(|ui| {
                        if !active && ui.button(app.t("load")).clicked() {
                            app.save_profile_now();
                            if let Ok(l) = app.store.load_profile(name) {
                                let changes = diff_profiles(&app.profile, &l.value);
                                app.ui.profile_preview = Some((l.value, changes));
                            }
                        }
                        if ui.button(app.t("rename")).clicked() {
                            app.ui.rename_profile = Some((name.clone(), name.clone()));
                        }
                        if ui.button(app.t("duplicate")).clicked()
                            && let Ok(l) = app.store.load_profile(name)
                        {
                            let mut p = l.value;
                            let mut n = format!("{name} (2)");
                            let mut k = 3;
                            while app.store.profile_exists(&n) {
                                n = format!("{name} ({k})");
                                k += 1;
                            }
                            p.name = n;
                            let _ = app.store.save_profile(&p);
                            app.profile_names = app.store.list_profiles();
                        }
                        if ui.button(app.t("export")).clicked()
                            && let Ok(l) = app.store.load_profile(name)
                            && let Some(path) = rfd::FileDialog::new()
                                .set_file_name(format!("{name}.toml"))
                                .add_filter("TOML", &["toml"])
                                .add_filter("JSON", &["json"])
                                .save_file()
                        {
                            match app.store.export_profile(&l.value, &path) {
                                Ok(()) => app.toast(Level::Info, app.t("export_ok")),
                                Err(e) => app.toast(Level::Error, e.to_string()),
                            }
                        }
                        if !active && ui.button(app.t("delete")).clicked() {
                            if app.config.general.confirm_destructive {
                                app.ui.confirm = Some(Confirm::DeleteProfile(name.clone()));
                            } else {
                                let _ = app.store.delete_profile(name);
                                app.profile_names = app.store.list_profiles();
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        });

        if let Some((old, mut new)) = app.ui.rename_profile.clone() {
            ui.horizontal(|ui| {
                ui.label(format!("{} '{old}':", app.t("rename")));
                ui.text_edit_singleline(&mut new);
                if ui.button(app.t("ok")).clicked() {
                    match app.store.rename_profile(&old, &new) {
                        Ok(p) => {
                            if app.profile.name == old {
                                app.profile.name = p.name.clone();
                                app.saved_profile = app.profile.clone();
                                app.config.active_profile = p.name;
                            }
                            app.profile_names = app.store.list_profiles();
                        }
                        Err(e) => app.toast(Level::Error, e.to_string()),
                    }
                    app.ui.rename_profile = None;
                } else if ui.button(app.t("cancel")).clicked() {
                    app.ui.rename_profile = None;
                } else {
                    app.ui.rename_profile = Some((old, new));
                }
            });
        }

        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label(app.t("new_from_template"));
            for s in Scenario::ALL {
                if ui.button(app.t(s.key())).clicked() {
                    app.save_profile_now();
                    app.new_profile_from_scenario(s);
                }
            }
        });
        if ui.button(app.t("import")).clicked()
            && let Some(path) = rfd::FileDialog::new().add_filter("NovaMix", &["toml", "json"]).pick_file()
        {
            match app.store.import_profile(&path) {
                Ok(l) => {
                    let mut p = l.value;
                    let base = p.name.clone();
                    let mut k = 2;
                    while app.store.profile_exists(&p.name) {
                        p.name = format!("{base} ({k})");
                        k += 1;
                    }
                    let _ = app.store.save_profile(&p);
                    app.profile_names = app.store.list_profiles();
                    let mut msg = format!("{}: {}", app.t("import_ok"), p.name);
                    if !l.issues.is_empty() {
                        msg.push_str(&format!(" ({} correções)", l.issues.len()));
                    }
                    app.toast(Level::Info, msg);
                }
                Err(e) => {
                    let msg = format!("{}: {e}", app.t("import_failed"));
                    app.toast(Level::Error, msg);
                }
            }
        }
    });
    app.ui.show_profiles = open;
}
