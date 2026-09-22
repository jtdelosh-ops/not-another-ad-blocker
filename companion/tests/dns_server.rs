use hickory_proto::{
    op::{Edns, Message, MessageType, OpCode, Query, ResponseCode},
    rr::{
        rdata::{A, TXT},
        DNSClass, Name, RData, Record, RecordType,
    },
};
use naab_companion::dns::{
    blocklist::DnsPolicy, diagnostics::Diagnostics, server::serve, DnsConfig,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::oneshot,
    task::JoinHandle,
    time::{sleep, timeout},
};

struct Upstream {
    address: SocketAddr,
    udp_count: Arc<AtomicUsize>,
    tcp_count: Arc<AtomicUsize>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn reply_to(request: Message, tcp: bool, wrong_id: bool) -> Option<Message> {
    let query = request.queries()[0].clone();
    let host = query.name().to_ascii();
    if host.starts_with("timeout.") {
        return None;
    }
    if wrong_id && host.starts_with("retry-timeout.") {
        return None;
    }
    let mut reply = Message::new();
    reply
        .set_id(if wrong_id {
            request.id().wrapping_add(1)
        } else {
            request.id()
        })
        .set_message_type(MessageType::Response)
        .set_recursion_desired(request.recursion_desired())
        .set_recursion_available(true)
        .add_query(query.clone());
    if let Some(edns) = request.extensions() {
        reply.set_edns(edns.clone());
    }
    if wrong_id && (host.starts_with("retry.") || host.starts_with("retry-refused.")) {
        reply
            .set_id(request.id())
            .set_response_code(if host.starts_with("retry-refused.") {
                ResponseCode::Refused
            } else {
                ResponseCode::ServFail
            });
        return Some(reply);
    }
    if host.starts_with("servfail.") {
        reply.set_response_code(ResponseCode::ServFail);
        return Some(reply);
    }
    if host.starts_with("truncated.") && !tcp {
        reply.set_truncated(true);
        return Some(reply);
    }
    if host.starts_with("large.") {
        for index in 0..12 {
            reply.add_answer(Record::from_rdata(
                query.name().clone(),
                30,
                RData::TXT(TXT::new(vec![format!("{index}{}", "x".repeat(160))])),
            ));
        }
    } else {
        let ttl = if host.starts_with("zero.") {
            0
        } else if host.starts_with("expire.") {
            1
        } else {
            30
        };
        reply.add_answer(Record::from_rdata(
            query.name().clone(),
            ttl,
            RData::A(A(Ipv4Addr::new(192, 0, 2, 1))),
        ));
    }
    Some(reply)
}

async fn upstream(wrong_id: bool) -> Upstream {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let listener = TcpListener::bind(address).await.unwrap();
    let udp_count = Arc::new(AtomicUsize::new(0));
    let tcp_count = Arc::new(AtomicUsize::new(0));
    let udp_seen = udp_count.clone();
    let udp_task = tokio::spawn(async move {
        let mut buffer = vec![0; 65_535];
        loop {
            let (length, peer) = socket.recv_from(&mut buffer).await.unwrap();
            udp_seen.fetch_add(1, Ordering::SeqCst);
            if let Some(reply) = reply_to(
                Message::from_vec(&buffer[..length]).unwrap(),
                false,
                wrong_id,
            ) {
                socket
                    .send_to(&reply.to_vec().unwrap(), peer)
                    .await
                    .unwrap();
            }
        }
    });
    let tcp_seen = tcp_count.clone();
    let tcp_task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let length = stream.read_u16().await.unwrap();
            let mut buffer = vec![0; usize::from(length)];
            stream.read_exact(&mut buffer).await.unwrap();
            tcp_seen.fetch_add(1, Ordering::SeqCst);
            if let Some(reply) = reply_to(Message::from_vec(&buffer).unwrap(), true, wrong_id) {
                let bytes = reply.to_vec().unwrap();
                stream.write_u16(bytes.len() as u16).await.unwrap();
                stream.write_all(&bytes).await.unwrap();
            }
        }
    });
    Upstream {
        address,
        udp_count,
        tcp_count,
        tasks: vec![udp_task, tcp_task],
    }
}

