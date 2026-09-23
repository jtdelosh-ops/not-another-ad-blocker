use super::{
    blocklist::{Decision, DnsPolicy},
    cache::{self, Cache},
    config::DnsConfig,
    diagnostics::Diagnostics,
    resolver,
};
use hickory_proto::{
    op::{Edns, Message, MessageType, OpCode, ResponseCode},
    rr::{DNSClass, RecordType},
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

async fn tcp_client(mut stream: TcpStream, state: Arc<State>) {
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
        let Some(reply) = state.handle(&packet, true).await else {
            break;
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
    let permits = Arc::new(Semaphore::new(config.max_in_flight));
    let state = Arc::new(State {
        cache: Mutex::new(Cache::new(config.cache_entries)),
        config,
        policy,
        diagnostics,
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
                let Ok(permit) = permits.clone().try_acquire_owned() else { continue };
                let state = state.clone();
                tasks.spawn(async move { let _permit = permit; tcp_client(stream, state).await; });
            },
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}
