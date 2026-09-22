# Not Another Ad Blocker
## Product & Engineering Roadmap

Current checkpoint (2026-09-22): [Phase 1 is complete](phase-1-exit-review.md) for the extension 0.4.1 / companion 0.2.1 developer preview. Phase 2 has begun with an opt-in [DNS development core](dns-core.md): loopback UDP/TCP, forwarding, bounded cache, conservative domain compilation, user overrides and local diagnostics. System integration, extension DNS controls, full-list compatibility and cross-platform verification remain unfinished. The requirements below describe intended scope, not a list of completed features.

**Project name:** Not Another Ad Blocker  
**Working abbreviation:** NAAB  
**Project type:** Local-first browser privacy and content-filtering system  
**Primary platform target:** Chromium browsers first, with architecture designed for later Firefox support  
**Core principle:** Use the browser extension for what the browser allows, and a local companion service for capabilities that Manifest V3 cannot provide cleanly.

---

# 1. Product Vision

Not Another Ad Blocker is not intended to be a simple clone of uBlock Origin.

The goal is to build a **local browser privacy firewall** with a Manifest V3 extension acting as the browser-facing interface and a local companion service handling richer filtering, rule compilation, diagnostics, and future system-level capabilities.

The project should prioritize:

- Local-first operation
- No account requirement
- No cloud dependency for filtering
- No browsing-history upload
- No telemetry by default
- Transparent explanations for why content was blocked or allowed
- Compatibility with widely used Adblock/uBlock-style filter lists where practical
- A simple default user experience with advanced diagnostics available when needed
- Incremental development so each phase produces a usable system before moving forward

---

# 2. High-Level Architecture

```text
┌────────────────────────────┐
│      Chromium Browser      │
│                            │
│     MV3 Extension          │
│  • Popup / site controls   │
│  • Cosmetic filtering      │
│  • Element picker          │
│  • Activity viewer         │
│  • MV3 network rules       │
└──────────────┬─────────────┘
               │
        Native Messaging
               │
┌──────────────▼─────────────┐
│      Local Companion       │
│         Rust               │
│                            │
│  • Rule parser/compiler    │
│  • Filter-list manager     │
│  • Native rule engine      │
│  • Diagnostics             │
│  • Local storage           │
│  • Future DNS module       │
│  • Future proxy module     │
└────────────────────────────┘
```

The system should treat Chrome's Manifest V3 APIs as one enforcement target rather than the entire filtering architecture.

A rule compiler should eventually decide whether a rule can be:

1. Enforced directly through Chrome Manifest V3.
2. Enforced cosmetically in the page.
3. Evaluated or enforced by the local companion.
4. Marked unsupported with an explanation.

---

# 3. Technology Choices

## Browser extension

Recommended:

- TypeScript
- Manifest V3
- Minimal UI framework initially
- React may be introduced only if UI complexity justifies it
- Chrome `declarativeNetRequest`
- Chrome `nativeMessaging`
- Chrome storage APIs
- Content scripts for cosmetic filtering and element selection

Avoid overbuilding the UI during Phase 1.

## Local companion

Recommended:

- Rust
- Tokio for async work if needed
- Serde for message serialization
- SQLite for local state
- Structured logging
- Native Messaging over stdin/stdout

The companion should run only on the user's machine and should not require a remote service.

---

# 4. Repository Structure

Initial recommended monorepo:

```text
not-another-ad-blocker/
├── README.md
├── LICENSE
├── docs/
│   ├── architecture.md
│   ├── roadmap.md
│   ├── privacy-model.md
│   └── native-messaging.md
├── extension/
│   ├── manifest.json
│   ├── src/
│   │   ├── background/
│   │   ├── content/
│   │   ├── popup/
│   │   ├── options/
│   │   └── shared/
│   ├── public/
│   ├── package.json
│   └── tsconfig.json
├── companion/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── messaging/
│       ├── rules/
│       ├── lists/
│       ├── storage/
│       └── diagnostics/
├── shared/
│   └── protocol/
├── tests/
│   ├── fixtures/
│   ├── integration/
│   └── regression/
└── scripts/
    ├── install-native-host/
    └── dev/
```

