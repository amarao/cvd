//! Run-wide destruction policy, including updates while adapters are running.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub struct KeepMode {
    enabled: Arc<AtomicBool>,
}

impl KeepMode {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    #[cfg(unix)]
    pub fn register_signals(&self) -> std::io::Result<KeepSignals> {
        let mut signals = KeepSignals(Vec::new());
        for (signal, enabled) in [(libc::SIGUSR1, true), (libc::SIGUSR2, false)] {
            let flag = Arc::clone(&self.enabled);
            // SAFETY: the handler only stores an atomic boolean. It does not
            // allocate, lock, perform I/O, or panic. The captured Arc keeps the
            // flag alive until the handler is unregistered.
            let id = unsafe {
                signal_hook_registry::register(signal, move || {
                    flag.store(enabled, Ordering::SeqCst);
                })
            }?;
            signals.0.push(id);
        }
        Ok(signals)
    }
}

/// Keep registrations alive for the run, including while subprocesses wait.
/// Dropping this guard also rolls back a partially successful registration.
#[cfg(unix)]
pub struct KeepSignals(Vec<signal_hook_registry::SigId>);

#[cfg(unix)]
impl Drop for KeepSignals {
    fn drop(&mut self) {
        for id in self.0.drain(..) {
            signal_hook_registry::unregister(id);
        }
    }
}
