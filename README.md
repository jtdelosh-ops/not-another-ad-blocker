# Not Another Ad Blocker

Version **0.2.0** adds manual EasyList and EasyPrivacy downloads to the Chromium Manifest V3 extension and Rust companion. Supported rules compile locally into browser network rules and simple cosmetic hiding. Local filters, global protection, and per-site settings remain separate and persist across updates.

This is an early developer build with partial subscription compatibility. Activity history, block counts, and the visual element picker remain unfinished, so the full Phase 1 roadmap is not complete. A new installation starts with zero rules until you download subscriptions or import local filters.

## Update an existing installation

If you already loaded the extension and registered the companion at this repository's release-binary path:

1. Open `chrome://extensions` (or `edge://extensions`) and click the extension's **Reload** button. Accept an updated permission prompt if the browser displays one. Version 0.2.0 adds `unlimitedStorage` for the local compiled-rule cache and its recovery journal.
2. Open the extension's **Lists & diagnostics** page and click **Check companion**. It should report **Local companion 0.2.0** and that EasyList/EasyPrivacy downloads are available.
3. Select **EasyList — ads**, **EasyPrivacy — trackers**, or both. Click **Download / refresh selected** and wait for the saved confirmation.
4. Review the list metadata and unsupported/safety-omission counts, then reload your open pages.

The extension folder and registered executable stay at the same paths, so you do not need to copy a new extension ID or repeat registration. Registration is needed again if the extension ID or executable path changes. If the companion still reports an older version, make sure the updated release executable is at the registered path.

Your earlier local test filters remain active alongside subscriptions. To remove only those tests, clear **Filter text** under **Local filter list** and click **Compile & replace local rules**. This leaves downloaded subscriptions and site settings intact.

## Build and install from source

