use naab_companion::dns::system::{
    journal::FileJournal,
    simulation::{SimulatedPlatform, SimulatedProbe, HEALTHY},
    *,
};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("naab-system-dns-{:032x}", rand::random::<u128>()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn journal(&self) -> FileJournal {
        FileJournal::open(&self.0).unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct MemoryState {
    record: Option<RecoveryRecord>,
    reports: Vec<Report>,
    fail_create: bool,
    fail_load: bool,
    fail_clear: bool,
    fail_report: bool,
}
#[derive(Clone, Default)]
struct MemoryJournal(Arc<Mutex<MemoryState>>);
impl Journal for MemoryJournal {
    fn load(&mut self) -> Result<Option<RecoveryRecord>, String> {
        let state = self.0.lock().unwrap();
        if state.fail_load {
            return Err("Injected read failure".into());
        }
        Ok(state.record.clone())
    }
    fn create(&mut self, record: &RecoveryRecord) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        if state.fail_create {
            return Err("Injected durable-write failure".into());
        }
        assert!(
            state.record.is_none(),
            "Never replace an outstanding backup"
        );
        state.record = Some(record.clone());
        Ok(())
    }
    fn clear(&mut self) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        if state.fail_clear {
            return Err("Injected removal failure".into());
        }
        state.record = None;
        Ok(())
    }
    fn save_report(&mut self, report: &Report) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        if state.fail_report {
            return Err("Injected report failure".into());
        }
        state.reports.push(report.clone());
        Ok(())
    }
}

fn local_failure() -> Health {
    Health {
        local: Check::Failed,
        upstream: Check::Unknown,
    }
}
fn upstream_failure() -> Health {
    Health {
        local: Check::Passed,
        upstream: Check::Failed,
    }
}

#[test]
fn later_family_reset_cannot_clear_backup_after_clobbering_an_earlier_restore() {
    struct BroadReset(SimulatedPlatform);
    impl Platform for BroadReset {
        fn snapshot(&mut self) -> Result<Vec<TargetState>, String> {
            self.0.snapshot()
        }
        fn read(&mut self, target: &Target) -> Result<Option<DnsSetting>, String> {
            self.0.read(target)
        }
        fn compare_and_set(
            &mut self,
            target: &Target,
            expected: &DnsSetting,
            replacement: &DnsSetting,
        ) -> Result<(), String> {
            self.0.compare_and_set(target, expected, replacement)?;
            if *replacement == DnsSetting::Automatic {
                // Model the Windows reset side effect: it changes the other
                // family after that family's immediate read-back succeeded.
                for state in &mut self.0 .0.lock().unwrap().targets {
                    state.setting = DnsSetting::Automatic;
                }
            }
            Ok(())
        }
    }
    let machine = SimulatedPlatform::example(); // automatic IPv4, static IPv6
    let journal = MemoryJournal::default();
    let mut controller = Controller::new(
        BroadReset(machine.clone()),
        journal.clone(),
        SimulatedProbe::default(),
    );
    assert_eq!(controller.enable().state, State::Active);
    let report = controller.disable();
    assert_eq!(report.state, State::RecoveryRequired);
    assert!(report.targets.iter().any(|result| {
        result.target.family == Family::Ipv6 && result.outcome == TargetOutcome::Conflict
    }));
    assert!(journal.0.lock().unwrap().record.is_some());
}

