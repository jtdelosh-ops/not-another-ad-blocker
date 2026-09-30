use super::{
    blocklist::{Decision, DnsPolicy},
    cache::{self, Cache},
    config::DnsConfig,
    diagnostics::Diagnostics,
    resolver,
};
use hickory_proto::{
    op::{Edns, Message, MessageType, OpCode, ResponseCode},
    rr::{rdata::TXT, DNSClass, RData, Record, RecordType},
};
use std::{
    future::Future,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::Semaphore,
    task::JoinSet,
    time::timeout,
};

struct State {
    config: DnsConfig,
    policy: Arc<DnsPolicy>,
    diagnostics: Arc<Diagnostics>,
    cache: Mutex<Cache>,
    health_token: Option<String>,
}

fn response(request: &Message, code: ResponseCode) -> Message {
    let mut result = Message::new();
    result
        .set_id(request.id())
        .set_message_type(MessageType::Response)
        .set_op_code(request.op_code())
        .set_recursion_desired(request.recursion_desired())
        .set_recursion_available(true)
        .set_checking_disabled(request.checking_disabled())
        .set_authentic_data(false)
        .set_response_code(code);
    if let Some(query) = request.queries().first() {
        result.add_query(query.clone());
    }
    if let Some(edns) = request.extensions() {
        let mut reply_edns = Edns::new();
        reply_edns
            .set_max_payload(1232)
            .set_dnssec_ok(edns.flags().dnssec_ok);
        result.set_edns(reply_edns);
    }
    result
}

fn validation(request: &Message) -> Result<(), ResponseCode> {
    if request.message_type() != MessageType::Query
        || request.truncated()
        || request.authoritative()
        || request.recursion_available()
        || request.response_code() != ResponseCode::NoError
    {
        return Err(ResponseCode::FormErr);
    }
    if request.op_code() != OpCode::Query {
        return Err(ResponseCode::NotImp);
    }
    if request.queries().len() != 1
        || request.queries()[0].query_class() != DNSClass::IN
        || !request.answers().is_empty()
        || !request.name_servers().is_empty()
        || !request.additionals().is_empty()
        || !request.signature().is_empty()
    {
        return Err(ResponseCode::FormErr);
    }
    if matches!(
        request.queries()[0].query_type(),
        RecordType::ANY | RecordType::AXFR | RecordType::IXFR | RecordType::OPT | RecordType::TSIG
    ) {
        return Err(ResponseCode::Refused);
    }
    if let Some(edns) = request.extensions() {
        if edns.version() != 0 {
            return Err(ResponseCode::BADVERS);
        }
        if !edns.options().as_ref().is_empty() || edns.flags().z != 0 {
            return Err(ResponseCode::Refused);
        }
    }
    Ok(())
}

fn cacheable(request: &Message) -> bool {
    !request.checking_disabled()
        && !request.authentic_data()
        && request
            .extensions()
            .as_ref()
            .is_none_or(|edns| !edns.flags().dnssec_ok && edns.options().as_ref().is_empty())
}

fn encode(mut reply: Message, request: &Message, tcp: bool) -> Option<Vec<u8>> {
    if let Some(edns) = reply.extensions_mut() {
        edns.set_max_payload(1232);
    }
    let max_size = if tcp {
        65_535
    } else {
        request
            .extensions()
            .as_ref()
            .map_or(512, |edns| usize::from(edns.max_payload()).clamp(512, 1232))
    };
    let bytes = reply.to_vec().ok()?;
    if bytes.len() <= max_size {
        return Some(bytes);
    }
    if tcp {
        return response(request, ResponseCode::ServFail).to_vec().ok();
    }
    reply.take_answers();
    reply.take_name_servers();
    reply.take_additionals();
    reply.take_signature();
    reply.set_truncated(true).set_authentic_data(false);
    if let Some(edns) = reply.extensions_mut() {
        let dnssec_ok = edns.flags().dnssec_ok;
        *edns = Edns::new();
        edns.set_max_payload(1232).set_dnssec_ok(dnssec_ok);
    }
    reply.to_vec().ok().filter(|bytes| bytes.len() <= max_size)
}

