# macOS DNS discovery preview

The first Mac system-DNS milestone is read-only. On a Mac with Rust installed, run:

```sh
cargo run --locked --manifest-path companion/Cargo.toml --bin naab-dns-macos -- inspect
```

The command lists macOS network services, whether each is enabled, and DNS server addresses **explicitly configured** on each service. `configuredDns: null` means no servers were set with the network service configuration; it does not mean the Mac has no working DNS. DHCP, VPN, and supplemental resolvers may supply effective DNS separately. The command calls only `networksetup -listallnetworkservices` and `networksetup -getdnsservers`, never a setter.

This output is an inventory, not a safe restoration snapshot. A display name is not a stable service ID, and the output does not establish which service or resolver currently handles a query. The system DNS controller is not connected to this Mac module. There is no `trial`, `apply`, or `recover` Mac command yet. A future guarded trial needs stable service and network identity, effective resolver/policy checks, a recovery helper, and live Mac verification before any DNS change.

The parser rejects unrecognized or localized output instead of guessing whether an unknown response means automatic DNS. Fixture tests run on other operating systems, but the command itself must still be checked on a Mac before treating its output as complete.
