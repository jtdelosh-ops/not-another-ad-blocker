//! Windows preview adapter. The embedded PowerShell bridge uses system modules,
//! JSON stdin and stable GUIDs; no supplied script or path
//! is executed. The resolver itself never runs in this privileged process.
use super::{DnsSetting, Platform, Target, TargetState};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

#[cfg(windows)]
#[path = "windows_job.rs"]
mod job;

#[derive(Debug)]
pub struct BridgeError {
    pub message: String,
    /// A killed/timed-out write may still be completing inside the OS provider.
    /// Poison this adapter instance so rollback cannot race another mutation.
    pub uncertain: bool,
}

pub trait Runner {
    fn call(&mut self, request: Value) -> Result<Value, BridgeError>;
}

pub struct PowerShell;

#[cfg(windows)]
fn system_directory() -> Result<PathBuf, String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }
    let mut buffer = [0u16; 32768];
    // SAFETY: writable buffer and its exact capacity; this call returns UTF-16.
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err("Cannot locate the Windows system directory".into());
    }
    use std::os::windows::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..length],
    )))
}

#[cfg(windows)]
fn encoded_script() -> String {
    // Strip only whole-line comments/blank lines to stay below Windows' command
    // length limit. Arguments from callers are never interpolated into this code.
    let script = include_str!("windows.ps1")
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    encode_script(&script)
}

#[cfg(windows)]
fn encode_script(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for part in bytes.chunks(3) {
        let n = (u32::from(part[0]) << 16)
            | (u32::from(*part.get(1).unwrap_or(&0)) << 8)
            | u32::from(*part.get(2).unwrap_or(&0));
        encoded.push(TABLE[((n >> 18) & 63) as usize] as char);
        encoded.push(TABLE[((n >> 12) & 63) as usize] as char);
        encoded.push(if part.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        encoded.push(if part.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

#[cfg(all(test, windows))]
mod tests {
    use super::{call_script, encode_script};
    use serde_json::json;
    use std::{
        ffi::c_void,
        fs,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        path::PathBuf,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn embedded_script_fits_windows_process_command_limit() {
        assert!(super::encoded_script().len() <= 31000);
    }

    // Like netsh, this harmless native child inherits PowerShell's pipe handles.
    // It has its own finite lifetime in case a containment regression is tested.
    // No DNS, adapter, registry, journal or host networking changes are made.
    const DESCENDANT_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$child = New-Object System.Diagnostics.Process
$child.StartInfo.FileName = Join-Path $PSHOME 'powershell.exe'
$child.StartInfo.Arguments = '-NoLogo -NoProfile -NonInteractive -Command "Start-Sleep -Seconds 30"'
$child.StartInfo.UseShellExecute = $false
$child.StartInfo.CreateNoWindow = $true
$null = $child.Start()
[IO.File]::WriteAllText($request.marker, [string]$child.Id)
while (![IO.File]::Exists($request.release)) { Start-Sleep -Milliseconds 25 }
if ($request.mode -eq 'timeout') { Start-Sleep -Seconds 30 }
if ($request.mode -eq 'error') { [Console]::Error.Write('intentional bridge failure'); exit 7 }
[Console]::Out.Write('{"ok":true}')
"#;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "naab-bridge-job-{}-{:016x}",
                std::process::id(),
                rand::random::<u64>()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0.join("pid"));
            let _ = fs::remove_file(self.0.join("release"));
            let _ = fs::remove_dir(&self.0);
        }
    }

    fn descendant_case(mode: &str) -> Result<serde_json::Value, super::BridgeError> {
        let scratch = Scratch::new();
        let marker = scratch.0.join("pid");
        let release = scratch.0.join("release");
        let request = json!({"mode": mode, "marker": marker, "release": release});
        let (send, receive) = mpsc::channel();
        let timeout = Duration::from_secs(6);
        let start = Instant::now();
        // An independent test deadline catches a stuck reader join without
        // hanging the test suite. The test script is also bounded to 30 seconds.
        thread::spawn(move || {
            let result = call_script(&encode_script(DESCENDANT_SCRIPT), request, timeout);
            let _ = send.send(result);
        });
        let process_id = loop {
            if let Ok(pid) = fs::read_to_string(&marker) {
                if let Ok(pid) = pid.parse::<u32>() {
                    break pid;
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "PowerShell never started the descendant: {:?}",
                receive.try_recv()
            );
            thread::sleep(Duration::from_millis(25));
        };
        // Open the process while it is alive, before releasing its parent. A
        // process handle remains the same identity even if Windows reuses a PID.
        // SAFETY: SYNCHRONIZE-only access to the child created by this fixture.
        let handle = unsafe { OpenProcess(0x00100000, 0, process_id) };
        assert!(!handle.is_null(), "{}", std::io::Error::last_os_error());
        // SAFETY: OpenProcess returned a uniquely owned, valid process handle.
        let process = unsafe { OwnedHandle::from_raw_handle(handle) };
        // SAFETY: a live process handle and a zero-duration bounded wait.
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 0) },
            258
        );
        fs::write(release, "continue").unwrap();
        let result = receive
            .recv_timeout(Duration::from_secs(10).saturating_sub(start.elapsed()))
            .expect("bridge failed to return within its deadline; an inherited pipe may be open");
        // SAFETY: process handle is still valid; wait at most one second for the
        // OS to finish terminating the descendant after closing the job.
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 1000) },
            0,
            "bridge descendant survived after the runner returned"
        );
        result
    }

    #[test]
    fn bridge_timeout_reaps_descendants_and_closes_pipes() {
        let error = descendant_case("timeout").unwrap_err();
        assert!(
            error.uncertain,
            "interrupted command must retain recovery state"
        );
        assert!(error.message.contains("timed out"), "{}", error.message);
    }

    #[test]
    fn bridge_success_reaps_descendants_and_preserves_output() {
        assert_eq!(descendant_case("success").unwrap(), json!({"ok": true}));
    }

    #[test]
    fn bridge_error_reaps_descendants_and_preserves_error() {
        let error = descendant_case("error").unwrap_err();
        assert!(!error.uncertain);
        assert_eq!(error.message, "intentional bridge failure");
    }
}

