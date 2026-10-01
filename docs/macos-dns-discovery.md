# macOS DNS discovery preview

The first Mac system-DNS milestone is read-only. On a Mac with Rust installed, run:

```sh
cargo run --locked --manifest-path companion/Cargo.toml --bin naab-dns-macos -- inspect
```

The command lists macOS network services, whether each is enabled, and DNS server addresses **explicitly configured** on each service. `configuredDns: null` means no servers were set with the network service configuration; it does not mean the Mac has no working DNS. DHCP, VPN, and supplemental resolvers may supply effective DNS separately. The command calls only `networksetup -listallnetworkservices` and `networksetup -getdnsservers`, never a setter.

This output is an inventory, not a safe restoration snapshot. A display name is not a stable service ID, and the output does not establish which service or resolver currently handles a query. The system DNS controller is not connected to this Mac module. There is no `trial`, `apply`, or `recover` Mac command yet. A future guarded trial needs stable service and network identity, effective resolver/policy checks, a recovery helper, and live Mac verification before any DNS change.

The parser rejects unrecognized or localized output instead of guessing whether an unknown response means automatic DNS. Fixture tests run on other operating systems. On October 1, 2026, the user ran `inspect` on an Intel Mac and reported Wi-Fi, iPhone USB, and Thunderbolt Bridge as enabled, each with `configuredDns: null`. That establishes one live inventory result, not the effective resolver or broader Mac compatibility.

## Read-only preflight

On the Mac, from the repository root, run:

```sh
cargo run --locked --manifest-path companion/Cargo.toml --bin naab-dns-macos -- preflight
```

This combines the service inventory with the primary IPv4 route (`route -n get default`), service-to-device order (`networksetup -listnetworkserviceorder`), and effective resolver entries (`scutil --dns`). `defaultDnsServers` copies the first resolver in the ordinary DNS section; `resolvers` also preserves supplemental and scoped entries as macOS reports them. A route interface is mapped to a service only when exactly one enabled service has that device. The output may include private DNS addresses or search domains; review it before sharing.

The preflight also reads the current network-location ID and each service's persistent ID through Apple's System Configuration API. It reports `primaryIpv4ServiceId` only when the route's device and service name agree with exactly one enabled native service. These IDs identify configuration objects; they do not identify the Wi-Fi network currently attached to the service.

`trialReady` is always `false`: the route is only an IPv4 observation, resolver selection can vary by domain, interface, VPN, or application, and no complete network-context identity or recovery snapshot has been established. This command neither changes DNS nor makes the existing system-DNS controller available on Mac. See the [Mac recovery design](macos-dns-recovery.md) for the remaining gates.

On October 1, 2026, the user ran the prior `preflight` build on the same Intel Mac. It mapped the default IPv4 route's `en0` interface to Wi-Fi, reported the three ordinary DNS servers seen in `scutil --dns`, preserved the separate mDNS and scoped resolver entries, and returned `trialReady: false`. The new native service-ID fields have not yet been run on that Mac. VPN, multiple active services, network transitions, and restoration remain untested.
