# macOS DNS recovery gates

The read-only preflight can now observe the current network location's persistent ID and the service IDs within that location. Apple's [System Configuration API](https://developer.apple.com/documentation/systemconfiguration/scnetworkconfiguration) provides these IDs independently of user-editable service names. The preflight cross-checks a primary IPv4 route against the native service's BSD device before reporting its ID. It never writes DNS settings, creates a recovery record, or declares a trial safe.

## Identity and context

The future recovery target needs the current location/set ID, service ID, and a verified identity for the **network attached to that service at activation time**. The set and service IDs remain useful when a service is renamed, but neither proves that the Mac is still on the same Wi-Fi network. A route interface or gateway alone is also insufficient: both can be reused after a network change. If the network context cannot be established or later differs, recovery must retain its record and report that manual attention is needed rather than overwrite a possibly new network's DNS settings.

Before activating on a service, the helper must capture its complete DNS configuration. `networksetup -getdnsservers` distinguishes an explicit server list from no explicit servers, but it does not describe all search domains, supplemental resolvers, managed DNS settings, or Network Extension policy. The helper must inspect the service's DNS protocol configuration and reject any mode it cannot round-trip exactly. Effective `scutil --dns` output remains an additional policy check, not the saved setting to restore.

## Offline helper and journal

The existing [platform-neutral controller](system-dns-design.md) already saves an original/applied setting before mutation, compares before writes, and retains a record on conflict. A Mac adapter must satisfy the same contract. It needs a separately launched, narrowly privileged settings/recovery helper in a protected local directory; the ordinary companion should not run as root. The helper must be installable and runnable independently of the resolver and browser, including after reboot or helper failure. Its recovery command must work offline without upstream DNS.

The helper's restore path must verify the saved location ID, service ID, network-context evidence, and current DNS setting. If the setting is already original, it can mark that target restored. If it is still exactly NAAB's applied setting in the same verified context, it can restore the complete original configuration and read it back. Any third-party edit, missing service, context change, unreadable record, or uncertain write must preserve the record and report a conflict. A schedule or `launchd` job may retry safe cases but must never force-reset DNS after a conflict.

The implementation includes a native comparison primitive for non-null DNS protocol dictionaries. It parses both property lists and compares their values, so XML key order does not decide whether a setting changed. It rejects invalid or non-dictionary data. On the user's Mac, a null Wi-Fi configuration returned System Configuration status 1004 (`kSCStatusNoKey`) with the VPN/DNS utility off; the preflight identifies this as no saved configuration without inferring a safe restoration mode. Other null statuses remain unknown. The offline recovery command now uses the comparison primitive, but no activation command creates a recovery journal yet.

The source now contains a guarded offline recovery command, a root-owned record store, and an optional `launchd` installation script. The command compares the saved location and service IDs, BSD device, primary IPv4 gateway address and gateway link-layer address, and complete DNS protocol dictionary. It restores only when the current dictionary still equals the recorded NAAB-applied dictionary. It verifies the original setting after writing and retains the record on conflict, missing context, or an uncertain result. The gateway identity is a conservative local-network check: a missing ARP entry or a changed router prevents automatic recovery, even when the service ID is unchanged. It is not a proof against a malicious network impersonating the original gateway.

`recovery.json` is private to root under `/Library/Application Support/NAAB-DNS-Preview`. The helper refuses a non-root caller, untrusted parent directories, unsafe ownership or modes, extended ACL entries on protected paths, and concurrent recovery. The installer also rejects inherited ACL entries on newly created files. A future activation must write and sync this record before changing DNS; no Mac activation path creates one yet. The root-only command is `sudo naab-dns-macos recover`. It does not need the resolver, browser, upstream DNS, or a live Internet connection.

On an **isolated test Mac**, after a successful native build and code review, the independent helper can be installed with:

```sh
cargo build --release --locked --manifest-path companion/Cargo.toml --bin naab-dns-macos
sudo sh scripts/install-macos-dns-recovery.sh companion/target/release/naab-dns-macos
```

The installer refuses to replace an existing helper while a recovery record is present. It installs a root-owned copy and a `launchd` task that runs at startup and every minute. Its installation and native restore path have **not** been executed on the user's Mac yet. Do not treat a successful source build or read-only preflight as permission to start a system-DNS trial.

The helper must still be exercised on an isolated Mac or VM through normal completion, resolver failure, forced termination, reboot, sleep/wake, Wi-Fi change, VPN activation, static DNS, and interrupted writes before a system-DNS trial is offered on an everyday Mac. A signed, authenticated installer and uninstall/upgrade verification remain future work.

**Current status:** the offline recovery engine and protected storage are implemented in source, with portable state-machine tests. The native macOS writer and `launchd` installer await a Mac build and controlled live verification. No activation command creates the journal or changes Mac DNS, and `trialReady` remains false.
