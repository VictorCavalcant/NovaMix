//! Application state and frame orchestration.
//!
//! The UI edits `self.profile` directly. At the end of every frame the
//! profile is compared with what was last sent and, if different, sent to
//! the controller, which reconciles only the changed parameters. Safe mode
//! is an overlay: the edited profile is untouched, the engine receives a
//! derived "safe" version.

use std::time::{Duration, Instant};

use egui::{Color32, RichText};
use novamix_config::{
    AppConfig, ConfigStore, HotkeyAction, Profile, ProfileChange, Scenario, SourceSpec, default_profile, safe_mode_of, scenario_profile,
};
use novamix_control::{ControlMsg, ControllerHandle, EndpointState, EngineStatus};
use novamix_diagnostics::Level;
use novamix_mixer_graph::{BusId, ChannelId, MAX_BUSES, MAX_CHANNELS, RouteCycle};

use crate::hotkeys::{HotkeyEvent, Hotkeys};
use crate::i18n::Tr;
use crate::theme::{self, Palette};
use crate::tray::{Tray, TrayCommand};
use crate::widgets::MeterState;

/// Modal confirmations.
#[derive(Debug, Clone)]
pub enum Confirm {
    RestoreDefaults,
    DeleteProfile(String),
    RestoreBackup(String),
    FixRoute(Vec<(ChannelId, BusId)>),
    EnableMicMonitoring { ch: usize, bus: usize },
    RemoveChannel(usize),
    RemoveBus(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    General,
    Devices,
    Voice,
    Hotkeys,
    Diagnostics,
}

#[derive(Default)]
pub struct UiState {
    pub selected_channel: usize,
    pub show_routing: bool,
    pub show_profiles: bool,
    pub show_settings: bool,
    pub show_diagnostics: bool,
    pub show_wizard: bool,
    pub show_voice_tune: bool,
    pub settings_tab: SettingsTab,
    pub renaming_channel: Option<(usize, String)>,
    pub renaming_bus: Option<(usize, String)>,
    pub confirm: Option<Confirm>,
    pub profile_preview: Option<(Profile, Vec<ProfileChange>)>,
    pub save_as_name: String,
    pub rename_profile: Option<(String, String)>,
    pub cycle_warning: Option<RouteCycle>,
    pub diag_anonymize: bool,
    pub log_filter: Option<Level>,
    pub hotkey_errors: Vec<String>,
    pub backups: Vec<String>,
    pub crash_notice: Option<String>,
}

pub struct Toast {
    pub text: String,
    pub until: Instant,
    pub level: Level,
}

/// Undo/redo of profile edits, grouped into bursts.
#[derive(Default)]
pub struct UndoStack {
    undo: Vec<Profile>,
    redo: Vec<Profile>,
    burst_until: Option<Instant>,
}

impl UndoStack {
    const MAX: usize = 100;

