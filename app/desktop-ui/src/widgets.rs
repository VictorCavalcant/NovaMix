//! Custom widgets: level meters, faders, gain-reduction bars, toggle
//! buttons, EQ and compressor curves, spectrum plot.
//!
//! Every indicator that uses color also carries text or shape (clip shows
//! "CLIP", states have labels), so color is never the only cue.

use egui::{Align2, Color32, CornerRadius, FontId, Key, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use novamix_dsp::params::{CompressorParams, EqParams};
use novamix_dsp::util::{MIN_DB, gain_to_db};

use crate::theme::{Palette, level_color};

pub const METER_MIN_DB: f32 = -60.0;

/// Display ballistics of a stereo meter.
#[derive(Debug, Default, Clone, Copy)]
pub struct MeterState {
    pub peak: [f32; 2],
    pub rms: [f32; 2],
    pub hold: [f32; 2],
    hold_until: [f64; 2],
    pub clip: bool,
}

impl MeterState {
    pub fn new() -> Self {
        Self { peak: [MIN_DB; 2], rms: [MIN_DB; 2], hold: [MIN_DB; 2], hold_until: [0.0; 2], clip: false }
    }

    /// Feeds new linear peak/RMS values (peaks are maxima since last read).
    pub fn update(&mut self, now: f64, dt: f32, peak: [f32; 2], rms: [f32; 2]) {
        for c in 0..2 {
            let p = gain_to_db(peak[c]);
            let decayed = self.peak[c] - 26.0 * dt;
            self.peak[c] = p.max(decayed).max(MIN_DB);
            self.rms[c] = gain_to_db(rms[c]);
            if p >= self.hold[c] || now > self.hold_until[c] {
                self.hold[c] = p.max(self.peak[c]);
                self.hold_until[c] = now + 1.5;
            }
        }
    }

    pub fn max_peak(&self) -> f32 {
        self.peak[0].max(self.peak[1])
    }
}

fn db_to_frac(db: f32) -> f32 {
    ((db - METER_MIN_DB) / -METER_MIN_DB).clamp(0.0, 1.0)
}

/// Vertical stereo (or mono) level meter with RMS body, peak line, hold
/// tick and a "CLIP" label.
pub fn level_meter(ui: &mut Ui, p: &Palette, size: Vec2, m: &MeterState, stereo: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(2), p.meter_bg);
    let clip_h = 12.0;
    let bars = rect.shrink2(vec2(1.0, 1.0));
    let bars = Rect::from_min_max(pos2(bars.min.x, bars.min.y + clip_h), bars.max);
    let n = if stereo { 2 } else { 1 };
    let gap = 2.0;
    let w = (bars.width() - gap * (n as f32 - 1.0)) / n as f32;
    for c in 0..n {
        let x0 = bars.min.x + c as f32 * (w + gap);
        let col = Rect::from_min_size(pos2(x0, bars.min.y), vec2(w, bars.height()));
        // RMS body, segmented by color zone.
        let rms = db_to_frac(m.rms[c]);
        let top = col.max.y - rms * col.height();
        for (lo, hi, color) in [(METER_MIN_DB, -18.0, p.meter_low), (-18.0, -6.0, p.meter_mid), (-6.0, 0.0, p.meter_high)] {
            let y_lo = col.max.y - db_to_frac(lo) * col.height();
            let y_hi = col.max.y - db_to_frac(hi) * col.height();
            let y_top = y_hi.max(top);
            if y_top < y_lo {
                painter.rect_filled(Rect::from_min_max(pos2(col.min.x, y_top), pos2(col.max.x, y_lo)), 0.0, color.gamma_multiply(0.85));
            }
        }
        // Peak line.
        if m.peak[c] > METER_MIN_DB {
            let y = col.max.y - db_to_frac(m.peak[c]) * col.height();
            painter.line_segment([pos2(col.min.x, y), pos2(col.max.x, y)], Stroke::new(2.0, level_color(p, m.peak[c])));
        }
        if m.hold[c] > METER_MIN_DB {
            let y = col.max.y - db_to_frac(m.hold[c]) * col.height();
            painter.line_segment([pos2(col.min.x, y), pos2(col.max.x, y)], Stroke::new(1.0, p.text_dim));
        }
    }
    // Scale ticks every 6 dB.
    let mut db = 0.0;
    while db >= METER_MIN_DB {
        let y = bars.max.y - db_to_frac(db) * bars.height();
        painter.line_segment([pos2(rect.max.x - 3.0, y), pos2(rect.max.x, y)], Stroke::new(1.0, p.grid));
        db -= 6.0;
    }
    let clip_rect = Rect::from_min_size(rect.min, vec2(rect.width(), clip_h));
    if m.clip {
        painter.rect_filled(clip_rect, CornerRadius::same(2), p.danger);
        painter.text(clip_rect.center(), Align2::CENTER_CENTER, "CLIP", FontId::proportional(8.0), Color32::WHITE);
    } else {
        painter.rect_filled(clip_rect, CornerRadius::same(2), p.strip_border);
    }
    resp.on_hover_text(format!("{:.1} dBFS peak / {:.1} dBFS RMS", m.max_peak().max(-99.0), m.rms[0].max(m.rms[1]).max(-99.0)))
}

