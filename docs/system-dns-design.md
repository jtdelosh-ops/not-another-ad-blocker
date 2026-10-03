# System DNS activation and recovery foundation

NAAB now has a platform-neutral controller for enabling DNS protection and restoring saved settings after a failure. An offline simulator exercises it using fictional network interfaces and health results. It makes no network requests and changes no operating-system settings.

The foundation now also powers a [time-limited Windows DNS preview](windows-dns-preview.md), with a real adapter, live probes, a separate administrator helper and offline recovery command. An opt-in Windows scheduled task can run that recovery independently at startup and once per minute; forced-helper-exit and reboot checks passed in one controlled VM. A permanent resolver service, macOS integration and UI remain unfinished. The offline simulator described here still makes no OS changes; `naab-dns-dev` still operates independently on its development port.

## Try the offline simulation

From the repository root, run one command:

```powershell
cargo run --locked --manifest-path companion/Cargo.toml --bin naab-dns-sim -- local-failure --output-dir work/dns-local-failure
```

The parent directory (`work` in this example) must already exist; create it if needed. Every run requires a new output directory, so an earlier recovery record cannot be overwritten accidentally. No administrator terminal, running resolver, or internet connection is needed once Rust dependencies are installed.

Expected sequence:

```text
Enable: Active (Enabled)
HealthPoll: Degraded (LocalHealthUncertain)
HealthPoll: Degraded (LocalHealthUncertain)
HealthPoll: Inactive (LocalHealthFailed)
```

The final `Inactive` means the simulated original settings were restored. This demonstration supplies health samples immediately; it does not wait for or run a real background watchdog.

Replace `local-failure` and the directory name to try another scenario:

| Scenario | What it demonstrates |
| --- | --- |
| `normal` | Enable, then disable; restore automatic IPv4 and the original static IPv6 server |
| `upstream-outage` | Local resolver stays healthy; retain protection, recover to active when upstream health returns, then disable |
| `local-failure` | Restore settings after three consecutive failed/unknown local-health samples |
| `interrupted` | Recreate the controller and recover from its journal without a health probe |
| `conflict` | Simulate a user/VPN DNS edit; preserve that edit, restore the other target, retain the journal for attention |

Successful CLI execution means the scenario ran, including an intentional `RecoveryRequired` result in the conflict scenario. Read the printed state and report rather than treating exit code zero as proof that recovery completed. The simulated interfaces disappear when the command exits; retained conflict files are evidence, not settings to apply to your computer.

## State and activation

The controller reports `Inactive`, `Active`, `Degraded`, or `RecoveryRequired`. Activation follows this sequence:

1. Refuse a new session if an unresolved or unreadable recovery record exists.
2. Capture each supported interface/address-family setting and its network identity.
3. Require passing local and upstream health results for the planned targets.
4. Persist the validated recovery record before the first settings write.
5. Compare the current setting with the captured original before changing each target. Read it back after the change.
6. Require passing health results and recheck all settings before reporting active.

Any failed apply or post-activation check enters recovery. Re-enabling an already active session preserves its original backup. After restart, an outstanding record requires recovery; the controller never silently adopts localhost DNS as the original configuration.

## Backup and restoration

Each entry preserves a stable interface ID, network-context ID, address family, automatic versus static DNS mode, ordered original servers, and NAAB's intended loopback setting. Version 1 supports up to 64 targets and eight original servers per target. It rejects duplicate targets, invalid addresses, and pre-existing loopback DNS. Complex platform modes must be rejected by future adapters until they can be represented and restored exactly.

Recovery does not require the resolver, upstream DNS, UI, or internet. It handles each target independently:

- Already original: verify it and leave it alone.
- Still exactly NAAB's applied setting in the same network context: restore the original and verify by reading it back.
- Changed by someone else: preserve the new setting and report a conflict.
- Missing interface, changed network identity, or read/write error: retain the backup and report the unresolved target.

Recovery proceeds for other targets when one fails. It clears the journal only after all targets are confirmed original. Retries are safe because already-restored entries need no new write. Conflicts require reconciliation before a new session; there is no forced reset to automatic DNS, which could destroy a user's static configuration.