The shared protocol directory should define message contracts between the extension and the companion.

---

# 5. Core Message Protocol

Native Messaging should use a small versioned JSON protocol.

Example request:

```json
{
  "version": 1,
  "id": "req-123",
  "type": "status.get",
  "payload": {}
}
```

Example response:

```json
{
  "version": 1,
  "id": "req-123",
  "ok": true,
  "payload": {
    "companionVersion": "0.1.0",
    "ruleCount": 125430,
    "listsLoaded": 2
  }
}
```

Initial message types may include:

- `status.get`
- `lists.get`
- `lists.refresh`
- `rules.evaluate`
- `rules.add`
- `rules.remove`
- `activity.query`
- `settings.get`
- `settings.update`

All protocol changes should be versioned and backward-compatible where practical.

---

# 6. Phase 1 — Core MVP

## Objective

Build the smallest complete system that proves the architecture works:

**Chromium MV3 extension + Rust local companion + filter-list ingestion + rule compilation + cosmetic filtering + basic diagnostics.**

Phase 1 should result in something that can be installed locally and used for day-to-day browsing in a limited but meaningful form.

## Phase 1 scope

### 1. Browser extension shell

Create a working MV3 extension with:

- Toolbar icon
- Popup
- Background service worker
- Content script
- Options/settings page
- Per-site protection toggle

Minimum popup:

```text
Not Another Ad Blocker

example.com

Protection        ON
Blocked today     23

[ Block something ]
[ View activity  ]
```

Do not spend significant time on visual polish yet.

---

### 2. Rust companion application

Create a local Rust executable capable of:

- Starting cleanly
- Reading Native Messaging frames
- Parsing JSON messages
- Returning responses
- Logging locally
- Reporting its version and health status

The first milestone is successful communication:

```text
Extension
   ↓
status.get
   ↓
Rust companion
   ↓
status response
```

---

### 3. Native Messaging installer

Provide scripts/instructions for registering the native host.

Support during Phase 1:

- macOS
- Windows

Linux may be added if it does not materially slow Phase 1.

The registration process must be documented clearly.

---

### 4. Filter-list ingestion

Support at minimum:

- EasyList
- EasyPrivacy

The companion should be able to:

1. Download or load the lists.
2. Parse supported rules.
3. Normalize them.
4. Store metadata locally.
5. Report unsupported rule syntax.
6. Refresh lists manually.

Automatic scheduling may be added late in Phase 1 if trivial.

---

### 5. Initial rule parser

Implement a practical subset of Adblock/uBO-style syntax.

Priority support:

```text
||example.com^
||ads.example.com^$third-party
@@||example.com/resource.js
example.com##.advertisement
##.generic-ad-class
```

Do not attempt complete uBlock compatibility during Phase 1.

Each rule should compile to an internal normalized representation.

Example:

```text
Rule
├── kind
├── pattern
├── domain scope
├── resource type
├── exception state
└── enforcement target
```

---

### 6. Rule classification/compiler

Each parsed rule should be classified as one of:

```text
MV3_NETWORK
COSMETIC
LOCAL_ONLY
UNSUPPORTED
```

Phase 1 does not need to enforce all `LOCAL_ONLY` rules.

The important requirement is that the architecture can identify the difference.

Example diagnostic:

```text
Rule: ||ads.example.com^$third-party
Target: MV3_NETWORK
Status: Compiled successfully
```

or:

```text
Rule: <complex unsupported rule>
Target: UNSUPPORTED
Reason: Requires capability not currently implemented
```

---

### 7. Manifest V3 network blocking

Compile supported network rules into Chrome `declarativeNetRequest` rules.

