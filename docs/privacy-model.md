# Privacy model — extension 0.4.1 / companion 0.2.1

NAAB has no account, telemetry or cloud filtering service. Browser filtering decisions and the bounded browser activity sample stay in the browser. The native host runs as the current user when the browser launches it through Native Messaging, opens no listening socket, and exits when stdin closes. The optional DNS development executable has the separate behavior described below.

## Optional DNS development process

`naab-dns-dev` opens loopback UDP/TCP sockets only when explicitly started. It reads the selected local configuration/filter files and forwards allowed DNS questions to the configured upstream IP addresses without encryption. Those upstreams can observe query names and connection metadata. Blocked queries are answered locally. The example configuration explicitly uses Cloudflare; no automatic system-resolver discovery occurs.

The DNS cache and recent activity stay in process memory. The default sample holds 300 queries; `activityCapacity: 0` disables retention of query details. `status` prints aggregate counters; `activity` explicitly prints recent names, record types, outcomes and matched rule/source. `clear` erases recent details. Stopping the process erases the sample and cache, and there is no automatic disk log or upload. Redirecting terminal output can create a user-managed file. Unlike the browser sample, DNS activity does not identify tabs or incognito state: any client targeting this development port participates under the same policy.

The process changes no system DNS settings and installs no service or certificates. The extension neither starts it nor receives its activity yet. See [DNS core details](dns-core.md).

## What leaves the machine

Clicking **Download / refresh selected** sends ordinary HTTPS requests directly to the selected public sources:

- `https://easylist.to/easylist/easylist.txt`
- `https://easylist.to/easylist/easyprivacy.txt`

The list server can see the normal request metadata, including the client's IP address and the companion's versioned user agent. NAAB sends no browser cookies, visited-page URLs, browsing history, or local filter text in these requests. Requests use Rustls TLS validation, reject redirects, and have bounded time and size limits. There are no automatic list downloads on startup or scheduled refreshes in this build.

Building from source also contacts npm and Rust package infrastructure to retrieve development dependencies. That is separate from filtering and list refreshes.

## What stays locally

The browser's extension storage holds local filter text, compiled rules, bounded compilation diagnostics, subscription metadata, and protection settings. These are not synchronized. The `unlimitedStorage` permission accommodates compiled subscriptions and the pending-state journal used for recovery; it is not permission to upload data.

The unpacked extension records up to 300 recent network-rule matches in `chrome.storage.session`, restricted to trusted extension contexts. Entries contain an observation time, tab ID and observed tab hostname, request origin/path and resource type, matched rule ID, and a bounded compiled-rule description. Request credentials, query strings, and fragments are removed before recording; paths can still contain identifying information. Incognito tabs and requests without a normal HTTP(S) tab are excluded. Closed-tab entries are removed. **Clear all recent activity** erases the sample; extension reload/disable/update and browser restart also clear session storage. The sample survives ordinary service-worker suspension. Batched writes and startup gaps mean it is a diagnostic sample, not a complete history. There is no disk-backed native activity database or upload.

The companion stores the downloaded public list text, metadata, and immutable compiled snapshots under the user's local data directory. On Windows this is `%LOCALAPPDATA%\NotAnotherAdBlocker`; on macOS it is `~/Library/Application Support/NotAnotherAdBlocker`. The Linux companion uses `$XDG_DATA_HOME/not-another-ad-blocker` or `~/.local/share/not-another-ad-blocker`, although the installer does not yet support Linux. `NAAB_DATA_DIR` can override this location. Three completed snapshots are retained under normal operation, with a 64 MiB cap per snapshot. Snapshot hashes detect local corruption; they do not constitute a publisher signature or prove rule quality.

Local imports travel only between the extension and the local companion. Raw subscription lists are read from/written to the dedicated cache directory; native protocol callers cannot request arbitrary URLs, filenames, or paths. Fatal framing errors go to stderr without filter contents. The companion has no browsing-activity logger.

The visual picker keeps its selection and preview in the current page's content script. A short-lived permission token, tab ID, hostname, and the chosen selector stay in trusted session storage. Closing the picker or tab removes its grant; grants also expire after ten minutes and are lost on extension/browser restart. Saving sends the generated site rule, together with existing local filter text, to the local companion for compilation. It sends no page HTML or screenshot and adds no external network request. Saved picker rules remain in the local list until removed. Private tabs cannot start or save picker rules.

## Browser access and untrusted lists

HTTP/HTTPS host permissions allow DNR and cosmetics to work on visited sites. The top-document content script receives only the scoped, validated cosmetic selectors it needs. The UI gets bounded diagnostics and metadata, while large network arrays remain in the background's committed state. General settings, list imports, and native status requests are accepted only from the extension's own UI. Content scripts may request cosmetics for their sending page; picker save/undo additionally requires a live grant created by the extension UI and bound to that tab and hostname. This does not grant page scripts access to native messaging or arbitrary filter imports.

`declarativeNetRequestFeedback` lets the unpacked developer extension observe its own rule matches and read Chrome's page counter. Other extensions' blocking and cosmetically hidden elements are not counted. Detailed debug events are unavailable in packaged installations; the UI reports unavailable capabilities without inventing totals. See the official [DNR API](https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest), [action counter API](https://developer.chrome.com/docs/extensions/reference/api/action#method-getBadgeText), and [session storage documentation](https://developer.chrome.com/docs/extensions/reference/api/storage#property-session).

Downloaded rules are data and never execute JavaScript. Cosmetic CSS accepts restricted tag/class/ID compounds and one optional direct-child `:has(> compound)` check, with a class or ID required on the child. Selectors are limited to 512 bytes; nesting, other pseudo-selectors/combinators, attributes, and scriptlets remain unsupported. The top-document content script installs validated CSS without sending page contents or marker text to the companion. Unsupported syntax is reported. Unsupported exceptions can deliberately weaken subscription coverage through allow guards or omitted rules, rather than applying an exception-free broader block. Local rules and site controls remain separate.

## Removal

The registration script previews changes by default. Applying it modifies only the current user's native-host registration for the chosen browser. It installs no background service, changes no DNS/proxy settings, installs no certificate, and does not elevate the companion.

**Remove subscriptions** removes browser subscription state and rules, while preserving local rules and settings. It does not erase the native disk cache. Unregistering the companion removes its matching browser registration and manifest, but preserves the executable and cache. Removing the extension removes its browser-owned configuration and DNR rules. To erase the remaining downloaded-list cache, manually delete the dedicated directory above (or your `NAAB_DATA_DIR` override).
