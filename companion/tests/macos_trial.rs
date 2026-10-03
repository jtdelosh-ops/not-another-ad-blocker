use naab_companion::dns::system::{
    macos_recovery::{Configuration, DnsState, Observation, Record, Report, Settings, Store},
    macos_trial::{self, End, Event, TrialSettings, TrialStore},
};
use std::{cell::RefCell, rc::Rc, time::Duration};

#[derive(Default)]
struct Memory {
    record: Option<Record>,
    dns: Option<DnsState>,
    writes: usize,
    fail_save: bool,
    fail_apply_after_write: bool,
    fail_restore: bool,
    fail_report: bool,
    fail_admission: bool,
    admitted: bool,
}
struct Journal(Rc<RefCell<Memory>>);
struct Platform(Rc<RefCell<Memory>>);
impl TrialSettings for Platform {
    fn admit(&mut self, _: &Record) -> Result<(), String> {
        self.0.borrow_mut().admitted = true;
        if self.0.borrow().fail_admission {
            Err("policy changed".into())
        } else {
            Ok(())
        }
    }
}
impl Store for Journal {
    fn load(&mut self) -> Result<Option<Record>, String> {
        Ok(self.0.borrow().record.clone())
    }
    fn clear(&mut self) -> Result<(), String> {
        self.0.borrow_mut().record = None;
        Ok(())
    }
    fn save_report(&mut self, _: &Report) -> Result<(), String> {
        if self.0.borrow().fail_report {
            Err("report failed".into())
        } else {
            Ok(())
        }
    }
}
impl TrialStore for Journal {
    fn create(&mut self, record: &Record) -> Result<(), String> {
        let mut m = self.0.borrow_mut();
        if m.fail_save {
            return Err("disk failed".into());
        }
        assert!(m.record.is_none());
        m.record = Some(record.clone());
        Ok(())
    }
}
impl Settings for Platform {
    fn observe(&mut self, record: &Record) -> Result<Observation, String> {
        Ok(Observation {
            set_id: record.set_id.clone(),
            service_id: record.service_id.clone(),
            device: record.device.clone(),
            network_context_sha256: record.network_context_sha256.clone(),
            dns: self.0.borrow().dns.clone().unwrap(),
        })
    }
    fn equivalent(&self, a: &DnsState, b: &DnsState) -> Result<bool, String> {
        Ok(a == b)
    }
    fn compare_and_restore(&mut self, record: &Record) -> Result<(), String> {
        let mut m = self.0.borrow_mut();
        assert!(m.record.is_some(), "mutation preceded durable journal");
        if m.dns.as_ref() != Some(&record.applied) {
            return Err("changed".into());
        }
        if m.writes > 0 && m.fail_restore {
            return Err("restore failed".into());
        }
        m.writes += 1;
        m.dns = Some(record.original.clone());
        if m.writes == 1 && m.fail_apply_after_write {
            return Err("apply failed after commit".into());
        }
        Ok(())
    }
    fn ensure_applied(&mut self) -> Result<(), String> {
        Ok(())
    }
}
fn fixture() -> (Record, Rc<RefCell<Memory>>) {
    let state = |text: &[u8]| DnsState {
        enabled: true,
        configuration: Configuration::Saved(text.to_vec()),
    };
    let r = Record {
        version: 1,
        session_id: "a".repeat(32),
        set_id: "set".into(),
        service_id: "service".into(),
        device: "en0".into(),
        network_context_sha256: "b".repeat(64),
        original: state(b"ordered-static-v4-v6-and-search-domain"),
        applied: state(b"loopback"),
    };
    let m = Rc::new(RefCell::new(Memory {
        dns: Some(r.original.clone()),
        ..Memory::default()
    }));
    (r, m)
}
#[test]
fn cancellation_and_deadline_restore_exact_original() {
    for cancel in [false, true] {
        let (r, m) = fixture();
        let active = std::cell::Cell::new(false);
        let (end, report) = macos_trial::run(
            &mut Journal(m.clone()),
            &mut Platform(m.clone()),
            &r,
            Duration::from_millis(1),
            || cancel && active.get(),
            || true,
            |event| {
                if event == Event::Active {
                    active.set(true);
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            end,
            if cancel {
                End::Cancelled
            } else {
                End::Deadline
            }
        );
        assert!(!report.recovery_pending);
        assert_eq!(m.borrow().dns.as_ref(), Some(&r.original));
        assert!(m.borrow().record.is_none());
    }
}
#[test]
fn pending_session_save_failure_and_pre_cancel_never_mutate() {
    for scenario in 0..3 {
        let (r, m) = fixture();
        if scenario == 0 {
            m.borrow_mut().record = Some(r.clone());
        }
        if scenario == 1 {
            m.borrow_mut().fail_save = true;
        }
        assert!(macos_trial::run(
            &mut Journal(m.clone()),
            &mut Platform(m.clone()),
            &r,
            Duration::from_secs(1),
            || scenario == 2,
            || true,
            |_| Ok(())
        )
        .is_err());
        assert_eq!(m.borrow().writes, 0);
        assert_eq!(m.borrow().record.is_some(), scenario == 0);
    }
}
#[test]
fn errors_panics_and_partial_activation_always_recover() {
    for scenario in 0..4 {
        let (r, m) = fixture();
        m.borrow_mut().fail_apply_after_write = scenario == 0;
        let result = macos_trial::run(
            &mut Journal(m.clone()),
            &mut Platform(m.clone()),
            &r,
            Duration::from_millis(1),
            || false,
            || true,
            |event| {
                if (scenario == 1 && event == Event::Active)
                    || (scenario == 3 && event == Event::Restoring)
                {
                    panic!("notification panic");
                }
                if scenario == 2 {
                    return Err("output pipe closed".into());
                }
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(m.borrow().dns.as_ref(), Some(&r.original));
        assert!(m.borrow().record.is_none());
    }
}
#[test]
fn changed_dns_is_preserved_with_pending_journal() {
    let (r, m) = fixture();
    let changed = DnsState {
        enabled: true,
        configuration: Configuration::Saved(b"user-dns".to_vec()),
    };
    let result = macos_trial::run(
        &mut Journal(m.clone()),
        &mut Platform(m.clone()),
        &r,
        Duration::from_secs(1),
        || false,
        || true,
        |event| {
            if event == Event::Active {
                m.borrow_mut().dns = Some(changed.clone());
            }
            Ok(())
        },
    );
    assert!(result.is_err());
    assert_eq!(m.borrow().dns.as_ref(), Some(&changed));
    assert!(m.borrow().record.is_some());
}
#[test]
fn three_local_failures_restore_and_failed_restore_retains_record() {
    for fail_restore in [false, true] {
        let (r, m) = fixture();
        m.borrow_mut().fail_restore = fail_restore;
        let active = std::cell::Cell::new(false);
        let result = macos_trial::run(
            &mut Journal(m.clone()),
            &mut Platform(m.clone()),
            &r,
            Duration::from_secs(10),
            || false,
            || !active.get(),
            |event| {
                if event == Event::Active {
                    active.set(true);
                }
                Ok(())
            },
        );
        if fail_restore {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap().0, End::ResolverFailed);
        }
        assert_eq!(m.borrow().record.is_some(), fail_restore);
    }
}
#[test]
fn report_failure_is_not_reported_as_success() {
    let (r, m) = fixture();
    m.borrow_mut().fail_report = true;
    assert!(macos_trial::run(
        &mut Journal(m.clone()),
        &mut Platform(m.clone()),
        &r,
        Duration::from_millis(1),
        || false,
        || true,
        |_| Ok(())
    )
    .is_err());
    assert_eq!(m.borrow().dns.as_ref(), Some(&r.original));
}

#[test]
fn missing_journal_is_not_successful_restoration() {
    let (r, m) = fixture();
    let result = macos_trial::run(
        &mut Journal(m.clone()),
        &mut Platform(m.clone()),
        &r,
        Duration::from_millis(1),
        || false,
        || true,
        |event| {
            if event == Event::Active {
                m.borrow_mut().record = None;
            }
            Ok(())
        },
    );
    assert!(result.unwrap_err().contains("recovery needs attention"));
    assert_eq!(m.borrow().dns.as_ref(), Some(&r.applied));
}

#[test]
fn rejected_admission_never_journals_or_changes_dns() {
    let (r, m) = fixture();
    m.borrow_mut().fail_admission = true;
    assert!(macos_trial::run(
        &mut Journal(m.clone()),
        &mut Platform(m.clone()),
        &r,
        Duration::from_secs(1),
        || false,
        || true,
        |_| Ok(())
    )
    .unwrap_err()
    .contains("policy changed"));
    assert_eq!(m.borrow().writes, 0);
    assert!(m.borrow().record.is_none());
}

#[test]
fn resolver_loss_or_cancellation_during_admission_prevents_journal_and_write() {
    for cancel in [false, true] {
        let (r, m) = fixture();
        let result = macos_trial::run(
            &mut Journal(m.clone()),
            &mut Platform(m.clone()),
            &r,
            Duration::from_secs(1),
            || cancel && m.borrow().admitted,
            || cancel || !m.borrow().admitted,
            |_| Ok(()),
        );
        assert!(result.unwrap_err().contains("after admission"));
        assert_eq!(m.borrow().writes, 0);
        assert!(m.borrow().record.is_none());
    }
}

#[test]
fn resolver_loss_after_journaling_clears_record_without_applying_dns() {
    let (r, m) = fixture();
    let result = macos_trial::run(
        &mut Journal(m.clone()),
        &mut Platform(m.clone()),
        &r,
        Duration::from_secs(1),
        || false,
        || m.borrow().record.is_none(),
        |_| Ok(()),
    );
    assert!(result.unwrap_err().contains("before activation"));
    assert_eq!(m.borrow().writes, 0);
    assert!(m.borrow().record.is_none());
    assert_eq!(m.borrow().dns.as_ref(), Some(&r.original));
}