Requirements:

- Deterministic rule IDs
- No collisions
- Controlled updates
- Graceful handling of Chrome rule limits
- Useful diagnostics if a rule cannot be loaded

The extension should be able to report approximate network blocks per site.

---

### 8. Cosmetic filtering

Implement content-script-based hiding for supported cosmetic rules.

Minimum support:

```text
example.com##.advertisement
##.generic-ad-class
```

Requirements:

- Apply site-scoped rules
- Apply supported generic rules
- Avoid obvious page breakage
- Record cosmetic matches in local activity data where practical

---

### 9. Per-site controls

Users must be able to disable protection for a site.

Example state:

```text
Protection for example.com: OFF
```

The setting should persist.

The extension should clearly distinguish:

- Global protection state
- Site exception state

---

### 10. Basic activity viewer

Implement a simple activity/log view.

Each entry should ideally contain:

- Timestamp
- Site
- Request or element
- Action
- Rule
- Enforcement target

Example:

```text
17:42:11
ads.example.net/banner.js

BLOCKED
EasyList
MV3_NETWORK
```

The logger does not need to be as powerful as uBlock Origin's logger during Phase 1.

It must, however, be understandable.

---

### 11. Visual element blocker — basic version

Implement:

**Block something on this page**

Behavior:

1. User activates element-picker mode.
2. Elements highlight on hover.
3. User selects an element.
4. Extension derives a reasonable CSS selector.
5. User previews the result.
6. User saves the cosmetic rule.

Example:

```text
example.com##div.sidebar-ad
```

Rules created manually should be stored separately from downloaded lists.

Advanced network correlation belongs in later phases.

---

### 12. Privacy baseline

Phase 1 must enforce these rules:

- No account system
- No remote telemetry
- No URL upload
- No browsing-history upload
- Local logs only
- Local settings only
- Filter lists are fetched directly from their configured sources

Any future telemetry must be explicitly opt-in.

---

# 7. Phase 1 Non-Goals

Do not include these unless they become trivial side effects of necessary work:

- HTTPS interception
- Root certificate installation
- Full system proxy
- Full DNS filtering
- Complete uBlock syntax compatibility
- Cloud account
- Sync service
- Mobile support
- Safari support
- Firefox support
- AI-generated filtering rules
- Automatic network correlation for selected page elements
- Full dynamic firewall functionality
- System-wide application filtering

These are intentionally deferred.

---

# 8. Phase 1 Definition of Done

Phase 1 is complete when all of the following are true:

- Chromium extension installs successfully in developer mode.
- Rust companion installs and starts successfully.
- Extension and companion communicate through Native Messaging.
- EasyList loads successfully.
- EasyPrivacy loads successfully.
- Supported network rules compile into Manifest V3 rules.
- Supported cosmetic rules hide matching page elements.
- User can disable protection for an individual site.
- User can create and persist a cosmetic rule with the visual picker.
- Activity view shows why at least common network requests were blocked.
- Unsupported rules fail safely and are reported rather than silently miscompiled.
- No browsing data is transmitted to a NAAB-operated server.
- Basic unit tests exist for parser and protocol behavior.
- At least one integration test proves extension ↔ companion communication.
- README contains install and development instructions.
- Known limitations are documented.

---

# 9. Phase 1 Exit Review

Before moving to Phase 2, review:

## Reliability

- Does normal browsing remain stable?
- Does the service worker recover after Chrome suspends it?
- Does Native Messaging reconnect cleanly?
- Can a bad filter list crash the companion?
- Are malformed messages safely rejected?

## Filtering quality

- Are common ads actually blocked?
- Are common trackers blocked?
- Are exceptions honored?
- Are cosmetic rules applied consistently?
- Are there obvious false positives?

## Performance

Measure:

- Extension startup
- Companion startup
- List parse time
- Rule compilation time
- Browser memory impact
- Companion memory impact
- Page-load impact

