# Install the NAAB browser preview

The package includes the browser extension and a prebuilt companion. You do not need Node.js, pnpm, Rust, or a source checkout. It installs no DNS helper or service and changes no DNS or proxy settings.

Use the **Windows x64** package on 64-bit Windows with Chrome or Edge, or the **Intel Mac** package on an Intel Mac running **macOS 12 or later** with Chrome or Edge. Apple Silicon and Safari are outside this preview. The browser must support the extension's Manifest V3 APIs; use a browser version supported on your OS. `BUILD-INFO.json` identifies the exact source commit, component versions, builder, platform checks, and package file hashes.

## Download

In the repository's **Actions** tab, open a successful **Browser preview** run. Download **NAAB-Windows-Preview-x64** or **NAAB-Mac-Preview-Intel** from its Artifacts section. Sign in to GitHub with access to the repository. These are preview build artifacts, not a store release; keep your downloaded copy. Current browser builds retain artifacts for 90 days, subject to repository policy. An expired download needs a new build of the same source or a newer successful run.

Extract the downloaded artifact ZIP. Windows: extract the inner preview ZIP. Mac: extract the inner `.tar.gz` archive, which preserves executable permissions. Move the resulting **NAAB-Windows-Preview** or **NAAB-Mac-Preview** folder into Documents before setup. Keep the whole folder in that permanent location.

Each package contains `extension/`, `companion/`, `README.md`, and `BUILD-INFO.json`. Windows also contains `Install.cmd`, `Install.ps1`, and `Uninstall.cmd`; Mac contains `Install.command`. The download's `SHA256SUMS.txt` identifies the inner archive checksum.

## Install

1. Open `chrome://extensions` or `edge://extensions` and turn on **Developer mode**.
2. Click **Load unpacked** and select the package's inner **extension** folder.
3. Copy the **32-character ID** from the NAAB extension card.
4. Windows: double-click **Install.cmd**, choose `chrome` or `edge`, and paste the ID. Mac: double-click **Install.command** and paste the ID; this defaults to Chrome. For Edge on Mac, run `bash Install.command --browser edge` in the package folder and paste the Edge ID when prompted.
5. Open NAAB → **Lists & diagnostics → Check companion**. It should report the companion as ready.
6. Select **EasyList**, **EasyPrivacy**, or both and click **Download / refresh selected**. Wait for **Saved**, then reload open pages. Alternatively, import your own supported local filters.

Run setup as your normal user. A fresh installation has zero rules until an import or download succeeds. Registration allows only the ID you supplied; each browser has its own registration and settings. A conflicting older registration is preserved: use its original package and extension ID to unregister it before switching paths or IDs.

The Windows companion is an unsigned developer preview. The Mac companion has an ad-hoc signature, not an Apple Developer ID or notarization. If macOS blocks a named package file, use its per-file approval in **System Settings → Privacy & Security**, then retry. Refer to [Apple's instructions](https://support.apple.com/en-us/102445). The installer does not launch the companion; the browser starts it on demand.

## Verify filtering

Confirm the rules summary shows saved network/cosmetic rules. On a normal website, compare global protection off and on, reloading the page each time. Try the site toggle too. Compilation diagnostics explain unsupported filters. The block count covers network requests, not cosmetically hidden elements; zero can be correct on a page without matching rules. This preview does not promise complete filter-list or video-ad coverage.

When the companion is unavailable, already saved browser rules continue filtering. New compilation and list downloads need the companion. If a save reply is lost, copy the draft before reloading NAAB from the extensions page, reopen settings, and check which rules were saved before retrying.

## Update without losing settings

1. Extract the new package into a temporary location. Quit the browser completely.
2. Replace the contents of the installed package with the new package's matching contents. Keep the **same parent folder and extension-folder path**; avoid an extra nested package folder.
3. Reopen the browser and click **Reload** on the existing NAAB extension. Do not remove and re-add it.
4. Check companion readiness and the local rules, subscription selections, site exceptions, and protection setting. `BUILD-INFO.json` identifies the new build.

Keeping the extension path preserves its ID and browser storage. Keeping the executable path preserves registration, so you normally do not run the installer again. Downloaded lists remain in the companion's per-user cache. A package update does not automatically refresh subscriptions.

## Remove

Windows: double-click **Uninstall.cmd**, choose the browser, and paste the installed ID. Mac: in the original package folder run `bash Install.command --uninstall --extension-id YOUR_ID`; add `--browser edge` for Edge.

Then remove NAAB from the browser's extensions page and delete the package folder if desired. Unregistration removes only a matching native-host registration. Removing the extension deletes its browser settings; the companion list cache remains in `%LOCALAPPDATA%\NotAnotherAdBlocker` on Windows or `~/Library/Application Support/NotAnotherAdBlocker` on Mac unless removed separately.

## Troubleshooting

- **No companion:** keep the package in place, confirm setup succeeded for this browser and ID, and retry Check companion. Use the matching architecture package.
- **Nothing blocked:** confirm rules loaded and global/site protection is enabled, then reload the page. Unsupported syntax is shown in diagnostics.
- **Different registration:** unregister the old copy using its original path and ID. The new installer refuses to replace it automatically.
- **Could not load extension:** select the extracted inner folder containing `manifest.json`.

## Verification scope

The browser-package workflow runs extension tests, browser-companion unit tests, real native-wire integration against the packaged executable, isolated installer tests, package checksums, and loaded-extension regressions. It does not run settings-changing DNS tests. The full general CI and separately opted-in DNS checks retain their existing tests; a successful browser package does not claim those separate checks passed. Finder/Gatekeeper/SmartScreen prompts and a user's existing browser profile still need the final Check companion check on that device.
