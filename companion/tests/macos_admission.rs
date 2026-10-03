use naab_companion::dns::system::{
    macos::Service,
    macos_admission::candidate,
    macos_identity::{DnsConfigurationState, DnsProtocolSummary, ServiceIdentity},
    macos_preflight::{Preflight, Resolver},
};

fn report() -> Preflight {
    let addresses = vec!["192.168.1.1".into(), "2001:db8::1".into()];
    let resolver = |scoped| Resolver {
        section: if scoped {
            "DNS configuration (for scoped queries)"
        } else {
            "DNS configuration"
        }
        .into(),
        number: 1,
        nameservers: addresses.clone(),
        domain: None,
        interface: Some("en0".into()),
        options: None,
        flags: Some(
            if scoped {
                "Scoped, Request A records, Request AAAA records"
            } else {
                "Request A records, Request AAAA records"
            }
            .into(),
        ),
        other_fields: vec![
            "search domain[0] : isp.example".into(),
            "reach : 0x00000002 (Reachable)".into(),
        ],
    };
    Preflight {
        services: vec![Service {
            name: "Wi-Fi".into(),
            enabled: true,
            configured_dns: None,
        }],
        primary_ipv4_interface: Some("en0".into()),
        primary_ipv4_service: Some("Wi-Fi".into()),
        primary_ipv4_service_id: Some("service".into()),
        current_set_id: Some("set".into()),
        service_identities: vec![ServiceIdentity {
            id: "service".into(),
            name: "Wi-Fi".into(),
            enabled: true,
            device: Some("en0".into()),
        }],
        primary_service_dns_protocol: Some(DnsProtocolSummary {
            service_id: "service".into(),
            protocol_present: true,
            protocol_enabled: true,
            configuration_state: DnsConfigurationState::Saved,
            configuration_present: true,
            configuration_bytes: 182,
            configuration_sha256: Some("a".repeat(64)),
            null_configuration_status: None,
        }),
        other_services_with_configured_dns: vec![],
        resolvers: vec![resolver(false), resolver(true)],
        default_dns_servers: addresses,
        warnings: vec![],
        trial_ready: false,
    }
}

#[test]
fn accepts_ordinary_scoped_copy_and_standard_mdns_without_public_approval() {
    let mut r = report();
    r.resolvers.push(Resolver {
        section: "DNS configuration".into(),
        number: 2,
        nameservers: vec![],
        domain: Some("local".into()),
        interface: None,
        options: Some("mdns".into()),
        flags: Some("Request A records, Request AAAA records".into()),
        other_fields: vec!["timeout : 5".into()],
    });
    let c = candidate(&r).unwrap();
    assert_eq!(c.device, "en0");
    assert_eq!(c.servers.len(), 2);
    assert!(!r.trial_ready);
    r.services[0].configured_dns = Some(c.servers.clone());
    assert!(candidate(&r).is_ok());
}

#[test]
fn refuses_unknown_supplemental_competing_or_ambiguous_resolver_policies() {
    for case in 0..9 {
        let mut r = report();
        match case {
            0 => r.resolvers[0].flags = Some("Supplemental, Request A records".into()),
            1 => r.resolvers[0].options = Some("private".into()),
            2 => r.resolvers[0].domain = Some("corp.example".into()),
            3 => r.resolvers[1].nameservers = vec!["10.0.0.1".into()],
            4 => r.resolvers[1].interface = Some("utun2".into()),
            5 => r.resolvers[1].section = "DNS configuration (for service-specific queries)".into(),
            6 => r.resolvers[0].other_fields.push("port : 5353".into()),
            7 => {
                let mut extra = report().resolvers.remove(0);
                extra.number = 3;
                r.resolvers.push(extra);
            }
            _ => r.resolvers.push(report().resolvers.remove(0)),
        }
        assert!(candidate(&r).is_err(), "accepted unsupported policy {case}");
    }
}

#[test]
fn refuses_missing_disabled_or_ambiguous_settings_identity() {
    for case in 0..7 {
        let mut r = report();
        match case {
            0 => r.primary_ipv4_interface = Some("utun0".into()),
            1 => r.service_identities.push(r.service_identities[0].clone()),
            2 => r.services[0].enabled = false,
            3 => {
                r.primary_service_dns_protocol
                    .as_mut()
                    .unwrap()
                    .configuration_state = DnsConfigurationState::Unknown
            }
            4 => {
                r.primary_service_dns_protocol
                    .as_mut()
                    .unwrap()
                    .protocol_enabled = false
            }
            5 => r.services.push(Service {
                name: "USB".into(),
                enabled: true,
                configured_dns: Some(vec!["10.0.0.1".parse().unwrap()]),
            }),
            _ => {
                r.primary_service_dns_protocol
                    .as_mut()
                    .unwrap()
                    .configuration_sha256 = None
            }
        }
        assert!(candidate(&r).is_err(), "accepted identity case {case}");
    }
}

#[test]
fn rejects_loopback_link_local_mapped_multicast_and_duplicate_servers() {
    for address in [
        "127.0.0.1",
        "::1",
        "169.254.1.1",
        "fe80::1",
        "fe80::1%en0",
        "::ffff:127.0.0.1",
        "255.255.255.255",
        "224.0.0.1",
        "::",
        "0.1.2.3",
    ] {
        let mut r = report();
        r.default_dns_servers = vec![address.into()];
        assert!(candidate(&r).is_err(), "accepted {address}");
    }
    let mut r = report();
    r.default_dns_servers = vec!["192.168.1.1".into(); 2];
    assert!(candidate(&r).is_err());
}

#[test]
fn static_order_must_agree_with_effective_dns() {
    let mut r = report();
    r.services[0].configured_dns = Some(
        r.default_dns_servers
            .iter()
            .rev()
            .map(|s| s.parse().unwrap())
            .collect(),
    );
    assert!(candidate(&r).is_err());
}

#[test]
fn unknown_mdns_policy_cannot_bypass_admission() {
    let mut r = report();
    r.resolvers[0].nameservers.clear();
    r.resolvers[0].options = Some("mdns".into());
    r.resolvers[0].domain = Some("corp.example".into());
    assert!(candidate(&r).is_err());
}

#[test]
fn requires_matching_nonempty_query_families_without_duplicate_flags() {
    for flags in [
        "Scoped",
        "Scoped, Request A records",
        "Scoped, Request AAAA records",
        "Scoped, Request A records, Request AAAA records, Request A records",
    ] {
        let mut r = report();
        r.resolvers[1].flags = Some(flags.into());
        assert!(candidate(&r).is_err(), "accepted {flags}");
    }
    let mut r = report();
    r.resolvers[0].flags = None;
    r.resolvers.remove(1);
    assert!(candidate(&r).is_err());
    let mut r = report();
    r.resolvers[0].flags = Some("Request A records".into());
    r.resolvers[1].flags = Some("Scoped, Request A records".into());
    assert!(candidate(&r).is_ok());
}

#[test]
fn accepts_one_root_dot_but_rejects_empty_labels() {
    for (domain, accepted) in [
        ("example.test", true),
        ("example.test.", true),
        ("example.test..", false),
        ("example..test", false),
    ] {
        let mut r = report();
        r.resolvers[0].other_fields[0] = format!("search domain[0] : {domain}");
        assert_eq!(candidate(&r).is_ok(), accepted, "{domain}");
    }
}
