# NAAB Mac preview — Chrome on Intel

This package contains extension 0.3.0 and the Intel Mac companion 0.2.0. You do not need Node, Rust, Homebrew, or Xcode to install it. The companion downloads and compiles lists; Chrome performs the filtering.

Use an Intel Mac and a current version of Chrome. Google's current Chrome releases require macOS 13 Ventura or newer; Ventura 13.3.1 meets that requirement. The registration helper requires macOS 12 or newer. This is a developer preview, signed locally for execution but not signed with an Apple Developer ID or notarized. The first installation on your own Mac still needs verification.

## 1. Download and put the folder somewhere permanent

On your Mac, sign in to GitHub with the account that can access the private NAAB repository. Open the supplied successful Mac preview build, scroll to **Artifacts**, and download **NAAB-Mac-Preview-Intel**.

1. Double-click the downloaded artifact ZIP to extract it.
2. Inside it, double-click the `.tar.gz` archive to extract **NAAB-Mac-Preview**. The extra archive preserves executable permissions.
3. Move **NAAB-Mac-Preview** into your **Documents** folder before continuing.

The extracted folder should contain:

```text
NAAB-Mac-Preview/
  extension/
    manifest.json
    ...
  companion/
    naab-companion
  Install.command
  README.md
  BUILD-INFO.json
```

Keep this folder in place. Moving or deleting it after installation breaks the unpacked extension or companion registration. `BUILD-INFO.json` records the source commit and package file hashes; the download also includes an archive SHA-256 checksum. GitHub build artifacts expire after seven days; keep your downloaded copy.

## 2. Load the extension in Chrome

1. Open Chrome on the Mac and enter `chrome://extensions` in the address bar.
2. Turn on **Developer mode** at the upper right.
3. Click **Load unpacked**.
4. Select **Documents → NAAB-Mac-Preview → extension**. Select this inner folder, not the whole package.
5. Find **Not Another Ad Blocker — Local Preview** and confirm version **0.3.0**.
6. Copy the **32-character ID** shown on its card. Use the ID displayed on this Mac; it can differ from your Windows ID.

## 3. Connect the Mac companion

1. In Finder, open **Documents → NAAB-Mac-Preview**.
2. Double-click **Install.command**. A Terminal window opens.
3. When it asks for the extension ID, paste the ID you just copied and press **Return** once.
4. Wait for **Registered NAAB for Chrome** (or **Already registered for Chrome**). You can then close that Terminal window.

The helper writes only your user's Chrome Native Messaging registration. It does not install a background service or require `sudo`. It refuses to replace a different existing NAAB registration.

If macOS blocks `Install.command` or `naab-companion` because the developer cannot be verified, first attempt the indicated operation, then open **System Settings → Privacy & Security** and use **Open Anyway** for that named file. Retry the installer or Chrome's companion check afterward. This package is an unnotarized development build; Apple's per-file approval may be needed. See [Apple's instructions](https://support.apple.com/en-us/102445).

## 4. Download rules and verify the connection

1. Click Chrome's extensions puzzle-piece icon and open **Not Another Ad Blocker**. Pin it if you want the icon visible on the toolbar.
2. Click **Lists & diagnostics**, then **Check companion**.
3. Confirm **Local companion 0.2.0** is reported as ready.
4. Select **EasyList — ads** and **EasyPrivacy — trackers**.
5. Click **Download / refresh selected** and wait for the saved confirmation.
6. Reload the pages you want to test.

A fresh Mac installation starts with zero rules. Loading the extension alone is not enough; the successful download in this step supplies its filtering rules. Your Windows settings and lists are not automatically synchronized to the Mac.

Open the NAAB popup on a normal website to see **Network requests blocked on this page**. Click **Recent activity** to inspect matched network rules and use **Refresh** to update the view. **Clear all recent activity** clears the sample without resetting the browser's page count. The session log keeps at most 300 recent matches and excludes private/incognito tabs. Counts exclude cosmetically hidden elements.

For a useful comparison, temporarily turn off Privacy Badger and any other content blockers in the test Chrome profile. Compare the same page after reloading with NAAB's global protection off and then on. A nonzero count confirms network blocking; it does not establish that every video advertisement is removed.

## Troubleshooting

- **Could not load extension:** choose the inner `extension` folder containing `manifest.json`, and keep it extracted in its permanent location.
- **Native host not found:** confirm the helper completed and you pasted this Mac's ID. This helper defaults to ordinary Google Chrome, not Chrome Beta/Canary or another browser.
- **Native host exited / companion unavailable:** check for the macOS approval described above. Confirm the package is the Intel build and its folder has not moved. Quit and reopen Chrome, then retry **Check companion**.
- **A different registration already exists:** use the original package and extension ID with the uninstall command below before registering a different copy. The helper leaves the existing registration untouched.
- **Zero blocks:** first confirm lists were downloaded, global/site protection is enabled, and the page was reloaded. Zero can be a correct result on a page without matching requests.

To repeat installation without an interactive ID prompt, open Terminal and use your actual Mac ID in place of `YOUR_EXTENSION_ID`:

```sh
cd "$HOME/Documents/NAAB-Mac-Preview"
bash Install.command --extension-id YOUR_EXTENSION_ID
```

Supply the ID either in that command or at the helper's prompt; it is not needed twice.

## Removal

From the same package folder, unregister only the matching companion:

```sh
bash Install.command --uninstall --extension-id YOUR_EXTENSION_ID
```

Then remove NAAB from `chrome://extensions`. You can delete the preview folder afterward. The downloaded-list cache remains at `~/Library/Application Support/NotAnotherAdBlocker` unless you delete it separately.

## Build and verification scope

The Mac preview workflow builds the Intel release companion and extension, runs native process/integration and isolated registration tests on an Intel macOS runner, checks the binary architecture/signature, and packages the result. This does not prove Finder/Gatekeeper prompts or native-host discovery in your installed Chrome profile; the **Check companion** step is that final device check.

The extension is Chromium-only; this package does not install a Safari extension. Detailed activity logging uses Chrome's unpacked-extension feedback API. The visual element picker remains unfinished.

References: [Chrome installation requirements](https://support.google.com/chrome/answer/95346?hl=en), [loading an unpacked extension](https://developer.chrome.com/docs/extensions/get-started/tutorial/hello-world#load-unpacked), and [Chrome Native Messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging).
