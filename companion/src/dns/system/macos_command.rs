//! Bounded capture for trusted, absolute-path read-only Mac utilities. No shell,
//! reader threads, or blocking pipe drains; timeout/error kills and reaps only
//! the child owned here. This does not put a deadline on native framework calls.
use std::{
    io::{ErrorKind, Read},
    os::fd::AsRawFd,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) const COMMAND_LIMIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub(crate) struct Budget(Instant);
impl Budget {
    pub(crate) fn new(duration: Duration) -> Self {
        Self(Instant::now() + duration)
    }
    fn command_deadline(self) -> Instant {
        self.0.min(Instant::now() + COMMAND_LIMIT)
    }
    pub(crate) fn check(self) -> Result<(), String> {
        if Instant::now() >= self.0 {
            Err("Mac network inspection budget expired".into())
        } else {
            Ok(())
        }
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> Result<(), String> {
    // The descriptor is owned and live for both calls. Preserve existing flags.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err("Could not configure bounded Mac command pipe".into());
    }
    Ok(())
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        // Trusted OS utilities do not run arbitrary user commands. Never kill
        // a process group or a PID discovered by name. Reaping a SIGKILLed child
        // is synchronous; kernel/process-spawn stalls are outside a userspace
        // real-time guarantee, as are the framework IPC calls elsewhere.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn read_chunk(pipe: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> Result<bool, String> {
    let mut chunk = [0; 4096];
    match pipe.read(&mut chunk) {
        Ok(0) => Ok(true),
        Ok(length) => {
            if bytes.len().saturating_add(length) > limit {
                return Err("Mac network command output exceeds limit".into());
            }
            bytes.extend_from_slice(&chunk[..length]);
            Ok(false)
        }
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
            Ok(false)
        }
        Err(_) => Err("Could not read Mac network command output".into()),
    }
}

pub(crate) fn run(
    program: &str,
    args: &[&str],
    budget: Budget,
    limit: usize,
) -> Result<String, String> {
    budget.check()?;
    if !std::path::Path::new(program).is_absolute() || limit == 0 || limit > 256 * 1024 {
        return Err("Invalid bounded Mac command specification".into());
    }
    let deadline = budget.command_deadline();
    let mut child = OwnedChild(
        Command::new(program)
            .args(args)
            .env_clear()
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start Mac network command: {e}"))?,
    );
    let mut stdout = child.0.stdout.take().ok_or("Missing command stdout")?;
    let mut stderr = child.0.stderr.take().ok_or("Missing command stderr")?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let (mut out_done, mut err_done) = (false, false);
    let mut status = None;
    loop {
        if Instant::now() >= deadline {
            return Err("Mac network command timed out".into());
        }
        // One chunk per pipe per iteration keeps noisy output from starving the
        // deadline or the other stream. No wait for EOF can block the caller.
        if !out_done {
            out_done = read_chunk(&mut stdout, &mut output, limit)?;
        }
        if !err_done {
            err_done = read_chunk(&mut stderr, &mut errors, 16 * 1024)?;
        }
        if status.is_none() {
            status = child.0.try_wait().map_err(|e| e.to_string())?;
        }
        if let Some(status) = status {
            if out_done && err_done {
                if !status.success() {
                    return Err("Mac network command exited unsuccessfully".into());
                }
                if !errors.is_empty() {
                    return Err("Mac network command returned unexpected diagnostics".into());
                }
                return String::from_utf8(output)
                    .map_err(|_| "Mac network output is not UTF-8".into());
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shell(script: &str, millis: u64, limit: usize) -> Result<String, String> {
        run(
            "/bin/sh",
            &["-c", script],
            Budget::new(Duration::from_millis(millis)),
            limit,
        )
    }
    #[test]
    fn captures_output_without_inheriting_input_or_locale() {
        assert_eq!(
            shell(
                "printf '%s' \"$LC_ALL\"; if read value; then exit 1; fi",
                1000,
                100
            )
            .unwrap(),
            "C"
        );
        assert!(run("sh", &[], Budget::new(Duration::from_secs(1)), 10).is_err());
    }
    #[test]
    fn limits_stdout_stderr_and_rejects_failed_or_invalid_output() {
        assert!(shell("while :; do printf 12345678; done", 1000, 32)
            .unwrap_err()
            .contains("exceeds limit"));
        assert!(shell("while :; do printf 12345678 >&2; done", 1000, 32)
            .unwrap_err()
            .contains("exceeds limit"));
        assert!(shell("printf warning >&2", 1000, 100).is_err());
        assert!(shell("exit 7", 1000, 100).is_err());
        assert!(shell("printf '\\377'", 1000, 100).is_err());
    }
    #[test]
    fn kills_and_reaps_a_silent_child_on_timeout() {
        use std::os::unix::fs::OpenOptionsExt;
        let path =
            std::env::temp_dir().join(format!("naab-command-{:032x}.pid", rand::random::<u128>()));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        let started = Instant::now();
        let result = run(
            "/bin/sh",
            &[
                "-c",
                "echo $$ > \"$1\"; exec /bin/sleep 30",
                "naab-test",
                path.to_str().unwrap(),
            ],
            Budget::new(Duration::from_millis(300)),
            100,
        );
        let pid: i32 = std::fs::read_to_string(&path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        std::fs::remove_file(path).unwrap();
        // An unreaped child would return its PID (exited) or zero (still alive).
        let waited = unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
        let error = std::io::Error::last_os_error();
        if waited == 0 {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
                libc::waitpid(pid, std::ptr::null_mut(), 0);
            }
        }
        assert_eq!(waited, -1, "timed-out child was not reaped");
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
        assert!(result.unwrap_err().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn pipe_held_by_a_descendant_cannot_block_capture() {
        // The short-lived descendant exits naturally; the capture must return
        // before its inherited pipe closes, without a stranded reader thread.
        let started = Instant::now();
        assert!(shell("/bin/sleep 1 & exit 0", 100, 100)
            .unwrap_err()
            .contains("timed out"));
        assert!(started.elapsed() < Duration::from_millis(800));
    }
    #[test]
    fn stops_when_shared_budget_is_spent() {
        let budget = Budget::new(Duration::ZERO);
        assert!(run("/does/not/exist", &[], budget, 100)
            .unwrap_err()
            .contains("budget expired"));
    }
}
