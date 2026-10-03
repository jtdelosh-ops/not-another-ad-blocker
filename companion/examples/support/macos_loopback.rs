//! CI-only bounded loopback trial; never packaged as a user command.
use super::live::{self, Holder};
use super::system_query::query_negative as system_query;
use hickory_proto::{
    op::{Message, MessageType, Query, ResponseCode},
    rr::{Name, RData, RecordType},
};
use naab_companion::dns::{
    blocklist::DnsPolicy,
    config::DnsConfig,
    diagnostics::Diagnostics,
    server,
    system::{health::local_probe, macos_preflight, macos_recovery::Configuration},
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::SocketAddr,
    process::{Command, Stdio},
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    runtime::Runtime,
};

fn require_opt_in() -> Result<(), String> {
    live::require_ci_environment()?;
    if std::env::var("NAAB_LIVE_LOOPBACK_TEST").as_deref() != Ok("disposable-mac-loopback") {
        return Err("Missing disposable loopback test acknowledgement".into());
    }
    Ok(())
}

fn runtime() -> Result<Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

unsafe extern "C" {
    fn getuid() -> u32;
    fn geteuid() -> u32;
    fn getgid() -> u32;
    fn getegid() -> u32;
    fn setuid(uid: u32) -> i32;
    fn setgid(gid: u32) -> i32;
    fn setgroups(count: i32, groups: *const u32) -> i32;
}

