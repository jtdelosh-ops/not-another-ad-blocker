//! Bounded foreground trial. Cancellation stays installed through restoration.
use super::{Controller, HealthProbe, Journal, Platform, Report, State};
use std::time::{Duration, Instant, SystemTime};

pub fn run<P: Platform, J: Journal, H: HealthProbe>(
    controller: &mut Controller<P, J, H>,
    duration: Duration,
    mut report: impl FnMut(&Report) -> Result<(), String>,
    mut restoring: impl FnMut(),
) -> Result<(), String> {
    // Install synchronously, before enable can write anything. Failure to install
    // cancellation must fail closed. Keep the guard alive until cleanup finishes.
    let stop = console::Stop::install()?;
    let started = Instant::now();
    let deadline = SystemTime::now() + duration;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if stop.requested() {
            return Err("Trial cancelled before activation".into());
        }
        let enabled = controller.enable();
        report(&enabled)?;
        if enabled.state != State::Active {
            return Err(
                "DNS was not enabled; resolve the reported preflight issue before retrying".into(),
            );
        }
        while !stop.requested() && started.elapsed() < duration && SystemTime::now() < deadline {
            std::thread::sleep(Duration::from_secs(2));
            if stop.requested() {
                break;
            }
            let health = controller.poll_health();
            report(&health)?;
            if health.state == State::Inactive {
                return Ok(());
            }
        }
        restoring();
        Ok(())
    }));
    // A signal requests cleanup; it never performs OS work on the handler thread.
    // Later Ctrl+C/Break events remain handled while synchronous writes/readbacks
    // run. Forced termination, terminal close and reboot still need recover.
    let restored = controller.disable();
    let cleanup = report(&restored);
    match outcome {
        Ok(Ok(())) => cleanup,
        Ok(Err(error)) => Err(error),
        Err(_) => Err("The helper panicked; inspect recovery/report status before retrying".into()),
    }
}

#[cfg(windows)]
mod console {
    use std::sync::atomic::{AtomicBool, Ordering};

    static INSTALLED: AtomicBool = AtomicBool::new(false);
    static REQUESTED: AtomicBool = AtomicBool::new(false);

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    unsafe extern "system" fn handler(event: u32) -> i32 {
        if matches!(event, 0 | 1) {
            // CTRL_C_EVENT | CTRL_BREAK_EVENT
            REQUESTED.store(true, Ordering::SeqCst);
            1
        } else {
            0
        }
    }

    pub struct Stop;
    impl Stop {
        pub fn install() -> Result<Self, String> {
            if INSTALLED
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return Err("A console trial is already running in this process".into());
            }
            REQUESTED.store(false, Ordering::SeqCst);
            // SAFETY: this static callback has the required ABI and touches only
            // process-lifetime atomics. It never unwinds or borrows the controller.
            if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
                let error = std::io::Error::last_os_error();
                INSTALLED.store(false, Ordering::SeqCst);
                return Err(format!("Cannot install console cancellation: {error}"));
            }
            Ok(Self)
        }

        pub fn requested(&self) -> bool {
            REQUESTED.load(Ordering::SeqCst)
        }
    }

    impl Drop for Stop {
        fn drop(&mut self) {
            // SAFETY: remove the exact static callback registered above, after
            // restoration and report writing. No callback references this guard.
            if unsafe { SetConsoleCtrlHandler(Some(handler), 0) } != 0 {
                INSTALLED.store(false, Ordering::SeqCst);
            }
        }
    }
}

#[cfg(not(windows))]
mod console {
    pub struct Stop;
    impl Stop {
        pub fn install() -> Result<Self, String> {
            Err("The console trial requires Windows".into())
        }
        pub fn requested(&self) -> bool {
            false
        }
    }
}
