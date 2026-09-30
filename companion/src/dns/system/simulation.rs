//! In-memory adapter for the offline demonstration and failure-injection tests.
//! This module never discovers or changes the host's real network configuration.
use super::*;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub struct SimulatedState {
    pub targets: Vec<TargetState>,
    pub writes: usize,
    pub fail_write: Option<usize>,
    pub fail_after_write: Option<usize>,
    pub ignore_write: Option<usize>,
    pub fail_reads: bool,
    pub fail_snapshot: bool,
}

#[derive(Clone)]
pub struct SimulatedPlatform(pub Arc<Mutex<SimulatedState>>);

impl SimulatedPlatform {
    pub fn example() -> Self {
        Self(Arc::new(Mutex::new(SimulatedState {
            targets: vec![
                TargetState {
                    target: Target {
                        interface_id: "simulated-wifi".into(),
                        network_id: "test-network".into(),
                        family: Family::Ipv4,
                    },
                    setting: DnsSetting::Automatic,
                },
                TargetState {
                    target: Target {
                        interface_id: "simulated-wifi".into(),
                        network_id: "test-network".into(),
                        family: Family::Ipv6,
                    },
                    setting: DnsSetting::Static(vec!["2001:db8::53".parse().unwrap()]),
                },
            ],
            ..Default::default()
        })))
    }
}

impl Platform for SimulatedPlatform {
    fn snapshot(&mut self) -> Result<Vec<TargetState>, String> {
        let state = self.0.lock().unwrap();
        if state.fail_snapshot {
            return Err("Simulated snapshot failure".into());
        }
        Ok(state.targets.clone())
    }
    fn read(&mut self, target: &Target) -> Result<Option<DnsSetting>, String> {
        let state = self.0.lock().unwrap();
        if state.fail_reads {
            return Err("Simulated read failure".into());
        }
        Ok(state
            .targets
            .iter()
            .find(|state| state.target == *target)
            .map(|state| state.setting.clone()))
    }
    fn compare_and_set(
        &mut self,
        target: &Target,
        expected: &DnsSetting,
        replacement: &DnsSetting,
    ) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        state.writes += 1;
        let count = state.writes;
        if state.fail_write == Some(count) {
            return Err("Simulated write failure".into());
        }
        let ignore = state.ignore_write == Some(count);
        let target = state
            .targets
            .iter_mut()
            .find(|state| state.target == *target)
            .ok_or("Simulated target missing")?;
        if target.setting != *expected {
            return Err("Simulated setting changed".into());
        }
        if !ignore {
            target.setting = replacement.clone();
        }
        if state.fail_after_write == Some(count) {
            return Err("Simulated failure after write".into());
        }
        Ok(())
    }
}

pub const HEALTHY: Health = Health {
    local: Check::Passed,
    upstream: Check::Passed,
};

#[derive(Clone)]
pub struct SimulatedProbe(pub Arc<Mutex<VecDeque<Health>>>);

impl Default for SimulatedProbe {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(VecDeque::new())))
    }
}

impl HealthProbe for SimulatedProbe {
    fn check(&mut self, _changes: &[Change]) -> Health {
        self.0.lock().unwrap().pop_front().unwrap_or(HEALTHY)
    }
}

/// Every scenario starts with a fresh in-memory machine. On-disk records from
/// another run must never be interpreted as that new machine's original state.
pub fn run(scenario: &str, journal: impl Journal) -> Result<Vec<Report>, String> {
    if ![
        "normal",
        "upstream-outage",
        "local-failure",
        "interrupted",
        "conflict",
    ]
    .contains(&scenario)
    {
        return Err("Unknown scenario: use normal, upstream-outage, local-failure, interrupted, or conflict".into());
    }
    let platform = SimulatedPlatform::example();
    let probe = SimulatedProbe::default();
    let mut controller = Controller::new(platform.clone(), journal, probe.clone());
    let enabled = controller.enable();
    if enabled.reason != Reason::Enabled || enabled.report_write_failed {
        return Err(format!("Simulation could not enable: {:?}", enabled.reason));
    }
    let mut reports = vec![enabled];
    match scenario {
        "upstream-outage" => {
            probe.0.lock().unwrap().push_back(Health {
                local: Check::Passed,
                upstream: Check::Failed,
            });
            reports.push(controller.poll_health());
            reports.push(controller.poll_health());
            reports.push(controller.disable());
        }
        "local-failure" => {
            for _ in 0..3 {
                probe.0.lock().unwrap().push_back(Health {
                    local: Check::Failed,
                    upstream: Check::Unknown,
                });
            }
            for _ in 0..3 {
                reports.push(controller.poll_health());
            }
        }
        "interrupted" => {
            let (platform, journal, probe) = controller.into_parts();
            let mut restarted = Controller::new(platform, journal, probe);
            reports.push(restarted.recover());
        }
        "conflict" => {
            platform.0.lock().unwrap().targets[0].setting =
                DnsSetting::Static(vec!["192.0.2.53".parse().unwrap()]);
            reports.push(controller.poll_health());
        }
        _ => reports.push(controller.disable()),
    }
    Ok(reports)
}