    /// Called with the state *before* a change.
    pub fn record(&mut self, before: &Profile, now: Instant) {
        let new_burst = self.burst_until.map(|t| now > t).unwrap_or(true);
        if new_burst {
            self.undo.push(before.clone());
            if self.undo.len() > Self::MAX {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.burst_until = Some(now + Duration::from_millis(700));
    }

    pub fn undo(&mut self, current: &Profile) -> Option<Profile> {
        let p = self.undo.pop()?;
        self.redo.push(current.clone());
        self.burst_until = None;
        Some(p)
    }

    pub fn redo(&mut self, current: &Profile) -> Option<Profile> {
        let p = self.redo.pop()?;
        self.undo.push(current.clone());
        self.burst_until = None;
        Some(p)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

pub struct Meters {
    pub ch_in: [MeterState; MAX_CHANNELS],
    pub ch_out: [MeterState; MAX_CHANNELS],
    pub bus: [MeterState; MAX_BUSES],
    pub clip_until_ch: [f64; MAX_CHANNELS],
    pub clip_until_bus: [f64; MAX_BUSES],
    last_clips_ch: [u32; MAX_CHANNELS],
    last_clips_bus: [u32; MAX_BUSES],
    pub source_clipped: [bool; MAX_CHANNELS],
    pub any_clip: bool,
}

impl Default for Meters {
    fn default() -> Self {
        Self {
            ch_in: [MeterState::new(); MAX_CHANNELS],
            ch_out: [MeterState::new(); MAX_CHANNELS],
            bus: [MeterState::new(); MAX_BUSES],
            clip_until_ch: [0.0; MAX_CHANNELS],
            clip_until_bus: [0.0; MAX_BUSES],
            last_clips_ch: [0; MAX_CHANNELS],
            last_clips_bus: [0; MAX_BUSES],
            source_clipped: [false; MAX_CHANNELS],
            any_clip: false,
        }
    }
}

pub struct NovaMixApp {
    pub store: ConfigStore,
    pub config: AppConfig,
    pub saved_config: AppConfig,
    pub applied_audio: novamix_config::AudioSettings,
    pub profile: Profile,
    pub sent_profile: Profile,
    pub saved_profile: Profile,
    pub profile_names: Vec<String>,
    pub previous_profile: Option<Profile>,
    pub safe_mode: bool,
    pub controller: ControllerHandle,
    pub tr: Tr,
    pub pal: Palette,
    pub ui: UiState,
    pub meters: Meters,
    pub undo: UndoStack,
    pub toasts: Vec<Toast>,
    pub tray: Option<Tray>,
    pub hotkeys: Hotkeys,
    pub spectrum: Option<novamix_dsp::spectrum::SpectrumAnalyzer>,
    pub spectrum_bands: Vec<(f32, f32)>,
    pub tap_target: Option<usize>,
    pub status: EngineStatus,
    pub wizard: crate::views::wizard::WizardState,
    pub voice_tune: crate::views::voice_tune::VoiceTuneState,
    pub test_mode: Option<usize>,
    pub window_visible: bool,
    pub quit_requested: bool,
    pub start_hidden: bool,
    pub start: Instant,
    pub last_frame: Instant,
    pub last_change: Instant,
    pub autostart_applied: Option<bool>,
}

impl NovaMixApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        store: ConfigStore,
        config: AppConfig,
        profile: Profile,
        controller: ControllerHandle,
        start_hidden: bool,
        safe_mode: bool,
        crash_notice: Option<String>,
    ) -> Self {
        let tr = Tr::new(config.general.language);
        let pal = theme::apply(&cc.egui_ctx, config.general.theme, config.general.ui_scale);
        let tray = Tray::new(&cc.egui_ctx, &tr).map_err(|e| tracing::warn!("tray unavailable: {e}")).ok();
        let mut hotkeys = Hotkeys::new(&cc.egui_ctx);
        let errors = hotkeys.apply(&config.hotkeys);
        let profile_names = store.list_profiles();
        let mut ui = UiState { diag_anonymize: config.diagnostics.anonymize_reports, hotkey_errors: errors, crash_notice, ..Default::default() };
        ui.show_wizard = !config.general.first_run_completed;
        let now = Instant::now();
        Self {
            store,
            saved_config: config.clone(),
            applied_audio: config.audio.clone(),
            config,
            sent_profile: profile.clone(),
            saved_profile: profile.clone(),
            profile,
            profile_names,
            previous_profile: None,
            safe_mode,
            controller,
            tr,
            pal,
            ui,
            meters: Meters::default(),
            undo: UndoStack::default(),
            toasts: Vec::new(),
            tray,
            hotkeys,
            spectrum: None,
            spectrum_bands: Vec::new(),
            tap_target: None,
            status: EngineStatus::default(),
            wizard: Default::default(),
            voice_tune: Default::default(),
            test_mode: None,
            window_visible: true,
            quit_requested: false,
            start_hidden,
            start: now,
            last_frame: now,
            last_change: now,
            autostart_applied: None,
        }
    }

    pub fn t(&self, key: &'static str) -> &'static str {
        self.tr.t(key)
    }

    pub fn toast(&mut self, level: Level, text: impl Into<String>) {
        self.toasts.push(Toast { text: text.into(), until: Instant::now() + Duration::from_secs(4), level });
    }

    /// Profile actually sent to the engine (safe mode overlay applied).
    pub fn effective_profile(&self) -> Profile {
        let mut p = if self.safe_mode { safe_mode_of(&self.profile) } else { self.profile.clone() };
        if let Some(ch) = &self.test_mode
            && let Some(c) = p.channels.get_mut(*ch)
        {
            c.enabled = true;
            c.source = SourceSpec::TestTone { kind: novamix_dsp::signal::ToneKind::Beep, frequency_hz: 880.0, level_db: -20.0 };
            c.chain = novamix_dsp::params::VoicePreset::Raw.chain();
            c.mute = false;
            c.sends.iter_mut().for_each(|s| s.enabled = false);
            if let Some(m) = p.monitor_bus()
                && let Some(s) = p.channels[*ch].sends.get_mut(m)
            {
                s.enabled = true;
                s.level_db = 0.0;
            }
        }
        p
    }

    pub fn sync_profile(&mut self) {
        let effective = self.effective_profile();
        if effective != self.sent_profile {
            self.controller.apply_profile(&effective);
            self.sent_profile = effective;
        }
    }

    /// Records undo history and change time when the edited profile changed.
    fn track_edits(&mut self, before: &Profile) {
        if *before != self.profile {
            self.undo.record(before, Instant::now());
            self.last_change = Instant::now();
        }
    }

    pub fn activate_profile(&mut self, p: Profile) {
        let name = p.name.clone();
        self.previous_profile = Some(self.profile.clone());
        self.undo.record(&self.profile, Instant::now());
        self.profile = p;
        self.config.active_profile = name.clone();
        self.ui.selected_channel = self.ui.selected_channel.min(self.profile.channels.len().saturating_sub(1));
        self.last_change = Instant::now();
        self.saved_profile = self.profile.clone();
        let msg = format!("{}: {name}", self.t("profile_activated"));
        self.toast(Level::Info, msg);
    }

    pub fn activate_profile_by_name(&mut self, name: &str) {
        match self.store.load_profile(name) {
            Ok(l) => self.activate_profile(l.value),
            Err(e) => self.toast(Level::Error, e.to_string()),
        }
    }

    pub fn return_to_previous_profile(&mut self) {
        if let Some(prev) = self.previous_profile.take() {
            let current = std::mem::replace(&mut self.profile, prev);
            self.previous_profile = Some(current);
            self.config.active_profile = self.profile.name.clone();
            self.saved_profile = self.profile.clone();
            let msg = format!("{}: {}", self.t("profile_activated"), self.profile.name);
            self.toast(Level::Info, msg);
        }
    }

    pub fn cycle_profile(&mut self, delta: i32) {
        self.profile_names = self.store.list_profiles();
        if self.profile_names.is_empty() {
            return;
        }
        let cur = self.profile_names.iter().position(|n| *n == self.profile.name).unwrap_or(0) as i32;
        let n = self.profile_names.len() as i32;
        let next = ((cur + delta) % n + n) % n;
        let name = self.profile_names[next as usize].clone();
        self.save_profile_now();
        self.activate_profile_by_name(&name);
    }

    pub fn save_profile_now(&mut self) {
        if self.profile != self.saved_profile {
            match self.store.save_profile(&self.profile) {
                Ok(()) => {
                    self.saved_profile = self.profile.clone();
                    if !self.profile_names.contains(&self.profile.name) {
                        self.profile_names = self.store.list_profiles();
                    }
                }
                Err(e) => self.toast(Level::Error, e.to_string()),
            }
        }
    }

    fn save_config_now(&mut self) {
        if self.config != self.saved_config {
            if let Err(e) = self.store.save_config(&self.config) {
                self.toast(Level::Error, e.to_string());
            }
            self.saved_config = self.config.clone();
        }
    }

    pub fn mic_channels(&self) -> Vec<usize> {
        self.profile.microphone_channels()
    }

    pub fn mic_muted(&self) -> bool {
        let mics = self.mic_channels();
        !mics.is_empty() && mics.iter().all(|&i| self.profile.channels[i].mute)
    }

    pub fn set_mic_mute(&mut self, mute: bool) {
        for i in self.mic_channels() {
            self.profile.channels[i].mute = mute;
        }
    }

    pub fn toggle_mic_mute(&mut self) {
        let mute = !self.mic_muted();
        self.set_mic_mute(mute);
        let msg = if mute { self.t("mic_muted") } else { self.t("mic_live") };
        self.toast(Level::Info, msg);
    }

    pub fn toggle_safe_mode(&mut self) {
        self.safe_mode = !self.safe_mode;
        let msg = if self.safe_mode { self.t("safe_mode_on") } else { self.t("safe_mode") };
        self.toast(if self.safe_mode { Level::Warning } else { Level::Info }, msg);
    }

    pub fn show_window(&mut self, ctx: &egui::Context, visible: bool) {
        self.window_visible = visible;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(visible));
        if visible {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    pub fn set_compact(&mut self, ctx: &egui::Context, compact: bool) {
        self.config.general.compact_mode = compact;
        let size = if compact {
            let rows = self.profile.channels.len() + self.profile.buses.len();
            egui::vec2(460.0, 110.0 + rows as f32 * 34.0)
        } else {
            egui::vec2(1280.0, 780.0)
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }

    pub fn fix_routes(&mut self, fixes: &[(ChannelId, BusId)]) {
        for (ChannelId(c), BusId(b)) in fixes {
            if let Some(s) = self.profile.channels.get_mut(*c as usize).and_then(|ch| ch.sends.get_mut(*b as usize)) {
                s.enabled = false;
            }
        }
    }

    pub fn new_profile_from_scenario(&mut self, s: Scenario) {
        let mut p = scenario_profile(s);
        let mut name = p.name.clone();
        let mut i = 2;
        while self.store.profile_exists(&name) {
            name = format!("{} {i}", p.name);
            i += 1;
        }
        p.name = name;
        if let Err(e) = self.store.save_profile(&p) {
            self.toast(Level::Error, e.to_string());
        }
        self.profile_names = self.store.list_profiles();
        self.activate_profile(p);
    }

    // -----------------------------------------------------------------------
    // Per-frame logic (also runs while the window is hidden)

    fn update_meters(&mut self, now: f64, dt: f32) {
        let Some(m) = self.controller.shared().meters() else { return };
        use novamix_audio_core::StripMeter as S;
        use std::sync::atomic::Ordering;
        let mut any_clip = false;
        for i in 0..self.profile.channels.len().min(MAX_CHANNELS) {
            let c = &m.channels[i];
            let pin = [S::take_peak(&c.input_peak[0]), S::take_peak(&c.input_peak[1])];
            let rin = [c.input_rms[0].load(), c.input_rms[1].load()];
            self.meters.ch_in[i].update(now, dt, pin, rin);
            let pout = [S::take_peak(&c.output_peak[0]), S::take_peak(&c.output_peak[1])];
            let rout = [c.output_rms[0].load(), c.output_rms[1].load()];
            self.meters.ch_out[i].update(now, dt, pout, rout);
            let clips = c.source_clips.load(Ordering::Relaxed) + c.output_clips.load(Ordering::Relaxed);
            if clips != self.meters.last_clips_ch[i] {
                self.meters.last_clips_ch[i] = clips;
                self.meters.clip_until_ch[i] = now + 3.0;
                self.meters.source_clipped[i] = c.source_clips.load(Ordering::Relaxed) > 0;
            }
            let clip = now < self.meters.clip_until_ch[i];
            self.meters.ch_out[i].clip = clip;
            any_clip |= clip;
        }
        for i in 0..self.profile.buses.len().min(MAX_BUSES) {
            let b = &m.buses[i];
            let pk = [S::take_peak(&b.peak[0]), S::take_peak(&b.peak[1])];
            let rms = [b.rms[0].load(), b.rms[1].load()];
            self.meters.bus[i].update(now, dt, pk, rms);
            let clips = b.clips.load(Ordering::Relaxed);
            if clips != self.meters.last_clips_bus[i] {
                self.meters.last_clips_bus[i] = clips;
                self.meters.clip_until_bus[i] = now + 3.0;
            }
            let clip = now < self.meters.clip_until_bus[i];
            self.meters.bus[i].clip = clip;
            any_clip |= clip;
        }
        self.meters.any_clip = any_clip;
    }

    fn update_spectrum(&mut self) {
        let want = self.config.general.show_spectrum && !self.config.general.power_saving && self.window_visible && !self.config.general.compact_mode;
        let target = if want { Some(self.ui.selected_channel) } else { None };
        if target != self.tap_target {
            self.tap_target = target;
            self.controller.send(ControlMsg::SetTap(target.map(|c| novamix_audio_core::TapPoint::ChannelPost(c as u8))));
            if target.is_some() && self.spectrum.is_none() {
                self.spectrum = Some(novamix_dsp::spectrum::SpectrumAnalyzer::new(2048));
            }
        }
        let mut samples = Vec::new();
        self.controller.shared().with_tap(|c| {
            let n = c.slots();
            if let Ok(chunk) = c.read_chunk(n) {
                let (a, b) = chunk.as_slices();
                if target.is_some() {
                    samples.extend_from_slice(a);
                    samples.extend_from_slice(b);
                }
                chunk.commit_all();
            }
        });
        if let (Some(an), true) = (self.spectrum.as_mut(), target.is_some()) {
            an.push(&samples);
            an.compute(0.85);
            let sr = self.status.sample_rate.max(8000) as f32;
            self.spectrum_bands = an.log_bands(sr, 64, 30.0, 18_000.0);
        }
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        let cmds: Vec<TrayCommand> = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        for c in cmds {
            match c {
                TrayCommand::ToggleWindow => {
                    let v = !self.window_visible;
                    self.show_window(ctx, v);
                }
                TrayCommand::Show => self.show_window(ctx, true),
                TrayCommand::ToggleMicMute => self.toggle_mic_mute(),
                TrayCommand::SafeMode => self.toggle_safe_mode(),
                TrayCommand::Profile(name) => {
                    self.save_profile_now();
                    self.activate_profile_by_name(&name);
                }
                TrayCommand::Quit => {
                    self.quit_requested = true;
                    self.show_window(ctx, true);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        let (tr, muted, safe, visible) = (self.tr, self.mic_muted(), self.safe_mode, self.window_visible);
        if let Some(t) = self.tray.as_mut() {
            t.refresh(&tr, muted, safe, &self.profile_names, &self.profile.name, visible);
        }
    }

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        for ev in self.hotkeys.poll() {
            let HotkeyEvent { binding, pressed } = ev;
            let Some(b) = self.config.hotkeys.bindings.get(binding).cloned() else { continue };
            match (b.action, pressed) {
                (HotkeyAction::ToggleMicMute, true) => self.toggle_mic_mute(),
                (HotkeyAction::PushToTalk, p) => self.set_mic_mute(!p),
                (HotkeyAction::PushToMute, p) => self.set_mic_mute(p),
                (HotkeyAction::NextProfile, true) => self.cycle_profile(1),
                (HotkeyAction::PreviousProfile, true) => self.cycle_profile(-1),
                (HotkeyAction::ActivateProfile, true) => {
                    if let Some(name) = b.profile.clone() {
                        self.save_profile_now();
                        self.activate_profile_by_name(&name);
                    }
                }
                (HotkeyAction::ToggleBusMute, true) => {
                    if let Some(bus) = b.bus.and_then(|i| self.profile.buses.get_mut(i as usize)) {
                        bus.mute = !bus.mute;
                    }
                }
                (HotkeyAction::SafeMode, true) => self.toggle_safe_mode(),
                (HotkeyAction::ToggleWindow, true) => {
                    let v = !self.window_visible;
                    self.show_window(ctx, v);
                }
                _ => {}
            }
        }
    }

    fn handle_window_events(&mut self, ctx: &egui::Context) {
        let (close, minimized) = ctx.input(|i| (i.viewport().close_requested(), i.viewport().minimized.unwrap_or(false)));
        if close && !self.quit_requested && self.config.general.close_to_tray && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.show_window(ctx, false);
        }
        if minimized && self.window_visible && self.config.general.minimize_to_tray && self.tray.is_some() {
            self.show_window(ctx, false);
        }
        if self.start_hidden {
            self.start_hidden = false;
            if self.tray.is_some() {
                self.show_window(ctx, false);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }
    }

    fn persist(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_change) > Duration::from_millis(1500) {
            self.save_profile_now();
        }
        if self.config != self.saved_config {
            // Apply audio settings immediately, save shortly after.
            if self.config.audio != self.applied_audio {
                self.controller.apply_audio_settings(&self.config.audio);
                self.applied_audio = self.config.audio.clone();
            }
            if self.config.hotkeys != self.saved_config.hotkeys {
                self.ui.hotkey_errors = self.hotkeys.apply(&self.config.hotkeys);
            }
            if self.config.general.language != self.saved_config.general.language {
                self.tr = Tr::new(self.config.general.language);
            }
            self.save_config_now();
        }
        let want_autostart = self.config.general.start_with_system;
        if self.autostart_applied != Some(want_autostart) {
            let cmd = std::env::current_exe().map(|p| format!("\"{}\" --minimized", p.display())).unwrap_or_default();
            let currently = novamix_device_windows::autostart_command().is_some();
            if (currently != want_autostart || (want_autostart && novamix_device_windows::autostart_command().as_deref() != Some(cmd.as_str())))
                && let Err(e) = novamix_device_windows::set_autostart(want_autostart, &cmd)
            {
                self.toast(Level::Error, e);
            }
            self.autostart_applied = Some(want_autostart);
        }
    }

    pub fn status_text(&self, s: &EndpointState) -> String {
        match s {
            EndpointState::Off => self.t("st_off").into(),
            EndpointState::Starting => self.t("st_starting").into(),
            EndpointState::Running => self.t("st_running").into(),
            EndpointState::Idle => self.t("st_idle").into(),
            EndpointState::TestTone => self.t("st_tone").into(),
            EndpointState::Unavailable(r) => format!("{}: {r}", self.t("st_unavailable")),
            EndpointState::Failed(r) => format!("{}: {r}", self.t("st_failed")),
        }
    }

    pub fn status_color(&self, s: &EndpointState) -> Color32 {
        match s {
            EndpointState::Running | EndpointState::TestTone => self.pal.ok,
            EndpointState::Idle | EndpointState::Starting => self.pal.warn,
            EndpointState::Unavailable(_) | EndpointState::Failed(_) => self.pal.danger,
            EndpointState::Off => self.pal.text_dim,
        }
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -40.0)).interactable(false).show(ctx, |ui| {
            for t in self.toasts.iter().rev().take(4) {
                let color = match t.level {
                    Level::Error => self.pal.danger,
                    Level::Warning => self.pal.warn,
                    Level::Info => self.pal.accent,
                };
                egui::Frame::popup(ui.style()).stroke(egui::Stroke::new(1.0, color)).show(ui, |ui| {
                    ui.label(RichText::new(&t.text).color(self.pal.text));
                });
            }
        });
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some()) && ctx.egui_wants_keyboard_input();
        if typing {
            return;
        }
        let (undo, redo, mute) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
                    || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::M),
            )
        });
        if undo && let Some(p) = self.undo.undo(&self.profile) {
            self.profile = p;
        }
        if redo && let Some(p) = self.undo.redo(&self.profile) {
            self.profile = p;
        }
        if mute {
            self.toggle_mic_mute();
        }
    }
}