/// Horizontal gain-reduction bar (0 to -24 dB, grows from the right).
pub fn gr_bar(ui: &mut Ui, p: &Palette, size: Vec2, gr_db: f32, label: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(2), p.meter_bg);
    let frac = (-gr_db / 24.0).clamp(0.0, 1.0);
    if frac > 0.0 {
        let r = Rect::from_min_max(pos2(rect.max.x - frac * rect.width(), rect.min.y), rect.max);
        painter.rect_filled(r, CornerRadius::same(2), p.gr);
    }
    painter.text(pos2(rect.min.x + 3.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(9.0), p.text_dim);
    resp.on_hover_text(format!("{label}: {gr_db:.1} dB"))
}

/// Fader position (0..1) ↔ dB, with finer resolution near 0 dB.
pub fn fader_db(pos: f32) -> f32 {
    let p = pos.clamp(0.0, 1.0);
    if p <= 0.0 {
        -90.0
    } else if p < 0.25 {
        -90.0 + p / 0.25 * 50.0
    } else {
        -40.0 + (p - 0.25) / 0.75 * 52.0
    }
}

pub fn fader_pos(db: f32) -> f32 {
    if db <= -90.0 {
        0.0
    } else if db < -40.0 {
        (db + 90.0) / 50.0 * 0.25
    } else {
        (0.25 + (db + 40.0) / 52.0 * 0.75).min(1.0)
    }
}

pub fn fmt_db(db: f32) -> String {
    if db <= -90.0 { "-∞".into() } else { format!("{db:+.1}") }
}

/// Vertical fader. Drag, scroll, arrow keys when focused (Shift = fine),
/// double-click resets to 0 dB.
pub fn fader(ui: &mut Ui, p: &Palette, value_db: &mut f32, size: Vec2, accent: Color32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(6.0, rect.height() - 16.0));
    let mut pos = fader_pos(*value_db);
    let old = *value_db;
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let delta = -resp.drag_delta().y / track.height() * if fine { 0.2 } else { 1.0 };
        pos = (pos + delta).clamp(0.0, 1.0);
        *value_db = fader_db(pos);
    }
    if resp.hovered() {
        let scroll: f32 =
            ui.input(|i| i.raw.events.iter().filter_map(|e| if let egui::Event::MouseWheel { delta, .. } = e { Some(delta.y) } else { None }).sum());
        if scroll != 0.0 {
            *value_db = (*value_db + scroll.signum() * 0.5).clamp(-90.0, 12.0);
            if *value_db < -60.0 && scroll < 0.0 {
                *value_db = -90.0;
            }
        }
    }
    if resp.has_focus() {
        let (up, down, fine) = ui.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown), i.modifiers.shift));
        let step = if fine { 0.1 } else { 1.0 };
        if up {
            *value_db = (value_db.max(-60.0) + step).min(12.0);
        }
        if down {
            *value_db -= step;
            if *value_db < -60.0 {
                *value_db = -90.0;
            }
        }
    }
    if resp.double_clicked() {
        *value_db = 0.0;
    }
    *value_db = value_db.clamp(-90.0, 12.0);
    if (*value_db - old).abs() > f32::EPSILON {
        resp.mark_changed();
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(track, CornerRadius::same(3), p.meter_bg);
    let unity_y = track.max.y - fader_pos(0.0) * track.height();
    for db in [12.0, 6.0, 0.0, -6.0, -12.0, -24.0, -40.0, -60.0] {
        let y = track.max.y - fader_pos(db) * track.height();
        painter.line_segment([pos2(rect.min.x + 2.0, y), pos2(track.min.x - 3.0, y)], Stroke::new(1.0, p.grid));
        if rect.width() > 40.0 {
            painter.text(pos2(rect.min.x, y), Align2::LEFT_CENTER, format!("{}", db as i32), FontId::proportional(8.0), p.text_dim);
        }
    }
    painter.line_segment([pos2(track.min.x - 6.0, unity_y), pos2(track.max.x + 6.0, unity_y)], Stroke::new(1.0, p.text_dim));
    let y = track.max.y - fader_pos(*value_db) * track.height();
    let fill_rect = Rect::from_min_max(pos2(track.min.x, y), track.max);
    painter.rect_filled(fill_rect, CornerRadius::same(3), accent.gamma_multiply(0.5));
    let cap = Rect::from_center_size(pos2(track.center().x, y), vec2(rect.width().min(34.0), 14.0));
    let focused = resp.has_focus();
    painter.rect_filled(cap, CornerRadius::same(3), if resp.hovered() || resp.dragged() { p.text } else { p.text_dim });
    painter.line_segment([pos2(cap.min.x + 3.0, y), pos2(cap.max.x - 3.0, y)], Stroke::new(2.0, accent));
    if focused {
        painter.rect_stroke(cap.expand(2.0), CornerRadius::same(4), Stroke::new(1.0, p.accent), StrokeKind::Outside);
    }
    resp
}