impl State {
    fn health_packet(&self, bytes: &[u8], tcp: bool) -> Option<Vec<u8>> {
        self.health_token.as_ref()?;
        let request = resolver::decode(bytes).ok()?;
        validation(&request).ok()?;
        self.health_reply(&request, tcp)
    }

    fn health_reply(&self, request: &Message, tcp: bool) -> Option<Vec<u8>> {
        let token = self.health_token.as_ref()?;
        let query = request.queries().first()?;
        let hostname = query
            .name()
            .to_ascii()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if !hostname.ends_with(".naab-health.invalid") {
            return None;
        }
        let mut reply = response(request, ResponseCode::NoError);
        if query.query_type() == RecordType::TXT {
            reply.add_answer(Record::from_rdata(
                query.name().clone(),
                0,
                RData::TXT(TXT::new(vec![token.clone()])),
            ));
        }
        encode(reply, request, tcp)
    }

    async fn handle(&self, bytes: &[u8], tcp: bool) -> Option<Vec<u8>> {
        let empty = Decision {
            blocked: false,
            source: None,
            rule: None,
        };
        let request = match resolver::decode(bytes) {
            Ok(request) => request,
            Err(_) => {
                self.diagnostics.record("", "", "malformed", &empty);
                return None;
            }
        };
        let hostname = request
            .queries()
            .first()
            .map(|query| {
                query
                    .name()
                    .to_ascii()
                    .trim_end_matches('.')
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        let query_type = request
            .queries()
            .first()
            .map(|query| query.query_type().to_string())
            .unwrap_or_default();
        if let Err(code) = validation(&request) {
            self.diagnostics.record(
                &hostname,
                &query_type,
                if code == ResponseCode::FormErr {
                    "malformed"
                } else {
                    "refused"
                },
                &empty,
            );
            return encode(response(&request, code), &request, tcp);
        }
        // This endpoint exists only in the explicitly selected system preview.
        // It bypasses filtering, cache and upstreams, so an outage cannot masquerade
        // as local resolver failure. The per-process token detects stale listeners.
        if let Some(reply) = self.health_reply(&request, tcp) {
            return Some(reply);
        }
        let decision = self.policy.decide(&hostname);
        if decision.blocked {
            self.diagnostics
                .record(&hostname, &query_type, "blocked", &decision);
            return encode(response(&request, ResponseCode::NXDomain), &request, tcp);
        }
        let cache_key = format!(
            "{}:edns{:?}",
            cache::key(&request.queries()[0], request.recursion_desired()),
            request.extensions().as_ref().map(|edns| edns.max_payload())
        );
        let cached = if cacheable(&request) {
            self.cache
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .get(&cache_key, Instant::now())
        } else {
            None
        };
        if let Some(mut reply) = cached {
            reply.set_id(request.id());
            reply.queries_mut().clear();
            reply.add_query(request.queries()[0].clone());
            self.diagnostics
                .record(&hostname, &query_type, "cache-hit", &decision);
            return encode(reply, &request, tcp);
        }
        let reply = match resolver::forward(
            &request,
            &self.config.upstreams,
            self.config.timeout_ms,
            tcp,
        )
        .await
        {
            Ok(reply) => {
                if cacheable(&request)
                    && reply
                        .extensions()
                        .as_ref()
                        .is_none_or(|edns| edns.options().as_ref().is_empty())
                    && reply.signature().is_empty()
                {
                    self.cache
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .insert(cache_key, &reply, Instant::now());
                }
                self.diagnostics
                    .record(&hostname, &query_type, "forwarded", &decision);
                reply
            }
            Err(_) => {
                self.diagnostics
                    .record(&hostname, &query_type, "upstream-error", &decision);
                response(&request, ResponseCode::ServFail)
            }
        };
        encode(reply, &request, tcp)
    }
}

async fn tcp_client(
    mut stream: TcpStream,
    state: Arc<State>,
    request_permits: Option<Arc<Semaphore>>,
) {
    let deadline = Duration::from_millis(state.config.timeout_ms);
    for _ in 0..64 {
        let packet = timeout(deadline, async {
            let length = stream.read_u16().await?;
            if length < 12 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Short DNS frame",
                ));
            }
            let mut packet = vec![0; usize::from(length)];
            stream.read_exact(&mut packet).await?;
            Ok::<_, io::Error>(packet)
        })
        .await;
        let Ok(Ok(packet)) = packet else { break };
        let reply = if let Some(reply) = state.health_packet(&packet, true) {
            reply
        } else {
            // The preview reserves TCP admission separately. Ordinary requests
            // still share the upstream-work limit with UDP; health bypasses it.
            let _request = match &request_permits {
                Some(permits) => match permits.clone().try_acquire_owned() {
                    Ok(permit) => Some(permit),
                    Err(_) => break,
                },
                None => None,
            };
            let Some(reply) = state.handle(&packet, true).await else {
                break;
            };
            reply
        };
        if !matches!(
            timeout(deadline, async {
                stream.write_u16(reply.len() as u16).await?;
                stream.write_all(&reply).await
            })
            .await,
            Ok(Ok(()))
        ) {
            break;
        }
    }
}

