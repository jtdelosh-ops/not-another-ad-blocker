# macOS DNS recovery gates

The read-only preflight can now observe the current network location's persistent ID and the service IDs within that location. Apple's [System Configuration API](https://developer.apple.com/documentation/systemconfiguration/scnetworkconfiguration) provides these IDs independently of user-editable service names. The preflight cross-checks a primary IPv4 route against the native service's BSD device before reporting its ID. It never writes DNS settings, creates a recovery record, or declares a trial safe.

## Identity and context

The future recovery target needs the current location/set ID, service ID, and a verified identity for the **network attached to that service at activation time**. The set and service IDs remain useful when a service is renamed, but neither proves that the Mac is still on the same Wi-Fi network. A route interface or gateway alone is also insufficient: both can be reused after a network change. If the network context cannot be established or later differs, recovery must retain its record and report that manual attention is needed rather than overwrite a possibly new network's DNS settings.

Before activating on a service, the helper must capture its complete DNS configuration. `networksetup -getdnsservers` distinguishes an explicit server list from no explicit servers, but it does not describe all search domains, supplemental resolvers, managed DNS settings, or Network Extension policy. The helper must inspect the service's DNS protocol configuration and reject any mode it cannot round-trip exactly. Effective `scutil --dns` output remains an additional policy check, not the saved setting to restore.

## Offline helper and journal

The existing [platform-neutral controller](system-dns-design.md) already saves an original/applied setting before mutation, compares before writes, and retains a record on conflict. A Mac adapter must satisfy the same contract. It needs a separately launched, narrowly privileged settings/recovery helper in a protected local directory; the ordinary companion should not run as root. The helper must be installable and runnable independently of the resolver and browser, including after reboot or helper failure. Its recovery command must work offline without upstream DNS.

The helper's restore path must verify the saved location ID, service ID, network-context evidence, and current DNS setting. If the setting is already original, it can mark that target restored. If it is still exactly NAAB's applied setting in the same verified context, it can restore the complete original configuration and read it back. Any third-party edit, missing service, context change, unreadable record, or uncertain write must preserve the record and report a conflict. A schedule or `launchd` job may retry safe cases but must never force-reset DNS after a conflict.

The record and its directory need ownership and permission checks before a privileged helper trusts them. Installation, uninstall, and upgrade must leave a usable offline recovery command. The helper must be exercised on an isolated Mac or VM through normal completion, resolver failure, forced termination, reboot, sleep/wake, Wi-Fi change, VPN activation, static DNS, and interrupted writes before a system-DNS trial is offered on an everyday Mac.

**Current status:** the persistent configuration IDs are read-only observations. Network-context identity, exact DNS-protocol snapshot, privileged helper, `launchd` recovery, and real restore tests are not implemented. `trialReady` remains false.
