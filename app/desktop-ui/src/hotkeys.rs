//! Optional global hotkeys (work while NovaMix is in the background).

use crossbeam_channel::{Receiver, unbounded};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use novamix_config::HotkeySettings;

pub struct HotkeyEvent {
    /// Index into `HotkeySettings::bindings`.
    pub binding: usize,
    pub pressed: bool,
}

pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<(HotKey, usize)>,
    rx: Receiver<GlobalHotKeyEvent>,
}

impl Hotkeys {
    pub fn new(ctx: &egui::Context) -> Self {
        let manager = GlobalHotKeyManager::new().map_err(|e| tracing::warn!("global hotkeys unavailable: {e}")).ok();
        let (tx, rx) = unbounded();
        let ctx = ctx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            let _ = tx.send(e);
            ctx.request_repaint();
        }));
        Self { manager, registered: Vec::new(), rx }
    }

    /// (Re)registers the bindings. Returns human-readable errors.
    pub fn apply(&mut self, settings: &HotkeySettings) -> Vec<String> {
        let mut errors = Vec::new();
        let Some(m) = &self.manager else {
            if settings.enabled {
                errors.push("global hotkeys are not available on this system".into());
            }
            return errors;
        };
        for (hk, _) in self.registered.drain(..) {
            let _ = m.unregister(hk);
        }
        if !settings.enabled {
            return errors;
        }
        for (i, b) in settings.bindings.iter().enumerate() {
            if !b.enabled || b.accelerator.trim().is_empty() {
                continue;
            }
            match b.accelerator.parse::<HotKey>() {
                Ok(hk) => {
                    if self.registered.iter().any(|(h, _)| h.id() == hk.id()) {
                        errors.push(format!("{}: duplicated", b.accelerator));
                        continue;
                    }
                    match m.register(hk) {
                        Ok(()) => self.registered.push((hk, i)),
                        Err(e) => errors.push(format!("{}: {e}", b.accelerator)),
                    }
                }
                Err(e) => errors.push(format!("{}: {e}", b.accelerator)),
            }
        }
        errors
    }

    pub fn poll(&self) -> Vec<HotkeyEvent> {
        let mut out = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            if let Some((_, idx)) = self.registered.iter().find(|(h, _)| h.id() == e.id) {
                out.push(HotkeyEvent { binding: *idx, pressed: e.state == HotKeyState::Pressed });
            }
        }
        out
    }
}
