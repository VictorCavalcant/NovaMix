//! DSP behaviour on fixture signals: block-size independence, edge-case
//! inputs, and the effect of the voice presets on speech-like material.

use novamix_dsp::chain::VoiceChain;
use novamix_dsp::params::{StageKind, VoicePreset};
use novamix_dsp::util::{gain_to_db, peak, rms};
use novamix_tests::{SR, speech_like};

fn run(preset: VoicePreset, input: &[f32], block: usize) -> Vec<f32> {
    let mut chain = VoiceChain::new(SR, 1024, preset.chain());
    let mut y = input.to_vec();
    for c in y.chunks_mut(block) {
        chain.process(c, None);
    }
    y
}

#[test]
fn output_is_independent_of_block_size() {
    let (x, _) = speech_like(2.0, 0.01, 1);
    for preset in VoicePreset::ALL {
        let reference = run(preset, &x, 480);
        for block in [1, 7, 64, 333, 1024, 4096] {
            let y = run(preset, &x, block);
            let max_diff = reference.iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(max_diff < 1e-4, "{preset:?} block {block}: diff {max_diff}");
        }
    }
}

#[test]
fn silence_dc_and_clipped_input_stay_finite_and_bounded() {
    let n = SR as usize * 2;
    let inputs: Vec<(&str, Vec<f32>)> = vec![
        ("silence", vec![0.0; n]),
        ("dc", vec![0.5; n]),
        ("clipped square", (0..n).map(|i| if (i / 50) % 2 == 0 { 1.0 } else { -1.0 }).collect()),
        ("tiny", vec![1e-30; n]),
        ("impulses", (0..n).map(|i| if i % 4800 == 0 { 1.0 } else { 0.0 }).collect()),
    ];
    for preset in VoicePreset::ALL {
        let chain = preset.chain();
        for (name, x) in &inputs {
            let y = run(preset, x, 256);
            assert!(y.iter().all(|v| v.is_finite()), "{preset:?}/{name}: non-finite");
            if chain.limiter.enabled {
                let ceiling = novamix_dsp::util::db_to_gain(chain.limiter.ceiling_db);
                assert!(peak(&y) <= ceiling + 1e-5, "{preset:?}/{name}: {} > ceiling", peak(&y));
            }
            if *name == "dc" && chain.hpf.enabled {
                let tail = &y[y.len() / 2..];
                assert!(rms(tail) < 1e-3, "{preset:?}: DC must be removed by the HPF");
            }
        }
    }
}

/// Noise-reducing presets must improve the ratio between burst energy and
/// gap energy (a proxy for speech-to-noise ratio).
#[test]
#[allow(clippy::needless_range_loop)]
fn noise_presets_improve_burst_to_gap_ratio() {
    let (x, mask) = speech_like(6.0, 0.02, 9);
    let ratio = |y: &[f32], latency: usize| {
        let start = SR as usize * 3; // let noise trackers converge
        let (mut on, mut off, mut n_on, mut n_off) = (0.0f64, 0.0f64, 0usize, 0usize);
        for i in start..y.len() {
            let src = i.saturating_sub(latency);
            // Skip transitions.
            let edge = (src % (SR as usize / 4)) < 2400;
            if edge {
                continue;
            }
            if mask[src] {
                on += (y[i] as f64).powi(2);
                n_on += 1;
            } else {
                off += (y[i] as f64).powi(2);
                n_off += 1;
            }
        }
        10.0 * ((on / n_on as f64) / (off / n_off as f64).max(1e-20)).log10()
    };
    let raw = ratio(&x, 0);
    for preset in [VoicePreset::Natural, VoicePreset::Call, VoicePreset::NoisyRoom] {
        let mut chain = VoiceChain::new(SR, 1024, preset.chain());
        let lat = chain.current_latency();
        let mut y = x.clone();
        for c in y.chunks_mut(480) {
            chain.process(c, None);
        }
        let processed = ratio(&y, lat);
        // Synthetic harmonics are not real speech (the neural model is
        // conservative on them), so require a clear but modest improvement.
        assert!(processed > raw + 3.0, "{preset:?}: {raw:.1} dB -> {processed:.1} dB");
    }
}

#[test]
fn raw_preset_is_bit_transparent() {
    let (x, _) = speech_like(1.0, 0.01, 3);
    let y = run(VoicePreset::Raw, &x, 480);
    assert_eq!(x, y);
}

#[test]
fn reordering_stages_keeps_output_bounded() {
    let (x, _) = speech_like(2.0, 0.01, 5);
    let mut p = VoicePreset::Stream.chain();
    for _ in 0..4 {
        p.move_stage(StageKind::Compressor, -1);
    }
    let mut chain = VoiceChain::new(SR, 1024, p);
    let mut y = x.clone();
    for c in y.chunks_mut(256) {
        chain.process(c, None);
    }
    assert!(y.iter().all(|v| v.is_finite()));
    assert!(gain_to_db(peak(&y)) <= -0.9);
}
