# Architecture — extension 0.4.1 / companion 0.2.1

The browser enforces filtering. The Rust companion compiles local filter text and downloads, compiles, and caches the fixed EasyList/EasyPrivacy subscriptions. Native Messaging starts the companion on demand; each `sendNativeMessage` request may run in a new process. Subscription snapshots therefore persist on disk. The browser retains its own committed compiled rules and settings, so ongoing filtering does not depend on the companion or network being available.

```text
Options: local filter text
    → background → native client → rules.compile → Rust local compiler
    ← small compilation + diagnostics

Options: Download / refresh selected
    → background → native client → lists.refresh
    → fixed HTTPS list sources → validation → subscription compiler
    → immutable local snapshot (raw lists + metadata + compiled pages)
    ← snapshot manifest
    → lists.page requests → fully validated compiled arrays

Local compilation + subscription compilation + user controls
    → atomic browser DNR replacement + committed browser-local configuration

Top-document content script → authenticated cosmetics.get
    ← scoped, restricted selectors only
```

## Local and subscription compilers

The original `rules.compile` contract stays bounded at 128 KiB/2,000 nonempty lines and preserves its narrow network grammar. Both compilers share the restricted cosmetic selector validation described below. The separate subscription compiler accepts the larger fixed-source lists and adds supported URL wildcards, resource restrictions, party/domain conditions, case matching, common exceptions, and `badfilter` handling. Neither compiler executes downloaded code. Main-frame requests are excluded from blocking rules; document exception guards may allow main/subframe traffic and descendants.

Full hostname filters sharing the same action and context are compacted into `requestDomains` groups of at most 1,000 hosts. Path/wildcard filters and rules with differing conditions stay separate. This reduces browser-rule use without removing those source-domain entries. Source-list statistics count accepted lines; final statistics count emitted DNR and cosmetic entries. Deduplication, compaction, exceptions, and omissions make these counts differ.

The subscription compiler retains supported network exceptions before selecting blocking rules within the budget. Unsupported exception syntax can generate a broader allow guard while retaining a representable URL boundary. If that boundary cannot be represented, the compiler conservatively suppresses affected subscription coverage. Supported cosmetic exceptions become excluded domains. Unrepresentable hiding/document exceptions can suppress selectors or larger cosmetic groups; unscopable generic-hiding exceptions suppress generic subscription cosmetics. These decisions appear in coverage counters and grouped diagnostics. They do not erase local custom rules.

Cosmetic selectors are restricted ASCII tag/class/ID compounds, optionally followed by one `:has(> child)` check. The child must be another compound containing at least one class or ID; for example, `div:has(> .ad-label)` is supported, while `div:has(> video)` is not. The entire selector stays within 512 bytes. Nested checks, other pseudo-selectors/combinators, and attribute selectors are rejected by both Rust compilation and browser-side validation. The content script installs native CSS; it does not run a procedural selector engine.

Positive and negative domain scope is supported for subscriptions; local imports keep positive scope only. Content scripts run only in the top document. They receive at most 12,000 validated selectors (10,000 subscription + 2,000 local), not raw lists, network arrays, or browsing history. A direct-child rule remains dependent on its marker element being present; support for this syntax does not automatically install a site-specific rule.

## Visual picker

The popup starts a picker only in an active, non-private HTTP(S) tab with protection enabled. The background stores a random, ten-minute grant in trusted session storage, bound to the tab and hostname. Only that top-frame content script may save or undo with the token; ordinary web-page senders cannot start a picker or use other privileged routes. Session storage retains the grant across service-worker suspension.

The content script builds a closed-shadow control panel and derives bounded candidates using the existing selector grammar. Page-root matches are rejected. Trusted user input selects an element, previews all matches using a temporary stylesheet, and confirms saving. Cancel removes only temporary UI/styles. No page content is sent to the companion; only the site-scoped filter travels through the existing compiler.

Saving appends a marked rule to the current local filter text inside the controller's mutation queue. Compilation and the existing commit/rollback path preserve subscriptions and other local changes. Immediate undo removes only the exact marked addition, refusing to erase it if it has been edited. A rule already present in local filters is not claimed for undo. Picker creation and undo require the companion; ongoing filtering does not.

## Quota and precedence

The controller reserves 2,200 dynamic-rule slots for all supported local rules and site controls before assigning a subscription budget, capped at 29,800. For a 30,000-rule browser this leaves 27,800 subscription slots. Subscription exceptions precede blocks when fitting that budget, with omissions reported explicitly. Browser rejection does not commit a partial rule set.

At application time the controller combines both compilations and assigns sequential, globally unique IDs in deterministic array order. IDs below 1,000,000,000 belong to compiled rules; the higher range is reserved for site controls. Local network block/allow priorities are 3/4; subscription block/allow priorities are 1/2; site protection overrides are priority 100. Global-off yields no active DNR rules or cosmetics.

## Download and snapshot lifecycle