#[cfg(windows)]
impl Runner for PowerShell {
    fn call(&mut self, request: Value) -> Result<Value, BridgeError> {
        call_script(
            &encoded_script(),
            request,
            std::time::Duration::from_secs(20),
        )
    }
}

// Only the fixed embedded bridge reaches this seam outside unit tests. It must
// read stdin to EOF before starting any subprocess or changing system settings.
#[cfg(windows)]
fn call_script(
    script: &str,
    request: Value,
    timeout: std::time::Duration,
) -> Result<Value, BridgeError> {
    use std::os::windows::process::CommandExt;
    use std::{
        io::{Read, Write},
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    let fail = |message: String| BridgeError {
        message,
        uncertain: false,
    };
    let directory = system_directory().map_err(fail)?;
    if script.len() > 31000 {
        return Err(fail(
            "Embedded adapter script exceeds the command limit".into(),
        ));
    }
    let input = serde_json::to_vec(&request).map_err(|e| fail(e.to_string()))?;
    if input.len() > 8192 {
        return Err(fail("Adapter request too large".into()));
    }
    let job = job::ProcessJob::new()
        .map_err(|e| fail(format!("Cannot contain Windows adapter process: {e}")))?;
    let mut child = Command::new(directory.join("WindowsPowerShell/v1.0/powershell.exe"))
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            script,
        ])
        .current_dir(&directory)
        .env(
            "PSModulePath",
            directory.join("WindowsPowerShell/v1.0/Modules"),
        )
        .creation_flags(0x08000000) // CREATE_NO_WINDOW, isolated from console Ctrl+C.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| fail(e.to_string()))?;
    // The bridge blocks reading stdin before doing any work. Assign it to
    // the kill-on-close job before supplying input, so a native child cannot
    // escape between spawn and assignment. No mutation occurs if this fails.
    if let Err(error) = job.assign(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(fail(format!(
            "Cannot contain Windows adapter process: {error}"
        )));
    }
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let output = thread::spawn(move || {
        let mut b = Vec::new();
        stdout.take(131073).read_to_end(&mut b).map(|_| b)
    });
    let errors = thread::spawn(move || {
        let mut b = Vec::new();
        stderr.take(8193).read_to_end(&mut b).map(|_| b)
    });
    // Include a blocked input write in the same deadline as process work.
    // Dropping the job unblocks this writer as well as both pipe readers.
    let write = thread::spawn(move || stdin.write_all(&input));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(25)),
            _ => {
                break Err(BridgeError { message: "Windows adapter command timed out or was interrupted; retain the recovery record and retry offline recovery after Windows settles".into(), uncertain: true });
            }
        }
    };
    // Always close the entire process tree BEFORE joining pipe threads. A
    // descendant can inherit the pipes even after PowerShell exits normally.
    drop(job);
    if status.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let write = write.join();
    let out = output
        .join()
        .map_err(|_| fail("Adapter output reader failed".into()))
        .and_then(|result| result.map_err(|e| fail(e.to_string())));
    let err = errors
        .join()
        .map_err(|_| fail("Adapter error reader failed".into()))
        .and_then(|result| result.map_err(|e| fail(e.to_string())));
    // Preserve uncertainty from an interrupted command even if a pipe also
    // failed. Callers must retain the recovery record in that case.
    let status = status?;
    if !matches!(write, Ok(Ok(()))) {
        return Err(BridgeError {
                message: "Windows adapter input failed; retain the recovery record and retry offline recovery after Windows settles".into(),
                uncertain: true,
            });
    }
    let out = out?;
    let err = err?;
    if out.len() > 131072 || err.len() > 8192 {
        return Err(fail("Adapter output exceeds its limit".into()));
    }
    if !status.success() {
        return Err(fail(String::from_utf8_lossy(&err).trim().to_owned()));
    }
    serde_json::from_slice(&out).map_err(|e| fail(format!("Invalid Windows adapter response: {e}")))
}