## Health and failure policy

`HealthProbe` currently receives the planned targets and supplies separate local and upstream observations; the simulator injects these results. The policy requires three consecutive failed or unknown local observations before restoring settings. One passing local observation resets the count. A healthy local resolver with an unavailable/unknown upstream remains `Degraded` and keeps its settings. Activation is stricter: both observations must pass.

The Windows preview helper owns its polling schedule. Its bounded live probes independently check local UDP/TCP on every enabled address family and test upstream reachability without treating a cached answer as fresh evidence. DNS query failures alone do not establish a local NAAB failure. The simulator continues to inject health results.

The Windows helper runs separately from the resolver and can restore settings after resolver failure. If the helper itself is killed, the optional scheduled recovery task can start a protected copy of the offline helper on its next run; without that task, recovery is manual. The task also retries at boot and after a temporarily unavailable network profile. Forced-exit and reboot restoration passed in one controlled VM; other network-profile conditions still need testing. A permanent resolver service with sleep/network/VPN handling remains required for a daily-use product; the scheduled task does not provide the resolver or a user-facing on/off switch.

## Local storage and reports

The file journal takes an exclusive OS file lock for its lifetime. The versioned `recovery.json` is flushed and committed with a rename before applying settings. A leftover `.pending` file is not a committed record. Invalid, oversized (over 128 KiB), or unsupported records block mutations and are preserved for investigation.

The simulator writes a bounded set of files in the chosen output directory:

- `recovery.json`: original and applied settings; retained while recovery is incomplete.
- `last-report.json`: latest action, timestamp, state, reason, health sample and per-target recovery outcomes.
- `last-incident.json`: latest problem report, preserved across later successful health polls or disable operations.
- `controller.lock`: coordination file; its existence alone does not mean a process holds the lock.

Reports contain no browsing queries or raw platform error strings. They stay local and are overwritten rather than forming an unlimited history. A report-write failure is returned to the caller and does not prevent restoration. Recovery depends on the backup, not on a report being writable. The backup contains DNS server addresses and interface identifiers and should be treated as private configuration.

This storage requires a trusted, private directory and cooperative writers. It is not a privileged API accepting caller-selected paths. The Windows preview enforces a fixed protected directory and machine/adapter/profile identities. Unix files/directories created by the library use restrictive modes; Windows inherits the protected directory's ACL and uses write-through file replacement. Power-loss durability and live Windows restoration still require platform testing before production use. A missing or damaged backup cannot safely be replaced with guessed DNS settings.

## Verification and next implementation

Run the deterministic foundation tests:

```sh
cargo test --locked --manifest-path companion/Cargo.toml --test system_dns
```

Tests cover preflight rejection, failed journal writes, partial activation, apply errors after mutation, false-success writes, postflight failure, interruptions around the first mutation, restart recovery, failed restoration and retry, external edits, missing interfaces, changed network identities, upstream outages, local-failure thresholds, journal locking/reopening, corrupt/versioned/oversized records, and the executable simulation/report output. Fault injection tests controller behavior with a simulated machine; it does not prove real OS rollback, process supervision, or live network health detection.

Static/mixed DNS restoration passed in one eligible controlled Windows VM. Next, validate network transitions, then add permanent resolver lifecycle and broader network handling. The Windows preview keeps the resolver at normal user privileges, serves loopback port 53 and elevates only the settings helper. macOS now has an offline recovery adapter and [disposable-runner evidence for exact restoration, scheduled recovery, guarded system DNS passthrough, and foreground controller cancellation/static cases](macos-dns-recovery.md). Actual SIGINT/SIGTERM, repeated signals during cleanup, and two static dictionary configurations passed on hosted macOS 15 Intel. Bounded native command execution, production admission and lifecycle integration, broader settings/network validation, and public Mac activation remain unfinished.

Before activation on an everyday computer, use a VM or spare machine to test enable/disable, resolver startup failure, allowed and blocked queries, exact restoration, forced process termination, reboot, sleep/wake, Wi-Fi and VPN changes, interrupted writes, service uninstall, and offline emergency recovery. Require evidence on both Windows and macOS before declaring system integration complete.
