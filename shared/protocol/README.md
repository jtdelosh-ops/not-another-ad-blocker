# Native protocol v1 — companion 0.2.0

Host name: `com.naab.companion`. UTF-8 JSON uses a native-endian uint32 byte-length prefix, with no other stdout output. The host can process multiple frames before EOF. The extension uses `sendNativeMessage`, so each request can start an independent process; persistent subscription snapshots support paging across those processes.

Request: `{ "version": 1, "id": "nonempty-string-up-to-128-bytes", "type": "status.get", "payload": {} }`

Success: `{ "version": 1, "id": "same-id", "ok": true, "payload": {} }`

Failure: `{ "version": 1, "id": "same-id-or-null-if-invalid", "ok": false, "error": { "code": "INVALID_REQUEST", "message": "Human readable explanation" } }`

## Messages

- `status.get`, payload `{}` returns `{ "companionVersion": "0.2.0", "protocolVersion": 1, "healthy": true, "capabilities": ["rules.compile", "lists.refresh", "lists.page"], "mode": "local-import" }`. The historical mode value stays unchanged for compatibility; clients detect subscription support through capabilities.
- `rules.compile`, payload `{ "text": "filter-list-text", "source": "Local import" }` returns the original local compilation shape below. Source is optional and limited to 128 UTF-8 bytes.
- `lists.refresh`, payload `{ "ids": ["easylist", "easyprivacy"], "networkBudget": 27800 }` downloads the selected fixed sources, validates/compiles them, commits an immutable native snapshot, and returns its manifest. IDs must be unique and nonempty; budget is an integer from 0 to 29,800. This does not itself install browser rules.
- `lists.page`, payload `{ "snapshotId": "64-lowercase-hex-digest", "kind": "network", "offset": 0 }` returns a saved compiled page. Kind is `network`, `cosmetic`, or `diagnostics`; offset must be a stored boundary. Pages contain at most 128 items and stay below 512 KiB including response overhead. An empty collection has an empty final page at offset zero.

The complete subscription manifest, page schema, coverage fields, compiler conditions, and browser state contract are in [subscriptions.md](subscriptions.md).

Unknown versions/types receive structured errors. Unknown or duplicate payload fields are rejected. No arbitrary filesystem paths, executable code, or URLs are accepted. A snapshot ID is validated before a cache path is constructed. Download, list-content, compilation, missing/stale snapshot, and integrity failures have explicit error codes. Clients must retain their previous committed configuration after any failed refresh or incomplete page traversal.

## Framing and local-import limits

All incoming JSON frames are capped at 256 KiB; all outgoing frames at 1 MiB. Local `rules.compile` text is limited to 128 KiB UTF-8, 2,000 nonempty lines, and 2,048 bytes per line. Oversized/truncated frames close the host with an error on stderr. A local compilation response exceeding the outgoing limit becomes a structured `OUTPUT_TOO_LARGE` error.

Downloaded subscriptions do not travel in incoming protocol frames. Each selected source is capped at 8 MiB/300,000 lines, and larger compiled results are transferred through pages. Diagnostic samples are bounded at 200 with complete totals/grouped reasons. Cache snapshots are immutable, integrity checked, capped at 64 MiB each, and retained as the three latest completed snapshots under normal operation. Fetching is manual and bounded to 25 seconds per source.

## Local compilation shape

```json
{
  "networkRules": [{"id":1,"priority":1,"action":{"type":"block"},"condition":{"urlFilter":"||ads.example.test^","resourceTypes":["sub_frame","stylesheet","script","image","font","object","xmlhttprequest","ping","media","websocket","other"]}}],
  "cosmeticRules": [{"domains":["example.test"],"selector":".advertisement","raw":"example.test##.advertisement","source":"Local import"}],
  "diagnostics": [{"line":1,"raw":"||ads.example.test^","target":"MV3_NETWORK","message":"Compiled successfully"}],
  "stats": {"network":1,"cosmetic":1,"unsupported":0,"ignored":0}
}
```

Local network IDs are deterministic, positive, unique, and below 1,000,000,000. The compiler uses exception priority 2 and block priority 1, explicit subresource types, domain-anchored `urlFilter`, and optional `domainType: "thirdParty"`. Main-frame blocking is excluded. Supported patterns require a full hostname boundary or literal path. Unsupported local modifiers are diagnosed without emitting that line.

Local cosmetics accept only ASCII compound tag/class/ID selectors, optionally with positive domains including subdomains. Empty domains means generic. Selectors are at most 512 bytes and domain scopes at most 200 normalized hostnames. Identifiers may begin with a letter, underscore, or a hyphen followed by a letter, underscore, or hyphen. Attributes, combinators, pseudo-selectors, CSS injection, scriptlets, and local cosmetic exceptions remain unsupported. No input executes as JavaScript.

The local `ignored` count includes blank lines and can exceed the 2,000 nonempty-line limit, bounded by the text byte limit plus one. Rejected selectors or excessive scope counts produce `UNSUPPORTED` diagnostics without invalidating unrelated supported lines. Input limit violations reject the entire import.

## Subscription differences and application

Subscription network conditions additionally support common ASCII wildcard/anchor patterns, positive/negative resource types, first/third-party matching, initiator-domain inclusions/exclusions, case matching, and `requestDomains` compaction of equivalent full-host filters (at most 1,000 domains per browser rule). Document exception guards may emit `allowAllRequests` for main and subframes; no subscription rule blocks main-frame navigation. Cosmetic rules may include `excludedDomains`, and supported `#@#` exceptions are resolved before application.

Unsupported exception semantics cause explicit conservative allowance/suppression, rather than simply retaining their related blocks. Supported exceptions survive budget selection before blocking rules. Coverage counters distinguish capacity omissions and safety omissions; source-line totals can differ from emitted rule totals after compaction and exception handling. Unscopable generic-hiding exceptions can suppress generic subscription cosmetics while local generic rules remain independent.

The extension validates the full manifest and every page before atomic DNR replacement. It combines subscriptions with the existing local list, promotes local network priorities to 3/4 above subscription priorities 1/2, and assigns globally unique IDs. Site overrides use priority 100. The browser reserves capacity for local rules and site overrides and persists committed configuration separately from the companion cache. Settings/activity messages are internal browser routes, not native protocol operations.
