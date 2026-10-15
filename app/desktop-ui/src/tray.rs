//! System tray icon and menu.
//!
//! Menu/icon events arrive on an OS thread; they are forwarded to a channel
//! and wake the UI with `request_repaint`, which also runs `App::logic`
//! while the window is hidden.

use crossbeam_channel::{Receiver, unbounded};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::i18n::Tr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayCommand {
    ToggleWindow,
    Show,
    ToggleMicMute,
    SafeMode,
    Profile(String),
    Quit,
}

enum Raw {
    Menu(MenuId),
    LeftClick,
}

pub struct Tray {
    icon: TrayIcon,
    rx: Receiver<Raw>,
    show: MenuItem,
    mute: CheckMenuItem,
    safe: CheckMenuItem,
    profiles: Submenu,
    profile_items: Vec<(CheckMenuItem, String)>,
    quit: MenuItem,
    last: Option<(String, bool, bool, Vec<String>, String, bool)>,
}

impl Tray {
    pub fn new(ctx: &egui::Context, tr: &Tr) -> Result<Self, String> {
        let menu = Menu::new();
        let show = MenuItem::new(tr.t("tray_hide"), true, None);
        let mute = CheckMenuItem::new(tr.t("tray_mute_mic"), true, false, None);
        let safe = CheckMenuItem::new(tr.t("safe_mode"), true, false, None);
        let profiles = Submenu::new(tr.t("profiles"), true);
        let quit = MenuItem::new(tr.t("tray_quit"), true, None);
        menu.append(&show).map_err(|e| e.to_string())?;
        menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
        menu.append(&mute).map_err(|e| e.to_string())?;
        menu.append(&safe).map_err(|e| e.to_string())?;
        menu.append(&profiles).map_err(|e| e.to_string())?;
        menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
        menu.append(&quit).map_err(|e| e.to_string())?;

        let icon = Icon::from_rgba(crate::icon::rgba(32), 32, 32).map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("NovaMix")
            .with_icon(icon)
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())?;

        let (tx, rx) = unbounded();
        let tx2 = tx.clone();
        let ctx1 = ctx.clone();
        let ctx2 = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.send(Raw::Menu(e.id));
            ctx1.request_repaint();
        }));
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                let _ = tx2.send(Raw::LeftClick);
                ctx2.request_repaint();
            }
        }));
        Ok(Self { icon: tray, rx, show, mute, safe, profiles, profile_items: Vec::new(), quit, last: None })
    }

    pub fn poll(&self) -> Vec<TrayCommand> {
        let mut out = Vec::new();
        while let Ok(raw) = self.rx.try_recv() {
            match raw {
                Raw::LeftClick => out.push(TrayCommand::Show),
                Raw::Menu(id) => {
                    if id == *self.show.id() {
                        out.push(TrayCommand::ToggleWindow);
                    } else if id == *self.mute.id() {
                        out.push(TrayCommand::ToggleMicMute);
                    } else if id == *self.safe.id() {
                        out.push(TrayCommand::SafeMode);
                    } else if id == *self.quit.id() {
                        out.push(TrayCommand::Quit);
                    } else if let Some((_, name)) = self.profile_items.iter().find(|(item, _)| *item.id() == id) {
                        out.push(TrayCommand::Profile(name.clone()));
                    }
                }
            }
        }
        out
    }

    /// Updates labels and check marks (only when something changed).
    pub fn refresh(&mut self, tr: &Tr, mic_muted: bool, safe: bool, profiles: &[String], active: &str, visible: bool) {
        let lang = format!("{:?}", tr.lang);
        let key = (lang, mic_muted, safe, profiles.to_vec(), active.to_string(), visible);
        if self.last.as_ref() == Some(&key) {
            return;
        }
        let lang_changed = self.last.as_ref().map(|l| l.0 != key.0).unwrap_or(true);
        let profiles_changed = self.last.as_ref().map(|l| l.3 != key.3 || l.4 != key.4).unwrap_or(true);
        self.show.set_text(if visible { tr.t("tray_hide") } else { tr.t("tray_show") });
        self.mute.set_checked(mic_muted);
        self.safe.set_checked(safe);
        if lang_changed {
            self.mute.set_text(tr.t("tray_mute_mic"));
            self.safe.set_text(tr.t("safe_mode"));
            self.profiles.set_text(tr.t("profiles"));
            self.quit.set_text(tr.t("tray_quit"));
        }
        if profiles_changed {
            for (item, _) in self.profile_items.drain(..) {
                let _ = self.profiles.remove(&item);
            }
            for p in profiles {
                let item = CheckMenuItem::new(p, true, p == active, None);
                let _ = self.profiles.append(&item);
                self.profile_items.push((item, p.clone()));
            }
        }
        let tip = format!("NovaMix — {}{}", active, if mic_muted { format!(" · {}", tr.t("mic_muted")) } else { String::new() });
        let _ = self.icon.set_tooltip(Some(tip));
        self.last = Some(key);
    }
}