#[cfg(not(windows))]
impl Runner for PowerShell {
    fn call(&mut self, _: Value) -> Result<Value, BridgeError> {
        Err(BridgeError {
            message: "This preview requires Windows".into(),
            uncertain: false,
        })
    }
}

pub fn inspect() -> Result<Value, String> {
    PowerShell
        .call(json!({"action":"inspect"}))
        .map_err(|e| e.message)
}

pub fn prepare_journal() -> Result<PathBuf, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Directory {
        directory: PathBuf,
    }
    let result = PowerShell
        .call(json!({"action":"prepare"}))
        .map_err(|e| e.message)?;
    serde_json::from_value::<Directory>(result)
        .map(|v| v.directory)
        .map_err(|e| e.to_string())
}

pub fn require_unelevated_resolver() -> Result<(), String> {
    PowerShell
        .call(json!({"action":"resolverCheck"}))
        .map(|_| ())
        .map_err(|e| e.message)
}

pub fn valid_guid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
            }
        })
}

fn valid_target(target: &Target) -> bool {
    let parts: Vec<_> = target.network_id.split(':').collect();
    valid_guid(&target.interface_id) && parts.len() == 2 && parts.iter().all(|p| valid_guid(p))
}

pub struct WindowsPlatform<R = PowerShell> {
    runner: R,
    selected: Option<String>,
    uncertain: bool,
}

impl<R: Runner> WindowsPlatform<R> {
    pub fn new(runner: R, selected: Option<String>) -> Result<Self, String> {
        if selected.as_ref().is_some_and(|id| !valid_guid(id)) {
            return Err("Adapter ID must be a lowercase GUID printed by inspect".into());
        }
        Ok(Self {
            runner,
            selected,
            uncertain: false,
        })
    }

    fn call(&mut self, request: Value) -> Result<Value, String> {
        if self.uncertain {
            return Err("A Windows command has an uncertain outcome; start a new recovery command after it settles".into());
        }
        self.runner.call(request).map_err(|e| {
            self.uncertain |= e.uncertain;
            eprintln!("Windows DNS adapter: {}", e.message);
            e.message
        })
    }
}

impl<R: Runner> Platform for WindowsPlatform<R> {
    fn check_environment(&mut self) -> Result<(), String> {
        let selected = self.selected.clone().ok_or("No selected adapter")?;
        self.call(json!({"action":"guard", "interfaceId":selected}))
            .map(|_| ())
    }
    fn snapshot(&mut self) -> Result<Vec<TargetState>, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Snapshot {
            states: Vec<TargetState>,
        }
        let selected = self
            .selected
            .clone()
            .ok_or("An explicit adapter ID is required for activation")?;
        let response = self.call(json!({"action":"snapshot", "interfaceId":selected}))?;
        let snapshot: Snapshot = serde_json::from_value(response).map_err(|e| e.to_string())?;
        if snapshot.states.is_empty()
            || snapshot.states.len() > 2
            || snapshot
                .states
                .iter()
                .any(|s| !valid_target(&s.target) || s.target.interface_id != selected)
        {
            return Err("Invalid Windows adapter snapshot".into());
        }
        super::RecoveryRecord::from_snapshot(snapshot.states.clone())?;
        Ok(snapshot.states)
    }

    fn read(&mut self, target: &Target) -> Result<Option<DnsSetting>, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Setting {
            setting: Option<DnsSetting>,
        }
        if !valid_target(target) {
            return Err("Recovery target is not a machine-bound Windows target".into());
        }
        let response = self.call(json!({"action":"read", "target":target}))?;
        serde_json::from_value::<Setting>(response)
            .map(|s| s.setting)
            .map_err(|e| e.to_string())
    }

    fn compare_and_set(
        &mut self,
        target: &Target,
        expected: &DnsSetting,
        replacement: &DnsSetting,
    ) -> Result<(), String> {
        if !valid_target(target) {
            return Err("Invalid Windows target".into());
        }
        self.call(json!({"action":"cas", "target":target, "expected":expected, "replacement":replacement})).map(|_| ())
    }
}
