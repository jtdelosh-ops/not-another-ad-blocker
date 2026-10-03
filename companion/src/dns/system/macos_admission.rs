//! Conservative, read-only policy admission for the internal Mac trial.
//! A candidate is not authorization, a recovery snapshot, or proof that a
//! managed/VPN network is absent. Public activation remains unavailable.
use super::{
    macos_identity::DnsConfigurationState,
    macos_preflight::{Preflight, Resolver},
};
use std::{collections::HashSet, net::IpAddr};

#[derive(Debug)]
pub struct Candidate {
    pub set_id: String,
    pub service_id: String,
    pub device: String,
    pub servers: Vec<IpAddr>,
    pub configured_servers: Option<Vec<IpAddr>>,
}

pub(crate) fn allowed_server(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.octets()[0] == 0)
        }
        IpAddr::V6(ip) => {
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || ip.is_unicast_link_local()
                || ip.to_ipv4_mapped().is_some())
        }
    }
}

pub(crate) fn domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value
            .strip_suffix('.')
            .unwrap_or(value)
            .split('.')
            .all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
}

fn servers(values: &[String]) -> Result<Vec<IpAddr>, String> {
    let result: Vec<IpAddr> = values
        .iter()
        .map(|s| {
            s.parse()
                .map_err(|_| "DNS server is not an unscoped IP address")
        })
        .collect::<Result<_, _>>()?;
    if result.is_empty()
        || result.len() > 8
        || result.iter().any(|ip| !allowed_server(ip))
        || result.iter().collect::<HashSet<_>>().len() != result.len()
    {
        return Err("DNS server list is unsupported or ambiguous".into());
    }
    Ok(result)
}

fn ordinary_fields(resolver: &Resolver) -> bool {
    resolver.other_fields.iter().all(|line| {
        let Some((key, value)) = line.split_once(':') else {
            return false;
        };
        let (key, value) = (key.trim(), value.trim());
        if let Some(index) = key
            .strip_prefix("search domain[")
            .and_then(|s| s.strip_suffix(']'))
        {
            return index.parse::<usize>().is_ok() && domain(value);
        }
        match key {
            "order" | "timeout" => value.parse::<u32>().is_ok(),
            "reach" => value
                .split_whitespace()
                .next()
                .and_then(|s| s.strip_prefix("0x"))
                .is_some_and(|s| u32::from_str_radix(s, 16).is_ok()),
            _ => false,
        }
    })
}