Install Node.js 22 or newer, pnpm, and a stable Rust toolchain. Windows Rust normally uses the MSVC C++ build prerequisites; a configured GNU toolchain also works. See [Rust installation](https://www.rust-lang.org/tools/install/).

From this repository:

```sh
cd extension
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
pnpm build
cd ..
cargo test --locked --manifest-path companion/Cargo.toml
cargo build --release --locked --manifest-path companion/Cargo.toml
```

Load `extension/dist` using **Load unpacked** in `chrome://extensions` or `edge://extensions`, with Developer mode enabled. Keep the directory in place and copy its 32-character extension ID.

Register the companion from the repository root. Registration is per user and browser, uses the exact extension ID and executable path, and does not need an administrator account. The script previews its changes unless `--apply` is supplied.

Windows, PowerShell:

```powershell
node scripts/native-host.mjs --extension-id YOUR_EXTENSION_ID --binary companion/target/release/naab-companion.exe
node scripts/native-host.mjs --extension-id YOUR_EXTENSION_ID --binary companion/target/release/naab-companion.exe --apply
```

macOS:

```sh
node scripts/native-host.mjs --extension-id YOUR_EXTENSION_ID --binary companion/target/release/naab-companion --apply
```

Add `--browser edge` for Microsoft Edge. Chrome is the default. macOS registration paths are implemented but still need verification on macOS hardware. The installer does not yet support other operating systems or browsers.

Open **Lists & diagnostics**, check the companion, and download your selected subscriptions. The bundled `.test` demo remains useful for isolated filtering tests, but its reserved domains are not an everyday ad-blocking list.

## Visual check without live advertising

From the repository root, run `node scripts/test-page.mjs` (or `powershell -File scripts/test-page.ps1` on Windows). Keep that terminal open, then copy `http://127.0.0.1:8765/` into the Chrome profile where NAAB is installed. The PowerShell launcher can use the bundled Node runtime when Node is not on PATH.

With Privacy Badger and other blockers off, switch **Global protection** off in NAAB's **Lists & diagnostics** and reload the test page: both sample banners should appear. Switch Global protection on with EasyList installed and reload: the normal banner should remain, while the ad-pattern image is blocked. The page checks its image endpoints independently and reports inconclusive results when the control checks fail. It does not change your saved rules or browser settings.

This verifies one actual EasyList image rule, not overall blocking coverage or cosmetic hiding. Both harmless images come from the same loopback-only server; no ad network is contacted. The server serves only its fixed page and images, and stops with Ctrl+C. `--port NUMBER` selects another port if needed. Future EasyList changes may require updating the test path.

## Subscription behavior and coverage

Refreshes run only when requested. The companion downloads the selected lists directly from [EasyList](https://easylist.to/easylist/easylist.txt) and [EasyPrivacy](https://easylist.to/easylist/easyprivacy.txt) over HTTPS, validates them, and compiles their supported subset. No account, telemetry, browsing-history upload, or remote filtering service is involved. Saved browser rules keep working when the companion or Internet connection is unavailable.

Subscription network support includes ASCII URL patterns with Adblock-style anchors and wildcards, common resource-type restrictions, first/third-party restrictions, positive and negative domain scopes, case matching, network exceptions, document exceptions, and supported `badfilter` cancellation. Top-level navigations are never blocked. Equivalent full-host filters can share a DNR rule with up to 1,000 domains; source-line counts and installed browser-rule counts therefore differ.

The browser's capacity is reserved for up to 2,000 local network rules and 200 site overrides before allocating subscription rules. Supported allow exceptions are kept before selecting blocks within the remaining budget. The UI reports unsupported lines, capacity omissions, and conservative safety omissions. Unsupported exception syntax can produce broader allowance guards or omit related blocking/hiding rules to reduce breakage; this lowers filtering coverage.

Cosmetic support remains limited to compound tag/class/ID selectors, such as `.advertisement`, `div.sidebar-ad`, and `#sponsor`, with positive/negative domain scopes and supported `#@#` exceptions. Cosmetics apply only to the top document. Complex selectors, attributes, combinators, pseudo-selectors, scriptlets, and procedural rules are unsupported. If a generic-hiding exception cannot be safely scoped, generic subscription cosmetics are suppressed conservatively; your local generic cosmetic rules still work. Unsupported document/hiding exceptions can also reduce cosmetic coverage.

There is no claim of complete EasyList, EasyPrivacy, Adblock Plus, or uBlock Origin compatibility. Refresh the lists and read the reported coverage rather than treating every downloaded line as an active rule.

## Local rules and site controls

Local imports retain their original, narrower syntax:

```text
||ads.example.test^
||tracker.example.test^$third-party
@@||ads.example.test/allowed.js
example.test##.advertisement
##.naab-demo-ad
```

A local network pattern must use a domain anchor ending in `^` or followed by a literal path; `@@` exceptions and `$third-party` are supported. Local cosmetics accept the same restricted compound selector grammar with optional positive domains. Local imports do not support the broader subscription modifiers or cosmetic exceptions. Limits remain 128 KiB UTF-8 text, 2,000 nonempty lines, and 2,048 bytes per line. Import replaces only the previous local list.

Local network blocks/allows use priorities 3/4, above subscription priorities 1/2. Site protection overrides use priority 100. A paused hostname also pauses its subdomains; remove a parent exception before re-enabling a child. Global-off pauses all protection. Reload pages after changing rules or site controls.

## Storage, failures, and removal

The browser stores local filters, compiled subscriptions, settings, and a recovery journal locally, using `unlimitedStorage`; it does not sync them. The companion keeps raw downloaded lists, metadata, and immutable paged snapshots in the current user's local data directory:

- Windows: `%LOCALAPPDATA%\NotAnotherAdBlocker`.
- macOS: `~/Library/Application Support/NotAnotherAdBlocker`.
- Linux companion cache: `$XDG_DATA_HOME/not-another-ad-blocker`, or `~/.local/share/not-another-ad-blocker` when unset. This does not add Linux installer support.

Each list is limited to 8 MiB and 300,000 lines, with a 25-second download timeout per source. The companion retains three completed snapshots, each capped at 64 MiB. `NAAB_DATA_DIR` can override the cache location for tests or portable use.

A failed download, validation, or rule update keeps the previously committed browser configuration. The controller also rolls back DNR if saving settings fails; a failed rollback requires reloading the extension and is reported explicitly. Cache fetch time and browser application time are recorded separately.

**Remove subscriptions** removes browser subscription rules and metadata while preserving local rules and protection settings. It does not delete the companion's disk cache. To unregister the native host, repeat its registration command with `--uninstall --apply`; the script refuses to remove a different registration. Removing the extension removes its browser-owned settings and rules. For a complete uninstall, manually delete the companion cache directory listed above as well as any executable you no longer need. No DNS, proxy, or certificate settings need restoration.

## Verification and remaining work

After building the extension and debug companion:

```sh
cargo build --locked --manifest-path companion/Cargo.toml
node --test tests/installer.test.mjs tests/native-integration.test.mjs
```

Native integration tests use the extension's actual client with framed stdin/stdout transport to the Rust executable. See [verification notes](docs/verification.md) for completed checks and remaining platform limitations.

GitHub Actions runs the deterministic extension, Rust, installer, and native integration checks on Windows and macOS for pull requests and pushes to `main`. CI uses Node.js 22, pnpm 11.19.0, and Rust 1.98.1 with locked dependencies; the required job names are `Test (windows-latest)` and `Test (macos-latest)`. Live list downloads and browser smoke tests remain separate checks, so CI does not verify browser registration or advertising availability.

Next work covers activity diagnostics and honest block counts; the visual element picker with preview/undo; more compatibility and performance tests; macOS installation verification; and the Phase 1 exit review. Phase 2 DNS work remains gated on Phase 1 completion.

EasyList and EasyPrivacy are maintained by **The EasyList authors** and are downloaded on request, not bundled into this repository. Their copyright and dual-license details are on the official [EasyList about page](https://easylist.to/pages/about.html). No distribution license has been selected for NAAB itself.

Further details: [architecture](docs/architecture.md), [privacy model](docs/privacy-model.md), [protocol](shared/protocol/README.md), and the unchanged [source roadmap](docs/roadmap.md).
