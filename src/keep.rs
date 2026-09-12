//! Run-wide destruction policy, including updates while adapters are running.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[cfg(unix)]
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    thread::{self, JoinHandle},
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
        let (receiver, sender) = notification_pipe()?;
        let receiver = Arc::new(receiver);
        let mut signals = KeepSignals {
            registrations: Vec::new(),
            sender: Some(Arc::new(sender)),
            _receiver: Arc::clone(&receiver),
            listener: None,
        };
        for (signal, enabled) in [(libc::SIGUSR1, true), (libc::SIGUSR2, false)] {
            let flag = Arc::clone(&self.enabled);
            let sender = Arc::clone(
                signals
                    .sender
                    .as_ref()
                    .expect("sender is open during setup"),
            );
            let notification = u8::from(enabled);
            // SAFETY: atomic stores and write are signal-safe. The pipe is
            // nonblocking and the guard retains its reader to prevent SIGPIPE.
            // The Arcs retain the flag and writer. No allocation, locking,
            // formatting, or panicking occurs here. Registry >=1.4.8 preserves
            // errno around the handler, including failed notification writes.
            let id = unsafe {
                signal_hook_registry::register(signal, move || {
                    flag.store(enabled, Ordering::SeqCst);
                    // Notifications are best-effort if the buffer is full;
                    // keep mode still changes even when reporting cannot.
                    libc::write(sender.as_raw_fd(), (&notification as *const u8).cast(), 1);
                })
            }?;
            signals.registrations.push(id);
        }
        signals.listener = Some(
            thread::Builder::new()
                .name("cvd-keep-notifications".into())
                .spawn(move || print_notifications(&*receiver, io::stderr()))?,
        );
        Ok(signals)
    }
}

/// Keep registrations alive for the run, including while subprocesses wait.
/// Dropping this guard also rolls back a partially successful registration.
#[cfg(unix)]
pub struct KeepSignals {
    registrations: Vec<signal_hook_registry::SigId>,
    sender: Option<Arc<File>>,
    // Keep a read endpoint open even if the listener encounters an I/O error.
    _receiver: Arc<File>,
    listener: Option<JoinHandle<()>>,
}

#[cfg(unix)]
impl Drop for KeepSignals {
    fn drop(&mut self) {
        for id in self.registrations.drain(..) {
            signal_hook_registry::unregister(id);
        }
        // EOF wakes an idle listener and lets it drain queued notifications
        // before the process exits, including on partial setup errors. Closing
        // all writers avoids relying on a separate wakeup syscall at shutdown.
        drop(self.sender.take());
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

#[cfg(unix)]
fn notification_pipe() -> io::Result<(File, File)> {
    let mut descriptors = [-1; 2];
    // SAFETY: pipe initializes exactly two descriptors on success.
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: these are fresh descriptors; File takes exclusive ownership so
    // every error path closes both ends. Setup precedes handlers and threads.
    let [receiver, sender] = descriptors.map(|fd| unsafe { File::from_raw_fd(fd) });
    for file in [&receiver, &sender] {
        // SAFETY: the descriptor is live and F_SETFD takes integer flags.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    // Only the writer is nonblocking; the listener sleeps until a signal or EOF.
    // SAFETY: this fresh pipe has no other status flags to preserve.
    if unsafe { libc::fcntl(sender.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok((receiver, sender))
}

#[cfg(unix)]
fn print_notifications(mut receiver: impl Read, mut output: impl Write) {
    let mut notifications = [0; 64];
    loop {
        match receiver.read(&mut notifications) {
            Ok(0) => break,
            Ok(count) => {
                for notification in &notifications[..count] {
                    let message: &[u8] = if *notification == 1 {
                        b"\n\nCVD: keep mode enabled (SIGUSR1)\n"
                    } else {
                        b"\n\nCVD: keep mode disabled (SIGUSR2)\n"
                    };
                    // A closed stderr must not panic or affect the policy.
                    let _ = output.write_all(message);
                    let _ = output.flush();
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn shutdown_drains_notifications_before_joining_listener() {
        let (receiver, sender) = notification_pipe().unwrap();
        let receiver = Arc::new(receiver);
        let (completed, received) = std::sync::mpsc::channel();
        let listener_reader = Arc::clone(&receiver);
        let signals = KeepSignals {
            registrations: Vec::new(),
            sender: Some(Arc::new(sender)),
            _receiver: receiver,
            listener: Some(thread::spawn(move || {
                let mut output = Vec::new();
                print_notifications(&*listener_reader, &mut output);
                completed.send(output).unwrap();
            })),
        };
        signals
            .sender
            .as_deref()
            .unwrap()
            .write_all(&[1, 1, 0])
            .unwrap();
        drop(signals);
        assert_eq!(
            received.try_recv().unwrap(),
            b"\n\nCVD: keep mode enabled (SIGUSR1)\n\n\nCVD: keep mode enabled (SIGUSR1)\n\n\nCVD: keep mode disabled (SIGUSR2)\n"
        );
    }

    #[test]
    fn stderr_failure_does_not_stop_draining_notifications() {
        struct BrokenStderr;

        impl Write for BrokenStderr {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }

            fn flush(&mut self) -> io::Result<()> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }

        // Span several reads, so returning at the first write failure would
        // leave unread notifications and could eventually fill the pipe.
        let mut input = io::Cursor::new([1; 200]);
        print_notifications(&mut input, BrokenStderr);
        assert_eq!(input.position(), 200);
    }
}
