//! Read-only macOS network-service inventory. This is deliberately separate
//! from `Platform`: service names alone cannot identify a recovery target.
#[cfg(target_os = "macos")]
use super::macos_command::{self, Budget};
use serde::Serialize;
use std::net::IpAddr;

#[cfg(any(test, target_os = "macos"))]
const MAX_OUTPUT: usize = 64 * 1024;
#[cfg(any(test, target_os = "macos"))]
const MAX_SERVICES: usize = 64;

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub name: String,
    pub enabled: bool,
    /// None means no DNS servers explicitly configured for this service.
    /// It does not describe DNS supplied by DHCP, VPN, or other resolvers.
    pub configured_dns: Option<Vec<IpAddr>>,
}

#[cfg(any(test, target_os = "macos"))]
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && !name.starts_with('-')
        && !name.chars().any(char::is_control)
}

#[cfg(any(test, target_os = "macos"))]
fn parse_services(output: &str) -> Result<Vec<(String, bool)>, String> {
    let mut lines = output.lines();
    if lines.next().map(str::trim)
        != Some("An asterisk (*) denotes that a network service is disabled.")
    {
        return Err("Unrecognized networksetup service inventory".into());
    }
    let mut services = Vec::new();
    for line in lines {
        let (name, enabled) = match line.strip_prefix('*') {
            Some(name) => (name, false),
            None => (line, true),
        };
        if !valid_name(name) || services.iter().any(|(seen, _)| seen == name) {
            return Err("Invalid or duplicate macOS network service name".into());
        }
        services.push((name.to_owned(), enabled));
        if services.len() > MAX_SERVICES {
            return Err("Too many macOS network services".into());
        }
    }
    Ok(services)
}

#[cfg(any(test, target_os = "macos"))]
fn parse_dns(output: &str, name: &str) -> Result<Option<Vec<IpAddr>>, String> {
    let trimmed = output.trim();
    if trimmed == format!("There aren't any DNS Servers set on {name}.") {
        return Ok(None);
    }
    let mut servers = Vec::new();
    for line in output.lines() {
        let server: IpAddr = line
            .trim()
            .parse()
            .map_err(|_| "Unrecognized networksetup DNS output")?;
        servers.push(server);
        if servers.len() > 64 {
            return Err("Too many configured DNS servers".into());
        }
    }
    if servers.is_empty() {
        return Err("Empty networksetup DNS output".into());
    }
    Ok(Some(servers))
}

#[cfg(any(test, target_os = "macos"))]
fn inspect_with(
    mut run: impl FnMut(&[&str]) -> Result<String, String>,
) -> Result<Vec<Service>, String> {
    let inventory = run(&["-listallnetworkservices"])?;
    if inventory.len() > MAX_OUTPUT {
        return Err("networksetup output too large".into());
    }
    let mut result = Vec::new();
    for (name, enabled) in parse_services(&inventory)? {
        let output = run(&["-getdnsservers", &name])?;
        if output.len() > MAX_OUTPUT {
            return Err("networksetup output too large".into());
        }
        result.push(Service {
            configured_dns: parse_dns(&output, &name)?,
            name,
            enabled,
        });
    }
    Ok(result)
}

#[cfg(target_os = "macos")]
pub fn inspect() -> Result<Vec<Service>, String> {
    inspect_with_budget(Budget::new(std::time::Duration::from_secs(15)))
}

#[cfg(target_os = "macos")]
pub(crate) fn inspect_with_budget(budget: Budget) -> Result<Vec<Service>, String> {
    inspect_with(|args| macos_command::run("/usr/sbin/networksetup", args, budget, MAX_OUTPUT))
}

#[cfg(not(target_os = "macos"))]
pub fn inspect() -> Result<Vec<Service>, String> {
    Err("Mac DNS inspection is available only on macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVENTORY: &str =
        "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Thunderbolt Bridge\n";

    #[test]
    fn reads_enabled_and_disabled_services_with_configured_dns() {
        let mut calls = Vec::new();
        let services = inspect_with(|args| {
            calls.push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
            Ok(match args {
                ["-listallnetworkservices"] => INVENTORY.into(),
                ["-getdnsservers", "Wi-Fi"] => "1.1.1.1\n2606:4700:4700::1111\n".into(),
                ["-getdnsservers", "Thunderbolt Bridge"] => {
                    "There aren't any DNS Servers set on Thunderbolt Bridge.\n".into()
                }
                _ => panic!("unexpected command"),
            })
        })
        .unwrap();
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].name, "Wi-Fi");
        assert!(services[0].enabled);
        assert_eq!(services[0].configured_dns.as_ref().unwrap().len(), 2);
        assert!(!services[1].enabled);
        assert_eq!(services[1].configured_dns, None);
        assert_eq!(calls[0], ["-listallnetworkservices"]);
    }

    #[test]
    fn rejects_ambiguous_output_and_option_like_names() {
        assert!(parse_services("Wi-Fi\n").is_err());
        assert!(parse_services(
            "An asterisk (*) denotes that a network service is disabled.\n-foo\n"
        )
        .is_err());
        assert!(parse_dns("DNS supplied by VPN\n", "Wi-Fi").is_err());
        assert!(parse_dns("", "Wi-Fi").is_err());
    }

    #[test]
    fn propagates_command_errors() {
        assert!(inspect_with(|_| Err("command failed".into())).is_err());
    }
}