/// Toggle button: highlighted when on; the label is always visible.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str, on_color: Color32, size: Vec2) -> Response {
    let text = egui::RichText::new(label).size(11.0).strong().color(if *on { Color32::WHITE } else { ui.visuals().text_color() });
    let btn = egui::Button::new(text).fill(if *on { on_color } else { ui.visuals().widgets.inactive.weak_bg_fill }).min_size(size);
    let mut resp = ui.add(btn);
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp
}

/// Small colored dot with a text label.
pub fn status_dot(ui: &mut Ui, color: Color32, label: &str) -> Response {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 4.0, color);
        ui.label(egui::RichText::new(label).small());
    })
    .response
}

fn log_x(freq: f32, rect: Rect) -> f32 {
    let t = (freq.max(20.0).ln() - 20f32.ln()) / (20_000f32.ln() - 20f32.ln());
    rect.min.x + t * rect.width()
}

fn x_to_freq(x: f32, rect: Rect) -> f32 {
    let t = ((x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
    (20f32.ln() + t * (20_000f32.ln() - 20f32.ln())).exp()
}

fn draw_freq_grid(painter: &egui::Painter, p: &Palette, rect: Rect, db_range: f32) {
    painter.rect_filled(rect, CornerRadius::same(4), p.meter_bg);
    for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0] {
        let x = log_x(f, rect);
        painter.line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], Stroke::new(1.0, p.grid));
        let label = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
        painter.text(pos2(x + 2.0, rect.max.y - 2.0), Align2::LEFT_BOTTOM, label, FontId::proportional(8.0), p.text_dim);
    }
    let mut db = -db_range;
    while db <= db_range {
        let y = rect.center().y - db / db_range * rect.height() * 0.5;
        painter.line_segment([pos2(rect.min.x, y), pos2(rect.max.x, y)], Stroke::new(if db == 0.0 { 1.5 } else { 1.0 }, p.grid));
        db += 6.0;
    }
}

/// EQ curve with draggable band handles. Returns `Some(band)` when a band
/// was edited (frequency/gain written into `eq`).
pub fn eq_curve(ui: &mut Ui, p: &Palette, eq: &mut EqParams, sample_rate: f32, size: Vec2) -> Option<usize> {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let range = 18.0;
    draw_freq_grid(&painter, p, rect, range);
    let to_y = |db: f32| rect.center().y - (db / range).clamp(-1.0, 1.0) * rect.height() * 0.5;
    let mut pts = Vec::with_capacity(rect.width() as usize);
    let mut probe = *eq;
    probe.enabled = true;
    for i in 0..(rect.width() as usize) {
        let x = rect.min.x + i as f32;
        let f = x_to_freq(x, rect);
        pts.push(pos2(x, to_y(probe.magnitude_db(f, sample_rate))));
    }
    let curve_color = if eq.enabled { p.curve } else { p.text_dim };
    painter.add(Shape::line(pts, Stroke::new(2.0, curve_color)));

    let mut edited = None;
    let drag_id = resp.id.with("band");
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id));
    let count = eq.band_count as usize;
    let handle_pos = |b: &novamix_dsp::params::EqBand| pos2(log_x(b.frequency_hz, rect), to_y(if b.kind.uses_gain() { b.gain_db } else { 0.0 }));
    if resp.drag_started()
        && let Some(ptr) = resp.interact_pointer_pos()
    {
        dragging = (0..count)
            .filter(|i| eq.bands[*i].enabled)
            .min_by(|a, b| {
                let da = handle_pos(&eq.bands[*a]).distance(ptr);
                let db = handle_pos(&eq.bands[*b]).distance(ptr);
                da.partial_cmp(&db).unwrap()
            })
            .filter(|i| handle_pos(&eq.bands[*i]).distance(ptr) < 18.0);
    }
    if resp.dragged()
        && let (Some(i), Some(ptr)) = (dragging, resp.interact_pointer_pos())
    {
        let b = &mut eq.bands[i];
        b.frequency_hz = x_to_freq(ptr.x, rect).clamp(20.0, 20_000.0);
        if b.kind.uses_gain() {
            b.gain_db = (-(ptr.y - rect.center().y) / (rect.height() * 0.5) * range).clamp(-18.0, 18.0);
        }
        edited = Some(i);
    }
    if resp.drag_stopped() {
        dragging = None;
    }
    ui.data_mut(|d| d.insert_temp(drag_id, dragging));
    for i in 0..count {
        let b = &eq.bands[i];
        if !b.enabled {
            continue;
        }
        let c = handle_pos(b);
        let active = dragging == Some(i);
        painter.circle_filled(c, if active { 7.0 } else { 5.5 }, curve_color.gamma_multiply(if active { 1.0 } else { 0.8 }));
        painter.text(c, Align2::CENTER_CENTER, format!("{}", i + 1), FontId::proportional(8.0), p.meter_bg);
    }
    resp.on_hover_text("Arraste os pontos para ajustar frequência e ganho / Drag points to adjust frequency and gain");
    edited
}

