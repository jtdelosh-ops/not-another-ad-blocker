# DNS core development preview

NAAB's DNS core is an optional local resolver that blocks matching domain lookups before forwarding allowed queries to a chosen upstream. It extends the Rust companion project with a separate `naab-dns-dev` executable. This is the first Phase 2 milestone, available from source; it is not system-wide protection yet.

The listener runs on an unprivileged loopback port, with UDP and TCP support. It changes no network settings and installs no service. The extension and its Native Messaging host continue working independently. Requests reach this resolver only when a DNS client explicitly targets its port.

## Try it

From the repository root, with Rust installed:

```sh
cargo run --locked --manifest-path companion/Cargo.toml --bin naab-dns-dev -- --config companion/examples/dns-dev.json --check
cargo run --locked --manifest-path companion/Cargo.toml --bin naab-dns-dev -- --config companion/examples/dns-dev.json --check --pretty
cargo run --locked --manifest-path companion/Cargo.toml --bin naab-dns-dev -- --config companion/examples/dns-dev.json
```

To save the JSON report for later comparison:

```powershell
cargo run --locked --manifest-path companion/Cargo.toml --bin naab-dns-dev -- --config companion/examples/dns-dev.json --check | Set-Content -Encoding UTF8 dns-coverage.json
```

The first command validates configuration and prints a JSON DNS coverage report without opening a socket. Add `--pretty` for a readable summary with diagnostics; JSON remains the default for scripts. The report distinguishes candidate block lines from deduplicated candidate rules and effective rules after safety suppression. The runtime command starts the foreground resolver. The example listens on `127.0.0.1:5354` and explicitly chooses Cloudflare's `1.1.1.1:53` / `1.0.0.1:53` upstreams. Edit `upstreams` to use your preferred resolver before running it. There is no automatic upstream discovery in this milestone.

The automated Windows `nslookup` invocation did not reach the development resolver in our test environment. Use NAAB's dependency-free PowerShell probe instead:

```powershell
PowerShell -ExecutionPolicy Bypass -File scripts\test-dns.ps1 -Name ads.example.test
PowerShell -ExecutionPolicy Bypass -File scripts\test-dns.ps1 -Name example.com
```

The sample rule blocks `ads.example.test` with **NXDOMAIN**. `example.com` should resolve through the configured upstream, assuming it is reachable. On macOS, `dig @127.0.0.1 -p 5354 example.com` is another option. With an unreachable upstream, allowed queries return **SERVFAIL**; that is different from a filter block.

Type these commands into the resolver terminal:

| Command | Result |
| --- | --- |
| `status` | Query totals, outcomes and retained activity count; no query names |
| `activity` | JSON containing the bounded recent DNS sample, outcome and matching rule/source |
| `clear` | Erase recent query details while retaining aggregate counters |
| `quit` or Ctrl+C | Stop listeners and outstanding work |

Closing stdin also stops the process. Stop and restart after changing configuration or filter files; this recompiles rules and clears the cache and activity sample. Bind conflicts fail startup. No OS DNS restoration is needed because this executable does not change OS settings.

## Configuration and rules

Copy [the example configuration](../companion/examples/dns-dev.json) for your own settings. Unknown or repeated configuration fields are errors. Filter file paths are relative to the JSON file, not the terminal's working directory. The resolver reads up to two explicit UTF-8 filter files; it does not download lists. Existing cached EasyList/EasyPrivacy text may be referenced, with the compatibility limits below.

Rules reuse NAAB's existing normalized parser. Supported DNS rules are unconditional `||domain.example^` blocks and `@@||domain.example^` exceptions. They match the exact domain and its subdomains, with label boundaries. User `allowlist` and `blocklist` entries are plain domains; case, a trailing root dot and IDNA names are normalized. Precedence is:

1. User allowlist.
2. User blocklist.
3. Filter-list exceptions and conservative exception guards.
4. Filter-list blocks.
5. Allow by default.

Paths, resource types, third-party conditions, page scopes, scripts and cosmetics cannot become DNS blocks. An unsupported network exception with a usable hostname gets a conservative domain allow guard. An unscopable or page-wide exception, or an unsupported `!#` preprocessing directive, suppresses list-derived blocking; user overrides remain active. `badfilter` handling may conservatively allow a domain instead of retaining a potentially cancelled block. Pure cosmetic exceptions are irrelevant to DNS. The compilation report exposes supported counts, omissions, safety suppression and up to 200 diagnostic samples. `effectiveListBlockRules` becomes zero when all list blocks are suppressed; a separate `suppressionReasons` sample remains available even if ordinary diagnostics fill up.

