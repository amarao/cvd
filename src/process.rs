//! One phase deadline shared by every adapter and inventory subprocess.

use std::{
    io::{self, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(crate) struct Deadline {
    started: Instant,
    budget: Duration,
}

impl Deadline {
    pub(crate) fn new(budget: Duration) -> Self {
        Self {
            started: Instant::now(),
            budget,
        }
    }

    pub(crate) fn check(&self) -> io::Result<()> {
        self.remaining().map(|_| ())
    }

    fn remaining(&self) -> io::Result<Duration> {
        self.budget
            .checked_sub(self.started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "phase timed out after {} seconds",
                        self.budget.as_secs_f64()
                    ),
                )
            })
    }

    pub(crate) fn status(&self, command: &mut Command) -> io::Result<ExitStatus> {
        self.execute(command, false).map(|output| output.status)
    }

    pub(crate) fn output(&self, command: &mut Command) -> io::Result<Output> {
        self.execute(command, true)
    }

    fn execute(&self, command: &mut Command, capture: bool) -> io::Result<Output> {
        self.check()?;
        // An isolated group prevents timeout signals from reaching CVD itself.
        command.process_group(0);
        if capture {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        }
        let mut child = RunningChild {
            child: command.spawn()?,
            finished: false,
        };
        if let Some(stdout) = &child.child.stdout {
            nonblocking(stdout)?;
        }
        if let Some(stderr) = &child.child.stderr {
            nonblocking(stderr)?;
        }
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut status = None;
        loop {
            self.check()?;
            let stdout_done = drain(&mut child.child.stdout, &mut stdout)?;
            let stderr_done = drain(&mut child.child.stderr, &mut stderr)?;
            if status.is_none() {
                status = child.child.try_wait()?;
            }
            if let Some(status) = status
                && stdout_done
                && stderr_done
            {
                child.finished = true;
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            thread::sleep(self.remaining()?.min(Duration::from_millis(10)));
        }
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: fd is borrowed from a live pipe; fcntl neither closes nor retains it.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain<R: Read>(pipe: &mut Option<R>, output: &mut Vec<u8>) -> io::Result<bool> {
    let Some(reader) = pipe else {
        return Ok(true);
    };
    let mut buffer = [0; 8192];
    // Limit work per tick so continuous output cannot starve deadline checks.
    for _ in 0..8 {
        match reader.read(&mut buffer) {
            Ok(0) => {
                *pipe = None;
                return Ok(true);
            }
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

struct RunningChild {
    child: Child,
    finished: bool,
}

impl Drop for RunningChild {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let pid = self.child.id() as libc::pid_t;
        // Ansible workers and SSH may call setsid(), escaping the original group.
        // Capture their identities before terminating the parent. Linux pidfds
        // keep the later escalation safe even if a process exits and its PID is reused.
        #[cfg(target_os = "linux")]
        let descendants = linux_descendants(pid);
        #[cfg(target_os = "linux")]
        for child in &descendants {
            signal_pidfd(child, libc::SIGTERM);
        }
        // SAFETY: the negative PID names the group established at spawn.
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        thread::sleep(Duration::from_millis(100));
        #[cfg(target_os = "linux")]
        for child in &descendants {
            signal_pidfd(child, libc::SIGKILL);
        }
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "linux")]
fn linux_descendants(root: libc::pid_t) -> Vec<std::os::fd::OwnedFd> {
    use std::{
        collections::BTreeSet,
        fs,
        os::fd::{FromRawFd, OwnedFd},
    };
    let mut tree = Vec::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<libc::pid_t>().ok())
            else {
                continue;
            };
            let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            let Some((_, fields)) = stat.rsplit_once(')') else {
                continue;
            };
            let Some(parent) = fields
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<libc::pid_t>().ok())
            else {
                continue;
            };
            tree.push((pid, parent));
        }
    }
    let mut pids = BTreeSet::from([root]);
    loop {
        let before = pids.len();
        for &(pid, parent) in &tree {
            if pids.contains(&parent) {
                pids.insert(pid);
            }
        }
        if pids.len() == before {
            break;
        }
    }
    pids.remove(&root);
    pids.into_iter()
        .filter_map(|pid| {
            // SAFETY: pidfd_open returns an owned descriptor or -1, never a borrowed fd.
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
            (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn signal_pidfd(pid: &std::os::fd::OwnedFd, signal: i32) {
    // SAFETY: the descriptor is live and a null siginfo requests the default.
    unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pid.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_both_streams_without_blocking_on_full_pipes() {
        let output = Deadline::new(Duration::from_secs(5)).output(
            Command::new("python3").args(["-c",
                "import sys; sys.stdout.write('a'*200000); sys.stderr.write('b'*200000); sys.exit(7)"])
        ).unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, vec![b'a'; 200000]);
        assert_eq!(output.stderr, vec![b'b'; 200000]);
    }

    #[test]
    fn expired_deadline_never_launches_a_command() {
        let error = Deadline::new(Duration::ZERO)
            .status(&mut Command::new("not-an-executable"))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn inherited_output_pipes_do_not_extend_the_deadline() {
        let started = Instant::now();
        let error = Deadline::new(Duration::from_millis(100))
            .output(Command::new("sh").args(["-c", "sleep 60 & exit 0"]))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn kills_a_term_resistant_worker_in_a_separate_session() {
        let path = std::env::temp_dir().join(format!("cvd-timeout-worker-{}", std::process::id()));
        let script = r#"
import os, signal, subprocess, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
subprocess.Popen([sys.executable, '-c', '''
import os, signal, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
with open(sys.argv[1], 'w') as f: f.write(str(os.getpid()))
time.sleep(60)
''', sys.argv[1]], start_new_session=True)
time.sleep(60)
"#;
        let error = Deadline::new(Duration::from_secs(1))
            .status(Command::new("python3").args(["-c", script]).arg(&path))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let pid = std::fs::read_to_string(&path).unwrap();
        let stat_path = format!("/proc/{pid}/stat");
        let stopped = || match std::fs::read_to_string(&stat_path) {
            Err(_) => true,
            Ok(stat) => stat.rsplit_once(')').unwrap().1.split_whitespace().next() == Some("Z"),
        };
        for _ in 0..100 {
            if stopped() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(stopped(), "detached worker {pid} is still running");
        std::fs::remove_file(path).unwrap();
    }
}
