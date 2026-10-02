# macOS DNS discovery preview

The first Mac system-DNS milestone is read-only. On a Mac with Rust installed, run:

```sh
cargo run --locked --manifest-path companion/Cargo.toml --bin naab-dns-macos -- inspect
```

The command lists macOS network services, whether each is enabled, and DNS server addresses **explicitly configured** on each service. `configuredDns: null` means no servers were set with the network service configuration; it does not mean the Mac has no working DNS. DHCP, VPN, and supplemental resolvers may supply effective DNS separately. The command calls only `networksetup -listallnetworkservices` and `networksetup -getdnsservers`, never a setter.

This output is an inventory, not a safe restoration snapshot. A display name is not a stable service ID, and the output does not establish which service or resolver currently handles a query. The system DNS controller is not connected to this Mac module. There is now a separate root-only offline `recover` command, but no user-facing `trial` or `apply` Mac command creates a journal or changes DNS. An opt-in disposable-runner fixture has passed two scoped live restoration checks; the remaining guarded-trial gates are listed in the [Mac recovery guide](macos-dns-recovery.md).

The parser rejects unrecognized or localized output instead of guessing whether an unknown response means automatic DNS. Fixture tests run on other operating systems. On October 1, 2026, the user ran `inspect` on an Intel Mac and reported Wi-Fi, iPhone USB, and Thunderbolt Bridge as enabled, each with `configuredDns: null`. That establishes one live inventory result, not the effective resolver or broader Mac compatibility.

## Read-only preflight

On the Mac, from the repository root, run:

```sh
cargo run --locked --manifest-path companion/Cargo.toml --bin naab-dns-macos -- preflight
```

This combines the service inventory with the primary IPv4 route (`route -n get default`), service-to-device order (`networksetup -listnetworkserviceorder`), and effective resolver entries (`scutil --dns`). `defaultDnsServers` copies the first resolver in the ordinary DNS section; `resolvers` also preserves supplemental and scoped entries as macOS reports them. A route interface is mapped to a service only when exactly one enabled service has that device. The output may include private DNS addresses or search domains; review it before sharing.

`otherServicesWithConfiguredDns` names enabled non-primary services with explicit DNS. This is a preflight warning, not proof of a VPN: a user can also configure those services manually. Future activation must account for such competing settings before it changes the primary service.

The preflight also reads the current network-location ID and each service's persistent ID through Apple's System Configuration API. It reports `primaryIpv4ServiceId` only when the route's device and service name agree with exactly one enabled native service. These IDs identify configuration objects; they do not identify the Wi-Fi network currently attached to the service.

The `primaryServiceDnsProtocol` field is a read-only observation of that service's complete stored DNS protocol property list. It reports whether the protocol and its configuration exist, whether the protocol is enabled, and the serialized configuration's size and SHA-256 digest. The current reader obtains the raw DNS entity from the same preferences session and preserves an empty dictionary as `configurationState: "saved"`. After finding the protocol, it rejects a missing or non-dictionary stored entity. A null protocol lookup remains unknown because Apple also uses null for errors. The legacy diagnostic field `nullConfigurationStatus` remains in the output but is null for current native reads; it is not used to infer automatic mode. The preflight keeps the property-list contents private because they may contain search domains and other local settings. The digest is diagnostic; it is not a recovery journal or a substitute for comparing parsed settings when restoring them.

`trialReady` is always `false`: the route is only an IPv4 observation, resolver selection can vary by domain, interface, VPN, or application, and no complete network-context identity or durable recovery snapshot has been established. This command neither changes DNS nor makes the existing system-DNS controller available on Mac. See the [Mac recovery design](macos-dns-recovery.md) for the remaining gates.

On October 1, 2026, the user ran the updated `preflight` on that Intel Mac. It mapped the default IPv4 route's `en0` interface to Wi-Fi, and the native Wi-Fi service ID matched `primaryIpv4ServiceId`. It also returned a current network-location ID, the three ordinary DNS servers seen in `scutil --dns`, separate mDNS and scoped resolver entries, and `trialReady: false`. This verifies read-only identity lookup on that Mac; it does not establish identity of the attached Wi-Fi network or a safe restoration snapshot. This observation did not test restoration, network transitions, or safe activation with VPNs or multiple active services.

An intervening read-only run reported explicit DNS servers on Wi-Fi, iPhone USB, and Thunderbolt Bridge, and a non-null Wi-Fi DNS protocol dictionary. The user confirmed a VPN or DNS utility was active. This was a different network-software state from the earlier automatic-looking result.

After the utility was off, the Mac again reported an enabled Wi-Fi DNS protocol with a null result from the higher-level configuration getter while effective DNS still came from the network. The follow-up build reported `nullConfigurationStatus: 1004`. This historical observation is ambiguous: Apple's [configuration getter implementation](https://github.com/apple-oss-distributions/configd/blob/main/SystemConfiguration.fproj/SCNetworkConfigurationInternal.c) can hide an existing empty or disabled-only dictionary as null without resetting the error status. The result therefore does not establish whether the stored DNS entity was absent or empty on the user's Mac. The effective ISP DNS servers were still present in `scutil --dns`. The corrected raw reader preserves empty dictionaries instead of inferring a setting from that status; the earlier observation does not authorize replacing the user's DNS settings.
