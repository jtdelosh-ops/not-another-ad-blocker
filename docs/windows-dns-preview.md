# Windows DNS preview

NAAB can now run a local DNS resolver on the standard DNS port and temporarily route **one Windows adapter** through it. A separate administrator helper saves the original settings, checks the resolver, and restores DNS when the trial ends or the resolver fails. The browser extension and native host continue to operate independently.

This is a **source-only, controlled-test preview**, not an everyday on/off switch. A controlled Hyper-V guest has passed the initial activation and recovery checks described below; the full platform test gate remains open. The opt-in scheduled recovery task has passed controlled forced-helper-exit and reboot checks in that guest. Do not begin with your everyday connection. No system settings are changed by building, inspecting, planning or starting the resolver; only `trial --apply`, `recover`, and a scheduled `recover-if-needed` with a pending record can change DNS.

## What is supported

- Windows 10/11 with Windows PowerShell 5.1 and the built-in NetAdapter, NetConnection and DnsClient modules.
- One connected adapter reported by Windows as physical, with one non-domain network profile; enabled IPv4 and IPv6 families are captured separately.
- Automatic DNS, or one to eight ordinary static DNS addresses per family, preserving their order.
- A normal-user resolver on `127.0.0.1:53` and `[::1]:53`, using both UDP and TCP.
- A separate, manually launched administrator helper, with a 30–300 second trial and offline recovery. An optional SYSTEM scheduled task can independently retry offline recovery after helper exit or reboot.

The preview refuses detected multiple DNS adapters, multiple network profiles, domain/NRPT policies, global DNS overrides, per-network DNS overrides, existing loopback DNS, scoped IPv6 DNS addresses, and ambiguous static/effective settings. VPNs, managed/enterprise networks, unusual encrypted-DNS policies, and network switching remain unsupported; detection is conservative, not a complete inventory of every third-party network product. A VM must also pass the inspector's adapter checks; there is no force/bypass option.

## Safe checks first

In a **normal PowerShell**, from the repository root:

```powershell
cargo build --locked --manifest-path companion/Cargo.toml --bin naab-dns-windows
.\companion\target\debug\naab-dns-windows.exe inspect
```

`inspect` prints the adapter name, stable `id`, provisional `eligible` flag and profile counts. If `eligible` is false, stop there: this network layout is outside the preview's supported scope. Do not disable unrelated adapters merely to make the flag pass.

For an eligible adapter, copy its ID into this **read-only** command:

```powershell
.\companion\target\debug\naab-dns-windows.exe plan --adapter YOUR_ADAPTER_ID
```

`plan` performs the full snapshot checks and shows the DNS mode and ordered servers that would be saved. It does not save a journal or change DNS. An error here explains why activation would be refused.

## Controlled trial: two windows

Before a live test, keep this guide and the compiled executable available locally. Use an eligible spare machine or VM with console access and a known working network. Have the original DNS settings available independently of NAAB's backup. Close VPN software and other DNS tools rather than testing competing settings writers together initially.

**Window 1 — normal PowerShell:** start the resolver:

```powershell
.\companion\target\debug\naab-dns-windows.exe serve --config companion/examples/dns-dev.json
```

This uses the existing sample rules, with standard port 53 replacing the configuration's development listener. All four loopback sockets must bind; an occupied port causes failure. Leave the window open. The command prints a trial command containing a fresh resolver token and the configured upstream addresses. A token from an earlier resolver process will not pass the health check.

**Window 2 — Administrator PowerShell:** navigate to the same repository root, paste that printed command and replace `ADAPTER_GUID` with the inspected ID. Its structure is:

```text
naab-dns-windows.exe trial --adapter GUID --token TOKEN --upstreams IP:PORT,IP:PORT --seconds 300 --apply
```

Wait for `Enable: Active (Enabled)` before testing. The helper saves a recovery record before changing settings and reads back each change. It refuses a second helper or an unresolved prior session. Local and upstream health must pass both before and immediately after activation.

Press **Ctrl+C in Window 2** to restore DNS early, or let the timer expire. Wait for `Inactive` and `Restored`/`AlreadyOriginal` for the saved families. Then stop Window 1. The deadline includes activation time, but an in-flight command/check and restoration can finish after the requested duration.

