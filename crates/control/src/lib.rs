//! # novamix-control
//!
//! The control plane between the UI and the real-time engine:
//!
//! * [`ControllerHandle`] spawns the controller thread that owns the audio
//!   backend, the device streams and the engine;
//! * profiles are applied declaratively: [`reconcile`] diffs the previous
//!   and new profile and sends only the changed parameters;
//! * device specifications are resolved to concrete endpoints, with
//!   name-based fallback, a recovery device for monitoring and automatic
//!   reconnection with back-off;
//! * routes are analysed for feedback loops before audio flows; sends that
//!   would close a loop are muted until the user fixes them;
//! * a status snapshot and an event log are published for the UI and for
//!   diagnostic reports.

pub mod controller;
pub mod diag;
pub mod reconcile;
pub mod resolve;
pub mod status;

pub use controller::{ControlMsg, ControllerHandle, ControllerOptions, Shared, StreamRole};
pub use diag::build_report;
pub use reconcile::{effective_sends, reconcile};
pub use resolve::{ResolvedDevice, ResolvedOutput, ResolvedSource, graph_input, resolve_output, resolve_source};
pub use status::{ClockSource, EndpointState, EndpointStatus, EngineStatus};

#[cfg(test)]
mod tests {
    use super::*;
    use novamix_config::{AudioSettings, SourceSpec, default_profile};
    use novamix_dsp::signal::ToneKind;

    /// The controller runs headless with the null backend: the engine uses
    /// the internal clock, test-tone channels work, device channels report
    /// "unavailable".
    #[test]
    fn controller_runs_headless_with_test_tone() {
        let mut p = default_profile();
        p.channels[0].source = SourceSpec::TestTone { kind: ToneKind::Sine, frequency_hz: 440.0, level_db: -12.0 };
        p.channels[0].sends[0].enabled = true;
        let mut h = ControllerHandle::spawn(AudioSettings::default(), p.clone(), ControllerOptions { null_backend: true, device_watcher: false });
        let mut ok = false;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let st = h.shared().status();
            if st.running
                && st.clock == ClockSource::Internal
                && let Some(m) = h.shared().meters()
                && m.channels[0].output_rms[0].load() > 0.01
            {
                ok = true;
                assert_eq!(st.channels[0].state, EndpointState::TestTone);
                assert!(matches!(st.channels[1].state, EndpointState::Unavailable(_)));
                break;
            }
        }
        assert!(ok, "engine did not process the test tone");
        // Live parameter change through a new profile.
        p.channels[0].mute = true;
        h.apply_profile(&p);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let m = h.shared().meters().unwrap();
        // Peaks accumulate until read: clear, wait, read again.
        novamix_audio_core::StripMeter::take_peak(&m.channels[0].output_peak[0]);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let peak = novamix_audio_core::StripMeter::take_peak(&m.channels[0].output_peak[0]);
        assert!(peak < 1e-4, "muted channel still outputs {peak}");
        h.shutdown();
    }
}
