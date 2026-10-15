//! Procedurally drawn application icon (window, tray and the .exe resource
//! via build.rs), so the repository has no binary image assets.
//!
//! Kept free of dependencies because `build.rs` includes this file.

/// Returns `size × size` RGBA pixels: a rounded teal/indigo square with
/// three white fader caps.
pub fn rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    let radius = s * 0.22;
    let faders = [0.30f32, 0.62, 0.45];
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            // Rounded-rect coverage with 1px anti-aliasing.
            let dx = (radius - fx).max(fx - (s - radius)).max(0.0);
            let dy = (radius - fy).max(fy - (s - radius)).max(0.0);
            let d = (dx * dx + dy * dy).sqrt() - radius;
            let alpha = (0.5 - d).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            let t = (fx + fy) / (2.0 * s);
            let (mut r, mut g, mut b) =
                (0x24 as f32 * (1.0 - t) + 0x3d as f32 * t, 0x2a as f32 * (1.0 - t) + 0xc9 as f32 * t, 0x8c as f32 * (1.0 - t) + 0xc0 as f32 * t);
            // Three vertical slots with caps.
            for (i, pos) in faders.iter().enumerate() {
                let cx = s * (0.28 + 0.22 * i as f32);
                let slot_w = s * 0.035;
                let in_slot = (fx - cx).abs() < slot_w && fy > s * 0.18 && fy < s * 0.82;
                let cap_y = s * (0.18 + 0.64 * pos);
                let in_cap = (fx - cx).abs() < s * 0.09 && (fy - cap_y).abs() < s * 0.045;
                if in_cap {
                    r = 255.0;
                    g = 255.0;
                    b = 255.0;
                } else if in_slot {
                    r *= 0.45;
                    g *= 0.45;
                    b *= 0.45;
                }
            }
            let i = ((y * size + x) * 4) as usize;
            px[i] = r as u8;
            px[i + 1] = g as u8;
            px[i + 2] = b as u8;
            px[i + 3] = (alpha * 255.0) as u8;
        }
    }
    px
}
