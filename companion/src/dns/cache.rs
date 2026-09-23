use hickory_proto::{
    op::{Message, Query, ResponseCode},
    rr::RData,
};
use std::{
    collections::{HashMap, VecDeque},
    time::Instant,
};

struct Entry {
    bytes: Vec<u8>,
    inserted: Instant,
    ttl: u32,
}

pub(super) struct Cache {
    capacity: usize,
    entries: HashMap<String, Entry>,
    order: VecDeque<String>,
}

pub(super) fn key(query: &Query, recursion: bool) -> String {
    format!(
        "{}:{:?}:{recursion}",
        query.name().to_lowercase(),
        query.query_type()
    )
}

impl Cache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn get(&mut self, key: &str, now: Instant) -> Option<Message> {
        let entry = self.entries.get(key)?;
        let age = now.saturating_duration_since(entry.inserted).as_secs();
        if age >= u64::from(entry.ttl) {
            self.entries.remove(key);
            self.order.retain(|item| item != key);
            return None;
        }
        let mut message = Message::from_vec(&entry.bytes).ok()?;
        for record in message.answers_mut() {
            record.set_ttl(record.ttl().saturating_sub(age as u32));
        }
        for record in message.name_servers_mut() {
            record.set_ttl(record.ttl().saturating_sub(age as u32));
        }
        for record in message.additionals_mut() {
            record.set_ttl(record.ttl().saturating_sub(age as u32));
        }
        Some(message)
    }

    pub fn insert(&mut self, key: String, message: &Message, now: Instant) {
        if self.capacity == 0 || message.truncated() {
            return;
        }
        let all_records = || {
            message
                .answers()
                .iter()
                .chain(message.name_servers())
                .chain(message.additionals())
        };
        let soa_ttl = message
            .name_servers()
            .iter()
            .filter_map(|record| match record.data() {
                RData::SOA(soa) => Some(record.ttl().min(soa.minimum())),
                _ => None,
            })
            .min();
        let ttl = match message.response_code() {
            ResponseCode::NoError if !message.answers().is_empty() => {
                all_records().map(|record| record.ttl()).min()
            }
            ResponseCode::NoError | ResponseCode::NXDomain => soa_ttl.map(|ttl| {
                all_records()
                    .map(|record| record.ttl())
                    .min()
                    .unwrap_or(ttl)
                    .min(ttl)
            }),
            _ => None,
        }
        .map(|ttl| soa_ttl.map_or(ttl, |negative| negative.min(ttl)));
        let Some(ttl) = ttl.filter(|ttl| *ttl > 0) else {
            return;
        };
        let mut stored = message.clone();
        for record in stored.name_servers_mut() {
            if let RData::SOA(soa) = record.data() {
                record.set_ttl(record.ttl().min(soa.minimum()));
            }
        }
        let Ok(bytes) = stored.to_vec() else { return };
        if bytes.len() > 16_384 {
            return;
        }
        self.order.retain(|item| item != &key);
        while self.entries.len() >= self.capacity && !self.entries.contains_key(&key) {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(
            key,
            Entry {
                bytes,
                inserted: now,
                ttl: ttl.min(3600),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::{
        op::ResponseCode,
        rr::{
            rdata::{A, SOA},
            Name, RData, Record,
        },
    };
    use std::{net::Ipv4Addr, time::Duration};

    fn answer(ttl: u32) -> Message {
        let mut message = Message::new();
        message.add_answer(Record::from_rdata(
            Name::from_ascii("example.test.").unwrap(),
            ttl,
            RData::A(A(Ipv4Addr::LOCALHOST)),
        ));
        message
    }

    #[test]
    fn cache_ages_ttls_expires_and_enforces_capacity() {
        let start = Instant::now();
        let mut cache = Cache::new(1);
        cache.insert("one".into(), &answer(10), start);
        assert_eq!(
            cache
                .get("one", start + Duration::from_secs(3))
                .unwrap()
                .answers()[0]
                .ttl(),
            7
        );
        assert!(cache.get("one", start + Duration::from_secs(10)).is_none());
        cache.insert("one".into(), &answer(10), start);
        cache.insert("two".into(), &answer(10), start);
        assert!(cache.get("one", start).is_none());
        assert!(cache.get("two", start).is_some());
    }

    #[test]
    fn cache_requires_valid_positive_or_soa_negative_ttl() {
        let start = Instant::now();
        let mut cache = Cache::new(10);
        for (key, mut message) in [
            ("zero", answer(0)),
            ("fail", answer(30)),
            ("tc", answer(30)),
            ("empty", Message::new()),
        ] {
            if key == "fail" {
                message.set_response_code(ResponseCode::ServFail);
            }
            if key == "tc" {
                message.set_truncated(true);
            }
            cache.insert(key.into(), &message, start);
            assert!(cache.get(key, start).is_none());
        }
        let mut negative = Message::new();
        negative.set_response_code(ResponseCode::NXDomain);
        negative.add_name_server(Record::from_rdata(
            Name::from_ascii("example.test.").unwrap(),
            60,
            RData::SOA(SOA::new(
                Name::from_ascii("ns.example.test.").unwrap(),
                Name::from_ascii("hostmaster.example.test.").unwrap(),
                1,
                60,
                60,
                60,
                5,
            )),
        ));
        cache.insert("negative".into(), &negative, start);
        assert_eq!(
            cache
                .get("negative", start + Duration::from_secs(4))
                .unwrap()
                .name_servers()[0]
                .ttl(),
            1
        );
        assert!(cache
            .get("negative", start + Duration::from_secs(5))
            .is_none());
        negative.set_response_code(ResponseCode::NoError);
        negative.add_answer(Record::from_rdata(
            Name::from_ascii("alias.test.").unwrap(),
            60,
            RData::CNAME(hickory_proto::rr::rdata::CNAME(
                Name::from_ascii("absent.example.test.").unwrap(),
            )),
        ));
        cache.insert("alias-negative".into(), &negative, start);
        assert_eq!(
            cache
                .get("alias-negative", start + Duration::from_secs(4))
                .unwrap()
                .name_servers()[0]
                .ttl(),
            1
        );
        assert!(cache
            .get("alias-negative", start + Duration::from_secs(5))
            .is_none());
    }

    #[test]
    fn cache_zero_capacity_and_packet_byte_limit_are_enforced() {
        let now = Instant::now();
        let mut disabled = Cache::new(0);
        disabled.insert("disabled".into(), &answer(30), now);
        assert!(disabled.get("disabled", now).is_none());
        let mut large = Message::new();
        for _ in 0..100 {
            large.add_answer(Record::from_rdata(
                Name::from_ascii("example.test.").unwrap(),
                30,
                RData::TXT(hickory_proto::rr::rdata::TXT::new(vec!["x".repeat(200)])),
            ));
        }
        assert!(large.to_vec().unwrap().len() > 16_384);
        let mut bounded = Cache::new(1);
        bounded.insert("large".into(), &large, now);
        assert!(bounded.get("large", now).is_none());
    }
}