## User experience

A user should understand:

- Whether protection is on
- How many things were blocked
- How to disable protection for a site
- Why a request was blocked
- How to block an unwanted visual element

If any of those remain confusing or unreliable, Phase 1 is not finished.

---

# 10. Phase 2 — DNS Filtering

## Objective

Add optional local DNS filtering without introducing HTTPS interception.

The DNS component should extend NAAB beyond the browser while remaining substantially simpler and less invasive than a full proxy.

The intended model is:

```text
Browser / OS / application
          ↓
      Local NAAB DNS
          ↓
  Is hostname blocked?
      ├── yes → blocked response
      └── no  → upstream resolver
```

## Phase 2 architecture

DNS should be implemented as a dedicated module inside the Rust companion rather than mixed directly into browser rule handling.

Recommended structure:

```text
companion/
└── src/
    ├── dns/
    │   ├── server.rs
    │   ├── resolver.rs
    │   ├── blocklist.rs
    │   ├── cache.rs
    │   ├── diagnostics.rs
    │   └── platform.rs
```

Recommended Rust DNS stack:

- Hickory DNS server/resolver ecosystem
- Tokio if async I/O is required
- Existing shared rule parser/compiler
- SQLite or existing local storage for configuration and metadata

Do not hand-roll DNS packet parsing unless a concrete requirement makes the established library insufficient.

## DNS request flow

Recommended processing order:

```text
Incoming query
    ↓
Normalize hostname
    ↓
User allowlist
    ↓
User blocklist
    ↓
Compiled filter-list rules
    ↓
Blocked?
   ├── yes → return blocked response
   └── no  → check cache
                  ↓
              cache hit?
              ├── yes → return
              └── no  → forward upstream
                              ↓
                          cache response
                              ↓
                           return
```

The implementation should be deterministic and diagnosable.

## Block response behavior

Start with `NXDOMAIN` as the default blocked response unless testing demonstrates that a sinkhole response is materially more compatible.

Alternative block behavior may later include:

- `NXDOMAIN`
- `0.0.0.0`
- `::`
- configurable local sinkhole

The default should minimize connection attempts and avoid creating unnecessary local listeners.

## Rule compilation for DNS

Do not build a completely separate rule language for DNS.

The normalized rule model should support multiple enforcement targets.

Conceptually:

```text
Rule {
    action: Block,
    host: "tracker.example.com",
    scope: Global,
    source: EasyPrivacy,
    targets: [MV3, DNS]
}
```

The compiler should produce:

```text
Normalized rule
    ├── MV3 compiler
    ├── Cosmetic compiler
    └── DNS compiler
```

Only rules safely reducible to hostname/domain behavior should be sent to the DNS engine.

Rules that depend on:

- URL paths
- headers
- resource type
- first-party/third-party context not reproducible at DNS level
- page origin
- request method
- response content

must not be silently converted into DNS blocks.

They should remain enforced by another backend or be reported as unsupported for DNS.

## Domain matcher

The DNS engine should use an efficient hostname matcher rather than scanning the entire rule list for every query.

Preferred approaches include:

- reversed-domain trie
- suffix tree
- radix trie
- equivalent optimized suffix matcher

Example conceptual structure:

```text
com
└── example
    ├── ads
    ├── metrics
    └── telemetry
```

A query for:

```text
foo.ads.example.com
```

should efficiently match a rule covering:

```text
ads.example.com
```

The implementation should normalize:

- case
- trailing dots
- punycode / IDNA where applicable
- malformed hostnames

before rule evaluation.

## Rule precedence

Explicit user intent should override downloaded lists.

Recommended precedence:

```text
User allowlist
      ↓
User blocklist
      ↓
Configured filter lists
      ↓
Allow by default
```

The exact conflict-resolution rules should be documented and covered by tests.

## Operating modes

Phase 2 should support system-level DNS filtering as the primary implementation.

Desired controls:

