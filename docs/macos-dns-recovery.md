# macOS DNS recovery gates

The read-only preflight can now observe the current network location's persistent ID and the service IDs within that location. Apple's [System Configuration API](https://developer.apple.com/documentation/systemconfiguration/scnetworkconfiguration) provides these IDs independently of user-editable service names. The preflight cross-checks a primary IPv4 route against the native service's BSD device before reporting its ID. It never writes DNS settings, creates a recovery record, or declares a trial safe.

## Identity and context

The future recovery target needs the current location/set ID, service ID, and a verified identity for the **network attached to that service at activation time**. The set and service IDs remain useful when a service is renamed, but neither proves that the Mac is still on the same Wi-Fi network. A route interface or gateway alone is also insufficient: both can be reused after a network change. If the network context cannot be established or later differs, recovery must retain its record and report that manual attention is needed rather than overwrite a possibly new network's DNS settings.

Before activating on a service, the helper must capture its complete DNS configuration. `networksetup -getdnsservers` distinguishes an explicit server list from no explicit servers, but it does not describe all search domains, supplemental resolvers, managed DNS settings, or Network Extension policy. The helper must inspect the service's DNS protocol configuration and reject any mode it cannot round-trip exactly. Effective `scutil --dns` output remains an additional policy check, not the saved setting to restore.

## Offline helper and journal

The existing [platform-neutral controller](system-dns-design.md) already saves an original/applied setting before mutation, compares before writes, and retains a record on conflict. A Mac adapter must satisfy the same contract. It needs a separately launched, narrowly privileged settings/recovery helper in a protected local directory; the ordinary companion should not run as root. The helper must be installable and runnable independently of the resolver and browser, including after reboot or helper failure. Its recovery command must work offline without upstream DNS.

The helper's restore path must verify the saved location ID, service ID, network-context evidence, and current DNS setting. If the setting is already original, it can mark that target restored. If it is still exactly NAAB's applied setting in the same verified context, it can restore the complete original configuration and read it back. Any third-party edit, missing service, context change, unreadable record, or uncertain write must preserve the record and report a conflict. A schedule or `launchd` job may retry safe cases but must never force-reset DNS after a conflict.

The implementation includes a native comparison primitive for DNS protocol dictionaries. It parses both property lists and compares their values, so XML key order does not decide whether a setting changed. It rejects invalid or non-dictionary data. Apple's higher-level DNS getter can hide a real empty (or disabled-only) dictionary as null without resetting `SCError`; an earlier user observation of status 1004 therefore did not prove that the stored entity was absent. Capture and the locked restore comparison now read the raw DNS entity from the same preferences session, preserving even an empty dictionary. A missing/non-dictionary raw entity remains an error. Legacy records that lack an exact original dictionary are retained for manual attention rather than filled in by inference. No user-facing activation command creates a recovery journal yet.

This distinction follows Apple's [configuration getter and setter implementation](https://github.com/apple-oss-distributions/configd/blob/main/SystemConfiguration.fproj/SCNetworkConfigurationInternal.c). Native regression tests stage empty and disabled-only dictionaries in a separate, uncommitted temporary preferences session; they do not change system network settings.

The source now contains a guarded offline recovery command, a root-owned record store, and an optional `launchd` installation script. The command compares the saved location and service IDs, BSD device, primary IPv4 gateway address and gateway link-layer address, and complete DNS protocol dictionary. It restores only when the current dictionary still equals the recorded NAAB-applied dictionary. It verifies the original setting after writing and retains the record on conflict, missing context, or an uncertain result. The gateway identity is a conservative local-network check: a missing ARP entry or a changed router prevents automatic recovery, even when the service ID is unchanged. It is not a proof against a malicious network impersonating the original gateway.

`recovery.json` is private to root under `/Library/Application Support/NAAB-DNS-Preview`. The helper refuses a non-root caller, untrusted parent directories, unsafe ownership or modes, extended ACL entries on protected paths, and concurrent recovery. The installer also rejects inherited ACL entries on newly created files. An activation must write and sync this record before changing DNS; only the opt-in CI fixture described below currently creates one on Mac. The root-only recovery command is `sudo naab-dns-macos recover`. It does not need the resolver, browser, upstream DNS, or a live Internet connection.

On an **isolated test Mac**, after a successful native build and code review, the independent helper can be installed with:

```sh
cargo build --release --locked --manifest-path companion/Cargo.toml --bin naab-dns-macos
sudo sh scripts/install-macos-dns-recovery.sh companion/target/release/naab-dns-macos
```

The installer refuses to replace an existing helper while a recovery record is present. It installs a root-owned copy and a `launchd` task that runs at startup and every minute. Installation and the no-record recovery path passed on a disposable Intel Mac runner in [run 37067964310](https://github.com/jtdelosh-ops/not-another-ad-blocker/actions/runs/37067964310), commit `3985881`. The user's everyday Mac has exercised only the no-record recovery path; the persistent helper is not installed there. Do not treat a successful source build or read-only preflight as permission to start a system-DNS trial.

## Disposable runner restoration test

The `Mac preview` workflow has a manually selected `live_recovery` input (off by default). It builds the source-only `macos_recovery_ci` example and runs it as root on the workflow's disposable `macos-15-intel` VM, after installing the reviewed recovery helper. Do not run this fixture on an everyday Mac. Its environment acknowledgements prevent accidental invocation; they do not prove that a machine is isolated.

The fixture captures the current service's complete DNS dictionary, enabled state, and network context, then syncs the production recovery journal before making a native settings change. It keeps the runner's existing upstream addresses and adds a temporary `naab-ci.invalid` search-domain marker. It checks two cases:

1. The separate installed helper restores the complete original state after normal completion and clears the journal.
2. The fixture holder is killed with SIGKILL while its journal lock is held. The installed 60-second `launchd` schedule must restore the complete original state and clear the journal within 100 seconds, without a manual recovery call or task kickstart during the measured interval.

Both cases require native read-back and a successful `restored` report. Failure cleanup invokes the same conflict-safe helper and cannot turn a failed test into a pass. The workflow also checks for a retained journal as root, since an ordinary user cannot see inside the protected recovery directory. This test does not exercise loopback DNS forwarding, a public activation command, Ctrl+C handling, reboot, sleep/wake, Wi-Fi or VPN transitions, or all static/mixed settings. Those remain separate gates.

The helper must still be exercised on an isolated Mac or VM through normal completion, resolver failure, forced termination, reboot, sleep/wake, Wi-Fi change, VPN activation, static DNS, and interrupted writes before a system-DNS trial is offered on an everyday Mac. A signed, authenticated installer and uninstall/upgrade verification remain future work.

**Current status:** the offline recovery engine, protected storage, and helper installation have portable tests and native Mac CI evidence. The opt-in live restoration fixture is prepared for controlled runner verification; a passing live run must be recorded separately. No user-facing activation command creates the journal or changes Mac DNS, and `trialReady` remains false. The test example is not included in the preview archive.
