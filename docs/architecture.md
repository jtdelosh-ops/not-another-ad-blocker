# Subscription milestone — 0.2.0

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

The original `rules.compile` contract stays bounded at 128 KiB/2,000 nonempty lines and preserves its narrow grammar. The separate subscription compiler accepts the larger fixed-source lists and adds supported URL wildcards, resource restrictions, party/domain conditions, case matching, common exceptions, and `badfilter` handling. Neither compiler executes downloaded code. Main-frame requests are excluded from blocking rules; document exception guards may allow main/subframe traffic and descendants.

Full hostname filters sharing the same action and context are compacted into `requestDomains` groups of at most 1,000 hosts. Path/wildcard filters and rules with differing conditions stay separate. This reduces browser-rule use without removing those source-domain entries. Source-list statistics count accepted lines; final statistics count emitted DNR and cosmetic entries. Deduplication, compaction, exceptions, and omissions make these counts differ.

The subscription compiler retains supported network exceptions before selecting blocking rules within the budget. Unsupported exception syntax can generate a broader allow guard while retaining a representable URL boundary. If that boundary cannot be represented, the compiler conservatively suppresses affected subscription coverage. Supported cosmetic exceptions become excluded domains. Unrepresentable hiding/document exceptions can suppress selectors or larger cosmetic groups; unscopable generic-hiding exceptions suppress generic subscription cosmetics. These decisions appear in coverage counters and grouped diagnostics. They do not erase local custom rules.

Cosmetic selectors remain restricted ASCII tag/class/ID compounds. Positive and negative domain scope is supported for subscriptions; local imports keep positive scope only. Content scripts run only in the top document. They receive at most 12,000 validated selectors (10,000 subscription + 2,000 local), not raw lists, network arrays, or browsing history.

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

Before DNR changes, intended state is journaled as `pending`. DNR replacement is atomic, but extension storage is a separate system. If committing storage fails, previous DNR rules are restored; rollback failure enters an explicit recovery-required state. On service-worker startup, committed storage is reconciled with DNR and uncommitted pending state is discarded. `unlimitedStorage` supports both the current large snapshot and its pending journal.

The official [Native Messaging documentation](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) and [declarativeNetRequest API](https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest) describe the underlying framing, response limit, rule priorities, quotas, and atomic update behavior.

Deferred work includes activity history and block counts, the element picker, fuller filter compatibility, SQLite if needed, Firefox, and all DNS/proxy/system-network features. `LOCAL_ONLY` remains a reserved diagnostic target; there is no native request-enforcement engine.