#[test]
fn environment_guard_changes_restore_matching_settings_without_blocking_recovery() {
    struct Guarded {
        machine: SimulatedPlatform,
        checks_left: usize,
    }
    impl Platform for Guarded {
        fn snapshot(&mut self) -> Result<Vec<TargetState>, String> {
            self.machine.snapshot()
        }
        fn read(&mut self, target: &Target) -> Result<Option<DnsSetting>, String> {
            self.machine.read(target)
        }
        fn compare_and_set(
            &mut self,
            target: &Target,
            expected: &DnsSetting,
            replacement: &DnsSetting,
        ) -> Result<(), String> {
            self.machine.compare_and_set(target, expected, replacement)
        }
        fn check_environment(&mut self) -> Result<(), String> {
            if self.checks_left == 0 {
                return Err("New VPN/policy".into());
            }
            self.checks_left -= 1;
            Ok(())
        }
    }
    for checks_left in [0, 1] {
        let machine = SimulatedPlatform::example();
        let original = machine.0.lock().unwrap().targets.clone();
        let journal = MemoryJournal::default();
        let mut controller = Controller::new(
            Guarded {
                machine: machine.clone(),
                checks_left,
            },
            journal.clone(),
            SimulatedProbe::default(),
        );
        let enabled = controller.enable();
        let report = if checks_left == 0 {
            enabled
        } else {
            assert_eq!(enabled.state, State::Active);
            controller.poll_health()
        };
        assert_eq!(report.state, State::Inactive);
        assert_eq!(report.reason, Reason::ConfigurationChanged);
        assert_eq!(machine.0.lock().unwrap().targets, original);
        assert!(journal.0.lock().unwrap().record.is_none());
    }
}

#[test]
fn enable_disable_preserves_automatic_static_and_address_families() {
    let scratch = Scratch::new();
    let platform = SimulatedPlatform::example();
    let original = platform.0.lock().unwrap().targets.clone();
    let mut controller = Controller::new(
        platform.clone(),
        scratch.journal(),
        SimulatedProbe::default(),
    );
    assert_eq!(controller.enable().state, State::Active);
    let saved = fs::read(scratch.0.join("recovery.json")).unwrap();
    let record: RecoveryRecord = serde_json::from_slice(&saved).unwrap();
    assert_eq!(record.changes[0].original, DnsSetting::Automatic);
    assert_eq!(record.changes[1].original, original[1].setting);
    assert_eq!(controller.enable().reason, Reason::AlreadyActive);
    assert_eq!(fs::read(scratch.0.join("recovery.json")).unwrap(), saved);
    assert_eq!(platform.0.lock().unwrap().writes, 2);
    let disabled = controller.disable();
    assert_eq!(disabled.state, State::Inactive);
    assert!(disabled
        .targets
        .iter()
        .all(|t| t.outcome == TargetOutcome::Restored));
    assert_eq!(platform.0.lock().unwrap().targets, original);
    assert!(!scratch.0.join("recovery.json").exists());
    assert_eq!(controller.disable().reason, Reason::NotActive);
    assert_eq!(controller.enable().state, State::Active);
    assert_eq!(controller.disable().state, State::Inactive);
}

#[test]
fn unhealthy_preflight_or_failed_backup_never_changes_platform() {
    for health in [
        local_failure(),
        upstream_failure(),
        Health {
            local: Check::Unknown,
            upstream: Check::Passed,
        },
    ] {
        let platform = SimulatedPlatform::example();
        let journal = MemoryJournal::default();
        let probe = SimulatedProbe::default();
        probe.0.lock().unwrap().push_back(health);
        let mut controller = Controller::new(platform.clone(), journal.clone(), probe);
        assert_eq!(controller.enable().reason, Reason::PreflightUnhealthy);
        assert_eq!(platform.0.lock().unwrap().writes, 0);
        assert!(journal.0.lock().unwrap().record.is_none());
    }
    let platform = SimulatedPlatform::example();
    let journal = MemoryJournal::default();
    journal.0.lock().unwrap().fail_create = true;
    let mut controller =
        Controller::new(platform.clone(), journal.clone(), SimulatedProbe::default());
    assert_eq!(controller.enable().reason, Reason::JournalUnavailable);
    assert_eq!(platform.0.lock().unwrap().writes, 0);
    journal.0.lock().unwrap().fail_create = false;
    assert_eq!(controller.enable().state, State::Active);
}