/// Child starts single-threaded, opens only the four fixed loopback sockets,
/// then irreversibly relinquishes root before creating a runtime or reading DNS.
pub(super) fn resolver_child(args: &[String]) -> Result<(), String> {
    require_opt_in()?;
    if args.len() != 3 || unsafe { getuid() != 0 || geteuid() != 0 } {
        return Err("Resolver bootstrap requires the CI root driver and three arguments".into());
    }
    let token = &args[0];
    if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid resolver token".into());
    }
    let upstreams: Vec<SocketAddr> = args[1]
        .split(',')
        .map(|s| s.parse().map_err(|_| "Invalid explicit upstream"))
        .collect::<Result<_, _>>()?;
    if upstreams.iter().any(|s| s.ip().is_loopback()) {
        return Err("Loopback upstreams are forbidden".into());
    }
    let config: DnsConfig = serde_json::from_value(serde_json::json!({
        "upstreams": upstreams, "cacheEntries": 0, "activityCapacity": 64,
        "timeoutMs": 1000
    }))
    .map_err(|e| e.to_string())?;
    config.validate()?;
    let uid: u32 = std::env::var("SUDO_UID")
        .map_err(|_| "Missing runner UID")?
        .parse()
        .map_err(|_| "Invalid runner UID")?;
    let gid: u32 = std::env::var("SUDO_GID")
        .map_err(|_| "Missing runner GID")?
        .parse()
        .map_err(|_| "Invalid runner GID")?;
    if uid == 0 || gid == 0 {
        return Err("Resolver must run under a non-root runner account".into());
    }
    let sockets = server::SystemPreviewSockets::bind().map_err(|e| e.to_string())?;
    // No threads or asynchronous runtime exist at this point. A failure exits
    // before any packets are processed, and drops all bound sockets.
    if unsafe { setgroups(0, std::ptr::null()) != 0 || setgid(gid) != 0 || setuid(uid) != 0 }
        || unsafe { getuid() != uid || geteuid() != uid || getgid() != gid || getegid() != gid }
    {
        return Err("Could not fully drop resolver privileges".into());
    }
    println!("RESOLVER_UNPRIVILEGED");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let policy = Arc::new(DnsPolicy::compile(&[], &[], &[])?);
    let diagnostics = Arc::new(Diagnostics::new(64));
    let observed = diagnostics.clone();
    let expected = args[2].trim_end_matches('.').to_owned();
    runtime()?
        .block_on(server::serve_system_preview_bound(
            config,
            policy,
            diagnostics,
            token.clone(),
            sockets,
            async move {
                let start = Instant::now();
                let mut reported = false;
                let mut observed_query = false;
                while start.elapsed() < Duration::from_secs(120) {
                    let snapshot = observed.snapshot();
                    if !observed_query {
                        if let Some(entry) = snapshot.recent.iter().find(|e| e.hostname == expected)
                        {
                            eprintln!(
                                "SYSTEM_QUERY_OBSERVED: type={}, outcome={}",
                                entry.query_type, entry.outcome
                            );
                            observed_query = true;
                        }
                    }
                    if !reported
                        && snapshot.recent.iter().any(|entry| {
                            entry.hostname == expected
                                && entry.query_type == "A"
                                && entry.outcome == "forwarded"
                        })
                    {
                        println!("FORWARDED_SYSTEM_QUERY");
                        let _ = std::io::stdout().flush();
                        reported = true;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
        ))
        .map_err(|e| e.to_string())
}

fn read_lines(child: &mut Holder) -> Result<mpsc::Receiver<String>, String> {
    let stdout = child.0.stdout.take().ok_or("Missing child output pipe")?;
    let (send, receive) = mpsc::sync_channel(64);
    std::thread::spawn(move || {
        // Child output is bounded even if an unexpected utility is noisy.
        for line in BufReader::new(stdout.take(64 * 1024)).lines() {
            match line {
                Ok(line) => {
                    if send.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    Ok(receive)
}

async fn healthy(token: &str) -> bool {
    let v4 = "127.0.0.1:53".parse().unwrap();
    let v6 = "[::1]:53".parse().unwrap();
    let (a, b, c, d) = tokio::join!(
        local_probe(v4, token, false),
        local_probe(v4, token, true),
        local_probe(v6, token, false),
        local_probe(v6, token, true)
    );
    a && b && c && d
}

fn check_live(child: &mut Holder, rt: &Runtime, token: &str) -> Result<(), String> {
    if child.0.try_wait().map_err(|e| e.to_string())?.is_some() || !rt.block_on(healthy(token)) {
        return Err("Loopback resolver exited or failed a local UDP/TCP health probe".into());
    }
    Ok(())
}

/// A separate wire client verifies real A answers, not just the local token.
async fn forwarded_a(destination: SocketAddr, tcp: bool) -> Result<(), String> {
    let mut query = Message::new();
    query
        .set_id(rand::random())
        .set_recursion_desired(true)
        .add_query(Query::query(
            Name::from_ascii("example.com.").unwrap(),
            RecordType::A,
        ));
    let bytes = query.to_vec().map_err(|e| e.to_string())?;
    let exchange = async {
        let reply = if tcp {
            let mut stream = tokio::net::TcpStream::connect(destination).await?;
            stream.write_u16(bytes.len() as u16).await?;
            stream.write_all(&bytes).await?;
            let length = stream.read_u16().await?;
            let mut reply = vec![0; usize::from(length)];
            stream.read_exact(&mut reply).await?;
            reply
        } else {
            let local = if destination.is_ipv4() {
                "0.0.0.0:0"
            } else {
                "[::]:0"
            };
            let socket = tokio::net::UdpSocket::bind(local).await?;
            socket.connect(destination).await?;
            socket.send(&bytes).await?;
            let mut reply = vec![0; 65_535];
            let length = socket.recv(&mut reply).await?;
            reply.truncate(length);
            reply
        };
        Ok::<_, std::io::Error>(reply)
    };
    let bytes = tokio::time::timeout(Duration::from_secs(6), exchange)
        .await
        .map_err(|_| "DNS forwarding probe timed out")?
        .map_err(|e| e.to_string())?;
    let reply = Message::from_vec(&bytes).map_err(|e| e.to_string())?;
    if reply.id() != query.id()
        || reply.queries() != query.queries()
        || reply.message_type() != MessageType::Response
        || reply.truncated()
        || reply.response_code() != ResponseCode::NoError
        || !reply
            .answers()
            .iter()
            .any(|answer| matches!(answer.data(), RData::A(_)))
    {
        return Err(format!(
            "DNS A probe failed: destination={destination}, tcp={tcp}, code={:?}, answers={}, truncated={}, idMatch={}, questionMatch={}, type={:?}",
            reply.response_code(), reply.answers().len(), reply.truncated(),
            reply.id() == query.id(), reply.queries() == query.queries(), reply.message_type()
        ));
    }
    Ok(())
}

fn case(force_resolver_failure: bool) -> Result<(), String> {
    let (mut record, captured_upstreams) = live::capture_with_upstreams()?;
    // The hosted VM's DNS gateway may not support TCP. Test explicitly chosen
    // upstreams without changing production forwarding or the recovery snapshot.
    let upstreams: Vec<SocketAddr> = std::env::var("NAAB_LOOPBACK_UPSTREAMS")
        .map_err(|_| "Missing explicit CI loopback upstreams")?
        .split(',')
        .map(|s| s.parse().map_err(|_| "Invalid explicit CI upstream"))
        .collect::<Result<_, _>>()?;
    let config: DnsConfig = serde_json::from_value(serde_json::json!({"upstreams": upstreams}))
        .map_err(|e| e.to_string())?;
    config.validate()?;
    if upstreams.iter().any(|s| s.ip().is_loopback()) {
        return Err("Loopback upstreams are forbidden".into());
    }
    record.applied.configuration = Configuration::Saved(br#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>ServerAddresses</key><array><string>127.0.0.1</string><string>::1</string></array></dict></plist>"#.to_vec());
    record.validate()?;
    // Observe the runner gateway separately from the required test upstreams.
    // Neither probe uses or changes system DNS; chosen upstreams must pass both.
    let baseline_runtime = runtime()?;
    for upstream in &captured_upstreams {
        for tcp in [false, true] {
            let result = baseline_runtime.block_on(forwarded_a(*upstream, tcp));
            println!("Captured upstream diagnostic: {upstream}, tcp={tcp}, result={result:?}");
        }
    }
    for upstream in &upstreams {
        for tcp in [false, true] {
            println!("Required explicit CI upstream A probe: {upstream}, tcp={tcp}");
            baseline_runtime.block_on(forwarded_a(*upstream, tcp))?;
        }
    }
    drop(baseline_runtime);
    let token = format!("{:032x}", rand::random::<u128>());
    let forwarded_name = format!("naab-ci-{:032x}.example.com.", rand::random::<u128>());
    let mut resolver = Holder(
        Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .args([
                "--resolver",
                &token,
                &upstreams
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                &forwarded_name,
            ])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?,
    );
    let messages = read_lines(&mut resolver)?;
    if messages.recv_timeout(Duration::from_secs(10)).as_deref() != Ok("RESOLVER_UNPRIVILEGED") {
        return Err("Resolver did not confirm dropped privileges before activation".into());
    }
    let rt = runtime()?;
    let ready = Instant::now();
    while !rt.block_on(healthy(&token)) {
        if ready.elapsed() > Duration::from_secs(10)
            || resolver.0.try_wait().map_err(|e| e.to_string())?.is_some()
        {
            return Err("Resolver was not ready; DNS was not changed".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for endpoint in ["127.0.0.1:53", "[::1]:53"] {
        for tcp in [false, true] {
            rt.block_on(forwarded_a(endpoint.parse().unwrap(), tcp))?;
        }
    }
    println!(
        "PASS: unprivileged resolver forwarded A queries over IPv4/IPv6 UDP/TCP before activation"
    );
    let mut store = live::open_store()?;
    let result: Result<(), String> = (|| {
        live::activate(&mut store, &record)?;
        let started = Instant::now();
        let guard = |resolver: &mut Holder| {
            if started.elapsed() >= Duration::from_secs(30) {
                return Err("Loopback trial exceeded its 30-second active limit".into());
            }
            check_live(resolver, &rt, &token)
        };
        // Committing preferences and notifying configd need not synchronously
        // update the effective resolver configuration. Wait for read-only OS
        // evidence before issuing the one-shot system query, never a fixed sleep.
        let readiness_started = Instant::now();
        let mut stable_since = None;
        let mut previous = None;
        loop {
            guard(&mut resolver)?;
            live::expect_state(&record, &record.applied)?;
            let observed = macos_preflight::preflight()?;
            let diagnostic = serde_json::to_string(&serde_json::json!({
                "primaryService": observed.primary_ipv4_service_id,
                "defaultDnsServers": observed.default_dns_servers,
                "resolvers": observed.resolvers
            }))
            .map_err(|e| e.to_string())?;
            if previous.as_ref() != Some(&diagnostic) {
                println!("Effective DNS after activation: {diagnostic}");
                previous = Some(diagnostic);
            }
            let ready = observed.primary_ipv4_service_id.as_deref()
                == Some(record.service_id.as_str())
                && observed.current_set_id.as_deref() == Some(record.set_id.as_str())
                && !observed.default_dns_servers.is_empty()
                && observed
                    .default_dns_servers
                    .iter()
                    .all(|s| s == "127.0.0.1" || s == "::1")
                && observed
                    .resolvers
                    .iter()
                    .all(|r| r.nameservers.iter().all(|s| s == "127.0.0.1" || s == "::1"));
            if ready {
                if stable_since.get_or_insert_with(Instant::now).elapsed()
                    >= Duration::from_millis(500)
                {
                    println!("PASS: effective macOS resolver configuration settled on NAAB loopback addresses");
                    break;
                }
            } else {
                stable_since = None;
            }
            if readiness_started.elapsed() >= Duration::from_secs(8) {
                return Err(
                    "Effective macOS DNS did not settle on the applied loopback setting".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        // .invalid token probes are direct wire checks only: system APIs may
        // synthesize a negative answer without querying DNS (RFC 6761 6.4).
        // Both live cases require a fresh ordinary-name query AND evidence that
        // this resolver actually forwarded it, not merely an OS negative reply.
        system_query(&forwarded_name, || guard(&mut resolver))?;
        if messages.recv_timeout(Duration::from_secs(3)).as_deref() != Ok("FORWARDED_SYSTEM_QUERY")
        {
            return Err("System query was not observed forwarding through NAAB".into());
        }
        live::expect_state(&record, &record.applied)?;
        println!("PASS: fresh macOS system query was forwarded through NAAB to the explicit CI upstreams");
        if force_resolver_failure {
            resolver.0.kill().map_err(|e| e.to_string())?;
            let status = resolver.0.wait().map_err(|e| e.to_string())?;
            use std::os::unix::process::ExitStatusExt;
            if status.signal() != Some(9) {
                return Err("Resolver did not exit by SIGKILL".into());
            }
            let failed_at = Instant::now();
            let mut failures = 0;
            while failures < 3 && failed_at.elapsed() < Duration::from_secs(8) {
                if rt.block_on(healthy(&token)) {
                    failures = 0;
                } else {
                    failures += 1;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if failures != 3 {
                return Err("Watchdog did not confirm resolver failure".into());
            }
            println!("Watchdog: three consecutive local probe failures; restoring DNS");
        } else {
            while started.elapsed() < Duration::from_secs(15) {
                guard(&mut resolver)?;
                live::expect_state(&record, &record.applied)?;
                std::thread::sleep(Duration::from_millis(300));
            }
            println!("Guarded deadline reached; restoring DNS before stopping resolver");
        }
        Ok(())
    })();
    drop(store);
    // Keep a healthy resolver alive until restoration and read-back finish,
    // including on a failed assertion. launchd remains the independent fallback.
    let recovery = live::run_helper().and_then(|_| live::verify_restored(&record));
    if let Err(error) = recovery {
        return Err(format!(
            "Loopback test recovery failed: {error}; trial result: {result:?}"
        ));
    }
    result?;
    println!(
        "PASS: {} restored exact DNS state; journal cleared",
        if force_resolver_failure {
            "resolver-failure watchdog"
        } else {
            "guarded loopback deadline"
        }
    );
    Ok(())
}

pub(super) fn test_cases() -> Result<(), String> {
    require_opt_in()?;
    live::require_recovery_task()?;
    println!("Loopback case 1: system DNS passthrough with a guarded deadline");
    case(false)?;
    println!("Loopback case 2: stop resolver and recover after local health failure");
    case(true)
}