**The cached EasyList/EasyPrivacy pair tested at this milestone triggers that safety suppression and contributes no active DNS list blocks.** Use explicit user domains or a DNS-compatible rule file for development testing. This first milestone does not claim useful full-list DNS coverage. Improving safe full-list compilation is follow-up work; dropping exceptions to inflate blocking counts would be incorrect.

Filtering applies to the **queried name**. The core does not inspect CNAME targets for extra blocking, URLs, page content or application identity. DNS cannot reliably separate an ad from other content served on the same hostname.

## Transport, cache and bounds

Hickory handles DNS message parsing and serialization; Tokio handles asynchronous sockets. Allowed requests use explicitly configured upstream IP/port addresses with fresh UDP sockets and random transaction IDs. Replies must match the transaction and question. Truncated UDP upstream replies fall back to TCP. Client UDP replies respect the advertised size, capped at 1,232 bytes; oversized replies request a TCP retry. DNS is sent to the upstream without encryption in this milestone.

Only standard single-question Internet-class queries are supported. Updates, transfers, ANY queries, signed requests and advanced EDNS options are rejected; malformed packets cannot enter the cache. DNSSEC-related requests bypass the local cache, and locally generated blocks never claim DNSSEC authenticity. This is not a validating resolver.

The response cache is bounded and held in memory. TTLs age while cached; negative caching uses SOA TTL/MINIMUM. Failures, truncated replies and zero-TTL data are not cached. Restart invalidates everything; no stale answers are served when upstreams fail.

| Resource | Default / limit |
| --- | --- |
| Listener | `127.0.0.1:5354`; loopback only, ports 1024–65535 |
| Upstreams | 1–4 explicit unicast IP/port addresses; direct local loops rejected |
| Upstream attempt timeout | 2 seconds; configurable 100–10,000 ms |
| Active queries/TCP clients | 64 combined; configurable 1–256 |
| TCP idle reads/writes | Bounded by the configured timeout; at most 64 queries per connection |
| Cache | 1,024 entries; configurable 0–4,096; responses above 16 KiB not cached; retention capped at one hour |
| Activity | Last 300 observations; configurable 0–1,000; aggregate counters remain |
| Filter input | Two files, each at most 8 MiB / 300,000 lines |
| User overrides | At most 10,000 allow and 10,000 block domains |
| Configuration | At most 1 MiB |

When concurrency is exhausted, additional UDP packets are dropped and TCP connections are closed. Activity is a diagnostic sample rather than an exhaustive record. The CLI writes query names only in response to `activity`; NAAB does not save them to disk or upload them. Redirecting that output to a file is a separate user choice. Allowed DNS questions are sent to the selected upstream, which can observe them.

## Verification and remaining work

Run the offline tests with:

```sh
cargo test --locked --manifest-path companion/Cargo.toml
```

The DNS tests use loopback upstream fixtures, exercise real UDP/TCP sockets, and require no public DNS access or system configuration changes. The final local suite passed 88 Rust tests, including 32 DNS tests, plus 8 existing installer/native-client tests. Native-host process tests guard the existing extension protocol. An independent raw UDP probe also received the expected NXDOMAIN from the actual executable.

Verification limits: Windows `nslookup` failed to connect from the automation environment over either UDP or TCP, while the resolver recorded no requests from that client; the direct probe and socket tests succeeded. That client interoperability check remains unresolved and should be repeated in a normal terminal before treating this as ready for system integration. macOS execution of the DNS milestone is also unverified. These are development-core results, not a Phase 2 exit approval.

Still ahead in Phase 2: connecting DNS controls/activity to the extension, managed process/service lifecycle, discovery of current resolvers, macOS and Windows system integration, narrowly scoped privileged operations, backup/restore, watchdog health/recovery, and platform testing. The existing Mac browser-extension package does not include this new source-only DNS preview.

Implementation references: [Hickory protocol library](https://docs.rs/hickory-proto/0.25.2/hickory_proto/), [DNS negative caching](https://www.rfc-editor.org/rfc/rfc2308), and [DNS over TCP](https://www.rfc-editor.org/rfc/rfc7766).