#[test]
fn partial_activation_and_postflight_failure_restore_every_changed_target() {
    for failure in 0..4 {
        let platform = SimulatedPlatform::example();
        let original = platform.0.lock().unwrap().targets.clone();
        let journal = MemoryJournal::default();
        let probe = SimulatedProbe::default();
        match failure {
            0 => platform.0.lock().unwrap().fail_write = Some(2),
            1 => platform.0.lock().unwrap().fail_after_write = Some(2),
            2 => platform.0.lock().unwrap().ignore_write = Some(2),
            _ => probe
                .0
                .lock()
                .unwrap()
                .extend([HEALTHY, upstream_failure()]),
        }
        let mut controller = Controller::new(platform.clone(), journal.clone(), probe);
        let result = controller.enable();
        assert_eq!(result.state, State::Inactive, "failure {failure}");
        assert_eq!(
            result.reason,
            if failure == 3 {
                Reason::PostflightUnhealthy
            } else {
                Reason::ApplyFailed
            }
        );
        assert_eq!(platform.0.lock().unwrap().targets, original);
        assert!(journal.0.lock().unwrap().record.is_none());
    }
}

#[test]
fn failed_restore_retains_original_record_and_retry_is_idempotent() {
    let scratch = Scratch::new();
    let platform = SimulatedPlatform::example();
    let original = platform.0.lock().unwrap().targets.clone();
    let mut controller = Controller::new(
        platform.clone(),
        scratch.journal(),
        SimulatedProbe::default(),
    );
    controller.enable();
    let bytes = fs::read(scratch.0.join("recovery.json")).unwrap();
    platform.0.lock().unwrap().fail_write = Some(3);
    let report = controller.disable();
    assert_eq!(report.state, State::RecoveryRequired);
    assert!(report
        .targets
        .iter()
        .any(|t| t.outcome == TargetOutcome::RestoreFailed));
    assert_eq!(fs::read(scratch.0.join("recovery.json")).unwrap(), bytes);
    assert_eq!(controller.enable().reason, Reason::PendingRecovery);
    drop(controller);
    let mut restarted = Controller::new(
        platform.clone(),
        scratch.journal(),
        SimulatedProbe::default(),
    );
    let report = restarted.recover();
    assert_eq!(report.state, State::Inactive);
    assert!(report
        .targets
        .iter()
        .any(|t| t.outcome == TargetOutcome::AlreadyOriginal));
    assert_eq!(platform.0.lock().unwrap().targets, original);
    assert_eq!(restarted.recover().reason, Reason::NotActive);
}

struct InterruptPlatform {
    inner: SimulatedPlatform,
    path: PathBuf,
    after: bool,
}
impl Platform for InterruptPlatform {
    fn snapshot(&mut self) -> Result<Vec<TargetState>, String> {
        self.inner.snapshot()
    }
    fn read(&mut self, target: &Target) -> Result<Option<DnsSetting>, String> {
        self.inner.read(target)
    }
    fn compare_and_set(
        &mut self,
        target: &Target,
        before: &DnsSetting,
        after: &DnsSetting,
    ) -> Result<(), String> {
        // A fully readable record must exist even at the first mutation boundary.
        let record: RecoveryRecord =
            serde_json::from_slice(&fs::read(self.path.join("recovery.json")).unwrap()).unwrap();
        record.validate().unwrap();
        if self.after {
            self.inner.compare_and_set(target, before, after)?;
        }
        panic!("Injected interruption at platform write boundary");
    }
}

#[test]
fn restart_recovers_interruptions_before_and_after_first_write_without_health_probes() {
    struct Offline;
    impl HealthProbe for Offline {
        fn check(&mut self, _: &[Change]) -> Health {
            panic!("Recovery must work offline");
        }
    }
    for after in [false, true] {
        let scratch = Scratch::new();
        let platform = SimulatedPlatform::example();
        let original = platform.0.lock().unwrap().targets.clone();
        let adapter = InterruptPlatform {
            inner: platform.clone(),
            path: scratch.0.clone(),
            after,
        };
        let mut controller = Controller::new(adapter, scratch.journal(), SimulatedProbe::default());
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| controller.enable())).is_err()
        );
        drop(controller);
        let mut restarted = Controller::new(platform.clone(), scratch.journal(), Offline);
        assert_eq!(restarted.enable().reason, Reason::PendingRecovery);
        assert_eq!(restarted.poll_health().reason, Reason::PendingRecovery);
        assert_eq!(restarted.recover().state, State::Inactive);
        assert_eq!(platform.0.lock().unwrap().targets, original);
    }
}

