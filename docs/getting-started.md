# Build, setup, and filtering reference

This guide covers the version on this branch. Start with the [project overview](../README.md) for NAAB's purpose and development status.

For a first installation, start with [Build and install from source](#build-and-install-from-source), or use the [Intel Mac preview guide](macos-testing.md) for the packaged build.

## Update an existing installation

If you already loaded the extension and registered the companion at this repository's release-binary path:

1. Build or replace both the extension and companion at their existing paths. For the downloaded Mac package, follow [Upgrade an existing Mac preview](macos-testing.md#upgrade-an-existing-mac-preview).
2. Open `chrome://extensions` (or `edge://extensions`), click the existing extension's **Reload** button, and confirm version **0.4.0**. Accept a permission prompt if upgrading from a version before 0.3.0, which added `declarativeNetRequestFeedback` for network counts and local debug activity.
3. Open **Lists & diagnostics**, click **Check companion**, and confirm **0.2.1** before compiling the new cosmetic syntax.
4. Reload an HTTP(S) page and open the NAAB popup. Its network block count and **Recent activity** remain separate from cosmetic hiding. Clearing the activity sample does not reset the browser's page counter.

Existing downloaded lists and protection settings remain saved. Both updated components are needed for the new cosmetic syntax; a new list download is not required to compile a local rule.

The extension folder and registered executable stay at the same paths, so you do not need to copy a new extension ID or repeat registration. Registration is needed again if the extension ID or executable path changes. If the companion still reports an older version, make sure the updated release executable is at the registered path.

Your earlier local test filters remain active alongside subscriptions. Edit only the unwanted test lines under **Local filter list**, retain your other filters, then click **Compile & replace local rules**. This leaves downloaded subscriptions and site settings intact.

## Build and install from source

For an Intel Mac test without installing development tools, use the [Mac preview setup guide](macos-testing.md) and the matching private GitHub Actions artifact. It includes the built extension, Mac companion, and a current-user registration helper.

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

Add `--browser edge` for Microsoft Edge. Chrome is the default. The packaged Intel Mac helper has been tested with Chrome on Ventura 13.3.1; broader platform and browser installation coverage remains in progress. The installer does not yet support other operating systems or browsers.

Open **Lists & diagnostics**, check the companion, and download your selected subscriptions. The bundled `.test` demo remains useful for isolated filtering tests, but its reserved domains are not an everyday ad-blocking list.

## Block an element with the picker

1. Open a normal website with NAAB protection enabled.
2. Open the NAAB popup and click **Block something on this page**.
3. Point at the unwanted element and click. The panel shows the site rule and how many elements match it.
4. Click **Preview**. Use **Restore preview**, **Select parent**, or **Pick another** to adjust the selection. **Cancel** or **Escape** restores the temporary preview without saving.
5. Click **Save rule** to append it to your local filters. The companion must be available to compile the rule. **Undo saved rule** removes that specific addition while the panel remains open; **Done** closes the panel.

For later removal, delete the corresponding rule and its `! NAAB picker` comment under **Local filter list**, retaining other rules, then compile. Rules apply to the selected hostname and its subdomains. They hide content visually and do not increment the network counter. Existing local filters, subscriptions, and site exceptions are preserved.

This basic picker supports the existing restricted class/ID selector grammar. It cannot pick inside embedded frames or a component's shadow tree. Elements without a supported selector require choosing a parent or another element. Class changes can break saved rules; broad selector generation and automatic network correlation are deferred. Private tabs are not supported. Picker permissions expire after ten minutes; restart from the popup if the session expires. Reload existing pages after updating the extension so they receive the new content script.

## Visual check without live advertising

From the repository root, run `node scripts/test-page.mjs` (or `powershell -File scripts/test-page.ps1` on Windows). Keep that terminal open, then copy `http://127.0.0.1:8765/` into the Chrome profile where NAAB is installed. The PowerShell launcher can use the bundled Node runtime when Node is not on PATH.

With Privacy Badger and other blockers off, switch **Global protection** off in NAAB's **Lists & diagnostics** and reload the test page: both sample banners should appear. Switch Global protection on with EasyList installed and reload: the normal banner should remain, while the ad-pattern image is blocked. The page checks its image endpoints independently and reports inconclusive results when the control checks fail. It does not change your saved rules or browser settings.

This verifies one actual EasyList image rule, not overall blocking coverage or cosmetic hiding. Both harmless images come from the same loopback-only server; no ad network is contacted. The server serves only its fixed page and images, and stops with Ctrl+C. `--port NUMBER` selects another port if needed. Future EasyList changes may require updating the test path.

## Subscription behavior and coverage

Refreshes run only when requested. The companion downloads the selected lists directly from [EasyList](https://easylist.to/easylist/easylist.txt) and [EasyPrivacy](https://easylist.to/easylist/easyprivacy.txt) over HTTPS, validates them, and compiles their supported subset. No account, telemetry, browsing-history upload, or remote filtering service is involved. Saved browser rules keep working when the companion or Internet connection is unavailable.

Subscription network support includes ASCII URL patterns with Adblock-style anchors and wildcards, common resource-type restrictions, first/third-party restrictions, positive and negative domain scopes, case matching, network exceptions, document exceptions, and supported `badfilter` cancellation. Top-level navigations are never blocked. Equivalent full-host filters can share a DNR rule with up to 1,000 domains; source-line counts and installed browser-rule counts therefore differ.

The browser's capacity is reserved for up to 2,000 local network rules and 200 site overrides before allocating subscription rules. Supported allow exceptions are kept before selecting blocks within the remaining budget. The UI reports unsupported lines, capacity omissions, and conservative safety omissions. Unsupported exception syntax can produce broader allowance guards or omit related blocking/hiding rules to reduce breakage; this lowers filtering coverage.

Cosmetics support ASCII tag/class/ID compounds, such as `.advertisement`, `div.sidebar-ad`, and `#sponsor`, plus one direct-child check: `div:has(> .ad-label)`. The parent and child must each be compounds, the child must include a class or ID, and the whole selector is limited to 512 bytes. Nesting, attributes, other pseudo-selectors/combinators, scriptlets, and procedural rules remain unsupported. Positive/negative domain scopes and supported `#@#` exceptions apply to subscriptions. Cosmetics affect only the top document. If a generic-hiding exception cannot be safely scoped, generic subscription cosmetics are suppressed conservatively; your local generic cosmetic rules still work. Unsupported document/hiding exceptions can also reduce cosmetic coverage.

There is no claim of complete EasyList, EasyPrivacy, Adblock Plus, or uBlock Origin compatibility. Refresh the lists and read the reported coverage rather than treating every downloaded line as an active rule.

## Local rules and site controls

Local imports retain their original, narrower syntax:

```text
||ads.example.test^
||tracker.example.test^$third-party
@@||ads.example.test/allowed.js
example.test##.advertisement
example.test##div:has(> .ad-label)
##.naab-demo-ad
```

A local network pattern must use a domain anchor ending in `^` or followed by a literal path; `@@` exceptions and `$third-party` are supported. Local cosmetics accept the same restricted compound/direct-child selector grammar with optional positive domains. Local imports do not support the broader subscription modifiers or cosmetic exceptions. Limits remain 128 KiB UTF-8 text, 2,000 nonempty lines, and 2,048 bytes per line. Import replaces only the previous local list.

The manual rule for the reported rotating-class banner is `pornhub.com##div:has(> .t-j-inbanlabel-container)`. It is not installed automatically. Replace only the obsolete local test rule for that banner and retain other filters. This hides a `div` with that direct-child marker even when the parent class changes; it depends on the marker remaining present and applies only to the top document. Automated verification uses an isolated fixture. The [Mac upgrade guide](macos-testing.md#upgrade-an-existing-mac-preview) includes the edit steps.

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

Native integration tests use the extension's actual client with framed stdin/stdout transport to the Rust executable. See [verification notes](verification.md) for completed checks and remaining platform limitations.

GitHub Actions runs the deterministic extension, Rust, installer, and native integration checks on Windows and macOS for pull requests and pushes to `main`. CI uses Node.js 22, pnpm 11.19.0, and Rust 1.98.1 with locked dependencies; the jobs are named `Test (windows-latest)` and `Test (macos-latest)`. Live list downloads and browser smoke tests remain separate checks, so CI does not verify browser registration or advertising availability.

Next work covers more compatibility and performance tests, broader macOS installation verification, and the Phase 1 exit review. Phase 2 DNS work remains gated on Phase 1 completion.

EasyList and EasyPrivacy are maintained by **The EasyList authors** and are downloaded on request, not bundled into this repository. Their copyright and dual-license details are on the official [EasyList about page](https://easylist.to/pages/about.html). No distribution license has been selected for NAAB itself.

Further details: [architecture](architecture.md), [privacy model](privacy-model.md), [protocol](../shared/protocol/README.md), and the unchanged [source roadmap](roadmap.md).