/// Static compressor curve with the current input level marker.
pub fn transfer_curve(ui: &mut Ui, p: &Palette, comp: &CompressorParams, size: Vec2, gr_db: f32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), p.meter_bg);
    let to = |db_in: f32, db_out: f32| pos2(rect.min.x + (db_in + 60.0) / 60.0 * rect.width(), rect.max.y - (db_out + 60.0) / 60.0 * rect.height());
    painter.line_segment([to(-60.0, -60.0), to(0.0, 0.0)], Stroke::new(1.0, p.grid));
    let pts: Vec<Pos2> = (0..=60)
        .map(|i| {
            let x = -60.0 + i as f32;
            to(x, comp.transfer_db(x))
        })
        .collect();
    painter.add(Shape::line(pts, Stroke::new(2.0, if comp.enabled { p.curve } else { p.text_dim })));
    let t = to(comp.threshold_db, comp.threshold_db);
    painter.circle_stroke(t, 4.0, Stroke::new(1.0, p.gr));
    if gr_db < -0.1 {
        painter.text(rect.left_top() + vec2(4.0, 2.0), Align2::LEFT_TOP, format!("GR {gr_db:.1} dB"), FontId::proportional(9.0), p.gr);
    }
}

/// Spectrum plot from `(frequency, dB)` bands.
pub fn spectrum_plot(ui: &mut Ui, p: &Palette, bands: &[(f32, f32)], size: Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), p.meter_bg);
    for f in [100.0, 1000.0, 10_000.0] {
        let x = log_x(f, rect);
        painter.line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], Stroke::new(1.0, p.grid));
    }
    if bands.is_empty() {
        return;
    }
    let w = rect.width() / bands.len() as f32;
    for (i, (_, db)) in bands.iter().enumerate() {
        let h = ((db + 90.0) / 90.0).clamp(0.0, 1.0) * rect.height();
        let x = rect.min.x + i as f32 * w;
        let r = Rect::from_min_max(pos2(x + 0.5, rect.max.y - h), pos2(x + w - 0.5, rect.max.y));
        painter.rect_filled(r, 0.0, level_color(p, *db + 12.0).gamma_multiply(0.8));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fader_mapping_roundtrips() {
        for db in [-90.0f32, -60.0, -40.0, -20.0, -6.0, 0.0, 6.0, 12.0] {
            let back = fader_db(fader_pos(db));
            assert!((back - db).abs() < 0.01, "{db} -> {back}");
        }
        assert_eq!(fader_db(0.0), -90.0);
        assert!((fader_db(1.0) - 12.0).abs() < 1e-4);
    }

    #[test]
    fn meter_ballistics_decay_and_hold() {
        let mut m = MeterState::new();
        m.update(0.0, 0.016, [1.0, 0.5], [0.5, 0.25]);
        assert!(m.peak[0] > -0.1);
        m.update(1.0, 1.0, [0.0, 0.0], [0.0, 0.0]);
        assert!(m.peak[0] < -20.0, "{}", m.peak[0]);
        assert!(m.hold[0] > -0.1, "held for 1.5 s");
        m.update(3.0, 0.016, [0.0, 0.0], [0.0, 0.0]);
        // After the hold time the marker falls back to the decaying peak.
        assert!(m.hold[0] < -20.0 && (m.hold[0] - m.peak[0]).abs() < 1e-3, "{} {}", m.hold[0], m.peak[0]);
    }
}