#[test]
fn upstream_outage_never_triggers_rollback_and_local_failures_need_three_consecutive_samples() {
    let platform = SimulatedPlatform::example();
    let journal = MemoryJournal::default();
    let probe = SimulatedProbe::default();
    let mut controller = Controller::new(platform.clone(), journal.clone(), probe.clone());
    controller.enable();
    for _ in 0..5 {
        probe.0.lock().unwrap().push_back(upstream_failure());
        let result = controller.poll_health();
        assert_eq!(result.state, State::Degraded);
        assert_eq!(result.reason, Reason::UpstreamUnavailable);
    }
    assert_eq!(platform.0.lock().unwrap().writes, 2);
    assert!(journal.0.lock().unwrap().record.is_some());
    probe.0.lock().unwrap().extend([
        local_failure(),
        local_failure(),
        HEALTHY,
        local_failure(),
        local_failure(),
        local_failure(),
    ]);
    assert_eq!(controller.poll_health().state, State::Degraded);
    assert_eq!(controller.poll_health().state, State::Degraded);
    assert_eq!(controller.poll_health().state, State::Active);
    assert_eq!(controller.poll_health().state, State::Degraded);
    assert_eq!(controller.poll_health().state, State::Degraded);
    let result = controller.poll_health();
    assert_eq!(result.state, State::Inactive);
    assert_eq!(result.reason, Reason::LocalHealthFailed);
    assert!(journal.0.lock().unwrap().record.is_none());
}

#[test]
fn external_changes_missing_interfaces_and_new_networks_are_never_overwritten() {
    for change_kind in 0..3 {
        let platform = SimulatedPlatform::example();
        let original = platform.0.lock().unwrap().targets.clone();
        let journal = MemoryJournal::default();
        let mut controller =
            Controller::new(platform.clone(), journal.clone(), SimulatedProbe::default());
        controller.enable();
        {
            let mut state = platform.0.lock().unwrap();
            match change_kind {
                0 => {
                    state.targets[0].setting =
                        DnsSetting::Static(vec!["192.0.2.53".parse().unwrap()])
                }
                1 => {
                    state.targets.remove(0);
                }
                _ => state.targets[0].target.network_id = "new-wifi-network".into(),
            }
        }
        let changed = platform.0.lock().unwrap().targets.clone();
        let result = controller.poll_health();
        assert_eq!(result.reason, Reason::ConfigurationChanged);
        assert_eq!(result.state, State::RecoveryRequired);
        assert_eq!(platform.0.lock().unwrap().writes, 3); // only the still-owned IPv6 setting restored
        assert!(journal.0.lock().unwrap().record.is_some());
        if change_kind != 1 {
            assert_eq!(platform.0.lock().unwrap().targets[0], changed[0]);
        }
        assert_eq!(controller.enable().reason, Reason::PendingRecovery);
        // User/adapter reconciles the conflict; next recovery verifies before clearing.
        platform.0.lock().unwrap().targets = original;
        assert_eq!(controller.recover().state, State::Inactive);
    }
}

#[test]
fn journal_failures_are_visible_and_report_failure_cannot_prevent_restoration() {
    let platform = SimulatedPlatform::example();
    let original = platform.0.lock().unwrap().targets.clone();
    let journal = MemoryJournal::default();
    let mut controller =
        Controller::new(platform.clone(), journal.clone(), SimulatedProbe::default());
    controller.enable();
    journal.0.lock().unwrap().fail_load = true;
    assert_eq!(controller.disable().reason, Reason::JournalUnavailable);
    assert_eq!(platform.0.lock().unwrap().writes, 2);
    {
        let mut state = journal.0.lock().unwrap();
        state.fail_load = false;
        state.fail_clear = true;
        state.fail_report = true;
    }
    let report = controller.recover();
    assert!(report.report_write_failed);
    assert_eq!(report.state, State::RecoveryRequired);
    assert_eq!(platform.0.lock().unwrap().targets, original);
    assert!(journal.0.lock().unwrap().record.is_some());
    journal.0.lock().unwrap().fail_clear = false;
    assert_eq!(controller.recover().state, State::Inactive);
    assert_eq!(platform.0.lock().unwrap().writes, 4); // no repeated writes to already-restored settings
}