The helper prints `Restoring saved DNS settings; please wait for the final status.` when it leaves the monitoring loop. Its console handler stays installed throughout restoration, including repeated Ctrl+C or Ctrl+Break events. Closing the window or using `Stop-Process` still forcibly interrupts cleanup; those actions can require offline recovery. Run the executable directly in the helper window, without piping its output through another command.

## What recovery does

| Event | Preview behavior |
| --- | --- |
| Resolver exits, hangs or loses a required listener | Three failed local samples trigger restoration while the separate helper is running |
| Upstreams fail but the local resolver still answers | Report `Degraded / UpstreamUnavailable`; retain the settings until local failure, manual stop or the trial deadline |
| Detected unsupported adapter/profile/policy change | Attempt restoration; preserve external edits and unresolved targets |
| Ctrl+C in the helper or trial deadline | Restore and verify the saved DNS modes/addresses |
| Helper forcibly killed, terminal closed, reboot or power loss | Without the optional recovery task, run the offline command below. With the task, startup and one-minute retries attempt offline restoration; forced-helper-exit and reboot passed in one controlled VM. An unavailable or changed network profile can still prevent restoration until it returns. |

Local health uses fresh TXT queries and a per-resolver token, checks UDP and TCP for each selected family, and bypasses filtering/cache/upstreams. A separate direct query to the configured upstreams tests reachability without consulting the OS resolver or NAAB cache. Samples are not a precise stopwatch: Windows discovery/commands have bounded waits (up to 20 seconds each), plus DNS deadlines and a two-second pause between polls.

Windows does not provide atomic compare-and-set for the cmdlet used here. The adapter rechecks identity and expected DNS immediately before each write, and the controller verifies afterward. This reduces races; it cannot make a concurrent VPN or administrator edit atomic. A timed-out Windows command poisons that helper instance so it will retain the backup rather than race another write; retry recovery after the outstanding OS operation settles.

The bridge contains PowerShell and native children in a Windows Job Object before supplying input. On exit or timeout it closes that process tree before waiting for pipe readers, so an inherited output pipe cannot keep the controller's recovery lock indefinitely. The controller also reads every saved target again after all restore writes, before clearing the journal; a later family write cannot silently invalidate an earlier successful read-back.

## Offline emergency recovery

In an **Administrator PowerShell**, from the repository root:

```powershell
.\companion\target\debug\naab-dns-windows.exe recover
```

This needs neither a running resolver nor working internet. Use the built executable so recovery does not depend on Cargo downloading anything. Stop a still-running helper with Ctrl+C first; its journal lock prevents competing writers.

Recovery restores only settings that still match NAAB's applied loopback value on the same machine, adapter and network profile. External edits are preserved. A disconnected adapter or different network profile leaves a retained record; reconnect the original network and retry. If a conflict persists, reconcile it against the saved original configuration manually. Do not delete the backup or blindly reset every adapter to automatic DNS.

## Optional independent recovery task

Install this only in the controlled VM after building the updated Windows helper. In its Administrator PowerShell, from the repository root (or pass `-SourceBinary` for the copied executable):

```powershell
PowerShell -NoProfile -ExecutionPolicy Bypass -File scripts/windows-dns-recovery-task.ps1 -Action Install
PowerShell -NoProfile -ExecutionPolicy Bypass -File scripts/windows-dns-recovery-task.ps1 -Action Status
```

Installation copies the helper into the protected recovery directory, validates the copied file and registers a SYSTEM task that runs at startup and once per minute. The task calls `recover-if-needed`: it does nothing when there is no recovery record, and otherwise uses the same offline, identity-checked restoration as manual `recover`. It needs neither the resolver nor internet. While a trial holds the journal lock, a task attempt cannot make a competing settings write; a later attempt retries after the helper exits. If the adapter or network profile is unavailable at boot, the record is retained for later retry. `Status` must report `Installed: True`; an absent, disabled or altered task is not protection.

Task Scheduler, the installed helper and the protected recovery directory must remain intact. A task failure, locked/hung helper, corrupt backup, changed DNS or network identity can still require manual investigation. This does not add a permanent DNS resolver service or turn the preview into a daily-use toggle. Controlled forced-exit and reboot checks passed in the test VM; broader configurations and lifecycle cases remain open.

Uninstall only after the trial has finished and no recovery record remains:

