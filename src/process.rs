//! Bounded capture of a helper owned by Bridge, never a generic process-kill interface.
use crate::{
    errors::{BridgeError, ErrorCode, Outcome},
    events,
};
use anyhow::Result;
use std::{
    io::{ErrorKind, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Output, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const DEFAULT_TIMEOUT: u64 = 60;
const DEFAULT_CAP: usize = 8 * 1024 * 1024;
const MAX_CAP: usize = 32 * 1024 * 1024;
static OVERRIDES: OnceLock<(Option<u64>, Option<usize>)> = OnceLock::new();

fn invalid(message: &str) -> anyhow::Error {
    BridgeError::new(ErrorCode::InvalidArguments, message, Outcome::NotStarted).into()
}

fn environment_limit(name: &str, overridden: bool) -> Result<Option<String>> {
    if overridden {
        return Ok(None);
    }
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(invalid(
            "Command limit environment variables must contain valid integer text",
        )),
    }
}

fn limits(
    timeout: Option<u64>,
    cap: Option<usize>,
    env_timeout: Option<&str>,
    env_cap: Option<&str>,
) -> Result<(Duration, usize)> {
    let timeout = match timeout {
        Some(value) => value,
        None => env_timeout
            .map(str::parse::<u64>)
            .transpose()
            .map_err(|_| {
                invalid("ORCA_BRIDGE_COMMAND_TIMEOUT_SECS must be an integer from 1 to 3600")
            })?
            .unwrap_or(DEFAULT_TIMEOUT),
    };
    let cap = match cap {
        Some(value) => value,
        None => env_cap
            .map(str::parse::<usize>)
            .transpose()
            .map_err(|_| {
                invalid("ORCA_BRIDGE_MAX_OUTPUT_BYTES must be an integer from 1 to 33554432")
            })?
            .unwrap_or(DEFAULT_CAP),
    };
    if !(1..=3600).contains(&timeout) {
        return Err(invalid(
            "Command timeout must be between 1 and 3600 seconds",
        ));
    }
    if !(1..=MAX_CAP).contains(&cap) {
        return Err(invalid(
            "Command output limit must be between 1 and 33554432 bytes",
        ));
    }
    Ok((Duration::from_secs(timeout), cap))
}

/// Configure CLI overrides once, without mutating process environment or shared thread state.
pub fn configure_command_limits(timeout: Option<u64>, cap: Option<usize>) -> Result<()> {
    let env_timeout = environment_limit("ORCA_BRIDGE_COMMAND_TIMEOUT_SECS", timeout.is_some())?;
    let env_cap = environment_limit("ORCA_BRIDGE_MAX_OUTPUT_BYTES", cap.is_some())?;
    limits(timeout, cap, env_timeout.as_deref(), env_cap.as_deref())?;
    if let Some(configured) = OVERRIDES.get() {
        if *configured != (timeout, cap) {
            return Err(invalid("Command limits were already configured"));
        }
    } else {
        OVERRIDES
            .set((timeout, cap))
            .map_err(|_| invalid("Command limits were already configured"))?;
    }
    Ok(())
}

pub fn command_limits() -> Result<(Duration, usize)> {
    let (timeout, cap) = OVERRIDES.get().copied().unwrap_or_default();
    let env_timeout = environment_limit("ORCA_BRIDGE_COMMAND_TIMEOUT_SECS", timeout.is_some())?;
    let env_cap = environment_limit("ORCA_BRIDGE_MAX_OUTPUT_BYTES", cap.is_some())?;
    limits(timeout, cap, env_timeout.as_deref(), env_cap.as_deref())
}

fn failure(code: ErrorCode, message: &str) -> anyhow::Error {
    BridgeError::new(code, message, Outcome::Unknown).into()
}

fn nonblocking(stream: &impl AsRawFd) -> Result<()> {
    let fd = stream.as_raw_fd();
    // The descriptor is owned by the supplied pipe for both operations.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(failure(
            ErrorCode::InvalidResponse,
            "Cannot configure bounded helper output",
        ));
    }
    Ok(())
}