Only `easylist` and `easyprivacy` IDs are accepted; they map to fixed HTTPS URLs. There is no caller-selected URL or path. Fetches are sequential, use Rustls certificate validation, reject redirects, time out after 25 seconds per source, and limit each source to 8 MiB and 300,000 lines. UTF-8, list header, and meaningful content are checked before compilation. Startup never fetches lists.

All selected sources must succeed before writing a new snapshot. Files are written and synchronized in a staging directory, then the directory is renamed into place. Snapshots include raw source text, source metadata, a manifest, a SHA-256-addressed index, and paged compiled output. Pages hold at most 128 items and stay below 512 KiB including response overhead. Hash and boundary checks detect missing, stale, or corrupt snapshots. The normal retention policy keeps three completed snapshots; each snapshot is capped at 64 MiB. Removing browser subscriptions leaves these native cache files in place.

`NAAB_DATA_DIR` overrides the default user-local cache root for deterministic offline tests and portable use. Protocol input never chooses a filesystem location. The [subscription contract](../shared/protocol/subscriptions.md) defines all manifest and page fields.

## Commit and recovery

Configuration version 1 remains readable. An optional subscription state is stored alongside the previous local import and user settings. Local import replaces only local rules; refresh replaces only subscriptions; removal retains local rules. UI snapshots omit large compiled rule arrays.

The browser validates every snapshot page, identity, total, schema, and traversal before installation. Controller mutations are serialized. Refresh does download/validation work outside the mutation queue, then commits against current settings so concurrent site/global/local changes are preserved. A subscription generation guard prevents a removed subscription from reappearing when an older refresh finishes.

Before DNR changes, intended state is journaled as `pending`. DNR replacement is atomic, but extension storage is a separate system. If committing storage fails, previous DNR rules are restored; rollback failure enters an explicit recovery-required state. A successful commit stores an SHA-256 rule stamp with its configuration. Because Chrome retains dynamic rules while an MV3 worker sleeps, a normal worker wake trusts a matching stamp and avoids replacing the full rule set. Pending, missing, or mismatched stamps force reconciliation before the state becomes trusted; uncommitted pending state is then discarded. `unlimitedStorage` supports the current large snapshot and its pending journal.

The official [Native Messaging documentation](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) and [declarativeNetRequest API](https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest) describe the underlying framing, response limit, rule priorities, quotas, and atomic update behavior.

## Activity and page counts

Chrome owns the page total through `setExtensionActionOptions({displayActionCountAsBadgeText:true})`; the popup and viewer read `action.getBadgeText`. This counts network blocking for the current block/allow/allowAllRequests rule set; allowances are excluded. The adapter rejects unsupported action types before labeling a total as blocked requests, so future redirect/header-modification support must revisit this contract. Totals reset on committed page navigation and remain separate from retained diagnostic rows and cosmetic selectors.

Unpacked-only `onRuleMatchedDebug` events feed a 300-entry session log. The controller synchronously resolves each installed rule ID to bounded compiled conditions and a local-list, combined-subscription-set, or site-exception label. It reports unavailable metadata during startup, rule mutations and rollback, and after failed recovery. Entries retain captured descriptions instead of reinterpreting reused rule IDs later. Network compilation currently loses exact source-line provenance through deduplication/packing; the viewer does not attribute a combined subscription rule to an individual list.

Debug events carry no rule-generation timestamp. Rule details and the tab hostname describe the state known when the event arrives; delayed events around a rule replacement or navigation may lack exact historical attribution. This diagnostic limitation does not affect Chrome's separate native page total.

Request credentials/query/fragment are removed before memory/storage. Writes are batched and serialized, and storage failure is surfaced separately from filtering. Only the extension's allowlisted top-level interfaces can read or clear activity. Content scripts cannot access the session store. Current-tab views retain earlier pages within that tab; the log is explicitly a recent sample rather than an exact page history. See [privacy details](privacy-model.md).

## DNS development core

`companion/src/dns` supplies a separate, opt-in `naab-dns-dev` executable. Its foreground lifecycle is independent of the browser's short-lived Native Messaging process. Configuration selects explicit upstreams and local filter files; the development listener accepts queries only on a loopback high port. Hickory parses/serializes messages, bounded Tokio tasks handle UDP/TCP, and the in-memory cache ages TTLs. DNS policy shares the existing normalized rule parser, with conservative exception handling and separate user overrides. A bounded local diagnostic sample identifies the DNS layer, outcome and rule/source.

See the [DNS core guide](dns-core.md) for its contract and limits. No DNS commands are added to Native Messaging yet, and the extension's activity viewer does not show this separate process's sample. System configuration, privilege helpers, watchdog recovery and DNS packaging remain unfinished. Advanced selector generation, fuller filter compatibility, SQLite if needed, Firefox and proxy features also remain deferred. `LOCAL_ONLY` remains a reserved browser diagnostic target.
