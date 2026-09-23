use super::blocklist::Decision;
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub timestamp_ms: u64,
    pub layer: &'static str,
    pub hostname: String,
    pub query_type: String,
    pub outcome: String,
    pub source: Option<String>,
    pub rule: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub total: u64,
    pub outcomes: BTreeMap<String, u64>,
    pub recent: VecDeque<ActivityEntry>,
}

pub struct Diagnostics {
    capacity: usize,
    state: Mutex<Snapshot>,
}

fn bounded(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

impl Diagnostics {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.min(1_000),
            state: Mutex::new(Snapshot::default()),
        }
    }

    pub fn record(&self, hostname: &str, query_type: &str, outcome: &str, decision: &Decision) {
        let mut state = self.state.lock().unwrap();
        state.total = state.total.saturating_add(1);
        let outcome = match outcome {
            "blocked" | "cache-hit" | "forwarded" | "upstream-error" | "refused" | "malformed" => {
                outcome
            }
            _ => "other",
        };
        let count = state.outcomes.entry(outcome.to_owned()).or_default();
        *count = count.saturating_add(1);
        if self.capacity == 0 {
            return;
        }
        if state.recent.len() == self.capacity {
            state.recent.pop_front();
        }
        state.recent.push_back(ActivityEntry {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u64::MAX as u128) as u64,
            layer: "DNS",
            hostname: bounded(hostname, 253),
            query_type: bounded(query_type, 24),
            outcome: outcome.to_owned(),
            source: decision.source.as_deref().map(|s| bounded(s, 128)),
            rule: decision.rule.as_deref().map(|s| bounded(s, 512)),
        });
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        self.state.lock().unwrap().recent.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activity_is_bounded_and_clear_preserves_counters() {
        let log = Diagnostics::new(2);
        let decision = Decision {
            blocked: false,
            source: None,
            rule: None,
        };
        for host in ["a.test", "b.test", "c.test"] {
            log.record(host, "A", "forwarded", &decision);
        }
        let view = log.snapshot();
        assert_eq!(view.total, 3);
        assert_eq!(view.recent.len(), 2);
        assert_eq!(view.recent[0].hostname, "b.test");
        assert_eq!(view.recent[0].layer, "DNS");
        log.clear();
        assert!(log.snapshot().recent.is_empty());
        assert_eq!(log.snapshot().total, 3);
        let disabled = Diagnostics::new(0);
        disabled.record("secret.test", "A", "forwarded", &decision);
        assert!(disabled.snapshot().recent.is_empty());
    }
}