```powershell
PowerShell -NoProfile -ExecutionPolicy Bypass -File scripts/windows-dns-recovery-task.ps1 -Action Uninstall
```

Uninstall checks offline recovery first and refuses to remove the task while restoration is incomplete. The task uses a separate protected copy of the executable so moving the development checkout cannot disable recovery.

The fixed recovery directory is `%ProgramData%\NAAB-DNS-Preview`, restricted to Administrators and SYSTEM. There is no caller-selected privileged storage path. It contains:

- `recovery.json`: original settings until all restoration is verified.
- `last-report.json`: latest state and per-family restoration outcomes.
- `last-incident.json`: latest failure, preserved after later successful cleanup.
- `controller.lock`: OS lock coordination; the file's presence alone does not mean it is locked.

Windows errors are also printed in the helper terminal. Reports stay local, are bounded, and contain no browsing history. The saved settings are flushed before application, and Windows file replacement uses write-through. Power-loss recovery still requires real platform testing; missing/corrupt backups are never replaced with guessed settings.

## Test gate and remaining work

Automated tests exercise the shared controller's rollback/failure cases, the actual PowerShell dispatch with isolated cmdlet doubles, mode/order/family preservation, conflict/network rejection, strict CLI inputs, and real loopback UDP/TCP health probes. The real read-only inspector was run on the development PC; it correctly reported that the current profile layout was ineligible. These checks do **not** establish live Windows rollback.

The Windows console regression suite launches hidden, isolated Windows PowerShell 5.1 consoles and sends actual console events to the production trial loop. It uses simulated DNS and temporary journals, never host DNS settings. It covers one Ctrl+C, another during slow restoration, cancellation during activation, restoration failure retaining its journal, Ctrl+Break, timeout and panic cleanup. Run it without starting a resolver or changing DNS:

```powershell
cargo test --locked --manifest-path companion/Cargo.toml --test windows_console
```

**Controlled guest evidence (2026-09-29):** user-observed Windows checks confirmed automatic IPv4/IPv6 activation, the default-resolver TXT token and ordinary forwarding, timed restoration, resolver-failure restoration, and offline recovery. Windows showed the original effective DNS again and no outstanding recovery record after cleanup. The guest's earlier Ctrl+C tests left unfinished sessions. An isolated reproducer confirmed that the old one-shot listener allowed a second Ctrl+C to terminate slow cleanup; the updated persistent handler passes the automated console suite. The user reported that the subsequent guarded guest Ctrl+C test with the updated binary passed. The user also reported that the guarded failed-upstream VM test passed: it checks `Degraded / UpstreamUnavailable` while local DNS remains healthy, then verifies restoration at the deadline. The user supplied a screenshot of the independent recovery task reporting `Installed: True`, `ProtectedBinaryPresent: True`, `RecoveryPending: False`, followed by a guarded forced-helper-exit test reporting `PASS` after scheduled recovery restored original DNS. The user then reported that the two-phase VM reboot recovery check passed. The guest result JSON files were not independently read back by the developer environment.

The first live static/mixed case **failed**. The journal saved static IPv4 `172.19.144.1` and automatic IPv6, but recovery reported IPv4 `Conflict`; Windows then showed automatic IPv4 with the same effective server. The adapter GUID remained the same while its interface index changed from 20 to 5. The guest's scheduled recovery subsequently cleared the journal after the saved static setting was reapplied; an explicit `recover` returned `Inactive (NotActive)`. The user ran the command to return IPv4 to automatic DNS without an error, but a final read-back was not obtained. The static VM script now resolves the adapter index by GUID for each read and saves its final modes, addresses, and report.

The repeat static/mixed run failed identically while the adapter index stayed 5. Its final snapshot confirmed automatic IPv4 and IPv6, and a guarded cleanup script subsequently verified the original automatic settings and a cleared journal. A separate VM-only experiment then reproduced the cause: piping the IPv6 CIM object into `Set-DnsClientServerAddress -ResetServerAddresses` also reset static IPv4 to automatic on this Windows 10 guest. The helper now uses Windows' family-specific `netsh interface ipv4|ipv6 set dnsservers ... source=dhcp` for automatic restoration. The isolated fixture, companion test suite, and controlled guest test passed.

