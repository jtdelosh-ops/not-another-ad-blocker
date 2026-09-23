# System DNS design and recovery checklist

This document defines the next system-integration milestone. It is a design and test checklist, not an implemented feature. The current `naab-dns-dev` executable remains loopback-only and does not change operating-system DNS settings.

## User experience

System DNS is an explicit toggle in the companion:

```text
System DNS: OFF

[Enable]
```

When enabled, the companion first proves that the local resolver is healthy, records the current DNS configuration, applies the smallest platform-specific change, and verifies a test lookup. Disabling reverses those steps and restores the saved configuration.

## Activation sequence

1. Read the active DNS settings for every relevant network interface.
2. Save an exact, versioned recovery record locally.
3. Start or verify the NAAB resolver and its health endpoint.
4. Apply the platform-specific DNS change through a narrowly scoped helper.
5. Query a known allowed name and a local blocked fixture.
6. Mark the feature active only after both checks pass.

If any step fails, do not leave localhost as the only resolver. Restore the saved settings and report the failed step.

## Disable and recovery

The recovery record must survive companion restarts and contain enough information to restore each interface. Recovery is required when:

- The user disables system DNS.
- The companion is uninstalled.
- The resolver fails its health checks.
- The process crashes or is force-terminated.
- The computer changes networks or wakes from sleep.

The helper should be idempotent: repeated restore attempts must be safe, and stale records must be detected rather than blindly applied. Provide a command-line recovery action for cases where the UI cannot start.

## Platform boundary

The main companion should run with ordinary user privileges. Any administrator/root operation must be isolated in a small platform helper with validated arguments and a narrow command surface. macOS and Windows are the initial targets; Linux is deferred unless it does not delay the milestone.

## Test matrix

Before enabling this on an everyday computer, run the following in a VM or spare machine:

- Enable with a healthy resolver.
- Reject activation when the resolver cannot start.
- Confirm allowed lookups resolve through the configured upstream.
- Confirm blocked fixtures return the documented response.
- Disable and compare every interface's DNS settings with the saved baseline.
- Kill the companion and verify recovery or safe fallback.
- Change Wi-Fi networks, sleep/wake, and reconnect a VPN.
- Repeat disable, uninstall, and stale-record recovery.

The system-integration milestone is complete only when these tests pass on both macOS and Windows and the recovery path is documented for users.
