use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Serialize;
use theorem_agent_core::ProviderError;

use crate::invalid;

pub(crate) const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Serialize)]
pub(crate) struct CommandOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

fn read_output(mut reader: impl Read) -> std::io::Result<(Vec<u8>, bool)> {
    let mut stored = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let admitted = count.min(OUTPUT_LIMIT.saturating_sub(stored.len()));
        stored.extend_from_slice(&buffer[..admitted]);
        truncated |= admitted < count;
    }
    Ok((stored, truncated))
}

/// Run one argv vector, draining bounded output and supervising its owned process.
/// No shell is used. A caller must treat a deadline as an unknown effect outcome.
pub(crate) fn run_command(
    program: &Path,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> Result<CommandOutput, ProviderError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| invalid("invalid_timeout", "command deadline cannot be represented"))?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.env("CARGO_TERM_COLOR", "never");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|error| {
        invalid(
            "command_unavailable",
            format!("cannot start {}: {error}", program.display()),
        )
    })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (sender, receiver) = mpsc::channel();
    let err_sender = sender.clone();
    // Reader handles are deliberately not joined: a detached descendant can retain
    // inherited pipes after the leader exits. The caller's deadline still applies.
    std::thread::spawn(move || {
        let _ = sender.send((false, read_output(stdout)));
    });
    std::thread::spawn(move || {
        let _ = err_sender.send((true, read_output(stderr)));
    });
    let status = wait_owned(child, deadline)?;
    let mut out = None;
    let mut err = None;
    for _ in 0..2 {
        let (is_stderr, result) = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => invalid("command_output_timeout", "command output did not close before the deadline; outcome requires reconciliation"),
                mpsc::RecvTimeoutError::Disconnected => invalid("command_io", "command output reader disconnected"),
            })?;
        let value = result.map_err(|_| invalid("command_io", "cannot read command output"))?;
        if is_stderr {
            err = Some(value);
        } else {
            out = Some(value);
        }
    }
    let out = bounded_text(out.ok_or_else(|| invalid("command_io", "stdout result is missing"))?);
    let err = bounded_text(err.ok_or_else(|| invalid("command_io", "stderr result is missing"))?);
    Ok(CommandOutput {
        success: status.success(),
        exit_code: status.code(),
        timed_out: false,
        stdout: out.0,
        stderr: err.0,
        stdout_truncated: out.1,
        stderr_truncated: err.1,
    })
}

fn wait_owned(mut child: Child, deadline: Instant) -> Result<ExitStatus, ProviderError> {
    loop {
        #[cfg(target_os = "linux")]
        {
            use nix::errno::Errno;
            use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
            use nix::unistd::Pid;
            let pid = i32::try_from(child.id())
                .map_err(|_| invalid("command_wait", "child PID is outside the supported range"))?;
            match waitid(
                Id::Pid(Pid::from_raw(pid)),
                WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
            ) {
                Ok(WaitStatus::Exited(..) | WaitStatus::Signaled(..)) => {
                    // WNOWAIT keeps the leader unreaped, retaining ownership of its
                    // PID/PGID until group teardown is complete. Never signal a group
                    // after std::Child::try_wait has reaped its leader.
                    stop_group(&mut child);
                    return child
                        .wait()
                        .map_err(|_| invalid("command_wait", "cannot reap completed command"));
                }
                Ok(_) | Err(Errno::EINTR) => {}
                Err(_) => {
                    reap_later(child);
                    return Err(invalid(
                        "command_wait",
                        "cannot inspect owned command status",
                    ));
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            // nix exposes waitid/WNOWAIT on Linux, but not macOS. try_wait reaps
            // an exited leader, so never signal its possibly reused group after
            // this returns a status. Output collection still has its deadline.
            match child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) => {}
                Err(_) => {
                    reap_later(child);
                    return Err(invalid("command_wait", "cannot inspect command status"));
                }
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            stop_group(&mut child);
            // Even SIGKILL can await an uninterruptible kernel operation; never
            // block the caller indefinitely while reaping a timed-out child.
            reap_later(child);
            return Err(invalid(
                "command_timeout",
                "command exceeded its deadline; outcome requires reconciliation",
            ));
        }
        std::thread::sleep(remaining.min(Duration::from_millis(20)));
    }
}

fn reap_later(mut child: Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

fn bounded_text((bytes, mut truncated): (Vec<u8>, bool)) -> (String, bool) {
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    // Bound JSON bytes as well as captured bytes: control characters and invalid
    // UTF-8 can expand significantly inside a checkpoint or protocol frame.
    loop {
        let encoded = serde_json::to_vec(&text).expect("strings serialize").len();
        if encoded <= OUTPUT_LIMIT {
            return (text, truncated);
        }
        let mut end = text.len() * (OUTPUT_LIMIT - 2) / (encoded - 2);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        truncated = true;
    }
}

fn stop_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;
        if let Ok(pid) = i32::try_from(child.id()) {
            let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detached_descendant_holding_output_cannot_hang_collection() {
        let directory = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let result = run_command(
            Path::new("python3"),
            &[
                "-c",
                "import subprocess,sys; subprocess.Popen([sys.executable,'-c','import time; time.sleep(5)'],start_new_session=True)",
            ],
            directory.path(),
            Duration::from_secs(2),
        );
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "output drain ignored its deadline"
        );
        assert_eq!(result.unwrap_err().code, "command_output_timeout");
    }

    #[test]
    fn captured_output_is_bounded_after_json_escaping() {
        let directory = tempfile::tempdir().unwrap();
        let output = run_command(
            Path::new("python3"),
            &["-c", "import sys; sys.stdout.write('\\0'*65536)"],
            directory.path(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(serde_json::to_vec(&output.stdout).unwrap().len() <= OUTPUT_LIMIT);
        assert!(output.stdout_truncated);
    }

    #[test]
    fn unrepresentable_deadline_is_rejected_before_spawning() {
        let directory = tempfile::tempdir().unwrap();
        let result = std::panic::catch_unwind(|| {
            run_command(
                Path::new("python3"),
                &["-c", "pass"],
                directory.path(),
                Duration::MAX,
            )
        });
        assert!(result.is_ok(), "an invalid deadline must not panic");
        assert_eq!(result.unwrap().unwrap_err().code, "invalid_timeout");
    }
}