For this particular guest, `scripts/test-windows-dns-updated-vm.ps1` consolidates the check into one invocation: it verifies the reviewed bundle hashes and automatic baseline, runs the family-reset experiment, updates the protected recovery helper with rollback if the update fails, then runs both static IPv4 cases. It binds itself to the known guest and adapter; it is not a general installer. The existing resolver may be reused. The helper update retains the original scheduled task, and the trial refuses a pending recovery record. An initial invocation stopped in preflight because Windows PowerShell 5.1 wrapped the parsed plan array; the corrected wrapper passed a direct PowerShell 5.1 regression check before being copied to the guest.

**Controlled guest result (2026-09-30):** the user-supplied output from the corrected wrapper reported `NOT REPRODUCED` for the earlier cross-family reset: resetting IPv6 with `netsh` preserved static IPv4. It then reported `PASS` for the protected helper update and both 60-second guarded trials: one static IPv4 server and two ordered static IPv4 servers, each with automatic IPv6. Both trial reports marked IPv4 and IPv6 `restored`. The final snapshot showed automatic IPv4 with `172.19.144.1`, automatic IPv6 with no servers, `dnsRestored: true`, `journalCleared: true`, and `manualRecoveryNeeded: false`. The guest result JSON was not independently read back by the developer environment. This validates these controlled static/mixed cases; broader Windows configurations remain open.

An opt-in test of all four standard-port listeners and their cleanup passed outside the development sandbox. The same test failed inside the sandbox. A normal-user CLI subprocess smoke test was also blocked in that restricted environment; later guest testing confirmed normal-user resolver startup and queries as described above. Running the production resolver CLI elevated is deliberately rejected. To repeat the isolated listener test when port 53 is unused (it never changes Windows DNS):

```powershell
cargo test --locked --manifest-path companion/Cargo.toml --lib standard_port_preview -- --ignored --nocapture
```

On an eligible controlled machine, record each of these before considering a daily-use trial:

1. Capture original automatic/static IPv4/IPv6 settings. `plan` agrees with Windows.
2. Activate and prove **Windows' default resolver** reaches NAAB: run the TXT marker check below without a `-Server` override and compare its returned `Strings` value with the current resolver token. Separately, direct probes to port 53 should block `ads.example.test` and resolve `example.com`. An NXDOMAIN for `ads.example.test` alone is not proof of NAAB filtering: that test name also fails upstream. The sample is not a full ad-blocking list. Repeat ordinary OS lookups, including `nslookup` in a normal terminal.
3. Press Ctrl+C, then repeat using the deadline. Verify exact original modes and server ordering in Windows, not merely the report.
4. Kill only the resolver. The still-running helper detects failure and restores settings.
5. Test failed upstreams while local health remains good. Expect degradation, then restoration at the deadline.
6. Test offline `recover` after forcibly stopping both processes. Repeat recovery to check it is harmless when already restored.
7. Test disconnected/reconnected original network, an external DNS edit, sleep/wake and reboot in the controlled environment. Expect unresolved cases to retain evidence. Without the optional recovery task, reboot requires manual `recover`; with it, verify that startup or a later scheduled retry restores DNS after the original network profile returns.

For step 2, in a normal PowerShell while the helper says `Active`:

```powershell
$naabHealthName = ([guid]::NewGuid().ToString('N')) + '.naab-health.invalid.'
Resolve-DnsName -Name $naabHealthName -Type TXT -DnsOnly
```

The random, fully qualified name avoids an old cached answer or DNS search suffix. NAAB answers with its current token and a zero TTL. A different token, no TXT result or an error means the routing check failed; do not treat the trial as validated. This checks the Windows resolver path, not applications or browsers using their own encrypted DNS.

Broader validation of the independent recovery task, a permanent resolver service, reliable network/VPN transitions, full uninstall lifecycle, extension controls, macOS integration and production platform verification remain later work. The existing Mac package does not contain this Windows preview.

Implementation references: Microsoft's [DNS settings cmdlet](https://learn.microsoft.com/en-us/powershell/module/dnsclient/set-dnsclientserveraddress) documents per-interface settings and CIM input objects; [netsh interface](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/netsh-interface) documents explicit IPv4/IPv6 DNS configuration with `source=dhcp`. The code reads the static per-family configuration separately so an automatic setting is not mistakenly saved as today's DHCP-provided addresses.
