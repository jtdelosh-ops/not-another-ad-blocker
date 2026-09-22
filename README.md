# Not Another Ad Blocker

**Not Another Ad Blocker (NAAB)** is a browser privacy and content-filtering project that blocks supported advertising and tracking requests and hides unwanted page elements. It combines a **Chrome or Edge extension** with a **Rust companion running on your computer**.

The goal is filtering you can understand and control: choose your lists, add your own rules, pause protection for a site, and see which rules the software can actually support. Rule compilation and browser filtering happen locally, without an account or a cloud filtering service.

**Status: early developer preview.** NAAB is usable for testing, but filter compatibility is incomplete and Phase 1 is still in progress. A fresh installation needs a filter-list download or a local rule import before it blocks anything.

[Get started](docs/getting-started.md) · [Roadmap](docs/roadmap.md) · [Architecture](docs/architecture.md) · [Privacy model](docs/privacy-model.md)

## What it does

- **Blocks matching network requests** using Chrome's Manifest V3 filtering API.
- **Hides matching page elements** with cosmetic rules, such as a site's advertisement containers.
- **Downloads EasyList and EasyPrivacy on request**, then compiles the supported rules on your machine.
- **Accepts your own filters** separately from downloaded lists, so refreshing a subscription preserves your custom rules.
- **Lets you pause protection globally or for a site**, including that site's subdomains.
- **Explains compilation results**, including unsupported syntax and rules omitted because of browser limits or exception handling.

Saved rules continue working when the companion is unavailable or you cannot download fresh lists.

## How it works

NAAB has two parts with different jobs:

| Component | Responsibility |
| --- | --- |
| **Browser extension — TypeScript / Manifest V3** | Applies network rules, hides matching elements, and provides settings and diagnostics. |
| **Local companion — Rust** | Downloads selected lists, validates and compiles filters, and caches list data locally. |

The two communicate through the browser's Native Messaging interface. The companion runs on demand; it is not an always-running service. Chrome or Edge performs the actual filtering under its existing API limits. The current build does not route browsing traffic through the companion, change DNS or proxy settings, or install certificates.

Cosmetic hiding and network blocking are different: hiding an element removes it from view but does not necessarily prevent its content from downloading.

## Privacy by design

NAAB has no telemetry, account requirement, or browsing-history upload. Filters and settings stay in your browser profile; downloaded list caches stay on your computer. Refreshes contact the public EasyList servers over HTTPS, so those servers receive normal connection information such as your IP address.

See the [privacy model](docs/privacy-model.md) for storage locations and the boundaries of the current implementation.

## Where the project stands

The **main branch** contains extension **0.2.0** and companion **0.2.0**, including subscriptions, local filters, cosmetic hiding, and site controls.

The **[development preview](https://github.com/jtdelosh-ops/not-another-ad-blocker/pull/1)** contains extension **0.3.1** and companion **0.2.1**. It adds page network-block counts, a local recent-activity viewer, an Intel Mac package, and a limited cosmetic rule that identifies a container by a child class or ID. Those changes are under development and have not yet been merged into `main`. The [Intel Mac preview guide](https://github.com/jtdelosh-ops/not-another-ad-blocker/blob/feature/activity-viewer/docs/macos-testing.md) covers that build.

Current limits include:

- Partial EasyList/EasyPrivacy compatibility; a downloaded line is not necessarily an active rule.
- Manual list refreshes and cosmetic filtering limited to the top-level page, outside embedded frames.
- No dedicated, reliable YouTube video-ad blocking.
- No visual element picker yet.
- Chromium browsers only; Firefox, Safari, and system-wide filtering are not implemented.

The next Phase 1 work is the visual element picker, broader compatibility and performance testing, and an exit review. The longer-term vision is a local browser privacy firewall with additional enforcement options. DNS filtering and possible proxy capabilities belong to later roadmap phases, not the current product.

## Try it or work on it

Use the **[build and setup guide](docs/getting-started.md)** to build this branch, load the unpacked extension, register the companion, and download your first lists. The guide also covers updates, custom-rule syntax, a local test page, and removal.

Source installation uses **Node.js 22+, pnpm, and Rust**. Registration tooling targets **Chrome and Edge on Windows and macOS**. For the separate Intel Mac preview package, use the preview guide linked above.

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
