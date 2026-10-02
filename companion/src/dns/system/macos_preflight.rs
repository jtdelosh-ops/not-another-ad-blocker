//! Read-only observations for a future macOS DNS trial. No observation here is
//! a stable recovery identity or permission to alter network settings.
use super::macos::Service;
#[cfg(target_os = "macos")]
use super::macos_identity::DnsConfigurationState;
use super::macos_identity::DnsProtocolSummary;
#[cfg(any(test, target_os = "macos"))]
use super::macos_identity::IdentitySnapshot;
use super::macos_identity::ServiceIdentity;
#[cfg(target_os = "macos")]
use super::{macos, macos_identity};
use serde::Serialize;
#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(any(test, target_os = "macos"))]
const MAX_OUTPUT: usize = 256 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    pub services: Vec<Service>,
    pub primary_ipv4_interface: Option<String>,
    pub primary_ipv4_service: Option<String>,
    pub primary_ipv4_service_id: Option<String>,
    pub current_set_id: Option<String>,
    pub service_identities: Vec<ServiceIdentity>,
    pub primary_service_dns_protocol: Option<DnsProtocolSummary>,
    pub other_services_with_configured_dns: Vec<String>,
    pub default_dns_servers: Vec<String>,
    pub resolvers: Vec<Resolver>,
    pub warnings: Vec<String>,
    pub trial_ready: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Resolver {
    pub section: String,
    pub number: usize,
    pub nameservers: Vec<String>,
    pub domain: Option<String>,
    pub interface: Option<String>,
    pub options: Option<String>,
    pub flags: Option<String>,
    pub other_fields: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
#[cfg(any(test, target_os = "macos"))]
struct OrderedService {
    name: String,
    device: Option<String>,
}

#[cfg(any(test, target_os = "macos"))]
fn bounded_lines(output: &str) -> Result<impl Iterator<Item = &str>, String> {
    if output.len() > MAX_OUTPUT || output.chars().any(|ch| ch == '\0') {
        return Err("macOS network output is invalid or too large".into());
    }
    Ok(output.lines().map(str::trim))
}

#[cfg(any(test, target_os = "macos"))]
fn parse_service_order(output: &str) -> Result<Vec<OrderedService>, String> {
    let mut result = Vec::new();
    let mut pending: Option<String> = None;
    for line in bounded_lines(output)? {
        if line.is_empty() {
            continue;
        }
        if line == "An asterisk (*) denotes that a network service is disabled." {
            continue;
        }
        if line.starts_with('(') && line.contains(") ") && !line.starts_with("(Hardware Port:") {
            if pending.is_some() {
                return Err("Incomplete network service order".into());
            }
            let (ordinal, name) = line
                .split_once(") ")
                .ok_or("Invalid network service order")?;
            if !(ordinal.len() > 1 && ordinal[1..].chars().all(|ch| ch.is_ascii_digit())
                || ordinal == "(*")
                || name.is_empty()
            {
                return Err("Invalid network service order".into());
            }
            pending = Some(name.trim_start_matches('*').to_owned());
        } else if line.starts_with("(Hardware Port:") {
            let name = pending.take().ok_or("Unpaired hardware port")?;
            let device = line.split("Device:").nth(1).and_then(|tail| {
                let value = tail.trim().trim_end_matches(')').trim();
                (!value.is_empty()).then(|| value.to_owned())
            });
            result.push(OrderedService { name, device });
            if result.len() > 64 {
                return Err("Too many network services".into());
            }
        } else {
            return Err("Unrecognized network service order".into());
        }
    }
    if pending.is_some() || result.is_empty() {
        return Err("Incomplete network service order".into());
    }
    Ok(result)
}

#[cfg(any(test, target_os = "macos"))]
fn parse_default_route(output: &str) -> Result<Option<String>, String> {
    let mut device = None;
    for line in bounded_lines(output)? {
        if let Some(value) = line.strip_prefix("interface:") {
            let value = value.trim();
            if value.is_empty()
                || !value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
                || device.replace(value.to_owned()).is_some()
            {
                return Err("Invalid primary IPv4 interface".into());
            }
        }
    }
    Ok(device)
}

#[cfg(any(test, target_os = "macos"))]
fn parse_resolvers(output: &str) -> Result<Vec<Resolver>, String> {
    let mut section: Option<String> = None;
    let mut result: Vec<Resolver> = Vec::new();
    for line in bounded_lines(output)? {
        if line.starts_with("DNS configuration") {
            section = Some(line.to_owned());
            continue;
        }
        if let Some(number) = line.strip_prefix("resolver #") {
            let number = number
                .parse()
                .map_err(|_| "Invalid macOS resolver number")?;
            result.push(Resolver {
                section: section.clone().ok_or("Missing DNS configuration header")?,
                number,
                nameservers: Vec::new(),
                domain: None,
                interface: None,
                options: None,
                flags: None,
                other_fields: Vec::new(),
            });
            if result.len() > 128 {
                return Err("Too many macOS resolvers".into());
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let current = result
            .last_mut()
            .ok_or("Unrecognized macOS DNS configuration")?;
        let (key, value) = line.split_once(':').ok_or("Invalid macOS resolver field")?;
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
            return Err("Invalid macOS resolver value".into());
        }
        match key {
            key if key.starts_with("nameserver[") && key.ends_with(']') => {
                if value.is_empty() {
                    return Err("Empty macOS nameserver".into());
                }
                current.nameservers.push(value.to_owned());
                if current.nameservers.len() > 64 {
                    return Err("Too many macOS nameservers".into());
                }
            }
            "domain" => current.domain = Some(value.to_owned()),
            "if_index" => {
                current.interface = value
                    .split_once('(')
                    .and_then(|(_, tail)| tail.strip_suffix(')').map(str::to_owned));
                if current.interface.is_none() {
                    current.other_fields.push(line.to_owned());
                }
            }
            "options" => current.options = Some(value.to_owned()),
            "flags" => current.flags = Some(value.to_owned()),
            _ => current.other_fields.push(line.to_owned()),
        }
    }
    if section.is_none() {
        return Err("Missing DNS configuration header".into());
    }
    Ok(result)
}

#[cfg(any(test, target_os = "macos"))]
fn preflight_with(
    services: Vec<Service>,
    service_order: &str,
    dns: &str,
    route: Option<&str>,
) -> Result<Preflight, String> {
    let ordered = parse_service_order(service_order)?;
    let resolvers = parse_resolvers(dns)?;
    let default_dns_servers = resolvers
        .iter()
        .find(|entry| entry.section == "DNS configuration")
        .map(|entry| entry.nameservers.clone())
        .unwrap_or_default();
    let primary_ipv4_interface = route.map(parse_default_route).transpose()?.flatten();
    let matching: Vec<_> = ordered
        .iter()
        .filter(|entry| entry.device.as_ref() == primary_ipv4_interface.as_ref())
        .filter(|entry| {
            services
                .iter()
                .any(|service| service.name == entry.name && service.enabled)
        })
        .collect();
    let primary_ipv4_service =
        (primary_ipv4_interface.is_some() && matching.len() == 1).then(|| matching[0].name.clone());
    let other_services_with_configured_dns: Vec<_> = primary_ipv4_service
        .as_deref()
        .map(|primary| {
            services
                .iter()
                .filter(|service| service.enabled && service.configured_dns.is_some())
                .filter(|service| service.name != primary)
                .map(|service| service.name.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut warnings = vec![
        "Service and location IDs identify configuration objects, not the current physical network or a recovery snapshot."
            .to_owned(),
        "DNS resolution can vary by domain, interface, VPN, and application; do not infer that one listed server handles every query."
            .to_owned(),
    ];
    if primary_ipv4_service.is_none() {
        warnings.push("Primary IPv4 route did not map to one enabled network service.".to_owned());
    }
    if resolvers.is_empty() || resolvers.iter().all(|entry| entry.nameservers.is_empty()) {
        warnings.push("No usable nameserver was shown by scutil --dns.".to_owned());
    }
    if !other_services_with_configured_dns.is_empty() {
        warnings.push(
            "Other enabled network services also have explicitly configured DNS; a network utility or service change may control these settings."
                .to_owned(),
        );
    }
    Ok(Preflight {
        services,
        primary_ipv4_interface,
        primary_ipv4_service,
        primary_ipv4_service_id: None,
        current_set_id: None,
        service_identities: Vec::new(),
        primary_service_dns_protocol: None,
        other_services_with_configured_dns,
        default_dns_servers,
        resolvers,
        warnings,
        trial_ready: false,
    })
}

#[cfg(any(test, target_os = "macos"))]
fn attach_identities(mut report: Preflight, snapshot: IdentitySnapshot) -> Preflight {
    if let (Some(name), Some(device)) = (
        report.primary_ipv4_service.as_deref(),
        report.primary_ipv4_interface.as_deref(),
    ) {
        let matches: Vec<_> = snapshot
            .services
            .iter()
            .filter(|service| {
                service.enabled && service.name == name && service.device.as_deref() == Some(device)
            })
            .collect();
        if matches.len() == 1 {
            report.primary_ipv4_service_id = Some(matches[0].id.clone());
        }
    }
    if report.primary_ipv4_service_id.is_none() {
        report.warnings.push(
            "Primary IPv4 service did not map uniquely to a System Configuration service ID."
                .to_owned(),
        );
    }
    report.current_set_id = Some(snapshot.current_set_id);
    report.service_identities = snapshot.services;
    report
}

#[cfg(target_os = "macos")]
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("Unable to run macOS network command: {error}"))?;
    if !output.status.success() || output.stdout.len() > MAX_OUTPUT {
        return Err("macOS network command failed or returned too much output".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "macOS network output is not UTF-8".into())
}

#[cfg(target_os = "macos")]
pub fn preflight() -> Result<Preflight, String> {
    let services = macos::inspect()?;
    let order = run("/usr/sbin/networksetup", &["-listnetworkserviceorder"])?;
    let dns = run("/usr/sbin/scutil", &["--dns"])?;
    let route = run("/sbin/route", &["-n", "get", "default"]).ok();
    let report = preflight_with(services, &order, &dns, route.as_deref())?;
    let mut report = attach_identities(report, macos_identity::inspect()?);
    if let (Some(set_id), Some(service_id)) = (
        report.current_set_id.as_deref(),
        report.primary_ipv4_service_id.as_deref(),
    ) {
        match macos_identity::inspect_dns_protocol(set_id, service_id) {
            Ok(snapshot) => {
                match snapshot.configuration_state() {
                    DnsConfigurationState::NoSavedConfiguration => report.warnings.push(
                        "The primary service has no saved DNS configuration. Effective DNS may come from DHCP or another resolver; this is not a recovery snapshot."
                            .to_owned(),
                    ),
                    DnsConfigurationState::Unknown => {
                        report.warnings.push(
                            "Mac DNS protocol or configuration is absent or unreadable; its saved setting cannot be classified."
                                .to_owned(),
                        );
                    }
                    DnsConfigurationState::Saved => {}
                }
                report.primary_service_dns_protocol = Some(snapshot.summary());
            }
            Err(_) => report.warnings.push(
                "Primary service DNS protocol could not be read exactly; no settings change is permitted."
                    .to_owned(),
            ),
        }
    }
    Ok(report)
}

#[cfg(not(target_os = "macos"))]
pub fn preflight() -> Result<Preflight, String> {
    Err("Mac DNS preflight is available only on macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn services() -> Vec<Service> {
        vec![Service {
            name: "Wi-Fi".into(),
            enabled: true,
            configured_dns: None,
        }]
    }

    #[test]
    fn reports_default_and_scoped_resolvers_without_trial_approval() {
        let result = preflight_with(
            services(),
            "An asterisk (*) denotes that a network service is disabled.\n(1) Wi-Fi\n(Hardware Port: Wi-Fi, Device: en0)\n(*) Bridge\n(Hardware Port: Bridge, Device: bridge0)\n",
            "DNS configuration\n\nresolver #1\n  nameserver[0] : 192.0.2.1\n  if_index : 4 (en0)\n\nresolver #2\n  domain : corp.example\n  nameserver[0] : 198.51.100.1\n  flags : Supplemental\n\nDNS configuration (for scoped queries)\n\nresolver #1\n  nameserver[0] : fe80::1%en0\n  if_index : 4 (en0)\n",
            Some("route to: default\n interface: en0\n"),
        ).unwrap();
        assert_eq!(result.primary_ipv4_service.as_deref(), Some("Wi-Fi"));
        assert_eq!(result.resolvers.len(), 3);
        assert_eq!(result.resolvers[1].domain.as_deref(), Some("corp.example"));
        assert_eq!(result.resolvers[2].nameservers, ["fe80::1%en0"]);
        assert!(!result.trial_ready);
    }

    #[test]
    fn ambiguous_route_does_not_select_a_service() {
        let result = preflight_with(
            services(),
            "(1) Wi-Fi\n(Hardware Port: Wi-Fi, Device: en0)\n(2) VPN\n(Hardware Port: VPN, Device: utun2)\n",
            "DNS configuration\nresolver #1\n nameserver[0] : 192.0.2.1\n",
            Some("interface: utun9\n"),
        ).unwrap();
        assert_eq!(result.primary_ipv4_service, None);
        assert!(!result.trial_ready);
    }

    #[test]
    fn reads_reported_intel_mac_shape_without_mistaking_mdns_for_upstream_dns() {
        let result = preflight_with(
            vec![
                Service { name: "Wi-Fi".into(), enabled: true, configured_dns: None },
                Service { name: "iPhone USB".into(), enabled: true, configured_dns: None },
                Service { name: "Thunderbolt Bridge".into(), enabled: true, configured_dns: None },
            ],
            "An asterisk (*) denotes that a network service is disabled.\n(1) Wi-Fi\n(Hardware Port: Wi-Fi, Device: en0)\n(2) iPhone USB\n(Hardware Port: iPhone USB, Device: en5)\n(3) Thunderbolt Bridge\n(Hardware Port: Thunderbolt Bridge, Device: bridge0)\n",
            "DNS configuration\nresolver #1\n  search domain[0] : isp.example\n  nameserver[0] : 2001:db8::1\n  nameserver[1] : 2001:db8::2\n  nameserver[2] : 192.0.2.1\n  if_index : 6 (en0)\n  flags : Request A records, Request AAAA records\n  reach : 0x00000002 (Reachable)\nresolver #2\n  domain : local\n  options : mdns\n  timeout : 5\n  flags : Request A records, Request AAAA records\n  reach : 0x00000000 (Not Reachable)\n  order : 300000\nDNS configuration (for scoped queries)\nresolver #1\n  search domain[0] : isp.example\n  nameserver[0] : 2001:db8::1\n  nameserver[1] : 2001:db8::2\n  nameserver[2] : 192.0.2.1\n  if_index : 6 (en0)\n  flags : Scoped, Request A records, Request AAAA records\n  reach : 0x00000002 (Reachable)\n",
            Some("route to: default\n destination: default\n gateway: 192.0.2.1\n interface: en0\n"),
        ).unwrap();
        assert_eq!(result.primary_ipv4_service.as_deref(), Some("Wi-Fi"));
        assert_eq!(result.resolvers.len(), 3);
        assert_eq!(result.resolvers[0].nameservers.len(), 3);
        assert_eq!(result.default_dns_servers.len(), 3);
        assert_eq!(result.resolvers[0].interface.as_deref(), Some("en0"));
        assert!(result.resolvers[1].nameservers.is_empty());
        assert_eq!(result.resolvers[1].options.as_deref(), Some("mdns"));
        assert_eq!(
            result.resolvers[2].section,
            "DNS configuration (for scoped queries)"
        );
        assert!(!result.trial_ready);
    }

    #[test]
    fn rejects_malformed_and_oversized_output() {
        assert!(parse_service_order("(1) Wi-Fi\n").is_err());
        assert!(parse_resolvers("resolver #1\n nameserver[0] : 1.1.1.1\n").is_err());
        assert!(parse_default_route("interface: en0\ninterface: en1\n").is_err());
        assert!(parse_resolvers(&"x".repeat(MAX_OUTPUT + 1)).is_err());
        assert!(parse_resolvers("DNS configuration\nresolver #1\n  flags : \n").is_ok());
    }

    #[test]
    fn only_matching_native_service_identity_is_selected() {
        let report = preflight_with(
            services(),
            "(1) Wi-Fi\n(Hardware Port: Wi-Fi, Device: en0)\n",
            "DNS configuration\nresolver #1\n nameserver[0] : 192.0.2.1\n",
            Some("interface: en0\n"),
        )
        .unwrap();
        let snapshot = IdentitySnapshot {
            current_set_id: "set-123".into(),
            services: vec![ServiceIdentity {
                id: "service-456".into(),
                name: "Wi-Fi".into(),
                enabled: true,
                device: Some("en0".into()),
            }],
        };
        let matching = attach_identities(report, snapshot.clone());
        assert_eq!(
            matching.primary_ipv4_service_id.as_deref(),
            Some("service-456")
        );
        assert_eq!(matching.current_set_id.as_deref(), Some("set-123"));
        assert!(!matching.trial_ready);
        let mismatched = attach_identities(
            preflight_with(
                services(),
                "(1) Wi-Fi\n(Hardware Port: Wi-Fi, Device: en0)\n",
                "DNS configuration\nresolver #1\n nameserver[0] : 192.0.2.1\n",
                Some("interface: en0\n"),
            )
            .unwrap(),
            IdentitySnapshot {
                services: vec![ServiceIdentity {
                    device: Some("en1".into()),
                    ..snapshot.services[0].clone()
                }],
                ..snapshot
            },
        );
        assert_eq!(mismatched.primary_ipv4_service_id, None);
        assert!(mismatched
            .warnings
            .iter()
            .any(|warning| warning.contains("did not map uniquely")));
    }

    #[test]
    fn notes_explicit_dns_on_other_enabled_services() {
        let report = preflight_with(
            vec![
                Service {
                    name: "Wi-Fi".into(),
                    enabled: true,
                    configured_dns: Some(vec!["192.0.2.1".parse().unwrap()]),
                },
                Service {
                    name: "iPhone USB".into(),
                    enabled: true,
                    configured_dns: Some(vec!["192.0.2.1".parse().unwrap()]),
                },
                Service {
                    name: "Disabled Bridge".into(),
                    enabled: false,
                    configured_dns: Some(vec!["192.0.2.1".parse().unwrap()]),
                },
            ],
            "(1) Wi-Fi\n(Hardware Port: Wi-Fi, Device: en0)\n(2) iPhone USB\n(Hardware Port: iPhone USB, Device: en5)\n",
            "DNS configuration\nresolver #1\n nameserver[0] : 192.0.2.1\n",
            Some("interface: en0\n"),
        )
        .unwrap();
        assert_eq!(report.other_services_with_configured_dns, ["iPhone USB"]);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("Other enabled network services")));
        assert!(!report.trial_ready);
    }

    #[test]
    fn does_not_call_services_other_when_primary_is_unknown() {
        let report = preflight_with(
            vec![Service {
                name: "Wi-Fi".into(),
                enabled: true,
                configured_dns: Some(vec!["192.0.2.1".parse().unwrap()]),
            }],
            "(1) Wi-Fi\n(Hardware Port: Wi-Fi, Device: en0)\n",
            "DNS configuration\nresolver #1\n nameserver[0] : 192.0.2.1\n",
            Some("interface: utun9\n"),
        )
        .unwrap();
        assert!(report.primary_ipv4_service.is_none());
        assert!(report.other_services_with_configured_dns.is_empty());
        assert!(!report.trial_ready);
    }
}