```text
Browser filtering: ON
DNS filtering:     ON
System-wide DNS:   ON
```

or:

```text
Browser filtering: ON
DNS filtering:     OFF
System-wide DNS:   OFF
```

A browser-only DNS mode may be explored if the browser provides a clean, reliable integration path, but it is not required for Phase 2 completion.

Do not add fragile browser launch flags or undocumented browser behavior merely to claim browser-only DNS support.

## Upstream resolvers

Support at minimum:

- Existing/system-configured resolver
- User-configured resolver
- Cloudflare
- Quad9

The default should preserve the user's current resolver behavior where practical.

Future optional support may include:

- DNS-over-HTTPS
- DNS-over-TLS

Encrypted upstream DNS is a future enhancement, not a requirement for initial Phase 2 completion.

## Caching

The DNS component should implement caching or use a resolver library with correct cache behavior.

Requirements:

- Respect DNS TTLs
- Do not cache malformed responses
- Avoid unbounded cache growth
- Flush or invalidate appropriately when DNS configuration changes
- Expose cache statistics for diagnostics if practical

## Diagnostics

DNS decisions should appear in the same user-facing activity model as browser decisions.

Example:

```text
17:46:03

telemetry.vendor.com

BLOCKED
Layer: DNS
Rule source: EasyPrivacy
Rule: ||telemetry.vendor.com^
```

Initial Phase 2 logging does not need to identify the originating application.

Application attribution is OS-specific and may be considered later.

The diagnostics model should distinguish:

```text
Layer: MV3
Layer: Cosmetic
Layer: DNS
Layer: Local rule engine
```

## System integration

System DNS configuration requires platform-specific code.

Support during Phase 2:

- macOS
- Windows

Linux may follow later unless implementation is straightforward and does not delay Phase 2.

The installer or platform helper must:

1. Read the current DNS configuration.
2. Store enough information to restore it exactly.
3. Configure the system to use the NAAB DNS service.
4. Verify the local resolver is reachable.
5. Restore the prior configuration on disable or uninstall.

## Privilege boundary

Do **not** run the entire NAAB companion as administrator/root merely to manage DNS.

If elevated privileges are required to:

- bind a privileged port,
- update system DNS settings,
- install a service,
- or restore configuration,

isolate that functionality into the smallest practical platform-specific helper.

The main companion should continue running with ordinary user privileges.

The privileged helper should expose only narrowly scoped operations and validate all input.

## Port strategy

Port 53 may require elevated privileges or conflict with existing local DNS services.

The implementation should evaluate the safest platform-specific approach.

Possible designs:

- bind directly to port 53 through a small privileged helper
- bind to a high local port and use supported OS forwarding/redirection
- integrate through a platform-supported local DNS mechanism

Do not choose a design that requires the entire companion to remain elevated.

## Failure recovery

Failure recovery is a Phase 2 release requirement, not an enhancement.

The system must not leave the user's machine in a state where:

```text
NAAB crashes
    ↓
DNS points only to unavailable localhost resolver
    ↓
Internet appears broken
```

Required behaviors:

- Store prior DNS configuration before changing it.
- Detect failed local DNS startup.
- Do not switch system DNS until the resolver is healthy.
- Restore previous DNS configuration on explicit disable.
- Restore previous DNS configuration on uninstall.
- Provide a recovery command or script if the UI cannot start.
- Detect stale configuration from a prior failed run where practical.
- Document manual recovery steps.

The design should prefer fail-safe behavior over maximum blocking coverage.

## Security requirements

Treat DNS queries and filter lists as untrusted input.

Requirements:

- Enforce message and packet size limits.
- Reject malformed DNS messages safely.
- Avoid unsafe parsing code.
- Do not expose an externally reachable DNS service by default.
- Bind only to loopback/local interfaces unless a future LAN mode is explicitly enabled.
- Do not log full query history indefinitely.
- Do not upload DNS activity.
- Keep DNS logs local.
- Maintain bounded caches and queues.