pub async fn serve(
    config: DnsConfig,
    policy: Arc<DnsPolicy>,
    diagnostics: Arc<Diagnostics>,
    shutdown: impl Future<Output = ()> + Send,
) -> io::Result<()> {
    config
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let socket = Arc::new(UdpSocket::bind(config.listen).await?);
    let listener = TcpListener::bind(config.listen).await?;
    serve_bound(
        config,
        policy,
        diagnostics,
        socket,
        listener,
        None,
        shutdown,
    )
    .await
}

/// Explicit system-preview entry point. All four sockets are acquired before any
/// queries are served. The ordinary development entry point still rejects port 53.
pub async fn serve_system_preview(
    config: DnsConfig,
    policy: Arc<DnsPolicy>,
    diagnostics: Arc<Diagnostics>,
    token: String,
    shutdown: impl Future<Output = ()> + Send,
) -> io::Result<()> {
    config.validate().map_err(io::Error::other)?;
    if config
        .upstreams
        .iter()
        .any(|s| s.ip().to_canonical().is_loopback())
    {
        return Err(io::Error::other(
            "System preview upstreams must not be loopback addresses",
        ));
    }
    if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::other("Invalid health token"));
    }
    let v4 = "127.0.0.1:53";
    let v6 = "[::1]:53";
    let udp4 = Arc::new(UdpSocket::bind(v4).await?);
    let tcp4 = TcpListener::bind(v4).await?;
    let udp6 = Arc::new(UdpSocket::bind(v6).await?);
    let tcp6 = TcpListener::bind(v6).await?;
    tokio::pin!(shutdown);
    tokio::select! {
        result = serve_bound(config.clone(), policy.clone(), diagnostics.clone(), udp4, tcp4, Some(token.clone()), std::future::pending()) => result,
        result = serve_bound(config, policy, diagnostics, udp6, tcp6, Some(token), std::future::pending()) => result,
        _ = &mut shutdown => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_bound(
    config: DnsConfig,
    policy: Arc<DnsPolicy>,
    diagnostics: Arc<Diagnostics>,
    socket: Arc<UdpSocket>,
    listener: TcpListener,
    health_token: Option<String>,
    shutdown: impl Future<Output = ()> + Send,
) -> io::Result<()> {
    let permits = Arc::new(Semaphore::new(config.max_in_flight));
    // At most max_in_flight ordinary upstream requests can occupy connections;
    // four extra bounded slots keep watchdog TCP probes admissible during an
    // upstream outage. Idle clients still have the existing read deadline.
    let client_permits = if health_token.is_some() {
        Arc::new(Semaphore::new(config.max_in_flight + 4))
    } else {
        permits.clone()
    };
    let state = Arc::new(State {
        cache: Mutex::new(Cache::new(config.cache_entries)),
        config,
        policy,
        diagnostics,
        health_token,
    });
    let mut tasks = JoinSet::new();
    let mut buffer = vec![0; 65_535];
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            packet = socket.recv_from(&mut buffer) => {
                let (length, peer) = packet?;
                if !peer.ip().is_loopback() { continue; }
                if let Some(reply) = state.health_packet(&buffer[..length], false) {
                    let _ = socket.send_to(&reply, peer).await;
                    continue;
                }
                let Ok(permit) = permits.clone().try_acquire_owned() else { continue };
                let packet = buffer[..length].to_vec();
                let state = state.clone();
                let socket = socket.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    if let Some(reply) = state.handle(&packet, false).await { let _ = socket.send_to(&reply, peer).await; }
                });
            },
            client = listener.accept() => {
                let (stream, peer) = client?;
                if !peer.ip().is_loopback() { continue; }
                let Ok(permit) = client_permits.clone().try_acquire_owned() else { continue };
                let request_permits = state.health_token.as_ref().map(|_| permits.clone());
                let state = state.clone();
                tasks.spawn(async move { let _permit = permit; tcp_client(stream, state, request_permits).await; });
            },
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

