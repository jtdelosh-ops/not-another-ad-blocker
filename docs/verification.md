# Verification

## Activity milestone — extension 0.3.0 / companion 0.2.0

The activity update adds Chrome's native page network-block total and a separate 300-entry session-only sample of matched network rules. TypeScript checking and all **58 extension tests** pass; the **8 installer/native integration tests** also pass against the unchanged companion. The 55 Rust tests passed at the previous milestone; no Rust source or native protocol changed in this update.

New tests cover bounded capture and restoration, URL redaction before storage, duplicate events, clear during an in-flight storage write, failed storage, tab cleanup, native-counter action/placeholder guards, trusted read/clear routes, installed-rule ID composition, metadata during commits/rollback, safe UI rendering, unavailable versus zero totals, and stale UI responses.

The optional `tests/activity-browser.mjs` regression **passed in isolated Chrome for Testing 151 on Windows** against the final 0.3.0 bundle. It verified an exact count of one block with two log entries (block and allowance), cosmetic exclusion, redaction before session storage, denied content-script reads/clears, clear without resetting the count, navigation resets, global/site controls, tab cleanup, actual worker termination and recovery with log/count retention, and browser restart clearing activity while preserving rules. The final viewer screenshot was inspected for readable, unclipped controls and correct count/site labels.

The regression uses loopback fixtures with known blocking/allowance/cosmetic rules. DNR feedback, browser counts, activity routes, session storage and viewer UI are real; only native-host fixture import/status responses are substituted with output from the actual Rust executable. Run it after the README build steps:

```sh
node tests/activity-browser.mjs
```

Detailed match events are an unpacked-extension capability. The sample is not a complete page history: it includes previous pages in the selected tab, drops older records, and can miss startup/termination events. Rule details and tab hostnames reflect the state observed when an event arrives; events delayed across updates/navigation may lack exact historical attribution. Packed/store logging behavior and installed macOS browser integration remain unverified. The native counter excludes allowances and cosmetic hiding for the currently supported action types.

## Subscription milestone — 0.2.0

Verified on Windows on 2026-09-21. This is an implementation checkpoint with partial EasyList/EasyPrivacy compatibility, not a completed Phase 1 exit review.

| Check | Result |
| --- | --- |
| Rust formatting and builds | Formatted; debug and release builds passed with Rust 1.98.1, Windows GNU target |
| Rust tests | 55 passed: 52 unit tests and 3 tests spawning the executable |
| TypeScript | Typecheck passed |
| Extension unit tests | 39 passed |
| Extension build | Version 0.2.0 unpacked MV3 bundle in `extension/dist` |
| Root installer/native integration tests | 8 passed, also rerun against the final release executable |
| Live subscription integration | Real HTTPS downloads, Rust compilation/cache, paged process transport, and the built extension NativeClient passed |
| Chromium local-import regression | Passed in isolated headless Chrome for Testing 151 |
| Chromium subscription regression | Real snapshot accepted; network/cosmetic effects, failure retention, local overrides, and offline restart passed |
| Independent review | Long-exception handling, cross-language counts, document/iframe exemptions, and hostname-boundary findings corrected and rechecked |

The 102 automated unit/process/integration tests cover bounded framing, strict protocol inputs, deterministic compilation, CSS restrictions, exception priority, download validation, cache integrity and retention, paging boundaries, failed refreshes, quota handling, storage rollback/recovery, preserved local state, diagnostic sampling, and late progress responses. Real network and browser checks are additional opt-in scripts, rather than dependencies of the ordinary test suite.

## Real subscription result

The final release executable downloaded both fixed official sources at **2026-09-21 23:22:20 UTC**, with source version **202609212310** for each. The complete download/compile/cache/native-page/client-validation run took about 7.9 seconds on this machine; this is one observation, not a performance guarantee.

| Source | Bytes | SHA-256 |
| --- | ---: | --- |
| EasyList | 2,182,105 | `3ba097c1d23f734f5388198d1731dcc851d53841d1b9f611ec3e4c092150a39d` |
| EasyPrivacy | 1,504,635 | `cdc76653740ca8d2975dc64204e10af482241828ee7b62e95cbda66f48b17a32` |

The compiled snapshot contains **12,822 network rules** (11,454 blocks, 1,364 allows, and 4 frame/page allowances) and **7,536 cosmetic rules**. Equivalent domain rules contain 98,331 domain entries across packed rules; this is not a unique-domain or blocked-request count. No rules were dropped for capacity in this snapshot. There are 7,232 unsupported source lines and 13,199 cosmetic safety omissions. Unrepresentable generic-hiding exceptions conservatively suppress generic subscription cosmetics; supported site-scoped cosmetics remain. Local cosmetics are independent. Live list contents and these counts will change.

