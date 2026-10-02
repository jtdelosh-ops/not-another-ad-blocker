//! Offline, conflict-safe macOS DNS recovery. No trial is exposed by this module.
//! The caller must persist a record before any future activation write.
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

pub const RECOVERY_DIRECTORY: &str = "/Library/Application Support/NAAB-DNS-Preview";
#[cfg(target_os = "macos")]
const MAX_RECORD_BYTES: u64 = 384 * 1024;
// JSON stores each byte as a decimal array element. Bound each dictionary so
// two worst-case configurations still fit the durable record size limit.
const MAX_CONFIGURATION_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "xml",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum Configuration {
    NoSavedConfiguration,
    Saved(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DnsState {
    pub enabled: bool,
    pub configuration: Configuration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub version: u32,
    pub session_id: String,
    pub set_id: String,
    pub service_id: String,
    pub device: String,
    /// SHA-256 of the observed primary route, gateway address, and gateway
    /// link-layer address. A missing observation must fail closed.
    pub network_context_sha256: String,
    pub original: DnsState,
    pub applied: DnsState,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

impl Record {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.session_id.len() != 32
            || !self.session_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !valid_id(&self.set_id)
            || !valid_id(&self.service_id)
            || self.device.is_empty()
            || self.device.len() > 32
            || !self
                .device
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || self.network_context_sha256.len() != 64
            || !self
                .network_context_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("Invalid Mac DNS recovery identity".into());
        }
        for state in [&self.original, &self.applied] {
            if let Configuration::Saved(xml) = &state.configuration {
                if xml.is_empty() || xml.len() > MAX_CONFIGURATION_BYTES {
                    return Err("Invalid Mac DNS recovery configuration length".into());
                }
            }
        }
        if !self.applied.enabled
            || !matches!(self.applied.configuration, Configuration::Saved(_))
            || self.original == self.applied
        {
            return Err("Invalid applied Mac DNS recovery setting".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub set_id: String,
    pub service_id: String,
    pub device: String,
    pub network_context_sha256: String,
    pub dns: DnsState,
}

pub trait Settings {
    fn observe(&mut self, record: &Record) -> Result<Observation, String>;
    fn equivalent(&self, left: &DnsState, right: &DnsState) -> Result<bool, String>;
    /// Compare again while holding the native System Configuration preferences
    /// lock; commit and apply only when the saved setting still equals expected.
    fn compare_and_restore(&mut self, record: &Record) -> Result<(), String>;
    /// A committed preference may not yet be active after a failed apply. Ask
    /// macOS to apply the saved configuration before discarding the record.
    fn ensure_applied(&mut self) -> Result<(), String>;
}

pub trait Store {
    fn load(&mut self) -> Result<Option<Record>, String>;
    fn clear(&mut self) -> Result<(), String>;
    fn save_report(&mut self, report: &Report) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Inactive,
    Restored,
    AlreadyOriginal,
    ContextChanged,
    Conflict,
    ReadFailed,
    RestoreFailed,
    JournalFailed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub outcome: Outcome,
    pub recovery_pending: bool,
    pub report_write_failed: bool,
}

fn finish<S: Store>(store: &mut S, outcome: Outcome, pending: bool) -> Report {
    let mut report = Report {
        outcome,
        recovery_pending: pending,
        report_write_failed: false,
    };
    if store.save_report(&report).is_err() {
        report.report_write_failed = true;
    }
    report
}

fn matching_context(record: &Record, observation: &Observation) -> bool {
    record.set_id == observation.set_id
        && record.service_id == observation.service_id
        && record.device == observation.device
        && record.network_context_sha256 == observation.network_context_sha256
}

/// Can run without the resolver, browser, network DNS, or a health probe.
pub fn recover<S: Store, P: Settings>(store: &mut S, settings: &mut P) -> Report {
    let record = match store.load() {
        Ok(None) => return finish(store, Outcome::Inactive, false),
        Ok(Some(record)) => record,
        Err(_) => return finish(store, Outcome::JournalFailed, true),
    };
    if record.validate().is_err() {
        return finish(store, Outcome::JournalFailed, true);
    }
    let observed = match settings.observe(&record) {
        Ok(observed) => observed,
        Err(_) => return finish(store, Outcome::ReadFailed, true),
    };
    if !matching_context(&record, &observed) {
        return finish(store, Outcome::ContextChanged, true);
    }
    let outcome = match settings.equivalent(&observed.dns, &record.original) {
        Ok(true) => Outcome::AlreadyOriginal,
        Ok(false) => match settings.equivalent(&observed.dns, &record.applied) {
            Ok(true) => {
                // A failed native call can still have changed persistent settings.
                let _ = settings.compare_and_restore(&record);
                match settings.observe(&record) {
                    Ok(after) if matching_context(&record, &after) => {
                        match settings.equivalent(&after.dns, &record.original) {
                            Ok(true) => Outcome::Restored,
                            _ => Outcome::RestoreFailed,
                        }
                    }
                    _ => Outcome::RestoreFailed,
                }
            }
            Ok(false) => Outcome::Conflict,
            Err(_) => Outcome::ReadFailed,
        },
        Err(_) => Outcome::ReadFailed,
    };
    if matches!(outcome, Outcome::Restored | Outcome::AlreadyOriginal) {
        if settings.ensure_applied().is_err() {
            return finish(store, Outcome::RestoreFailed, true);
        }
        // The first read may have raced another writer. Never discard the only
        // recovery copy without verifying the complete state once more.
        match settings.observe(&record) {
            Ok(after) if matching_context(&record, &after) => {
                if settings.equivalent(&after.dns, &record.original) != Ok(true) {
                    return finish(store, Outcome::RestoreFailed, true);
                }
            }
            _ => return finish(store, Outcome::ReadFailed, true),
        }
        if store.clear().is_err() {
            return finish(store, Outcome::JournalFailed, true);
        }
        finish(store, outcome, false)
    } else {
        finish(store, outcome, true)
    }
}

/// The fixed directory is owned by root and cannot be supplied by the caller.
#[cfg(target_os = "macos")]
pub struct FileStore {
    directory: PathBuf,
    _lock: std::fs::File,
}

#[cfg(target_os = "macos")]
fn run_network_command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("Could not inspect Mac network context: {e}"))?;
    if !output.status.success() || output.stdout.len() > 16 * 1024 {
        return Err("Mac network context is unavailable".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "Invalid Mac network context output".into())
}

#[cfg(any(test, target_os = "macos"))]
fn parse_route(output: &str) -> Result<(String, String), String> {
    let mut device = None;
    let mut gateway = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("interface:") {
            if device.replace(value.trim().to_owned()).is_some() {
                return Err("Duplicate default-route interface".into());
            }
        } else if let Some(value) = line.strip_prefix("gateway:") {
            if gateway.replace(value.trim().to_owned()).is_some() {
                return Err("Duplicate default-route gateway".into());
            }
        }
    }
    let device = device.ok_or("Default-route interface is missing")?;
    if device.is_empty()
        || device.len() > 32
        || !device
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("Invalid default-route interface".into());
    }
    let gateway = gateway.ok_or("Default-route gateway is missing")?;
    let address: std::net::Ipv4Addr = gateway.parse().map_err(|_| "Invalid IPv4 gateway")?;
    if address.is_unspecified() || address.is_loopback() || address.is_multicast() {
        return Err("Unsupported IPv4 gateway".into());
    }
    Ok((device, address.to_string()))
}

#[cfg(any(test, target_os = "macos"))]
fn parse_gateway_mac(output: &str, gateway: &str, device: &str) -> Result<String, String> {
    let mut found = None;
    for line in output.lines() {
        let marker = format!("({gateway}) at ");
        if let Some((_, tail)) = line.split_once(&marker) {
            let (address, interface) = tail.split_once(" on ").ok_or("Invalid gateway neighbor")?;
            let octets: Option<Vec<u8>> = address
                .split(':')
                .map(|part| {
                    (part.len() >= 1 && part.len() <= 2)
                        .then(|| u8::from_str_radix(part, 16).ok())
                        .flatten()
                })
                .collect();
            let octets = octets.ok_or("Invalid gateway link-layer address")?;
            if interface.split_whitespace().next() != Some(device)
                || octets.len() != 6
                || octets.iter().all(|byte| *byte == 0)
                || octets.iter().all(|byte| *byte == 255)
                || found
                    .replace(
                        octets
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<Vec<_>>()
                            .join(":"),
                    )
                    .is_some()
            {
                return Err("Gateway link-layer identity is unavailable".into());
            }
        }
    }
    found.ok_or("Gateway link-layer identity is unavailable".into())
}

#[cfg(target_os = "macos")]
fn network_context(expected_device: &str) -> Result<String, String> {
    let route = run_network_command("/sbin/route", &["-n", "get", "default"])?;
    let (device, gateway) = parse_route(&route)?;
    if device != expected_device {
        return Err("Primary Mac network interface changed".into());
    }
    let neighbor = run_network_command("/usr/sbin/arp", &["-n", &gateway])?;
    let mac = parse_gateway_mac(&neighbor, &gateway, &device)?;
    let key = format!("ipv4-gateway-v1\0{device}\0{gateway}\0{mac}");
    Ok(Sha256::digest(key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Production adapter for the fixed, privileged offline recovery command.
#[cfg(target_os = "macos")]
pub struct NativeSettings;

#[cfg(target_os = "macos")]
impl Settings for NativeSettings {
    fn observe(&mut self, record: &Record) -> Result<Observation, String> {
        use super::macos_identity::{self, DnsConfigurationState};
        let identity = macos_identity::inspect()?;
        let service = identity
            .services
            .iter()
            .find(|service| service.id == record.service_id)
            .ok_or("Saved Mac network service is missing")?;
        if !service.enabled || service.device.as_deref() != Some(&record.device) {
            return Err("Saved Mac network service changed".into());
        }
        let network_context_sha256 = network_context(&record.device)?;
        let protocol =
            macos_identity::inspect_dns_protocol(&identity.current_set_id, &record.service_id)?;
        let configuration = match protocol.configuration_state() {
            DnsConfigurationState::Saved => Configuration::Saved(
                protocol
                    .configuration_xml
                    .ok_or("Missing saved Mac DNS configuration")?,
            ),
            DnsConfigurationState::NoSavedConfiguration => Configuration::NoSavedConfiguration,
            DnsConfigurationState::Unknown => {
                return Err("Mac DNS protocol configuration is unknown".into())
            }
        };
        Ok(Observation {
            set_id: identity.current_set_id,
            service_id: service.id.clone(),
            device: record.device.clone(),
            network_context_sha256,
            dns: DnsState {
                enabled: protocol.protocol_enabled,
                configuration,
            },
        })
    }

    fn equivalent(&self, left: &DnsState, right: &DnsState) -> Result<bool, String> {
        if left.enabled != right.enabled {
            return Ok(false);
        }
        match (&left.configuration, &right.configuration) {
            (Configuration::NoSavedConfiguration, Configuration::NoSavedConfiguration) => Ok(true),
            (Configuration::Saved(left), Configuration::Saved(right)) => {
                super::macos_identity::equivalent_dns_configuration(left, right)
            }
            _ => Ok(false),
        }
    }

    fn compare_and_restore(&mut self, record: &Record) -> Result<(), String> {
        super::macos_identity::restore_dns_protocol(record, || {
            Ok(network_context(&record.device)? == record.network_context_sha256)
        })
    }

    fn ensure_applied(&mut self) -> Result<(), String> {
        super::macos_identity::request_dns_apply()
    }
}

#[cfg(target_os = "macos")]
fn reject_extended_acl(path: &Path) -> Result<(), String> {
    use std::ffi::{c_char, c_void, CString};
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn acl_get_link_np(path: *const c_char, acl_type: i32) -> *mut c_void;
        fn acl_get_entry(acl: *mut c_void, entry_id: i32, entry: *mut *mut c_void) -> i32;
        fn acl_free(acl: *mut c_void) -> i32;
    }
    // Darwin sys/acl.h: ACL_TYPE_EXTENDED = 0x100, ACL_FIRST_ENTRY = 0.
    // An ACL can grant access that the BSD mode bits appear to deny.
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "Invalid Mac recovery path".to_owned())?;
    let acl = unsafe { acl_get_link_np(path.as_ptr(), 0x100) };
    if acl.is_null() {
        let error = std::io::Error::last_os_error();
        // Darwin also returns ENOENT when the existing path has no extended
        // ACL. The caller already verified the path with symlink_metadata.
        if error.raw_os_error() == Some(2) {
            return Ok(());
        }
        return Err(format!("Could not inspect Mac recovery ACL: {error}"));
    }
    let mut entry = std::ptr::null_mut();
    let first = unsafe { acl_get_entry(acl, 0, &mut entry) };
    let error = std::io::Error::last_os_error();
    unsafe { acl_free(acl) };
    if first == 0 {
        return Err("Mac recovery path has an access-control list".into());
    }
    // Darwin reports EINVAL when an otherwise valid ACL has no first entry.
    if first != -1 || error.raw_os_error() != Some(22) {
        return Err(format!(
            "Could not inspect Mac recovery ACL entries: {error}"
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn check_private(path: &Path, directory: bool) -> Result<bool, String> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if (directory && !meta.is_dir())
                || (!directory && (!meta.is_file() || meta.nlink() != 1))
                || meta.uid() != 0
                || meta.mode() & 0o077 != 0
            {
                return Err("Mac recovery storage ownership or permissions are unsafe".into());
            }
            reject_extended_acl(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(target_os = "macos")]
impl FileStore {
    pub fn open_fixed() -> Result<Self, String> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        if unsafe { geteuid() } != 0 {
            return Err("Run Mac DNS recovery as root".into());
        }
        let directory = PathBuf::from(RECOVERY_DIRECTORY);
        let parent = directory.parent().ok_or("Missing recovery parent")?;
        // /Library and Application Support are system-owned; neither may be
        // replaced by a user-controlled symlink before creating the child.
        for ancestor in [Path::new("/Library"), parent] {
            let meta = std::fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?;
            use std::os::unix::fs::MetadataExt;
            if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
                return Err("Mac recovery parent is not trusted".into());
            }
            reject_extended_acl(ancestor)?;
        }
        if !check_private(&directory, true)? {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .map_err(|e| e.to_string())?;
        }
        check_private(&directory, true)?;
        let lock_path = directory.join("controller.lock");
        check_private(&lock_path, false)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .open(&lock_path)
            .map_err(|e| e.to_string())?;
        check_private(&lock_path, false)?;
        lock.try_lock()
            .map_err(|e| format!("Mac recovery is already running: {e}"))?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }

    fn write_json(&self, name: &str, value: &impl Serialize) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err("Mac recovery file exceeds size limit".into());
        }
        let target = self.directory.join(name);
        check_private(&target, false)?;
        let temporary = self
            .directory
            .join(format!("{name}.{:016x}.pending", rand::random::<u64>()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        check_private(&temporary, false)?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, &target).map_err(|e| e.to_string())?;
        std::fs::File::open(&self.directory)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())
    }

    /// Future activation must call this before any DNS mutation.
    pub fn create(&mut self, record: &Record) -> Result<(), String> {
        record.validate()?;
        if check_private(&self.directory.join("recovery.json"), false)? {
            return Err("Recover the pending Mac DNS session first".into());
        }
        self.write_json("recovery.json", record)
    }
}

#[cfg(target_os = "macos")]
impl Store for FileStore {
    fn load(&mut self) -> Result<Option<Record>, String> {
        use std::io::Read;
        let path = self.directory.join("recovery.json");
        if !check_private(&path, false)? {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(MAX_RECORD_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err("Mac recovery record exceeds size limit".into());
        }
        let record: Record = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        record.validate()?;
        Ok(Some(record))
    }

    fn clear(&mut self) -> Result<(), String> {
        let path = self.directory.join("recovery.json");
        if check_private(&path, false)? {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        std::fs::File::open(&self.directory)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())
    }

    fn save_report(&mut self, report: &Report) -> Result<(), String> {
        self.write_json("last-report.json", report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Record {
        Record {
            version: 1,
            session_id: "1".repeat(32),
            set_id: "set".into(),
            service_id: "service".into(),
            device: "en0".into(),
            network_context_sha256: "a".repeat(64),
            original: DnsState {
                enabled: true,
                configuration: Configuration::NoSavedConfiguration,
            },
            applied: DnsState {
                enabled: true,
                configuration: Configuration::Saved(b"applied".to_vec()),
            },
        }
    }

    struct MemoryStore {
        record: Option<Record>,
        report: Option<Report>,
    }
    impl Store for MemoryStore {
        fn load(&mut self) -> Result<Option<Record>, String> {
            Ok(self.record.clone())
        }
        fn clear(&mut self) -> Result<(), String> {
            self.record = None;
            Ok(())
        }
        fn save_report(&mut self, report: &Report) -> Result<(), String> {
            self.report = Some(report.clone());
            Ok(())
        }
    }

    struct FakeSettings {
        observation: Observation,
        write_count: usize,
        fail_after_write: bool,
        reads: usize,
        edit_on_read: Option<usize>,
        apply_fails: bool,
    }
    impl Settings for FakeSettings {
        fn observe(&mut self, _: &Record) -> Result<Observation, String> {
            self.reads += 1;
            if self.edit_on_read == Some(self.reads) {
                self.observation.dns = DnsState {
                    enabled: true,
                    configuration: Configuration::Saved(b"third party".to_vec()),
                };
            }
            Ok(self.observation.clone())
        }
        fn equivalent(&self, left: &DnsState, right: &DnsState) -> Result<bool, String> {
            Ok(left == right)
        }
        fn compare_and_restore(&mut self, record: &Record) -> Result<(), String> {
            self.write_count += 1;
            self.observation.dns = record.original.clone();
            if self.fail_after_write {
                Err("apply uncertain".into())
            } else {
                Ok(())
            }
        }
        fn ensure_applied(&mut self) -> Result<(), String> {
            if self.apply_fails {
                Err("apply failed".into())
            } else {
                Ok(())
            }
        }
    }

    fn fixture(current: DnsState) -> (MemoryStore, FakeSettings) {
        let record = record();
        let observation = Observation {
            set_id: record.set_id.clone(),
            service_id: record.service_id.clone(),
            device: record.device.clone(),
            network_context_sha256: record.network_context_sha256.clone(),
            dns: current,
        };
        (
            MemoryStore {
                record: Some(record),
                report: None,
            },
            FakeSettings {
                observation,
                write_count: 0,
                fail_after_write: false,
                reads: 0,
                edit_on_read: None,
                apply_fails: false,
            },
        )
    }

    #[test]
    fn restores_applied_setting_and_clears_record() {
        let applied = record().applied;
        let (mut store, mut settings) = fixture(applied);
        assert_eq!(
            recover(&mut store, &mut settings).outcome,
            Outcome::Restored
        );
        assert!(store.record.is_none());
        assert_eq!(settings.write_count, 1);
    }

    #[test]
    fn retains_record_on_context_change_or_third_party_edit() {
        let applied = record().applied;
        let (mut store, mut settings) = fixture(applied);
        settings.observation.network_context_sha256 = "b".repeat(64);
        assert_eq!(
            recover(&mut store, &mut settings).outcome,
            Outcome::ContextChanged
        );
        assert_eq!(settings.write_count, 0);
        assert!(store.record.is_some());
        settings.observation.network_context_sha256 = "a".repeat(64);
        settings.observation.dns = DnsState {
            enabled: true,
            configuration: Configuration::Saved(b"third party".to_vec()),
        };
        assert_eq!(
            recover(&mut store, &mut settings).outcome,
            Outcome::Conflict
        );
        assert_eq!(settings.write_count, 0);
        assert!(store.record.is_some());
    }

    #[test]
    fn readback_reconciles_uncertain_write() {
        let (mut store, mut settings) = fixture(record().applied);
        settings.fail_after_write = true;
        assert_eq!(
            recover(&mut store, &mut settings).outcome,
            Outcome::Restored
        );
        assert!(store.record.is_none());
    }

    #[test]
    fn already_original_needs_no_write() {
        let (mut store, mut settings) = fixture(record().original);
        assert_eq!(
            recover(&mut store, &mut settings).outcome,
            Outcome::AlreadyOriginal
        );
        assert_eq!(settings.write_count, 0);
        assert!(store.record.is_none());
    }

    #[test]
    fn final_readback_retains_record_when_state_changes_before_clear() {
        let (mut store, mut settings) = fixture(record().original);
        settings.edit_on_read = Some(2);
        assert_eq!(
            recover(&mut store, &mut settings).outcome,
            Outcome::RestoreFailed
        );
        assert!(store.record.is_some());
        assert_eq!(settings.write_count, 0);
    }

    #[test]
    fn failed_apply_keeps_original_setting_recorded_for_retry() {
        let (mut store, mut settings) = fixture(record().original);
        settings.apply_fails = true;
        assert_eq!(
            recover(&mut store, &mut settings).outcome,
            Outcome::RestoreFailed
        );
        assert!(store.record.is_some());
    }

    #[test]
    fn context_parsing_requires_matching_gateway_neighbor() {
        let (device, gateway) =
            parse_route("route to: default\ngateway: 192.0.2.1\ninterface: en0\n").unwrap();
        assert_eq!(device, "en0");
        assert_eq!(gateway, "192.0.2.1");
        assert_eq!(
            parse_gateway_mac(
                "? (192.0.2.1) at aa:bb:cc:dd:ee:ff on en0 ifscope [ethernet]",
                &gateway,
                &device
            )
            .unwrap(),
            "aa:bb:cc:dd:ee:ff"
        );
        assert!(
            parse_gateway_mac("? (192.0.2.1) at (incomplete) on en0", &gateway, &device).is_err()
        );
        assert!(parse_gateway_mac(
            "? (192.0.2.1) at aa:bb:cc:dd:ee:ff on en1",
            &gateway,
            &device
        )
        .is_err());
    }
}
