//! Internal clock: drives the engine when no output device is the master
//! (e.g. only the virtual microphone is used, or the monitor device is
//! missing). Outputs are then fed through drift-compensated bridges.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::engine::{SharedCore, with_core};

pub struct InternalClock {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl InternalClock {
    /// Starts the clock thread. `on_start` runs first on the new thread
    /// (used to request real-time scheduling, e.g. MMCSS "Pro Audio").
    pub fn start(
        core: SharedCore,
        failed: Arc<AtomicBool>,
        sample_rate: f32,
        block: usize,
        on_start: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let block = block.max(32);
        let period = Duration::from_secs_f64(block as f64 / sample_rate as f64);
        let handle = std::thread::Builder::new().name("novamix-clock".into()).spawn(move || {
            on_start();
            let mut next = Instant::now();
            while !stop2.load(Ordering::Relaxed) {
                next += period;
                with_core(&core, &failed, |c| c.process_detached(block));
                loop {
                    let now = Instant::now();
                    if now >= next {
                        // Far behind (system suspended, debugger…): resync.
                        if now - next > period * 4 {
                            next = now;
                        }
                        break;
                    }
                    let remaining = next - now;
                    if remaining > Duration::from_micros(1500) {
                        std::thread::sleep(remaining - Duration::from_millis(1));
                    } else {
                        std::thread::yield_now();
                    }
                }
            }
        })?;
        Ok(Self { stop, handle: Some(handle) })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for InternalClock {
    fn drop(&mut self) {
        self.stop();
    }
}
