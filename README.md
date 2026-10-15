# Not Another Ad Blocker

**Not Another Ad Blocker (NAAB)** is a browser privacy and content-filtering project that blocks supported advertising and tracking requests and hides unwanted page elements. It combines a **Chrome or Edge extension** with a **Rust companion running on your computer**.

The goal is filtering you can understand and control: choose your lists, add your own rules, pause protection for a site, and see which rules the software can actually support. Rule compilation and browser filtering happen locally, without an account or a cloud filtering service.

**Status: early developer preview.** Phase 1 is complete for the preview, but filter compatibility is incomplete. A fresh installation needs a filter-list download or a local rule import before it blocks anything.

**Current milestone: a dependable browser preview.** Package the extension and companion for Windows and Intel Mac, simplify setup, and verify filtering, controls, updates, and companion-unavailable behavior. DNS development is paused; the existing DNS previews remain experimental. The proxy has been removed from the product roadmap. See the [milestone and finish criteria](docs/roadmap.md#current-milestone--dependable-browser-preview).

[Get started](docs/getting-started.md) · [Roadmap](docs/roadmap.md) · [Architecture](docs/architecture.md) · [Privacy model](docs/privacy-model.md)

## What it does

- **Blocks matching network requests** using Chrome's Manifest V3 filtering API.
- **Hides matching page elements** with cosmetic rules, including a limited check that identifies an ad container by a child class or ID.
- **Downloads EasyList and EasyPrivacy on request**, then compiles the supported rules on your machine.
- **Accepts your own filters** separately from downloaded lists, so refreshing a subscription preserves your custom rules.
- **Lets you pick an unwanted element**, preview the matching elements, and save a site-specific cosmetic rule with immediate undo.
- **Lets you pause protection globally or for a site**, including that site's subdomains.
- **Shows page network-block counts and recent rule matches** in the unpacked extension, with a bounded activity sample stored locally for the browser session.
- **Explains compilation results**, including unsupported syntax and rules omitted because of browser limits or exception handling.

Saved rules continue working when the companion is unavailable or you cannot download fresh lists.

## How it works

NAAB has two parts with different jobs:

| Component | Responsibility |
| --- | --- |
| **Browser extension — TypeScript / Manifest V3** | Applies network rules, hides matching elements, and provides settings and diagnostics. |
| **Local companion — Rust** | Downloads selected lists, validates and compiles filters, and caches list data locally. |

The two communicate through the browser's Native Messaging interface. The native host runs on demand. Chrome or Edge performs browser filtering under its existing API limits. An optional, source-only [DNS development core](docs/dns-core.md) now runs separately for explicit local DNS queries. It does not change system DNS or proxy settings, install a service, or install certificates.

Cosmetic hiding and network blocking are different: hiding an element removes it from view but does not necessarily prevent its content from downloading.

## Privacy by design

NAAB has no telemetry, account requirement, or browsing-history upload. Filters and settings stay in your browser profile; downloaded list caches stay on your computer. Refreshes contact the public EasyList servers over HTTPS, so those servers receive normal connection information such as your IP address.

The preview's recent-activity viewer keeps up to 300 matches for the browser session. Request paths remain local; credentials, query strings, and fragments are removed.

See the [privacy model](docs/privacy-model.md) for storage locations and the boundaries of the current implementation.

## Where the project stands

This branch contains extension **0.4.2** and companion **0.2.1**, including subscriptions, local filters, the basic element picker, cosmetic hiding, site controls, page network-block counts, and the recent-activity viewer. The [browser preview guide](docs/browser-preview.md) covers the Windows and Intel Mac packages. The user reports successful 0.4.0 picker testing on an Intel Mac running Ventura 13.3.1; broader platform coverage remains limited.

Current limits include:

- Partial EasyList/EasyPrivacy compatibility; a downloaded line is not necessarily an active rule.
- Manual list refreshes and cosmetic filtering limited to the top-level page, outside embedded frames.
- No dedicated, reliable YouTube video-ad blocking.
- Click-triggered popup ad tabs can still open; top-level page navigations are not blocked.
- The basic picker uses supported class/ID selectors in the top document; changing class names can make a saved rule stop matching.
- Browser support is Chromium only. Firefox and Safari are not implemented. A source-only [Windows DNS trial](docs/windows-dns-preview.md) is available for controlled testing; everyday system-wide protection is not ready.

The [Phase 1 exit review](docs/phase-1-exit-review.md) records the completed performance follow-up. Previous DNS work produced a [local DNS core](docs/dns-core.md) and controlled platform previews. That work is paused and is not required to complete the browser milestone. Existing recovery tools and regression tests remain available; everyday system-wide DNS protection is unfinished. No proxy is planned.

## Try it or work on it

For prebuilt Windows and Intel Mac downloads, follow the **[browser preview setup guide](docs/browser-preview.md)**. It covers installation, updates that preserve settings, and removal without development tools. Packages come from successful **Browser preview** workflow runs and identify their source commit in `BUILD-INFO.json`.

For source development, use the **[build and setup guide](docs/getting-started.md)**. It also covers custom-rule syntax and a local test page.

Source installation uses **Node.js 22+, pnpm, and Rust**. Registration tooling targets **Chrome and Edge on Windows and macOS**. For an Intel Mac package without development tools, follow the [Mac preview guide](docs/macos-testing.md).

| Area | Location |
| --- | --- |
| Browser extension and interface | [`extension/`](extension/) |
| Rust companion and rule compiler | [`companion/`](companion/) |
| Native Messaging contract | [`shared/protocol/`](shared/protocol/) |
| Installation and development helpers | [`scripts/`](scripts/) |
| Integration tests and fixtures | [`tests/`](tests/) |

The [verification notes](docs/verification.md) document completed checks and remaining gaps. The [roadmap](docs/roadmap.md) describes the intended direction; it is not a list of finished features.

## Filter lists and licensing

EasyList and EasyPrivacy are maintained by **The EasyList authors** and downloaded on request, not bundled with NAAB. Their copyright and licensing information is available on the [EasyList about page](https://easylist.to/pages/about.html).

**No distribution license has been selected for NAAB itself.**