impl eframe::App for NovaMixApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now_i = Instant::now();
        let dt = now_i.duration_since(self.last_frame).as_secs_f32().min(0.5);
        self.last_frame = now_i;
        let now = now_i.duration_since(self.start).as_secs_f64();
        let before = self.profile.clone();
        self.handle_tray(ctx);
        self.handle_hotkeys(ctx);
        self.handle_window_events(ctx);
        self.status = self.controller.shared().status();
        if self.window_visible {
            self.update_meters(now, dt);
        }
        self.update_spectrum();
        self.track_edits(&before);
        self.sync_profile();
        self.persist();
        let fps = if self.config.general.power_saving { 15 } else { self.config.general.meter_fps.clamp(10, 120) };
        let interval = if self.window_visible { Duration::from_secs_f32(1.0 / fps as f32) } else { Duration::from_millis(250) };
        ctx.request_repaint_after(interval);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pal = theme::apply(&ctx, self.config.general.theme, self.config.general.ui_scale);
        if !self.config.general.show_tooltips {
            ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 1e6);
        } else {
            ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 0.5);
        }
        let before = self.profile.clone();
        self.handle_shortcuts(&ctx);
        if self.config.general.compact_mode {
            crate::views::compact::show(self, ui);
        } else {
            crate::views::top_bar(self, ui);
            crate::views::status_bar(self, ui);
            crate::views::effects::panel(self, ui);
            crate::views::mixer::central(self, ui);
        }
        crate::views::routing::window(self, &ctx);
        crate::views::profiles::window(self, &ctx);
        crate::views::settings::window(self, &ctx);
        crate::views::diagnostics::window(self, &ctx);
        crate::views::wizard::window(self, &ctx);
        crate::views::voice_tune::window(self, &ctx);
        crate::views::modals(self, &ctx);
        self.draw_toasts(&ctx);
        self.track_edits(&before);
        self.sync_profile();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_profile_now();
        self.save_config_now();
        self.controller.shutdown();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.pal.bg.to_normalized_gamma_f32()
    }
}

/// Fallback profile when the stored one cannot be loaded.
pub fn startup_profile(store: &ConfigStore, config: &AppConfig) -> (Profile, Option<String>) {
    match config.general.resume {
        novamix_config::ResumeBehavior::DefaultProfile => (default_profile(), None),
        _ => match store.load_profile(&config.active_profile) {
            Ok(l) => {
                let note = (!l.issues.is_empty()).then(|| format!("{} corrections while loading profile", l.issues.len()));
                (l.value, note)
            }
            Err(_) => (default_profile(), None),
        },
    }
}
