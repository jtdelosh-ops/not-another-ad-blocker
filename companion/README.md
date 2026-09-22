# Rust companion

`naab-companion` is an on-demand Native Messaging host. Protocol version 1 supports `status.get`, the original bounded `rules.compile`, and the new `lists.refresh`/`lists.page` operations. See the [protocol overview](../shared/protocol/README.md) and [subscription contract](../shared/protocol/subscriptions.md).

The native host compiles filters, downloads only the fixed EasyList/EasyPrivacy sources when asked, and keeps subscription snapshots in a dedicated user-local cache. It has no network listener or background service. The extension enforces compiled rules and keeps working from committed browser state if the companion is unavailable.

The separate, opt-in `naab-dns-dev` executable is the first Phase 2 source milestone. It provides a loopback DNS development listener, forwarding, bounded caching, domain rules and local diagnostics. It is not launched by Native Messaging and does not change system DNS settings. See the [DNS core guide](../docs/dns-core.md) for commands, compatibility limits and privacy behavior.

Build and verify from this directory with stable Rust:

```sh
cargo test --locked
cargo build --release --locked
```

The executable is `target/release/naab-companion` (`.exe` on Windows). It reads binary Native Messaging frames from stdin, writes only framed JSON to stdout, and reports fatal framing/I/O errors to stderr without filter contents. Running it in a terminal without framed input waits for input; closing stdin exits normally. Keep an updated executable at the registered path to avoid registering it again.

## Compilers

The local compiler in `rules.rs` preserves its original small-import behavior:

- Domain anchors ending with `^` or followed by a literal path.
- `@@` network exceptions and the `$third-party` modifier.
- Simple ASCII tag/class/ID compound cosmetics, optionally scoped to positive domains.
- At most 128 KiB text, 2,000 nonempty lines, 2,048 bytes per line, 512 bytes per cosmetic selector, and 200 normalized cosmetic domains.

Unsupported local syntax produces per-line diagnostics. Imports exceeding an input bound fail as a whole. Local hostnames normalize to lowercase; already-punycoded names are accepted. BOM/CRLF, comments, and standard headers are supported. Blank/comment/header lines count as ignored.

The subscription compiler in `subscription_rules.rs` handles larger selected lists, Adblock-style ASCII URL anchors/wildcards, common positive/negative resource restrictions, first/third-party conditions, positive/negative domain scope, case matching, network and document exceptions, supported `badfilter` cancellation, and scoped cosmetic exceptions. Complex CSS, procedural cosmetics, scriptlets, redirects, regex network rules, and unknown modifiers are outside the supported subset.

Subscription cosmetic selectors use the same restricted compounds. `#@#` and representable hiding exceptions become exclusions; unsupported exception semantics trigger conservative safety handling and reported omissions. For example, an unscopable generic-hiding exception suppresses generic subscription cosmetics. Network exceptions with a usable pattern may produce broader allow guards; unrepresentable exception patterns may suppress network coverage. These measures do not remove the user's independent local rules. This is partial compatibility, not full EasyList/Adblock/uBlock compatibility.

Equivalent full-host network filters can be compacted into `requestDomains` groups of at most 1,000 hosts, preserving their shared action and context. Path/wildcard/context differences remain separate. Supported allow rules are retained before budgeted blocking rules. Subscription output is bounded at 29,800 network entries, 10,000 cosmetic entries, and 200 diagnostic samples, with full totals and grouped reasons retained. Accepted source-line totals can differ from output counts because of compaction, deduplication, exceptions, and omissions.

## Download and cache

`lists.refresh` accepts a unique, nonempty subset of `easylist` and `easyprivacy`, plus a network budget from 0 to 29,800. Each ID maps to its fixed `easylist.to` HTTPS URL. Requests cannot supply a URL or path. Reqwest with Rustls checks TLS, rejects redirects, and uses a 10-second connection limit and 25-second total download timeout per source. Two selected sources are fetched sequentially. Each source is limited to 8 MiB and 300,000 lines; UTF-8, an Adblock header, and meaningful content are required.

All selected downloads and compilation must succeed before a new snapshot is committed. Raw texts, title/version/URL/fetch-time/hash/size metadata, a manifest, and compiled pages are staged and synchronized locally, then renamed into an immutable directory. The directory ID is the SHA-256 digest of its index, which includes hashes of metadata and compiled pages. Page readers verify hashes, offsets, counts, and sizes. Native processes can retrieve the same snapshot independently.

Pages contain at most 128 items and fit below 512 KiB with response overhead. Individual native responses still have the original 1 MiB hard limit. A snapshot is capped at 64 MiB; normal retention keeps three completed snapshots. Failed staged writes do not replace old snapshots. A stale/missing/corrupt snapshot causes a structured error, leaving browser rules authoritative.

Default cache roots:

- Windows: `%LOCALAPPDATA%\NotAnotherAdBlocker`.
- macOS: `~/Library/Application Support/NotAnotherAdBlocker`.
- Other platforms: `$XDG_DATA_HOME/not-another-ad-blocker`, falling back to `~/.local/share/not-another-ad-blocker`.

`NAAB_DATA_DIR` overrides this root for tests or portable installations. It is an environment setting, not a protocol parameter. Removing browser subscriptions or unregistering the host does not purge this cache. Delete the dedicated directory manually when a full removal is desired.

## Browser application and precedence

The companion returns local block/allow priorities 1/2. The extension promotes local rules to 3/4 when combining them with subscription priorities 1/2; site overrides use priority 100. Neither compiler emits a main-frame block. Subscription document exceptions can use `allowAllRequests` for main and subframes.

Local compilation uses deterministic FNV-1a IDs with collision probing. Subscription output is deterministic; the extension reassigns globally unique IDs when combining both sources, below the range reserved for site overrides. The browser then atomically replaces its previous DNR set and commits its configuration using a recovery journal. Downloaded cache state is not proof of successful browser application.

## Protocol bounds and tests

Incoming frames remain capped at 256 KiB; IDs and local source labels at 128 UTF-8 bytes; outgoing frames at 1 MiB. Oversized/truncated framing terminates the host stream. Validly framed malformed messages receive structured errors and allow the stream to continue. Old local compilation results exceeding the response limit become `OUTPUT_TOO_LARGE` errors.

Tests cover framing, strict/duplicate JSON fields, local and subscription grammar, exception handling, compaction, quotas, normalization, deterministic IDs, real executable messaging, injected offline source downloads, failed refresh preservation, page boundaries/byte bounds, corruption, invalid paths, stale snapshots, and retention. Tests do not require live list downloads or the real user cache.

EasyList/EasyPrivacy source lists are downloaded on request and remain attributable to [The EasyList authors](https://easylist.to/pages/about.html), whose page provides their copyright/license information.

Implementation references: [Chrome Native Messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) and [declarativeNetRequest](https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest).