LAN-wide DNS filtering is explicitly out of scope for Phase 2.

## Phase 2 implementation order

Recommended sequence:

```text
1. Add dns module skeleton
2. Start local resolver on development port
3. Forward allowed queries upstream
4. Add caching
5. Add normalized hostname matching
6. Compile compatible existing rules into DNS rules
7. Add user allowlist/blocklist precedence
8. Add DNS activity diagnostics
9. Add macOS system DNS integration
10. Add Windows system DNS integration
11. Add configuration backup/restore
12. Add watchdog and startup health checks
13. Add failure-recovery tests
14. Add uninstall/disable restoration tests
15. Run Phase 2 exit review
```

## Phase 2 non-goals

Do not add during this phase:

- HTTPS interception
- TLS decryption
- root certificate installation
- full HTTP proxy
- application-level process attribution
- LAN-wide filtering
- parental controls
- enterprise network policy
- remote management

These belong to later phases, if ever.

## Phase 2 completion criteria

Move to Phase 3 only when all of the following are true:

- DNS filtering can be enabled and disabled safely.
- Allowed DNS queries resolve normally through the configured upstream.
- Blocked domains receive the documented blocked response.
- User allowlist and blocklist precedence works correctly.
- Compatible filter-list rules compile into the DNS matcher.
- Unsupported contextual rules are not silently over-applied at DNS level.
- Domain-block decisions appear in diagnostics.
- DNS caching behaves correctly.
- The local resolver is not externally reachable by default.
- The main companion does not need to run as administrator/root.
- Required privileged operations are isolated.
- System DNS changes are backed up before activation.
- System networking is restored correctly after disable.
- System networking is restored correctly after uninstall.
- A companion crash does not permanently strand the machine on a dead resolver.
- Recovery behavior is tested.
- Resolver failure and malformed-query tests exist.
- macOS behavior is tested.
- Windows behavior is tested.
- Known platform limitations are documented.

## Phase 2 exit review

Before beginning system-wide filtering work, verify:

### Reliability

- Does DNS recover cleanly after companion restart?
- Does changing networks break configuration?
- Does sleep/wake affect resolver availability?
- Does VPN usage expose conflicts?
- Are previous DNS settings restored correctly?

### Filtering quality

- Are obvious advertising/tracking domains blocked?
- Are legitimate shared-host domains avoided?
- Do allowlist overrides work immediately?
- Are DNS rules appropriately narrower than browser rules?

### Performance

Measure:

- query latency
- cache hit rate
- memory usage
- CPU usage under sustained lookup load
- rule lookup cost

### Product boundary

Confirm that Phase 2 remains a DNS filtering feature and has not drifted into proxy-style traffic inspection.

A full proxy should not begin merely because it is technically possible.

Proxy work should start only when there is a documented, concrete use case that:

1. MV3 cannot solve,
2. cosmetic filtering cannot solve,
3. DNS cannot solve,
4. and provides enough user value to justify the additional security and maintenance burden.

---

# 11. Phase 3 — System-Wide Filtering

## Objective

Extend protection beyond the browser while keeping the privacy model understandable.

## Scope

Potential targets:

- Desktop applications
- Electron apps
- Background telemetry domains
- Other browsers

Capabilities may include:

- System DNS filtering
- Per-application visibility where the OS permits it
- Domain-level controls
- Expanded activity logs

Do not introduce HTTPS decryption simply to increase coverage.

## Phase 3 completion criteria

Advance only when:

- System-wide mode is reversible.
- Application/network failure recovery is reliable.
- The user can clearly tell which layer performed a block.
- Browser-specific exceptions do not unexpectedly break system-level behavior.
- Privacy implications are documented accurately.

---

# 12. Phase 4 — Advanced Rule Engine

## Objective

Increase compatibility with uBlock-style filtering and introduce more powerful local evaluation.

