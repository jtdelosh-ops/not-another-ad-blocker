# Phase 1 exit review

Reviewed 2026-09-22 against roadmap sections 8 and 9. Current implementation: extension **0.4.1**, companion **0.2.1**. This review adds evidence and documentation; it does not publish a release or implement DNS or popup blocking.

## Decision

**Phase 1 is complete for the developer preview.** No required filtering, recovery, privacy, or measured-performance blocker was found in the inspected scope. Fresh deterministic tests and browser regressions pass, and the user reports successful Windows/Mac use.

The initial performance pass found a full-list extension worker restart/reconciliation cost of approximately **1.2–1.9 seconds**, plus substantial aggregate browser memory overhead in the controlled fixture. Extension 0.4.1 stores an SHA-256 stamp after a successful DNR commit, skips replacement on an ordinary worker wake, and avoids deep clones when assembling rules. The matched post-change wake median is **567 ms**, down from **1,689 ms** before the change, a 66% reduction. Post-GC aggregate private browser memory fell from about **515 MiB** to **460 MiB** in the one-batch comparison. A memory leak has **not** been established.

The user completed exploratory browsing and reported no visible ads apart from click-triggered popup ad pages. Popup blocking is an accepted, deferred limitation; another broad manual testing round is not required to act on this review.

## Completion criteria and evidence

| Roadmap requirement | Assessment and evidence |
| --- | --- |
| Chromium extension and Rust companion install/start | Met for the tested developer-preview scope: user-reported Windows installation and Intel Mac Chrome installation; Mac is Ventura 13.3.1. Hands-on Mac picker testing was user-reported successful for extension 0.4.0; the 0.4.1 source update has not been repackaged for Mac. Other OS/browser combinations are not certified. |
| Extension ↔ companion Native Messaging | Actual TypeScript client and framed Rust-process integration pass. Previous installed-browser checks and user operation supplement these tests. Automated browser fixtures bridge native calls; they do not independently prove OS host discovery. |
| EasyList and EasyPrivacy load | Earlier live HTTPS refresh validated both fixed sources, cache and paged delivery. This review reuses that recorded snapshot; it does not claim a fresh download. |
| Supported network rules compile and block | Current Chromium subscription regression accepts all 12,822 cached network rules and checks representative blocking, local overrides and allowances. |
| Supported cosmetic rules hide elements | Subscription and picker regressions verify actual hiding. Top-document scope and supported syntax limits remain. |
| Per-site protection control | Current browser regressions verify site/global pause and resume; settings persist. |
| Picker creates and persists a cosmetic rule | Current browser test covers selection, preview/cancel, save, scoped page reload, immediate undo, and preserving later local edits. User reports successful Mac testing. |
| Activity explains common blocked requests | Current activity regression verifies real DNR matches, rule conditions, block counts, allowances, clear, redaction and worker recovery. Detailed matches are an unpacked-preview capability. |
| Unsupported rules rejected and reported | Compiler/client tests pass for the supported contract and diagnostic bounds. Unsupported local lines are omitted while supported lines can still apply; unsupported local exceptions are not converted into subscription-style safety guards. See limits below. |
| No browsing upload to a NAAB server | Inspected local storage, native protocol, fixed-source downloads and session-only activity paths. No telemetry/account/upload feature found; this is scoped inspection, not a comprehensive security audit. |
| Parser/protocol unit tests and native integration | 71 extension tests, 56 Rust tests (53 library + 3 process), and 8 root installer/native tests pass: **135 total**. TypeScript checking and extension build pass. |
| README/setup instructions and known limits | Project overview links to setup, privacy, architecture and preview guides. Current Mac status, popup limitation, and the performance findings are recorded. |

## Reliability and filtering review

The current activity browser regression stops and restarts the actual extension worker and verifies session-log and count recovery, then restarts the browser and verifies saved rules remain while the session log clears. The subscription regression verifies failed-refresh retention, local-rule preservation, site exceptions, and saved filtering after an offline browser restart. Unit tests cover pending-update recovery, browser/storage failure rollback, invalid messages, limits and explicit recovery errors. The picker regression verifies authorization boundaries and failed-save retention.

Native Messaging is request-based: repeated independent process exchanges succeed, malformed JSON can be followed by a valid request, and truncated/oversized frames fail safely. This is not a guarantee against every possible process interruption, machine crash, or malicious input.

The user's exploratory browsing is useful compatibility evidence, but no site inventory, duration, or controlled tracker coverage study was supplied. No additional breakage was reported. Synthetic browser tests supply reproducible blocking/allowance evidence; ad-test percentages and absence of visible ads do not establish universal coverage.

## Performance evidence

Windows 10 build 19045, Xeon E3-1245 v6 at 3.70 GHz, 8 logical CPUs, approximately 64 GiB RAM; Chromium for Testing 151.0.7922.34, Node 24.19.0 and a release companion build. These are approximate local baselines, not cross-platform performance guarantees. No Mac timing or memory measurements were collected.

