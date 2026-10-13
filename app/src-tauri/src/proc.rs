//! Child-process helpers: run a command with a hard timeout, killing the
//! whole process **group** on expiry so grandchildren (cargo → rustc,
//! trunk → wasm-bindgen, agent CLI → spawned dev servers) don't outlive
//! the parent and keep ports or file locks held. A hung child without
//! this wedges a lineage job in `Implementing` forever — the single worst
//! failure mode for an unattended pipeline.

use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub enum WaitOutcome {
    Exited(ExitStatus),
    /// The deadline passed; the process group was killed.
    TimedOut,
}

/// Spawn `command` in its own process group and wait for it, killing the
/// whole group if `timeout` expires. The caller configures stdout/stderr
/// before calling (the dispatcher points them at log files; validators
/// null them).
pub fn run_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<WaitOutcome> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // New process group so a timeout kill reaches every descendant,
        // not just the direct child.
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(WaitOutcome::Exited(status));
        }
        if Instant::now() >= deadline {
            kill_process_group(&mut child);
            let _ = child.wait();
            return Ok(WaitOutcome::TimedOut);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Best-effort kill of the child's whole process group, then the child
/// itself as a fallback (covers non-unix and the pre-exec race).
fn kill_process_group(child: &mut Child) {
    #[cfg(unix)]
    {
        // The child IS the group leader (process_group(0) above), so its
        // pid doubles as the pgid. `kill -9 -<pgid>` signals the group.
        let pgid = child.id();
        let _ = Command::new("kill")
            .arg("-9")
            .arg(format!("-{pgid}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_child_exits_normally() {
        let mut cmd = Command::new("true");
        match run_with_timeout(&mut cmd, Duration::from_secs(5)).unwrap() {
            WaitOutcome::Exited(s) => assert!(s.success()),
            WaitOutcome::TimedOut => panic!("true(1) should not time out"),
        }
    }

    #[test]
    fn failing_child_reports_status() {
        let mut cmd = Command::new("false");
        match run_with_timeout(&mut cmd, Duration::from_secs(5)).unwrap() {
            WaitOutcome::Exited(s) => assert!(!s.success()),
            WaitOutcome::TimedOut => panic!("false(1) should not time out"),
        }
    }

    #[test]
    fn hung_child_is_killed_with_its_group() {
        // Parent shell spawns a grandchild sleep; both must die on timeout.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 300 & sleep 300"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let started = Instant::now();
        match run_with_timeout(&mut cmd, Duration::from_millis(600)).unwrap() {
            WaitOutcome::TimedOut => {}
            WaitOutcome::Exited(_) => panic!("sleep 300 cannot have exited"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "kill must be prompt, not waiting out the sleep"
        );
    }
}