struct Server {
    address: SocketAddr,
    diagnostics: Arc<Diagnostics>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl Server {
    async fn stop(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        timeout(Duration::from_secs(1), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(TcpStream::connect(self.address).await.is_err());
    }
}

async fn start(upstreams: &[SocketAddr], capacity: usize, concurrency: usize) -> Server {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    let config: DnsConfig = serde_json::from_value(serde_json::json!({
        "listen": address, "upstreams": upstreams, "timeoutMs": 150, "cacheEntries": capacity, "maxInFlight": concurrency
    })).unwrap();
    let policy = Arc::new(
        DnsPolicy::compile(
            &[],
            &["allowed.blocked.test".into()],
            &["blocked.test".into()],
        )
        .unwrap(),
    );
    let diagnostics = Arc::new(Diagnostics::new(100));
    let (sender, receiver) = oneshot::channel();
    let record = diagnostics.clone();
    let task = tokio::spawn(serve(config, policy, record, async {
        let _ = receiver.await;
    }));
    for _ in 0..100 {
        if TcpStream::connect(address).await.is_ok() {
            sleep(Duration::from_millis(5)).await;
            return Server {
                address,
                diagnostics,
                stop: Some(sender),
                task: Some(task),
            };
        }
        sleep(Duration::from_millis(5)).await;
    }
    panic!("DNS server did not bind");
}

fn query(name: &str, kind: RecordType, id: u16) -> Message {
    let mut message = Message::new();
    message
        .set_id(id)
        .set_recursion_desired(true)
        .add_query(Query::query(Name::from_ascii(name).unwrap(), kind));
    message
}

async fn udp(address: SocketAddr, request: &Message) -> Message {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(address).await.unwrap();
    socket.send(&request.to_vec().unwrap()).await.unwrap();
    let mut buffer = vec![0; 65_535];
    let length = timeout(Duration::from_secs(2), socket.recv(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    Message::from_vec(&buffer[..length]).unwrap()
}

async fn tcp(address: SocketAddr, request: &Message) -> Message {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let bytes = request.to_vec().unwrap();
    stream.write_u16(bytes.len() as u16).await.unwrap();
    stream.write_all(&bytes).await.unwrap();
    let length = timeout(Duration::from_secs(2), stream.read_u16())
        .await
        .unwrap()
        .unwrap();
    let mut buffer = vec![0; usize::from(length)];
    stream.read_exact(&mut buffer).await.unwrap();
    Message::from_vec(&buffer).unwrap()
}

#[tokio::test]
async fn block_allow_cache_and_client_ids_work_over_real_sockets() {
    let upstream = upstream(false).await;
    let server = start(&[upstream.address], 10, 8).await;
    for kind in [RecordType::A, RecordType::AAAA] {
        let blocked = udp(server.address, &query("child.blocked.test", kind, 31)).await;
        assert_eq!(blocked.response_code(), ResponseCode::NXDomain);
        assert!(!blocked.authentic_data());
    }
    assert_eq!(upstream.udp_count.load(Ordering::SeqCst), 0);
    let first = udp(
        server.address,
        &query("allowed.blocked.test", RecordType::A, 10),
    )
    .await;
    assert_eq!(first.response_code(), ResponseCode::NoError);
    assert_eq!(first.answers().len(), 1);
    let second = udp(
        server.address,
        &query("ALLOWED.blocked.test", RecordType::A, 20),
    )
    .await;
    assert_eq!(second.id(), 20);
    assert_eq!(
        second.queries()[0].name().to_ascii(),
        "ALLOWED.blocked.test."
    );
    assert_eq!(upstream.udp_count.load(Ordering::SeqCst), 1);
    let from_tcp = tcp(
        server.address,
        &query("allowed.blocked.test", RecordType::A, 21),
    )
    .await;
    assert_eq!(from_tcp.id(), 21);
    assert_eq!(upstream.tcp_count.load(Ordering::SeqCst), 0);
    assert_eq!(server.diagnostics.snapshot().outcomes["cache-hit"], 2);
    server.stop().await;
}

#[tokio::test]
async fn upstream_truncation_uses_tcp_and_large_udp_replies_request_tcp() {
    let upstream = upstream(false).await;
    let server = start(&[upstream.address], 10, 8).await;
    let fallback = udp(server.address, &query("truncated.test", RecordType::A, 42)).await;
    assert_eq!(fallback.answers().len(), 1);
    assert!(!fallback.truncated());
    assert_eq!(upstream.tcp_count.load(Ordering::SeqCst), 1);
    let large = query("large.test", RecordType::TXT, 43);
    let truncated = udp(server.address, &large).await;
    assert!(truncated.truncated());
    assert!(truncated.to_vec().unwrap().len() <= 512);
    let complete = tcp(server.address, &large).await;
    assert!(!complete.truncated());
    assert_eq!(complete.answers().len(), 12);
    let mut edns = large.clone();
    edns.set_edns(Edns::new().set_max_payload(4096).clone());
    let truncated = udp(server.address, &edns).await;
    assert!(truncated.truncated());
    assert!(truncated.to_vec().unwrap().len() <= 1232);
    assert_eq!(truncated.extensions().as_ref().unwrap().max_payload(), 1232);
    edns.extensions_mut().as_mut().unwrap().set_dnssec_ok(true);
    let dnssec_truncated = udp(server.address, &edns).await;
    assert!(dnssec_truncated.truncated());
    assert!(
        dnssec_truncated
            .extensions()
            .as_ref()
            .unwrap()
            .flags()
            .dnssec_ok
    );
    server.stop().await;
}

#[tokio::test]
async fn bad_upstream_ids_fall_back_and_exhausted_upstreams_return_servfail() {
    let bad = upstream(true).await;
    let good = upstream(false).await;
    let server = start(&[bad.address, good.address], 10, 8).await;
    let reply = udp(server.address, &query("valid.test", RecordType::A, 100)).await;
    assert_eq!(reply.response_code(), ResponseCode::NoError);
    assert_eq!(reply.id(), 100);
    assert_eq!(bad.udp_count.load(Ordering::SeqCst), 1);
    assert_eq!(good.udp_count.load(Ordering::SeqCst), 1);
    for host in ["retry.test", "retry-refused.test", "retry-timeout.test"] {
        assert_eq!(
            udp(server.address, &query(host, RecordType::A, 102))
                .await
                .response_code(),
            ResponseCode::NoError
        );
    }
    assert_eq!(bad.udp_count.load(Ordering::SeqCst), 4);
    assert_eq!(good.udp_count.load(Ordering::SeqCst), 4);
    let reply = udp(server.address, &query("timeout.test", RecordType::A, 101)).await;
    assert_eq!(reply.response_code(), ResponseCode::ServFail);
    assert!(!reply.authentic_data());
    assert_eq!(server.diagnostics.snapshot().outcomes["upstream-error"], 1);
    server.stop().await;
}

#[tokio::test]
async fn unsupported_queries_and_malformed_packets_never_reach_upstream() {
    let upstream = upstream(false).await;
    let server = start(&[upstream.address], 10, 8).await;
    for kind in [RecordType::ANY, RecordType::AXFR, RecordType::IXFR] {
        assert_eq!(
            udp(server.address, &query("valid.test", kind, 1))
                .await
                .response_code(),
            ResponseCode::Refused
        );
    }
    let mut update = query("valid.test", RecordType::A, 2);
    update.set_op_code(OpCode::Update);
    assert_eq!(
        udp(server.address, &update).await.response_code(),
        ResponseCode::NotImp
    );
    let mut multiple = query("valid.test", RecordType::A, 3);
    multiple.add_query(Query::query(
        Name::from_ascii("other.test").unwrap(),
        RecordType::A,
    ));
    assert_eq!(
        udp(server.address, &multiple).await.response_code(),
        ResponseCode::FormErr
    );
    let mut class = query("valid.test", RecordType::A, 4);
    class.queries_mut()[0].set_query_class(DNSClass::CH);
    assert_eq!(
        udp(server.address, &class).await.response_code(),
        ResponseCode::FormErr
    );
    let mut edns = query("valid.test", RecordType::A, 5);
    edns.set_edns(Edns::new().set_version(1).clone());
    assert_eq!(
        u16::from(udp(server.address, &edns).await.response_code()),
        u16::from(ResponseCode::BADVERS)
    );
    let mut option = Edns::new();
    option
        .options_mut()
        .insert(hickory_proto::rr::rdata::opt::EdnsOption::Unknown(
            65001,
            vec![1, 2],
        ));
    edns.set_edns(option);
    assert_eq!(
        udp(server.address, &edns).await.response_code(),
        ResponseCode::Refused
    );
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.send_to(&[1, 2, 3], server.address).await.unwrap();
    let mut trailing = query("trailing.test", RecordType::A, 1).to_vec().unwrap();
    trailing.push(1);
    socket.send_to(&trailing, server.address).await.unwrap();
    sleep(Duration::from_millis(10)).await;
    assert_eq!(upstream.udp_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        udp(server.address, &query("valid.test", RecordType::A, 9))
            .await
            .response_code(),
        ResponseCode::NoError
    );
    assert!(server.diagnostics.snapshot().outcomes["malformed"] >= 3);
    server.stop().await;
}

#[tokio::test]
async fn cache_expires_evicts_and_bypasses_zero_ttl_errors_and_dnssec_queries() {
    let upstream = upstream(false).await;
    let server = start(&[upstream.address], 1, 8).await;
    for name in ["one.test", "two.test", "one.test"] {
        udp(server.address, &query(name, RecordType::A, 1)).await;
    }
    assert_eq!(upstream.udp_count.load(Ordering::SeqCst), 3);
    let expiring = query("expire.test", RecordType::A, 1);
    udp(server.address, &expiring).await;
    sleep(Duration::from_millis(1100)).await;
    udp(server.address, &expiring).await;
    assert_eq!(upstream.udp_count.load(Ordering::SeqCst), 5);
    for name in ["zero.test", "servfail.test"] {
        for _ in 0..2 {
            udp(server.address, &query(name, RecordType::A, 1)).await;
        }
    }
    assert_eq!(upstream.udp_count.load(Ordering::SeqCst), 9);
    let mut dnssec = query("dnssec.test", RecordType::A, 1);
    dnssec.set_edns(Edns::new().set_dnssec_ok(true).clone());
    for _ in 0..2 {
        udp(server.address, &dnssec).await;
    }
    let mut checking = query("checking.test", RecordType::A, 1);
    checking.set_checking_disabled(true);
    for _ in 0..2 {
        udp(server.address, &checking).await;
    }
    assert_eq!(upstream.udp_count.load(Ordering::SeqCst), 13);
    server.stop().await;
}

#[tokio::test]
async fn idle_tcp_clients_are_bounded_and_shutdown_cancels_pending_requests() {
    let upstream = upstream(false).await;
    let server = start(&[upstream.address], 10, 1).await;
    let idle = TcpStream::connect(server.address).await.unwrap();
    sleep(Duration::from_millis(10)).await;
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket
        .send_to(
            &query("valid.test", RecordType::A, 1).to_vec().unwrap(),
            server.address,
        )
        .await
        .unwrap();
    let mut buffer = [0; 512];
    assert!(
        timeout(Duration::from_millis(40), socket.recv_from(&mut buffer))
            .await
            .is_err()
    );
    assert_eq!(upstream.udp_count.load(Ordering::SeqCst), 0);
    drop(idle);
    sleep(Duration::from_millis(10)).await;
    assert_eq!(
        udp(server.address, &query("valid.test", RecordType::A, 1))
            .await
            .response_code(),
        ResponseCode::NoError
    );
    socket
        .send_to(
            &query("timeout.test", RecordType::A, 2).to_vec().unwrap(),
            server.address,
        )
        .await
        .unwrap();
    sleep(Duration::from_millis(10)).await;
    server.stop().await;
}