| Requested measurement | Observation | Interpretation |
| --- | --- | --- |
| Extension startup | Initial empty-profile browser launch to worker appearance: about 323–361 ms. Before 0.4.1, full-list worker wake + configuration reconciliation had a matched median of 1,689 ms. After 0.4.1, full-list committed-state wake had a median of 567 ms, range 538–592 ms. | 66% lower in this controlled run. Empty worker appearance is not loaded-extension readiness. Stored network rules remain browser-enforced while the worker sleeps, while UI/cosmetic responses await initialization. |
| Companion startup | Nine fresh-process status round trips: median 20.79 ms, range 20.20–23.41 ms. | Includes process launch, native framing, validation and exit; not the additional Chrome native-host discovery path. |
| List parsing | Included in the combined offline compiler run below. | Parser/compiler stages are integrated; no independent parse-only duration was measured. |
| Rule compilation | Seven offline release-example runs: combined median 2.341 s, range 2.314–2.571 s. | Includes launch, file reads, parsing, compilation, serialization and output write. Separate compile-only attribution remains unavailable. |
| Companion memory | Offline shared compiler sampled peak private memory about 348 MiB; working set about 340 MiB. | Sampled about every 5 ms; lower bounds on peaks. Includes the same compiler used by the companion, but not the complete download/cache/paging workflow. The on-demand process exits when finished. |
| Browser memory | In the matched 40-navigation diagnostic, post-GC aggregate private memory was about 295–314 MiB without NAAB and 515–547 MiB with the cached lists and activity page; paired differences about 217–234 MiB. The optimized one-batch diagnostic measured 460 MiB after forced collection, compared with 515 MiB in the pre-change one-batch run. | Includes all CDP-reported browser processes and extension UI; not an isolated extension heap. Forced garbage collection is a diagnostic intervention, not normal behavior. The optimized sample suggests lower retained allocation, but it does not prove a general memory reduction. |
| Page-load impact | Matched diagnostic batch medians: no extension 54.9–63.1 ms; protection on 61.4/69.6 ms; protection off 61.7/64.0 ms. Original run on/off distributions also overlap. | Local neutral page: 2,000 elements + 30 scripts, one warm-up and nine samples per batch. No universal percentage overhead or real-site speed claim follows from this small fixture. |

The original memory experiment compared one baseline navigation batch with four extension batches and repeated settings changes. Its rise to approximately 1,157 MiB is not a valid isolated measure of NAAB growth. A follow-up used four equal navigation batches for both browsers and forced collection between batches: much of the allocation was reclaimable. Browser caching, history, allocator retention, extension UI, worker restarts and instrumentation remain confounders. The first browser experiment overlapped some compiler samples, so its timing comparisons are approximate; the matched follow-up ran without that compiler workload.

Raw observations: [initial browser run](phase-1-performance.json), [matched navigation/GC diagnostic](phase-1-performance-gc.json), [optimized full-list wake run](phase-1-performance-after.json), and [offline native compiler](phase-1-native-performance.json). Browser snapshot source hashes are recorded in the browser JSON. Native input hashes and output counts differ slightly because its previously cached text files are a different list snapshot; these measurements must not be described as the same input.

To repeat the measurement design, build release `compile_subscriptions` and time seven invocations against fixed cached EasyList/EasyPrivacy files, sampling process memory. For browser comparisons, use disposable Chromium profiles with the unpacked extension versus `--disable-extensions`, fixed cached subscriptions, a loopback neutral page, equal navigation counts and browser-process private-memory sampling. Restart the extension worker and time the first successful `config.get`; report this separately from browser launch. Compare ordinary runs with a separately labeled `--js-flags=--expose-gc` diagnostic. Keep network downloads and personal browser profiles out of the benchmark.

## Accepted preview limits

- Click-triggered popup ad tabs remain possible. Network blocking excludes `main_frame`; the user agreed to defer navigation blocking. No popup destination/reproduction URL was provided, so the particular site's mechanism was not independently reproduced.
- EasyList/EasyPrivacy support is partial; unsupported subscription exceptions can conservatively reduce coverage. Local imports support a narrower grammar: an unsupported local exception can be omitted while a supported block remains active. Import diagnostics must be checked; arbitrary full lists should not be pasted into Local filters expecting subscription semantics.
- Cosmetics and the picker operate in the top document; selectors can become stale when a site changes. Cosmetic hiding does not necessarily stop downloads.
- No reliable dedicated YouTube video-ad blocking, automatic list refresh, Firefox/Safari support, DNS filtering, or proxy filtering.
- Mac success is one user's Intel Ventura/Chrome report. Broader Edge/platform and long-duration performance testing are not established. The package remains an unnotarized developer preview.

## Next milestone

The performance follow-up is sufficient to close this developer-preview phase based on the measured improvement and retained recovery behavior. Popup blocking does not need to be implemented to close this phase. DNS planning and implementation, merging the preview branch, and publishing a release remain separate next actions.
