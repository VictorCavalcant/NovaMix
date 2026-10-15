//! Colors and visual style.

use egui::{Color32, CornerRadius, Stroke, Visuals};
use novamix_config::ThemePref;

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub strip: Color32,
    pub strip_selected: Color32,
    pub strip_border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub meter_bg: Color32,
    pub meter_low: Color32,
    pub meter_mid: Color32,
    pub meter_high: Color32,
    pub mute: Color32,
    pub solo: Color32,
    pub cue: Color32,
    pub send: Color32,
    pub gr: Color32,
    pub grid: Color32,
    pub curve: Color32,
}

pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x14, 0x15, 0x19),
    panel: Color32::from_rgb(0x1b, 0x1d, 0x23),
    strip: Color32::from_rgb(0x22, 0x25, 0x2d),
    strip_selected: Color32::from_rgb(0x26, 0x33, 0x3d),
    strip_border: Color32::from_rgb(0x33, 0x37, 0x42),
    text: Color32::from_rgb(0xe6, 0xe8, 0xee),
    text_dim: Color32::from_rgb(0x9a, 0xa0, 0xad),
    accent: Color32::from_rgb(0x3d, 0xc9, 0xc0),
    ok: Color32::from_rgb(0x3f, 0xb9, 0x50),
    warn: Color32::from_rgb(0xe3, 0xa0, 0x08),
    danger: Color32::from_rgb(0xf0, 0x52, 0x4f),
    meter_bg: Color32::from_rgb(0x10, 0x11, 0x14),
    meter_low: Color32::from_rgb(0x2e, 0xc2, 0x6a),
    meter_mid: Color32::from_rgb(0xe6, 0xc2, 0x29),
    meter_high: Color32::from_rgb(0xf0, 0x52, 0x4f),
    mute: Color32::from_rgb(0xd9, 0x48, 0x3b),
    solo: Color32::from_rgb(0xe3, 0xb3, 0x08),
    cue: Color32::from_rgb(0x58, 0x8f, 0xf0),
    send: Color32::from_rgb(0x3d, 0xc9, 0xc0),
    gr: Color32::from_rgb(0xf0, 0x8a, 0x24),
    grid: Color32::from_rgb(0x30, 0x34, 0x3e),
    curve: Color32::from_rgb(0x58, 0xd6, 0xff),
};

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xf3, 0xf4, 0xf7),
    panel: Color32::from_rgb(0xe9, 0xeb, 0xf0),
    strip: Color32::from_rgb(0xff, 0xff, 0xff),
    strip_selected: Color32::from_rgb(0xdc, 0xf3, 0xf1),
    strip_border: Color32::from_rgb(0xc9, 0xcd, 0xd6),
    text: Color32::from_rgb(0x1a, 0x1c, 0x22),
    text_dim: Color32::from_rgb(0x5c, 0x62, 0x70),
    accent: Color32::from_rgb(0x0f, 0x8f, 0x87),
    ok: Color32::from_rgb(0x1a, 0x7f, 0x37),
    warn: Color32::from_rgb(0x9a, 0x67, 0x00),
    danger: Color32::from_rgb(0xc9, 0x2a, 0x2a),
    meter_bg: Color32::from_rgb(0xd8, 0xdb, 0xe2),
    meter_low: Color32::from_rgb(0x1f, 0xa3, 0x55),
    meter_mid: Color32::from_rgb(0xc7, 0x9a, 0x00),
    meter_high: Color32::from_rgb(0xd1, 0x30, 0x30),
    mute: Color32::from_rgb(0xc9, 0x37, 0x2c),
    solo: Color32::from_rgb(0xb8, 0x8a, 0x00),
    cue: Color32::from_rgb(0x2f, 0x6f, 0xd6),
    send: Color32::from_rgb(0x0f, 0x8f, 0x87),
    gr: Color32::from_rgb(0xd2, 0x6a, 0x00),
    grid: Color32::from_rgb(0xcf, 0xd3, 0xdb),
    curve: Color32::from_rgb(0x00, 0x7a, 0xc2),
};

/// Applies theme and scale; returns the active palette.
pub fn apply(ctx: &egui::Context, pref: ThemePref, scale: f32) -> Palette {
    let dark = match pref {
        ThemePref::Dark => true,
        ThemePref::Light => false,
        ThemePref::System => ctx.input(|i| i.raw.system_theme).map(|t| t == egui::Theme::Dark).unwrap_or(true),
    };
    let p = if dark { DARK } else { LIGHT };
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    v.extreme_bg_color = p.meter_bg;
    v.selection.bg_fill = p.accent.gamma_multiply(0.6);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;
    v.window_corner_radius = CornerRadius::same(8);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.corner_radius = CornerRadius::same(4);
    v.widgets.hovered.corner_radius = CornerRadius::same(4);
    v.widgets.active.corner_radius = CornerRadius::same(4);
    ctx.set_visuals(v);
    let zoom = scale.clamp(0.75, 2.0);
    if (ctx.zoom_factor() - zoom).abs() > 0.001 {
        ctx.set_zoom_factor(zoom);
    }
    p
}

/// Meter color for a level in dBFS.
pub fn level_color(p: &Palette, db: f32) -> Color32 {
    if db >= -6.0 {
        p.meter_high
    } else if db >= -18.0 {
        p.meter_mid
    } else {
        p.meter_low
    }
}