## Potential features

- More filter modifiers
- Regex rules
- Redirect rules
- Scriptlet-style functionality where safe
- Advanced domain scoping
- Resource-type matching
- Dynamic filtering concepts
- Temporary rules
- Rule priority and conflict resolution
- Better exception handling

The internal rule engine should become authoritative, with individual enforcement backends receiving compiled subsets.

Conceptually:

```text
Filter lists
    ↓
Parser
    ↓
Normalized AST
    ↓
Rule engine
    ↓
Compiler
    ├── MV3
    ├── Cosmetic
    ├── DNS
    └── Native
```

## Phase 4 completion criteria

Do not proceed until:

- Rule precedence is deterministic.
- Conflicting rules are diagnosable.
- Regression tests exist for supported syntax.
- Performance remains acceptable with real-world list sizes.
- Compatibility claims are documented by feature, not broadly stated.

---

# 13. Phase 5 — Intelligent Visual Rule Builder

## Objective

Make custom rule creation significantly easier than traditional ad blockers.

This is a major differentiator.

## Desired experience

User selects:

**Block something on this page**

The system analyzes:

- DOM selector
- Parent structure
- Source URL
- Related network requests
- Script ownership
- Existing rules
- Domain scope

Then proposes the safest rule.

Example:

```text
Selected element:
div.promoted-content

Related request:
ads.example.net/widget.js

Recommended action:
Block network request on example.com

Suggested rule:
||ads.example.net/widget.js$domain=example.com

Estimated impact:
3 requests blocked
No required page resources detected
```

The user can preview before saving.

## Important principle

Do not use opaque AI-generated rules without showing exactly what will be applied.

Rule creation should remain deterministic and inspectable.

## Phase 5 completion criteria

- Suggested rules can be previewed.
- Users can undo generated rules.
- Recommendations explain scope and expected impact.
- Tests cover selector stability.
- Network correlation does not produce unacceptable false positives.
- Manual fallback remains available.

---

# 14. Phase 6 — Optional Local Proxy

## Objective

Explore capabilities unavailable through MV3 and DNS while preserving an explicit trust boundary.

## Default position

The proxy is optional and should not be required for normal use.

Initial proxy work should avoid TLS interception.

Potential capabilities:

- HTTP request inspection
- Metadata-based filtering
- Routing
- Request diagnostics
- Future advanced enforcement

HTTPS interception must be considered a separate sub-project because it requires installation of a trusted local certificate authority.

## HTTPS interception gate

Do not implement TLS decryption unless there is a specific capability that:

1. Cannot reasonably be achieved another way.
2. Provides substantial user value.
3. Can be explained clearly.
4. Can be implemented with strong key protection.
5. Can be completely removed on uninstall.

## Phase 6 completion criteria

- Proxy can fail open or safely disable without breaking networking.
- Certificate installation is not required for base proxy use.
- Security model has undergone focused review.
- Uninstall fully restores networking.
- Proxy mode is clearly differentiated from standard protection.

---

# 15. Phase 7 — Cross-Browser Support

## Candidates

- Firefox
- Edge
- Brave
- Other Chromium browsers

Firefox should be evaluated separately because its extension capabilities may permit functionality unavailable in Chromium.

The architecture should avoid making the Rust companion dependent on Chrome-specific concepts.

Recommended abstraction:

```text
Core rule engine
      ↓
Browser adapter
      ├── Chromium MV3
      └── Firefox
```

## Completion criteria

- Shared rule behavior is tested across browsers.
- Browser-specific limitations are surfaced in the UI.
- Feature parity is documented honestly.
- No weakest-common-denominator requirement is imposed on more capable browsers.

---

# 16. Phase 8 — Packaging and Public Release

## Objective

Turn the project into something ordinary users can safely install.

## Scope