## Browser checks

The original Chromium regression verifies local imports, real DNR block/allow enforcement against a loopback HTTP fixture, cosmetic hiding, global/site pause and resume, and failed-import retention. A paused hostname embedded under a protected top-level page does not bypass that parent's filtering. Injected storage and rollback failures produce an explicit unknown-protection state and disable mutation controls. The regression passes with the updated extension.

The subscription regression loads the full real snapshot through the options interface and verifies that Chrome accepts every compiled network rule. It maps fixture hostnames to loopback, so the test sends no requests to advertising sites. It verifies actual subscription blocking and site-scoped CSS hiding, preserved local rules/global settings/site exceptions, local allowance priority, site bypass, partial-transfer failure retention, a full browser restart without the companion, and subscription removal that leaves local protection intact. No page JavaScript errors occurred. The options screenshot was inspected for readable controls and metadata.

**Browser transport limit:** these browser regressions substitute `chrome.runtime.sendNativeMessage` with responses from the real Rust compiler/snapshot. The live integration script separately uses the built NativeClient and real framed Rust processes for downloads and every page, including persistence across host processes. Automated checks do not prove installed-browser native-host discovery/launch. The user reported the earlier installation worked; version 0.2.0 should be checked in the installed browser after reloading the extension. No registration or existing browser profile was modified by these tests.

**Remaining limits:** macOS browser installation/registration is untested on macOS hardware; hosted Windows and macOS CI passed the deterministic baseline checks when the repository was published. Broad day-to-day site compatibility, interruption at every refresh stage, formal performance budgets, and release packaging remain unverified. Advanced filter syntax and the element picker remain unfinished. A failed refresh preserves committed filtering; unsupported exceptions can deliberately reduce subscription coverage. Activity and page counts are covered by the newer milestone above.

The Windows GNU linker emitted a nonfatal `.drectve` warning for debug/test binaries. All resulting binaries used in the tests ran successfully.

## Reproduce

### Local visual check

The added `scripts/test-page.mjs` server serves identical harmless SVG banners under a normal path and `/adimage.svg`. The tested real snapshot contains the unscoped EasyList `/adimage.` rule for image requests. A separate browser fetch verifies that the same endpoint can return the identical image bytes before an image-load failure is classified as blocking. Timeouts and failed control requests produce an inconclusive result.

`tests/local-check-browser.mjs` passed in isolated Chrome for Testing 151 with all 12,822 real subscription rules. It verified the empty baseline, global off/on, repeated checks, site pause/resume, and a deliberately failed reference request. On/off screenshots were inspected: the normal banner stays visible, the test banner disappears only with filtering active, and both appear with filtering off. The unique test profile was removed afterward. This test seeds a previously validated snapshot into isolated extension storage, so it does not exercise native-host discovery or downloads. It checks one network rule and does not claim overall filtering coverage or cosmetic validation.

To run this additional optional regression after generating the snapshot below:

```sh
node tests/local-check-browser.mjs test-results/live-subscriptions.json
```

The interactive page can be started with `node scripts/test-page.mjs` or the PowerShell launcher described in the README. The server binds only to 127.0.0.1 and serves fixed assets without contacting ad networks or changing extension state.

### Existing checks

Run the root README build and test commands. After building the extension and companion, run the process integration tests:

```sh
node --test tests/installer.test.mjs tests/native-integration.test.mjs
```

For optional browser checks, install Playwright and Chromium in your development environment. `NAAB_PLAYWRIGHT` may point to an installed Playwright package, `NAAB_CHROMIUM` to a Chromium executable, and `NAAB_BINARY` to a companion executable (debug is the default). Each browser script uses a fresh test profile under `test-results` and a loopback server.

```sh
node tests/browser-smoke.mjs
```

For the live subscription check on Windows PowerShell, from the repository root:

```powershell
New-Item -ItemType Directory -Force test-results | Out-Null
$env:NAAB_DATA_DIR = Join-Path (Get-Location) 'test-results\native-cache'
$env:NAAB_BINARY = Join-Path (Get-Location) 'companion\target\release\naab-companion.exe'
node tests/live-subscriptions.mjs test-results/live-subscriptions.json
node tests/subscriptions-browser.mjs test-results/live-subscriptions.json
```

The live script contacts only the two fixed official list URLs and writes to the isolated cache specified above. The browser script uses the resulting saved snapshot without downloading lists. Its representative fixture rules must still exist in that snapshot; if upstream lists remove them, update the fixture assertions to another verified rule. Test profiles/cache are disposable and can be removed after their browser processes close.

For the existing installation, reload the extension, check that the companion reports **0.2.0**, and click **Download / refresh selected**. Existing paths and registration remain valid. Reload pages after changing protection.
