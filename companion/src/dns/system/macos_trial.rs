//! Foreground Mac transaction controller. Callers supply an already admitted
//! snapshot, local resolver health, and cancellation installed before entry.
//! This is not a public activation command or a physical-network admission gate.
use super::macos_recovery::{self, Configuration, DnsState, Record, Report, Settings, Store};
use std::time::{Duration, Instant};

pub trait TrialStore: Store {
    /// Persist and sync the new record without replacing an existing session.
    /// Hold exclusive ownership until run returns, including through recovery.
    fn create(&mut self, record: &Record) -> Result<(), String>;
}

#[cfg(target_os = "macos")]
impl TrialStore for macos_recovery::FileStore {
    fn create(&mut self, record: &Record) -> Result<(), String> {
        self.create(record)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Deadline,
    Cancelled,
    ResolverFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Active,
    Restoring,
}

pub fn expect<P: Settings>(
    settings: &mut P,
    record: &Record,
    dns: &DnsState,
) -> Result<(), String> {
    let observed = settings.observe(record)?;
    if record.set_id != observed.set_id
        || record.service_id != observed.service_id
        || record.device != observed.device
        || record.network_context_sha256 != observed.network_context_sha256
        || !settings.equivalent(&observed.dns, dns)?
    {
        return Err("Mac DNS state or network context changed".into());
    }
    Ok(())
}

/// Cancellation and health callbacks must be bounded; notification errors and
/// unwinding panics also lead through recovery. Cancellation remains owned by
/// the caller until this function (including restoration) returns. Native calls
/// may overrun the polling deadline; it is not a hard process execution timeout.
pub fn run<S: TrialStore, P: Settings>(
    store: &mut S,
    settings: &mut P,
    record: &Record,
    duration: Duration,
    mut cancelled: impl FnMut() -> bool,
    mut healthy: impl FnMut() -> bool,
    mut notify: impl FnMut(Event) -> Result<(), String>,
) -> Result<(End, Report), String> {
    if duration.is_zero() || duration > Duration::from_secs(300) {
        return Err("Mac trial duration must be greater than zero and at most 300 seconds".into());
    }
    record.validate()?;
    if !record.original.enabled
        || !matches!(record.original.configuration, Configuration::Saved(_))
        || settings.equivalent(&record.original, &record.applied)?
    {
        return Err("Mac trial needs distinct, complete, enabled DNS snapshots".into());
    }
    if store.load()?.is_some() {
        return Err("Recover the pending Mac DNS session first".into());
    }
    if cancelled() || !healthy() || cancelled() {
        return Err("Mac trial cancelled or resolver unhealthy before activation".into());
    }
    expect(settings, record, &record.original)?;
    // A failed save never permits an OS mutation. Its possible partial record
    // remains available to offline recovery rather than being silently removed.
    store.create(record)?;
    let started = Instant::now();
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<End, String> {
            if cancelled() {
                return Ok(End::Cancelled);
            }
            let mut inverse = record.clone();
            std::mem::swap(&mut inverse.original, &mut inverse.applied);
            settings.compare_and_restore(&inverse)?;
            expect(settings, record, &record.applied)?;
            notify(Event::Active)?;
            let mut failures = 0;
            loop {
                if cancelled() {
                    return Ok(End::Cancelled);
                }
                if started.elapsed() >= duration {
                    return Ok(End::Deadline);
                }
                expect(settings, record, &record.applied)?;
                failures = if healthy() { 0 } else { failures + 1 };
                if failures >= 3 {
                    return Ok(End::ResolverFailed);
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }));
    // Notification must never prevent cleanup, even if it panics or fails.
    let notification =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| notify(Event::Restoring)));
    let report = macos_recovery::recover(store, settings);
    if report.recovery_pending
        || report.report_write_failed
        || !matches!(
            report.outcome,
            macos_recovery::Outcome::Restored | macos_recovery::Outcome::AlreadyOriginal
        )
    {
        return Err(format!(
            "Mac trial recovery needs attention: {report:?}; trial: {outcome:?}"
        ));
    }
    notification.map_err(|_| "Mac trial restoration notification panicked")??;
    let end = outcome.map_err(|_| "Mac trial panicked; recovery completed")??;
    Ok((end, report))
}

/// Tokio installs process-lifetime Unix handlers synchronously when registering
/// these streams. Repeated SIGINT/SIGTERM cannot revert to default termination
/// during cleanup. The streams/runtime must outlive run and this helper is for
/// the dedicated foreground helper process, not an embedded GUI lifecycle.
#[cfg(target_os = "macos")]
pub struct Cancellation {
    runtime: tokio::runtime::Runtime,
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    requested: bool,
}

#[cfg(target_os = "macos")]
impl Cancellation {
    pub fn install() -> Result<Self, String> {
        use tokio::signal::unix::{signal, SignalKind};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let (interrupt, terminate) = {
            let _entered = runtime.enter();
            (
                signal(SignalKind::interrupt()).map_err(|e| e.to_string())?,
                signal(SignalKind::terminate()).map_err(|e| e.to_string())?,
            )
        };
        Ok(Self {
            runtime,
            interrupt,
            terminate,
            requested: false,
        })
    }

    pub fn requested(&mut self) -> bool {
        // Yield to the IO driver as well as polling the registered receivers.
        self.requested |= self.runtime.block_on(async {
            tokio::select! {
                biased;
                _ = self.interrupt.recv() => true,
                _ = self.terminate.recv() => true,
                _ = tokio::time::sleep(Duration::from_millis(1)) => false,
            }
        });
        self.requested
    }
}