pub fn candidate(report: &Preflight) -> Result<Candidate, String> {
    let set_id = report
        .current_set_id
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or("Missing Mac location identity")?;
    let service_id = report
        .primary_ipv4_service_id
        .as_ref()
        .ok_or("Missing primary service identity")?;
    let name = report
        .primary_ipv4_service
        .as_ref()
        .ok_or("Missing primary service name")?;
    let device = report
        .primary_ipv4_interface
        .as_ref()
        .ok_or("Missing primary interface")?;
    if !device
        .strip_prefix("en")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("Only one primary en-numbered interface is supported".into());
    }
    let identities: Vec<_> = report
        .service_identities
        .iter()
        .filter(|s| s.id == *service_id || s.name == *name || s.device.as_ref() == Some(device))
        .collect();
    if identities.len() != 1
        || !identities[0].enabled
        || identities[0].id != *service_id
        || identities[0].name != *name
        || identities[0].device.as_ref() != Some(device)
    {
        return Err("Primary service identity is ambiguous or changed".into());
    }
    let matches: Vec<_> = report.services.iter().filter(|s| s.name == *name).collect();
    if matches.len() != 1
        || !matches[0].enabled
        || report
            .services
            .iter()
            .any(|s| s.enabled && s.name != *name && s.configured_dns.is_some())
        || !report.other_services_with_configured_dns.is_empty()
    {
        return Err("Other services or ambiguous inventory control DNS".into());
    }
    let protocol = report
        .primary_service_dns_protocol
        .as_ref()
        .ok_or("Missing exact DNS protocol snapshot")?;
    if protocol.service_id != *service_id
        || !protocol.protocol_present
        || !protocol.protocol_enabled
        || protocol.configuration_state != DnsConfigurationState::Saved
        || !protocol.configuration_present
        || !(1..=32 * 1024).contains(&protocol.configuration_bytes)
        || !protocol
            .configuration_sha256
            .as_ref()
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("DNS protocol cannot be restored exactly".into());
    }
    let default = servers(&report.default_dns_servers)?;
    if matches[0]
        .configured_dns
        .as_ref()
        .is_some_and(|v| v != &default)
    {
        return Err("Configured and effective DNS servers differ".into());
    }
    let mut seen = HashSet::new();
    let mut default_count = 0;
    let mut scoped_count = 0;
    let mut default_queries = 0;
    let mut scoped_queries = 0;
    for resolver in &report.resolvers {
        let scoped = match resolver.section.as_str() {
            "DNS configuration" => false,
            "DNS configuration (for scoped queries)" => true,
            _ => return Err("Unknown resolver policy section".into()),
        };
        if !seen.insert((&resolver.section, resolver.number)) || !ordinary_fields(resolver) {
            return Err("Ambiguous or unsupported resolver fields".into());
        }
        let flags: Vec<_> = resolver
            .flags
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if flags
            .iter()
            .any(|s| !matches!(*s, "Request A records" | "Request AAAA records" | "Scoped"))
            || flags.contains(&"Scoped") != scoped
            || flags.iter().collect::<HashSet<_>>().len() != flags.len()
        {
            return Err("Supplemental, scoped, or unknown resolver flags are unsupported".into());
        }
        if resolver.options.as_deref() == Some("mdns") {
            if scoped
                || !resolver.nameservers.is_empty()
                || resolver.interface.is_some()
                || !matches!(
                    resolver.domain.as_deref(),
                    Some(
                        "local"
                            | "254.169.in-addr.arpa"
                            | "8.e.f.ip6.arpa"
                            | "9.e.f.ip6.arpa"
                            | "a.e.f.ip6.arpa"
                            | "b.e.f.ip6.arpa"
                    )
                )
            {
                return Err("Nonstandard multicast resolver policy".into());
            }
            continue;
        }
        let queries = u8::from(flags.contains(&"Request A records"))
            | (u8::from(flags.contains(&"Request AAAA records")) << 1);
        if queries == 0 {
            return Err("Unicast DNS query-family flags are missing".into());
        }
        if resolver.domain.is_some()
            || resolver.options.is_some()
            || resolver.interface.as_ref().is_some_and(|v| v != device)
            || (scoped && resolver.interface.as_ref() != Some(device))
            || servers(&resolver.nameservers)? != default
        {
            return Err("Competing or domain-specific resolver policy".into());
        }
        if scoped {
            scoped_count += 1;
            scoped_queries = queries;
        } else {
            default_count += 1;
            default_queries = queries;
        }
    }
    if default_count != 1
        || scoped_count > 1
        || (scoped_count == 1 && scoped_queries != default_queries)
    {
        return Err("Expected one default DNS policy and at most its primary scoped copy".into());
    }
    Ok(Candidate {
        set_id: set_id.clone(),
        service_id: service_id.clone(),
        device: device.clone(),
        servers: default,
        configured_servers: matches[0].configured_dns.clone(),
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn admit_record(record: &super::macos_recovery::Record) -> Result<(), String> {
    use super::{macos_identity, macos_preflight, macos_recovery::Configuration};
    use sha2::{Digest, Sha256};
    let report = macos_preflight::preflight()?;
    let current = candidate(&report)?;
    if current.set_id != record.set_id
        || current.service_id != record.service_id
        || current.device != record.device
    {
        return Err("Mac trial target changed during admission".into());
    }
    let Configuration::Saved(original) = &record.original.configuration else {
        return Err("Missing exact original dictionary".into());
    };
    let digest = format!("{:x}", Sha256::digest(original));
    if report
        .primary_service_dns_protocol
        .as_ref()
        .and_then(|s| s.configuration_sha256.as_ref())
        != Some(&digest)
        || macos_identity::trial_dns_servers(original)? != current.configured_servers
    {
        return Err("Saved DNS dictionary changed or disagrees with service inventory".into());
    }
    let Configuration::Saved(applied) = &record.applied.configuration else {
        return Err("Missing applied DNS dictionary".into());
    };
    const LOOPBACK: &[u8] = br#"<plist version="1.0"><dict><key>ServerAddresses</key><array><string>127.0.0.1</string><string>::1</string></array></dict></plist>"#;
    if !macos_identity::equivalent_dns_configuration(applied, LOOPBACK)? {
        return Err("Mac trial applies only the fixed loopback DNS dictionary".into());
    }
    Ok(())
}