#[test]
fn lost_journal_does_not_claim_successful_disable() {
    let journal = MemoryJournal::default();
    let platform = SimulatedPlatform::example();
    let mut controller =
        Controller::new(platform.clone(), journal.clone(), SimulatedProbe::default());
    controller.enable();
    journal.0.lock().unwrap().record = None;
    let report = controller.disable();
    assert_eq!(report.state, State::RecoveryRequired);
    assert_eq!(report.reason, Reason::JournalUnavailable);
    assert_eq!(platform.0.lock().unwrap().writes, 2);
    assert_eq!(controller.enable().state, State::RecoveryRequired);
    assert_eq!(platform.0.lock().unwrap().writes, 2);
}

#[test]
fn activation_rechecks_settings_after_health_and_restore_read_failures_remain_retryable() {
    struct ChangingProbe {
        platform: SimulatedPlatform,
        calls: usize,
    }
    impl HealthProbe for ChangingProbe {
        fn check(&mut self, changes: &[Change]) -> Health {
            assert_eq!(changes.len(), 2);
            self.calls += 1;
            if self.calls == 2 {
                self.platform.0.lock().unwrap().targets[0].setting = DnsSetting::Automatic;
            }
            HEALTHY
        }
    }
    let platform = SimulatedPlatform::example();
    let probe = ChangingProbe {
        platform: platform.clone(),
        calls: 0,
    };
    let mut controller = Controller::new(platform.clone(), MemoryJournal::default(), probe);
    let report = controller.enable();
    assert_eq!(report.state, State::Inactive);
    assert_eq!(report.reason, Reason::ConfigurationChanged);
    let mut controller = Controller::new(
        platform.clone(),
        MemoryJournal::default(),
        SimulatedProbe::default(),
    );
    controller.enable();
    platform.0.lock().unwrap().fail_reads = true;
    let report = controller.disable();
    assert_eq!(report.state, State::RecoveryRequired);
    assert!(report
        .targets
        .iter()
        .all(|t| t.outcome == TargetOutcome::ReadFailed));
    platform.0.lock().unwrap().fail_reads = false;
    assert_eq!(controller.recover().state, State::Inactive);
}

#[test]
fn file_journal_locks_refuses_replacement_and_recovers_after_reopen() {
    let scratch = Scratch::new();
    let mut journal = scratch.journal();
    assert!(FileJournal::open(&scratch.0).is_err());
    let record =
        RecoveryRecord::from_snapshot(SimulatedPlatform::example().snapshot().unwrap()).unwrap();
    journal.create(&record).unwrap();
    assert!(journal.create(&record).is_err());
    // A process interrupted while preparing a replacement cannot corrupt the committed record.
    fs::write(scratch.0.join("recovery.json.pending"), b"{interrupted").unwrap();
    drop(journal);
    let mut journal = scratch.journal();
    assert_eq!(journal.load().unwrap(), Some(record));
    journal.clear().unwrap();
    assert!(journal.load().unwrap().is_none());
    assert!(scratch.0.join("controller.lock").exists());
}