#[cfg(test)]
mod system_preview_tests {
    use super::*;
    use crate::dns::system::{
        health::{local_probe, LiveProbe},
        Change, Check, DnsSetting, Family, HealthProbe, Target,
    };
    use std::net::SocketAddr;

    #[tokio::test]
    async fn health_remains_available_when_upstream_work_exhausts_normal_capacity() {
        for tcp_work in [false, true] {
            let upstream_tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let upstream = UdpSocket::bind(upstream_tcp.local_addr().unwrap())
                .await
                .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let udp = Arc::new(UdpSocket::bind(address).await.unwrap());
            let config: DnsConfig = serde_json::from_value(serde_json::json!({"listen":address,"upstreams":[upstream.local_addr().unwrap()],"timeoutMs":3000,"maxInFlight":1})).unwrap();
            let token = "0123456789abcdef0123456789abcdef";
            let task = tokio::spawn(serve_bound(
                config,
                Arc::new(DnsPolicy::compile(&[], &[], &[]).unwrap()),
                Arc::new(Diagnostics::new(0)),
                udp,
                listener,
                Some(token.into()),
                std::future::pending(),
            ));
            let client = tokio::spawn(async move {
                let mut query = Message::new();
                query.add_query(hickory_proto::op::Query::query(
                    hickory_proto::rr::Name::from_ascii("busy.example.").unwrap(),
                    RecordType::A,
                ));
                resolver::forward(&query, &[address], 4000, tcp_work).await
            });
            let _silent_stream = if tcp_work {
                let (mut stream, _) = timeout(Duration::from_secs(2), upstream_tcp.accept())
                    .await
                    .unwrap()
                    .unwrap();
                let length = stream.read_u16().await.unwrap();
                let mut bytes = vec![0; usize::from(length)];
                stream.read_exact(&mut bytes).await.unwrap();
                Some(stream)
            } else {
                let mut bytes = [0u8; 512];
                timeout(Duration::from_secs(2), upstream.recv_from(&mut bytes))
                    .await
                    .unwrap()
                    .unwrap();
                None
            };
            // The one normal permit is now held waiting for an upstream that never
            // answers. Both health transports must still pass concurrently.
            let (udp_ok, tcp_ok) = tokio::join!(
                local_probe(address, token, false),
                local_probe(address, token, true)
            );
            client.abort();
            task.abort();
            let _ = client.await;
            let _ = task.await;
            assert!(
                udp_ok && tcp_ok,
                "upstream TCP={tcp_work}: UDP health={udp_ok}, TCP health={tcp_ok}"
            );
        }
    }

