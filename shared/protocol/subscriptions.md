# Subscription protocol addition (version 1, companion 0.2.0)

Existing status.get/rules.compile remain backward compatible. status.get still reports mode local-import, and adds capabilities lists.refresh and lists.page. The browser requires those capabilities before showing subscription support as available.

## lists.refresh

Request payload: `{ "ids": ["easylist", "easyprivacy"], "networkBudget": 27800 }`. IDs are a nonempty unique subset of the two fixed sources; budget is an integer 0..29800. No caller-supplied URL/path. Fetch sources directly over HTTPS with bounded timeout and size. All selected sources must download and validate before returning a new immutable snapshot. Existing installed browser rules remain authoritative until a complete snapshot is validated and applied. No automatic download on startup.

Sources: https://easylist.to/easylist/easylist.txt and https://easylist.to/easylist/easyprivacy.txt . Metadata preserves title, URL, fetch time, content hash, size, and source list version when available. No cookies, browser URLs, or browsing history are sent. Cache root uses the platform's user-local data directory (NAAB_DATA_DIR override for tests). Raw lists and paged snapshots stay local; snapshots survive independent sendNativeMessage host processes.

Response payload:

```json
{
  "snapshotId": "64-lowercase-hex-digest",
  "fetchedAt": "ISO-8601 UTC timestamp",
  "lists": [{"id":"easylist","title":"EasyList","url":"https://easylist.to/easylist/easylist.txt","fetchedAt":"ISO timestamp","sha256":"64 hex","bytes":123,"version":"source version or empty string"}],
  "counts": {"network":100,"cosmetic":20,"diagnostics":30},
  "stats": {"network":100,"cosmetic":20,"unsupported":30,"ignored":10},
  "coverage": {"networkSupported":120,"networkDropped":20,"cosmeticSupported":20,"cosmeticDropped":0,"exceptionSafetySuppressed":0,"diagnosticsTotal":30,"diagnosticsTruncated":false},
  "listStats": [{"id":"easylist","network":100,"cosmetic":20,"unsupported":30,"ignored":10}],
  "unsupportedReasons": [{"reason":"Unsupported modifier","count":30}]
}
```

## lists.page

Request: `{ "snapshotId":"64 hex", "kind":"network", "offset":0 }`. Kind is network, cosmetic, or diagnostics. An empty-set response is `{ "snapshotId":"same", "kind":"network", "offset":0, "total":0, "items":[], "nextOffset":null }`. Nonempty pages contain compiled objects in items; nextOffset equals offset plus the item count, or null at the end. Use pages of at most 128 items AND at most 512 KiB serialized; offsets must equal a stored page boundary. Response always remains below Native Messaging's 1 MiB limit. Client verifies identity, order, progress, total, per-item schema, and final totals before committing. Empty arrays return an empty final page if requested.

## Shared compilation shape

Root compiler module exports `compile_subscriptions(sources: &[(&str, &str)], network_budget: usize) -> Result<serde_json::Value, String>` (source id + text) returning networkRules, cosmeticRules, diagnostics, stats, coverage, listStats, unsupportedReasons. List transport consumes that result and adds metadata/pagination. Diagnostic samples are capped at 200, per-rule raw text at 2048 UTF-8 bytes, message at 2048 bytes, source id optional. Full totals and grouped reasons remain available. Input per list at most 8 MiB, 300000 lines. Compiled output at most 29800 network rules and 10000 cosmetic rules. All supported exceptions are retained before selecting deterministic blocking rules under the budget; fail if exceptions alone exceed budget.

Subscription network conditions support EITHER urlFilter (ASCII ABP/DNR wildcard anchors, no regex) OR requestDomains (up to1000 valid hostnames), explicit resourceTypes, optional domainType firstParty/thirdParty, initiatorDomains/excludedInitiatorDomains (at most 200 each), and isUrlFilterCaseSensitive boolean. Simple ||hostname^ rules with identical remaining conditions/actions are packed into requestDomains groups to fit browser quotas without discarding domains; counts.network and coverage.networkSupported refer to packed DNR rules, while listStats.network counts supported source lines. Action is block (priority1), allow (priority2), or allowAllRequests (priority2, main_frame and/or sub_frame; document exception). No main-frame block. Unknown semantics are rejected with counts and samples. Cosmetic rules use the existing restricted selectors, domains, raw/source, with optional excludedDomains (at most 200). Unsupported exceptions require explicit conservative suppression of related subscription blocks/hides, not just dropping the exception.

## Browser state

Coverage invariants: networkSupported equals emitted network rules plus networkDropped after packing and safety handling. cosmeticSupported counts accepted source rows before deduplication and exception safety handling, so it is at least emitted cosmetic rules plus cosmeticDropped. Safety omissions are reported separately; source-row counts need not equal installed-rule counts.

Keep existing local import and settings intact (including old v1 config). Add optional subscription state `{manifest: refresh-response, compiled: compilation-arrays/stats, appliedAt: ISO timestamp}`. Local import still replaces only the local list. Download selected subscriptions replaces only subscriptions. Removing subscriptions leaves local rules/settings. Reassign combined network IDs deterministically without collisions; local block/allow priorities3/4 supersede subscription priorities1/2; site override priority100 remains. Reserve 200 site rules +2000 local network rules from browser quota, so refresh budget is min(29800,maxDynamicRules-2200). Explicitly show omitted/unsupported/safety-suppressed counts and selected source metadata. Preserve previous active config on any download, paging, validation, DNR, or persistence error, using existing rollback/recovery behavior.

Snapshot storage in the browser requires unlimitedStorage for journal + committed large configurations. UI renders only bounded diagnostic samples and does not send full source text or full network arrays to page content scripts. Per-page cosmetic selector response cap is 12000 (10000 subscriptions +2000 local) and remains strictly validated.