#[test]
fn corrupt_unsupported_and_oversized_journals_fail_closed_without_mutation() {
    let mut valid = serde_json::to_value(
        RecoveryRecord::from_snapshot(SimulatedPlatform::example().snapshot().unwrap()).unwrap(),
    )
    .unwrap();
    valid["version"] = 2.into();
    for bytes in [
        b"{truncated".to_vec(),
        serde_json::to_vec(&valid).unwrap(),
        vec![b' '; 128 * 1024 + 1],
    ] {
        let scratch = Scratch::new();
        fs::write(scratch.0.join("recovery.json"), &bytes).unwrap();
        let platform = SimulatedPlatform::example();
        let mut controller = Controller::new(
            platform.clone(),
            scratch.journal(),
            SimulatedProbe::default(),
        );
        assert_eq!(controller.enable().reason, Reason::JournalUnavailable);
        assert_eq!(controller.recover().state, State::RecoveryRequired);
        assert_eq!(fs::read(scratch.0.join("recovery.json")).unwrap(), bytes);
        assert_eq!(platform.0.lock().unwrap().writes, 0);
    }
}

#[test]
fn invalid_snapshots_are_rejected_and_serialization_preserves_server_order() {
    let base = SimulatedPlatform::example().snapshot().unwrap();
    let mut duplicates = base.clone();
    duplicates.push(base[0].clone());
    let mut changed_network_duplicate = base.clone();
    let mut another = base[0].clone();
    another.target.network_id = "other-network".into();
    changed_network_duplicate.push(another);
    let mut empty_id = base.clone();
    empty_id[0].target.interface_id.clear();
    let mut too_many = base.clone();
    too_many.resize(MAX_TARGETS + 1, base[0].clone());
    for snapshot in [
        vec![],
        duplicates,
        changed_network_duplicate,
        empty_id,
        too_many,
    ] {
        assert!(RecoveryRecord::from_snapshot(snapshot).is_err());
    }
    for server in [
        "127.0.0.1",
        "0.0.0.0",
        "224.0.0.1",
        "255.255.255.255",
        "::1",
        "::ffff:127.0.0.1",
    ] {
        let mut snapshot = base.clone();
        snapshot[0].setting = DnsSetting::Static(vec![server.parse().unwrap()]);
        assert!(RecoveryRecord::from_snapshot(snapshot).is_err(), "{server}");
    }
    let mut snapshot = base;
    snapshot[0].setting = DnsSetting::Static(vec![
        "192.0.2.2".parse().unwrap(),
        "192.0.2.1".parse().unwrap(),
    ]);
    let record = RecoveryRecord::from_snapshot(snapshot).unwrap();
    let roundtrip: RecoveryRecord =
        serde_json::from_slice(&serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(record, roundtrip);
}

#[test]
fn simulation_cli_writes_local_reports_and_preserves_conflict_backup() {
    let scratch = Scratch::new();
    for (scenario, state) in [
        ("normal", State::Inactive),
        ("upstream-outage", State::Inactive),
        ("local-failure", State::Inactive),
        ("interrupted", State::Inactive),
        ("conflict", State::RecoveryRequired),
    ] {
        let directory = scratch.0.join(scenario);
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_naab-dns-sim"))
            .args([scenario, "--output-dir"])
            .arg(&directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("SIMULATION ONLY"));
        let report: Report =
            serde_json::from_slice(&fs::read(directory.join("last-report.json")).unwrap()).unwrap();
        assert_eq!(report.state, state);
        assert!(!report.report_write_failed);
        if scenario == "upstream-outage" {
            let incident: Report =
                serde_json::from_slice(&fs::read(directory.join("last-incident.json")).unwrap())
                    .unwrap();
            assert_eq!(incident.reason, Reason::UpstreamUnavailable);
            assert_eq!(report.reason, Reason::Disabled);
        }
        assert_eq!(
            directory.join("recovery.json").exists(),
            scenario == "conflict"
        );
        let repeated = std::process::Command::new(env!("CARGO_BIN_EXE_naab-dns-sim"))
            .args([scenario, "--output-dir"])
            .arg(&directory)
            .output()
            .unwrap();
        assert!(
            !repeated.status.success(),
            "Existing output must not be overwritten"
        );
    }
    let invalid = scratch.0.join("invalid");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_naab-dns-sim"))
        .args(["invalid", "--output-dir"])
        .arg(&invalid)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!invalid.exists());
}
