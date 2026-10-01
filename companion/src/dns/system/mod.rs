//! System DNS transaction foundation, offline simulator and Windows preview.
//! Each caller owns one controller and its polling schedule; importing this
//! module neither changes OS settings nor installs a service.

pub mod health;
pub mod journal;
pub mod macos;
pub mod macos_preflight;
pub mod simulation;
pub mod trial;
pub mod windows;

use serde::{Deserialize, Serialize};
use std::{collections::HashSet, net::IpAddr, time::SystemTime};

pub const MAX_TARGETS: usize = 64;
pub const RECORD_VERSION: u32 = 1;
const LOCAL_FAILURE_LIMIT: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Family {
    Ipv4,
    Ipv6,
}

/// Adapter-owned stable identity, including the network context. Never a display
/// name or an interface index that may have been reused after reboot.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub interface_id: String,
    pub network_id: String,
    pub family: Family,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    content = "servers",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum DnsSetting {
    Automatic,
    Static(Vec<IpAddr>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetState {
    pub target: Target,
    pub setting: DnsSetting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub target: Target,
    pub original: DnsSetting,
    pub applied: DnsSetting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryRecord {
    pub version: u32,
    pub session_id: String,
    pub created_unix_ms: u64,
    pub changes: Vec<Change>,
}

fn loopback(family: Family) -> DnsSetting {
    DnsSetting::Static(vec![match family {
        Family::Ipv4 => IpAddr::from([127, 0, 0, 1]),
        Family::Ipv6 => IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1]),
    }])
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

impl RecoveryRecord {
    pub fn from_snapshot(snapshot: Vec<TargetState>) -> Result<Self, String> {
        let record = Self {
            version: RECORD_VERSION,
            session_id: format!("{:032x}", rand::random::<u128>()),
            created_unix_ms: now_ms(),
            changes: snapshot
                .into_iter()
                .map(|state| Change {
                    applied: loopback(state.target.family),
                    target: state.target,
                    original: state.setting,
                })
                .collect(),
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != RECORD_VERSION
            || self.session_id.len() != 32
            || !self.session_id.bytes().all(|b| b.is_ascii_hexdigit())
            || self.changes.is_empty()
            || self.changes.len() > MAX_TARGETS
        {
            return Err("Unsupported or invalid DNS recovery record".into());
        }
        let mut seen = HashSet::new();
        for change in &self.changes {
            if !valid_id(&change.target.interface_id)
                || !valid_id(&change.target.network_id)
                || !seen.insert((&change.target.interface_id, change.target.family))
                || change.applied != loopback(change.target.family)
            {
                return Err("Invalid or duplicate DNS recovery target".into());
            }
            if let DnsSetting::Static(servers) = &change.original {
                if servers.is_empty() || servers.len() > 8 {
                    return Err("Invalid original DNS server count".into());
                }
                for server in servers {
                    let canonical = server.to_canonical();
                    if server.is_ipv4() != (change.target.family == Family::Ipv4)
                        || canonical.is_unspecified()
                        || canonical.is_loopback()
                        || canonical.is_multicast()
                        || matches!(canonical, IpAddr::V4(ip) if ip.is_broadcast())
                    {
                        return Err("Unsupported original DNS server address".into());
                    }
                }
            }
        }
        Ok(())
    }
}

/// A missing target includes an interface whose network identity has changed.
/// Implementations must not substitute a different interface or network.
pub trait Platform {
    fn snapshot(&mut self) -> Result<Vec<TargetState>, String>;
    /// Activation-only constraints (for example a new VPN or managed DNS policy).
    /// Restoration deliberately does not depend on this guard: it still attempts
    /// to restore matching original targets if the wider environment changed.
    fn check_environment(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn read(&mut self, target: &Target) -> Result<Option<DnsSetting>, String>;
    /// Recheck expected state inside the adapter immediately before mutation.
    /// An error may occur after the mutation; recovery therefore always reads back.
    /// Each family must be changed as one setting. Adapters must coordinate their
    /// writers and detect unsupported configurations rather than approximate them.
    fn compare_and_set(
        &mut self,
        target: &Target,
        expected: &DnsSetting,
        replacement: &DnsSetting,
    ) -> Result<(), String>;
}

/// The journal owns exclusive process access for the controller's lifetime.
/// `create` must persist a validated record before returning success, refusing to
/// replace an outstanding record. Reports must never be needed for recovery.
pub trait Journal {
    fn load(&mut self) -> Result<Option<RecoveryRecord>, String>;
    fn create(&mut self, record: &RecoveryRecord) -> Result<(), String>;
    fn clear(&mut self) -> Result<(), String>;
    fn save_report(&mut self, report: &Report) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Check {
    Passed,
    Failed,
    Unknown,
}

/// Local health must be established independently of public DNS, for both UDP
/// and TCP and every enabled address family. A cached public answer alone cannot
/// establish upstream reachability. The probe adapter enforces bounded deadlines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Health {
    pub local: Check,
    pub upstream: Check,
}

pub trait HealthProbe {
    fn check(&mut self, changes: &[Change]) -> Health;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Inactive,
    Active,
    Degraded,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Enable,
    Disable,
    Recover,
    HealthPoll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    Enabled,
    Disabled,
    Recovered,
    NotActive,
    AlreadyActive,
    PreflightUnhealthy,
    SnapshotFailed,
    InvalidSnapshot,
    JournalUnavailable,
    PendingRecovery,
    ApplyFailed,
    PostflightUnhealthy,
    UpstreamUnavailable,
    LocalHealthUncertain,
    LocalHealthFailed,
    ConfigurationChanged,
    Healthy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetOutcome {
    Restored,
    AlreadyOriginal,
    Missing,
    Conflict,
    ReadFailed,
    RestoreFailed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetReport {
    pub target: Target,
    pub outcome: TargetOutcome,
}

/// Fixed-size local diagnostic report; contains no browsing queries, credentials,
/// or unbounded platform error strings. `report_write_failed` is visible to the
/// caller even when disk failure prevents saving the report itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Report {
    pub version: u32,
    pub timestamp_unix_ms: u64,
    pub action: Action,
    pub state: State,
    pub reason: Reason,
    pub health: Option<Health>,
    pub targets: Vec<TargetReport>,
    pub report_write_failed: bool,
}

impl Report {
    pub fn is_incident(&self) -> bool {
        self.state == State::RecoveryRequired
            || !matches!(
                self.reason,
                Reason::Enabled
                    | Reason::Disabled
                    | Reason::Recovered
                    | Reason::NotActive
                    | Reason::AlreadyActive
                    | Reason::Healthy
            )
    }
}

pub struct Controller<P, J, H> {
    platform: P,
    journal: J,
    probe: H,
    state: State,
    local_failures: u8,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl<P: Platform, J: Journal, H: HealthProbe> Controller<P, J, H> {
    pub fn new(platform: P, journal: J, probe: H) -> Self {
        // No implicit resume after restart: `recover` must reconcile disk and OS.
        Self {
            platform,
            journal,
            probe,
            state: State::Inactive,
            local_failures: 0,
        }
    }

    pub fn into_parts(self) -> (P, J, H) {
        (self.platform, self.journal, self.probe)
    }

    fn report(
        &mut self,
        action: Action,
        reason: Reason,
        health: Option<Health>,
        targets: Vec<TargetReport>,
    ) -> Report {
        let mut report = Report {
            version: 1,
            timestamp_unix_ms: now_ms(),
            action,
            state: self.state,
            reason,
            health,
            targets,
            report_write_failed: false,
        };
        report.report_write_failed = self.journal.save_report(&report).is_err();
        report
    }

    fn record(&mut self) -> Result<Option<RecoveryRecord>, String> {
        let record = self.journal.load()?;
        if let Some(record) = &record {
            record.validate()?;
        }
        Ok(record)
    }

    pub fn enable(&mut self) -> Report {
        match self.record() {
            Err(_) => {
                self.state = State::RecoveryRequired;
                return self.report(Action::Enable, Reason::JournalUnavailable, None, vec![]);
            }
            Ok(Some(_)) => {
                let reason = if matches!(self.state, State::Active | State::Degraded) {
                    Reason::AlreadyActive
                } else {
                    self.state = State::RecoveryRequired;
                    Reason::PendingRecovery
                };
                return self.report(Action::Enable, reason, None, vec![]);
            }
            Ok(None) => {
                if self.state != State::Inactive {
                    self.state = State::RecoveryRequired;
                    return self.report(Action::Enable, Reason::JournalUnavailable, None, vec![]);
                }
            }
        }
        self.state = State::Inactive;
        self.local_failures = 0;
        let snapshot = match self.platform.snapshot() {
            Ok(snapshot) => snapshot,
            Err(_) => return self.report(Action::Enable, Reason::SnapshotFailed, None, vec![]),
        };
        let record = match RecoveryRecord::from_snapshot(snapshot) {
            Ok(record) => record,
            Err(_) => return self.report(Action::Enable, Reason::InvalidSnapshot, None, vec![]),
        };
        let health = self.probe.check(&record.changes);
        if health.local != Check::Passed || health.upstream != Check::Passed {
            return self.report(
                Action::Enable,
                Reason::PreflightUnhealthy,
                Some(health),
                vec![],
            );
        }
        if self.journal.create(&record).is_err() {
            // No platform write has happened. If a committed record exists (or
            // cannot be read), keep it for recovery; otherwise a retry is safe.
            self.state = if matches!(self.record(), Ok(None)) {
                State::Inactive
            } else {
                State::RecoveryRequired
            };
            return self.report(
                Action::Enable,
                Reason::JournalUnavailable,
                Some(health),
                vec![],
            );
        }
        for change in &record.changes {
            if self
                .platform
                .compare_and_set(&change.target, &change.original, &change.applied)
                .is_err()
                || self.platform.read(&change.target).ok().flatten().as_ref()
                    != Some(&change.applied)
            {
                return self.restore(Action::Enable, Reason::ApplyFailed, Some(health));
            }
        }
        let health = self.probe.check(&record.changes);
        if health.local != Check::Passed || health.upstream != Check::Passed {
            return self.restore(Action::Enable, Reason::PostflightUnhealthy, Some(health));
        }
        if self.platform.check_environment().is_err() {
            return self.restore(Action::Enable, Reason::ConfigurationChanged, Some(health));
        }
        for change in &record.changes {
            if self.platform.read(&change.target).ok().flatten().as_ref() != Some(&change.applied) {
                return self.restore(Action::Enable, Reason::ConfigurationChanged, Some(health));
            }
        }
        self.state = State::Active;
        self.report(Action::Enable, Reason::Enabled, Some(health), vec![])
    }

    pub fn disable(&mut self) -> Report {
        self.restore(Action::Disable, Reason::Disabled, None)
    }

    /// Also used on service startup and by a future independent recovery helper.
    /// Requires no probe, internet access, or running resolver.
    pub fn recover(&mut self) -> Report {
        self.restore(Action::Recover, Reason::Recovered, None)
    }

    fn restore(&mut self, action: Action, reason: Reason, health: Option<Health>) -> Report {
        let record = match self.record() {
            Err(_) => {
                self.state = State::RecoveryRequired;
                return self.report(action, Reason::JournalUnavailable, health, vec![]);
            }
            Ok(None) => {
                // Losing the journal while active is not evidence of restoration.
                if matches!(
                    self.state,
                    State::Active | State::Degraded | State::RecoveryRequired
                ) {
                    self.state = State::RecoveryRequired;
                    return self.report(action, Reason::JournalUnavailable, health, vec![]);
                }
                return self.report(action, Reason::NotActive, health, vec![]);
            }
            Ok(Some(record)) => record,
        };
        self.state = State::RecoveryRequired;
        self.local_failures = 0;
        let mut results = Vec::new();
        for change in record.changes.iter().rev() {
            let outcome = match self.platform.read(&change.target) {
                Err(_) => TargetOutcome::ReadFailed,
                Ok(None) => TargetOutcome::Missing,
                Ok(Some(current)) if current == change.original => TargetOutcome::AlreadyOriginal,
                Ok(Some(current)) if current == change.applied => {
                    // An adapter can report failure after changing the OS. Read-back
                    // establishes the outcome, including a successful retry.
                    let _ = self.platform.compare_and_set(
                        &change.target,
                        &change.applied,
                        &change.original,
                    );
                    match self.platform.read(&change.target) {
                        Ok(Some(setting)) if setting == change.original => TargetOutcome::Restored,
                        _ => TargetOutcome::RestoreFailed,
                    }
                }
                Ok(Some(_)) => TargetOutcome::Conflict,
            };
            results.push(TargetReport {
                target: change.target.clone(),
                outcome,
            });
        }
        if results.iter().all(|result| {
            matches!(
                result.outcome,
                TargetOutcome::Restored | TargetOutcome::AlreadyOriginal
            )
        }) {
            // A later platform write can disturb a family already restored.
            // Recheck the complete saved state before discarding its backup.
            for (change, result) in record.changes.iter().rev().zip(results.iter_mut()) {
                result.outcome = match self.platform.read(&change.target) {
                    Ok(Some(setting)) if setting == change.original => result.outcome,
                    Ok(Some(setting)) if setting == change.applied => TargetOutcome::RestoreFailed,
                    Ok(Some(_)) => TargetOutcome::Conflict,
                    Ok(None) => TargetOutcome::Missing,
                    Err(_) => TargetOutcome::ReadFailed,
                };
            }
            if results.iter().all(|result| {
                matches!(
                    result.outcome,
                    TargetOutcome::Restored | TargetOutcome::AlreadyOriginal
                )
            }) {
                if self.journal.clear().is_err() {
                    return self.report(action, Reason::JournalUnavailable, health, results);
                }
                self.state = State::Inactive;
            }
        }
        self.report(action, reason, health, results)
    }

    pub fn poll_health(&mut self) -> Report {
        let record = match self.record() {
            Err(_) => {
                self.state = State::RecoveryRequired;
                return self.report(Action::HealthPoll, Reason::JournalUnavailable, None, vec![]);
            }
            Ok(None) => {
                let reason = if self.state != State::Inactive {
                    self.state = State::RecoveryRequired;
                    Reason::JournalUnavailable
                } else {
                    Reason::NotActive
                };
                return self.report(Action::HealthPoll, reason, None, vec![]);
            }
            Ok(Some(record)) => record,
        };
        if !matches!(self.state, State::Active | State::Degraded) {
            self.state = State::RecoveryRequired;
            return self.report(Action::HealthPoll, Reason::PendingRecovery, None, vec![]);
        }
        if self.platform.check_environment().is_err() {
            return self.restore(Action::HealthPoll, Reason::ConfigurationChanged, None);
        }
        for change in &record.changes {
            if self.platform.read(&change.target).ok().flatten().as_ref() != Some(&change.applied) {
                return self.restore(Action::HealthPoll, Reason::ConfigurationChanged, None);
            }
        }
        let health = self.probe.check(&record.changes);
        if health.local != Check::Passed {
            self.local_failures = self.local_failures.saturating_add(1);
            if self.local_failures >= LOCAL_FAILURE_LIMIT {
                return self.restore(Action::HealthPoll, Reason::LocalHealthFailed, Some(health));
            }
            self.state = State::Degraded;
            return self.report(
                Action::HealthPoll,
                Reason::LocalHealthUncertain,
                Some(health),
                vec![],
            );
        }
        self.local_failures = 0;
        let reason = if health.upstream == Check::Passed {
            self.state = State::Active;
            Reason::Healthy
        } else {
            self.state = State::Degraded;
            Reason::UpstreamUnavailable
        };
        self.report(Action::HealthPoll, reason, Some(health), vec![])
    }
}