    #[tokio::test]
    #[ignore = "Opt-in standard-port test; requires unused loopback port 53. Never changes OS DNS."]
    async fn standard_port_preview_serves_all_four_listeners_and_releases_them() {
        let config: DnsConfig =
            serde_json::from_value(serde_json::json!({"upstreams":["192.0.2.53:53"]})).unwrap();
        let policy = Arc::new(DnsPolicy::compile(&[], &[], &[]).unwrap());
        let token = "0123456789abcdef0123456789abcdef";
        let (stop, shutdown) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(serve_system_preview(
            config,
            policy,
            Arc::new(Diagnostics::new(0)),
            token.into(),
            async {
                let _ = shutdown.await;
            },
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut outcomes = Vec::new();
        for destination in ["127.0.0.1:53", "[::1]:53"] {
            for tcp in [false, true] {
                outcomes.push((
                    destination,
                    tcp,
                    local_probe(destination.parse().unwrap(), token, tcp).await,
                ));
            }
        }
        let _ = stop.send(());
        let result = task.await.unwrap();
        assert!(result.is_ok(), "listener startup failed: {result:?}");
        assert!(
            outcomes.iter().all(|(_, _, passed)| *passed),
            "{outcomes:?}"
        );
        for destination in ["127.0.0.1:53", "[::1]:53"] {
            let _udp = UdpSocket::bind(destination).await.unwrap();
            let _tcp = TcpListener::bind(destination).await.unwrap();
        }
    }

    #[tokio::test]
    async fn health_uses_live_udp_tcp_token_without_filter_cache_or_upstream() {
        for host in ["127.0.0.1:0", "[::1]:0"] {
            let listener = TcpListener::bind(host).await.unwrap();
            let address = listener.local_addr().unwrap();
            let udp = Arc::new(UdpSocket::bind(address).await.unwrap());
            let config: DnsConfig = serde_json::from_value(
                serde_json::json!({"listen":address,"upstreams":["127.0.0.1:9"],"timeoutMs":100}),
            )
            .unwrap();
            let policy = Arc::new(
                DnsPolicy::compile(
                    &[],
                    &[],
                    &["naab-health.invalid".into(), "ads.example.test".into()],
                )
                .unwrap(),
            );
            let diagnostics = Arc::new(Diagnostics::new(10));
            let token = "0123456789abcdef0123456789abcdef";
            let task = tokio::spawn(serve_bound(
                config,
                policy,
                diagnostics,
                udp,
                listener,
                Some(token.into()),
                std::future::pending(),
            ));
            assert!(local_probe(address, token, false).await);
            assert!(local_probe(address, token, true).await);
            assert!(!local_probe(address, "ffffffffffffffffffffffffffffffff", false).await);
            let change = Change {
                target: Target {
                    interface_id: "test".into(),
                    network_id: "test".into(),
                    family: if address.is_ipv4() {
                        Family::Ipv4
                    } else {
                        Family::Ipv6
                    },
                },
                original: DnsSetting::Automatic,
                applied: DnsSetting::Static(vec![address.ip()]),
            };
            let health = tokio::task::spawn_blocking(move || {
                LiveProbe::new(
                    token.into(),
                    vec!["127.0.0.1:9".parse::<SocketAddr>().unwrap()],
                    address.port(),
                )
                .unwrap()
                .check(&[change])
            })
            .await
            .unwrap();
            assert_eq!(health.local, Check::Passed);
            assert_eq!(health.upstream, Check::Failed);
            task.abort();
            let _ = task.await;
            assert!(!local_probe(address, token, true).await);
        }
    }
}