- Signed binaries
- macOS packaging
- Windows installer
- Native host registration
- Clean uninstall
- Extension-store packaging
- Update strategy
- Release notes
- Privacy policy
- Threat model
- Reproducible build exploration
- Crash recovery

## Public release gate

Do not call the project production-ready until:

- Installation is straightforward.
- Uninstall restores all modified system settings.
- Updates do not silently break filtering.
- No browsing data is sent externally without consent.
- Security-sensitive components have been reviewed.
- Rule updates are authenticated or integrity-checked where practical.
- The project has repeatable regression testing.

---

# 17. Future Ideas — Not Yet Scheduled

These should remain ideas rather than roadmap commitments until the core project proves itself:

- Firefox-native enhanced mode
- Sync using user-owned storage
- Filter-list recommendation system
- Temporary "strict mode"
- Per-container or browser-profile policies
- Parental-control capabilities
- Enterprise policy deployment
- Mobile companion
- Safari support
- LAN-wide DNS filtering
- Home-network appliance mode
- Rule subscription publishing
- Community rule marketplace
- Optional AI-assisted diagnostics

AI should not be required for basic blocking.

---

# 18. Testing Strategy

Testing should begin in Phase 1 rather than being added later.

## Unit tests

Cover:

- Parser
- Rule normalization
- Rule precedence
- Message protocol
- Settings
- List metadata
- Selector generation

## Integration tests

Cover:

- Extension ↔ companion communication
- Filter-list loading
- Rule compilation
- Rule updates
- Site exceptions
- Cosmetic filtering

## Regression fixtures

Maintain representative pages or local test fixtures for:

- Banner ads
- Tracking pixels
- Third-party scripts
- Cosmetic ads
- Exceptions
- Broken-page scenarios

Every confirmed filtering bug should ideally become a regression test.

---

# 19. Security Principles

Because NAAB sits near browser traffic, security must be treated as a core product requirement.

Rules:

- Never execute arbitrary code from filter lists.
- Treat downloaded lists as untrusted input.
- Parse defensively.
- Enforce message-size limits.
- Validate all Native Messaging input.
- Avoid unnecessary filesystem access.
- Run companion with normal user privileges.
- Never require administrator/root privileges for normal browser filtering.
- Keep sensitive configuration local.
- Minimize persistent request logs.
- Provide log retention controls later.
- Do not expose a network-listening management API by default.

---

# 20. Development Principles

When implementing any phase:

1. Prefer simple, inspectable behavior over clever behavior.
2. Do not silently ignore unsupported rules.
3. Explain limitations in diagnostics.
4. Keep browser-specific code behind adapters where practical.
5. Preserve local-first operation.
6. Avoid introducing backend infrastructure unless absolutely necessary.
7. Add regression tests for real bugs.
8. Keep advanced capabilities optional.
9. Make every system-level modification reversible.
10. Never weaken privacy merely for analytics convenience.

---

# 21. Immediate Build Order

Work should begin in this sequence:

```text
1. Initialize monorepo
2. Create MV3 extension shell
3. Create Rust companion shell
4. Implement Native Messaging handshake
5. Define versioned protocol
6. Load local test filter list
7. Build parser subset
8. Build normalized rule model
9. Compile basic MV3 rules
10. Add EasyList
11. Add EasyPrivacy
12. Implement cosmetic filtering
13. Add site toggle
14. Add activity viewer
15. Add basic element picker
16. Add persistent custom rules
17. Add tests
18. Document installation
19. Run Phase 1 exit review
20. Decide whether Phase 2 gates are satisfied
```

Do not begin DNS work simply because the Phase 1 core architecture exists.

Phase 2 starts only after the Phase 1 definition of done and exit review are satisfied.

---

# 22. Success Standard

The long-term goal is not:

> Block more ads than every other tool.

The goal is:

> Give users strong local control over unwanted browser and network content while making filtering behavior understandable, inspectable, and increasingly independent of browser-extension limitations.

That principle should guide decisions throughout the project.