fn cancel(child: &mut Child, deadline: Instant) {
    // The helper was created in its own process group; never signal Orca's runtime group.
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.kill();
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

pub fn output(command: &mut Command, timeout: Duration, cap: usize) -> Result<Output> {
    output_cancellable(command, timeout, cap, &events::STOP)
}

fn output_cancellable(
    command: &mut Command,
    timeout: Duration,
    cap: usize,
    stop: &AtomicBool,
) -> Result<Output> {
    if timeout.is_zero() || timeout > Duration::from_secs(3600) || !(1..=MAX_CAP).contains(&cap) {
        return Err(invalid(
            "Helper timeout and output limit must be positive and within command bounds",
        ));
    }
    if stop.load(Ordering::Relaxed) {
        return Err(BridgeError::new(
            ErrorCode::Interrupted,
            "Helper interrupted before it started",
            Outcome::NotStarted,
        )
        .into());
    }
    let deadline = Instant::now() + timeout;
    let reserve = Duration::from_millis(30).min(timeout / 4);
    let work_deadline = deadline - reserve;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|_| {
            BridgeError::new(
                ErrorCode::RuntimeUnavailable,
                "Cannot execute Orca helper; verify the configured CLI is installed and executable",
                Outcome::NotStarted,
            )
        })?;
    let result = (|| {
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| failure(ErrorCode::InvalidResponse, "Helper stdout unavailable"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| failure(ErrorCode::InvalidResponse, "Helper stderr unavailable"))?;
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut combined = 0usize;
        let mut stdout_eof = false;
        let mut stderr_eof = false;
        let mut status = None;
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err(failure(
                    ErrorCode::Interrupted,
                    "Orca helper interrupted; its request outcome is unknown; inspect before retrying",
                ));
            }
            if Instant::now() >= work_deadline {
                return Err(failure(
                    ErrorCode::CommandTimeout,
                    "Orca helper timed out; its request outcome is unknown; inspect before retrying",
                ));
            }
            // Bound work per pipe so flooding one cannot starve the other or cancellation.
            for (stream, bytes, eof) in [
                (&mut stdout as &mut dyn Read, &mut out, &mut stdout_eof),
                (&mut stderr as &mut dyn Read, &mut err, &mut stderr_eof),
            ] {
                for _ in 0..16 {
                    if *eof {
                        break;
                    }
                    if stop.load(Ordering::Relaxed) {
                        return Err(failure(
                            ErrorCode::Interrupted,
                            "Orca helper interrupted; its request outcome is unknown; inspect before retrying",
                        ));
                    }
                    if Instant::now() >= work_deadline {
                        return Err(failure(
                            ErrorCode::CommandTimeout,
                            "Orca helper timed out; its request outcome is unknown; inspect before retrying",
                        ));
                    }
                    let mut buffer = [0u8; 8192];
                    match stream.read(&mut buffer) {
                        Ok(0) => {
                            *eof = true;
                            break;
                        }
                        Ok(count) => {
                            if combined.saturating_add(count) > cap {
                                return Err(failure(
                                    ErrorCode::OutputLimit,
                                    "Orca helper output exceeded its limit; its request outcome is unknown; inspect before retrying",
                                ));
                            }
                            combined += count;
                            bytes.extend_from_slice(&buffer[..count]);
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                        Err(_) => {
                            return Err(failure(
                                ErrorCode::InvalidResponse,
                                "Cannot read Orca helper response; its request outcome is unknown",
                            ));
                        }
                    }
                }
            }
            if status.is_none() {
                status = child.try_wait().map_err(|_| {
                    failure(
                        ErrorCode::InvalidResponse,
                        "Cannot inspect Orca helper completion; its request outcome is unknown",
                    )
                })?;
            }
            if stdout_eof
                && stderr_eof
                && let Some(status) = status
            {
                return Ok(Output {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
            std::thread::sleep(
                Duration::from_millis(5)
                    .min(work_deadline.saturating_duration_since(Instant::now())),
            );
        }
    })();
    if result.is_err() {
        cancel(&mut child, deadline);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::envelope;

    #[test]
    fn command_limits_validate_bounds_and_flag_precedence_without_mutating_environment() {
        assert_eq!(
            limits(None, None, None, None).unwrap(),
            (Duration::from_secs(60), 8 * 1024 * 1024)
        );
        assert_eq!(
            limits(Some(2), Some(100), Some("invalid"), Some("invalid")).unwrap(),
            (Duration::from_secs(2), 100)
        );
        for (timeout, cap) in [
            (Some("0"), None),
            (Some("3601"), None),
            (Some("invalid"), None),
            (None, Some("0")),
            (None, Some("33554433")),
        ] {
            assert_eq!(
                envelope(&limits(None, None, timeout, cap).unwrap_err())["error"]["code"],
                "invalid_arguments"
            );
        }
    }

    #[test]
    fn captures_both_streams_and_preserves_exit_status() {
        let output = output_cancellable(
            Command::new("/bin/sh")
                .args(["-c", "printf fixture-out; printf fixture-err >&2; exit 7"]),
            Duration::from_secs(1),
            1024,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(output.stdout, b"fixture-out");
        assert_eq!(output.stderr, b"fixture-err");
        assert_eq!(output.status.code(), Some(7));
    }

    #[test]
    fn hanging_helper_and_descendants_are_bounded_and_killed() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("descendant-finished");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("(sleep 0.3; touch \"$1\") & sleep 5")
            .arg("fixture")
            .arg(&marker);
        let started = Instant::now();
        let error = output_cancellable(
            &mut command,
            Duration::from_millis(120),
            1024,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(envelope(&error)["error"]["code"], "command_timeout");
        assert_eq!(envelope(&error)["error"]["outcome"], "unknown");
        assert!(started.elapsed() < Duration::from_secs(1));
        std::thread::sleep(Duration::from_millis(350));
        assert!(!marker.exists());
    }

    #[test]
    fn shared_cap_counts_stdout_and_stderr_and_reports_unknown_outcome() {
        let error = output_cancellable(
            Command::new("/bin/sh").args(["-c", "printf 12345678; printf abcdefgh >&2"]),
            Duration::from_secs(1),
            12,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(envelope(&error)["error"]["code"], "output_limit");
        assert_eq!(envelope(&error)["error"]["outcome"], "unknown");
    }

    #[test]
    fn inherited_pipes_cannot_outlive_the_helper_deadline() {
        let error = output_cancellable(
            Command::new("/bin/sh").args(["-c", "sleep 5 & exit 0"]),
            Duration::from_millis(120),
            1024,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(envelope(&error)["error"]["code"], "command_timeout");
    }

    #[test]
    fn cancellation_distinguishes_unstarted_and_in_flight_helpers() {
        let stopped = AtomicBool::new(true);
        let error = output_cancellable(
            &mut Command::new("/does/not/exist"),
            Duration::from_secs(1),
            100,
            &stopped,
        )
        .unwrap_err();
        assert_eq!(envelope(&error)["error"]["code"], "interrupted");
        assert_eq!(envelope(&error)["error"]["outcome"], "not-started");
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            signal.store(true, Ordering::Relaxed);
        });
        let error = output_cancellable(
            Command::new("/bin/sh").args(["-c", "sleep 5"]),
            Duration::from_secs(1),
            100,
            &stop,
        )
        .unwrap_err();
        assert_eq!(envelope(&error)["error"]["code"], "interrupted");
        assert_eq!(envelope(&error)["error"]["outcome"], "unknown");
        canceller.join().unwrap();
    }
}
