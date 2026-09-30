//! Bounded probes using explicit destinations, independent of system DNS settings.
use super::{Change, Check, Family, Health, HealthProbe};
use crate::dns::resolver;
use hickory_proto::{
    op::{Message, Query, ResponseCode},
    rr::{Name, RData, RecordType},
};
use std::net::SocketAddr;

pub struct LiveProbe {
    runtime: tokio::runtime::Runtime,
    token: String,
    upstreams: Vec<SocketAddr>,
    port: u16,
}

impl LiveProbe {
    pub fn new(token: String, upstreams: Vec<SocketAddr>, port: u16) -> Result<Self, String> {
        if token.len() != 32
            || !token.bytes().all(|b| b.is_ascii_hexdigit())
            || port == 0
            || upstreams.is_empty()
            || upstreams.len() > 4
        {
            return Err("Invalid live probe configuration".into());
        }
        Ok(Self {
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?,
            token,
            upstreams,
            port,
        })
    }
}

pub async fn local_probe(destination: SocketAddr, token: &str, tcp: bool) -> bool {
    let mut query = Message::new();
    let name = Name::from_ascii(format!(
        "{:032x}.naab-health.invalid.",
        rand::random::<u128>()
    ))
    .unwrap();
    query
        .set_id(rand::random())
        .add_query(Query::query(name, RecordType::TXT));
    let Ok(reply) = resolver::forward(&query, &[destination], 750, tcp).await else {
        return false;
    };
    reply.response_code() == ResponseCode::NoError
        && reply.answers().len() == 1
        && reply.answers()[0].name() == query.queries()[0].name()
        && reply.answers()[0].ttl() == 0
        && matches!(reply.answers()[0].data(), RData::TXT(txt) if txt.txt_data().len() == 1 && txt.txt_data()[0].as_ref() == token.as_bytes())
}

impl HealthProbe for LiveProbe {
    fn check(&mut self, changes: &[Change]) -> Health {
        self.runtime.block_on(async {
            let mut local = true;
            for family in [Family::Ipv4, Family::Ipv6] {
                if !changes.iter().any(|c| c.target.family == family) { continue; }
                let ip = match family { Family::Ipv4 => "127.0.0.1", Family::Ipv6 => "::1" };
                let destination = SocketAddr::new(ip.parse().unwrap(), self.port);
                let (udp, tcp) = tokio::join!(local_probe(destination, &self.token, false), local_probe(destination, &self.token, true));
                local &= udp && tcp;
            }
            let mut query = Message::new();
            query.set_id(rand::random()).set_recursion_desired(true).set_checking_disabled(true)
                .add_query(Query::query(Name::from_ascii("example.com.").unwrap(), RecordType::A));
            // Direct exchanges never consult NAAB's cache or the OS resolver.
            let upstream = matches!(resolver::forward(&query, &self.upstreams, 750, false).await,
                Ok(reply) if reply.response_code() == ResponseCode::NoError && !reply.answers().is_empty());
            Health { local: if local { Check::Passed } else { Check::Failed }, upstream: if upstream { Check::Passed } else { Check::Failed } }
        })
    }
}
