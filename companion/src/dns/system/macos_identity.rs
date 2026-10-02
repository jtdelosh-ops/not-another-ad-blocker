//! Read-only identities from macOS System Configuration. Service display names
//! and BSD device names are diagnostics; the set/service IDs identify settings.
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceIdentity {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub device: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentitySnapshot {
    pub current_set_id: String,
    pub services: Vec<ServiceIdentity>,
}

/// A read-only observation of the complete DNS protocol property list. The
/// serialized bytes are kept private so preflight does not publish search
/// domains or other configuration values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsProtocolSnapshot {
    pub current_set_id: String,
    pub service_id: String,
    pub protocol_present: bool,
    pub protocol_enabled: bool,
    pub configuration_xml: Option<Vec<u8>>,
    /// System Configuration's status immediately after a NULL configuration
    /// result. None when a dictionary was returned.
    pub null_configuration_status: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsProtocolSummary {
    pub service_id: String,
    pub protocol_present: bool,
    pub protocol_enabled: bool,
    pub configuration_present: bool,
    pub configuration_bytes: usize,
    pub configuration_sha256: Option<String>,
    pub null_configuration_status: Option<i32>,
}

impl DnsProtocolSnapshot {
    pub fn summary(&self) -> DnsProtocolSummary {
        DnsProtocolSummary {
            service_id: self.service_id.clone(),
            protocol_present: self.protocol_present,
            protocol_enabled: self.protocol_enabled,
            configuration_present: self.configuration_xml.is_some(),
            configuration_bytes: self.configuration_xml.as_ref().map_or(0, Vec::len),
            configuration_sha256: self.configuration_xml.as_ref().map(|xml| {
                Sha256::digest(xml)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect()
            }),
            null_configuration_status: self.null_configuration_status,
        }
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::{IdentitySnapshot, ServiceIdentity};
    use std::ffi::{c_char, c_void, CStr};
    use std::ptr;

    type Cf = *const c_void;
    const UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(allocator: Cf, text: *const c_char, encoding: u32) -> Cf;
        fn CFStringGetCString(value: Cf, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFArrayGetCount(array: Cf) -> isize;
        fn CFArrayGetValueAtIndex(array: Cf, index: isize) -> Cf;
        fn CFRelease(value: Cf);
        fn CFPropertyListCreateData(
            allocator: Cf,
            property_list: Cf,
            format: isize,
            options: usize,
            error: *mut Cf,
        ) -> Cf;
        fn CFDataGetLength(data: Cf) -> isize;
        fn CFDataGetBytePtr(data: Cf) -> *const u8;
        fn CFDataCreate(allocator: Cf, bytes: *const u8, length: isize) -> Cf;
        fn CFPropertyListCreateWithData(
            allocator: Cf,
            data: Cf,
            options: usize,
            format: *mut isize,
            error: *mut Cf,
        ) -> Cf;
        fn CFGetTypeID(value: Cf) -> usize;
        fn CFDictionaryGetTypeID() -> usize;
        fn CFEqual(left: Cf, right: Cf) -> u8;
    }

    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCPreferencesCreate(allocator: Cf, name: Cf, prefs_id: Cf) -> Cf;
        fn SCNetworkSetCopyCurrent(preferences: Cf) -> Cf;
        fn SCNetworkSetGetSetID(set: Cf) -> Cf;
        fn SCNetworkSetCopyServices(set: Cf) -> Cf;
        fn SCNetworkServiceGetServiceID(service: Cf) -> Cf;
        fn SCNetworkServiceGetName(service: Cf) -> Cf;
        fn SCNetworkServiceGetEnabled(service: Cf) -> u8;
        fn SCNetworkServiceGetInterface(service: Cf) -> Cf;
        fn SCNetworkInterfaceGetBSDName(interface: Cf) -> Cf;
        fn SCNetworkServiceCopyProtocol(service: Cf, protocol_type: Cf) -> Cf;
        fn SCNetworkProtocolGetEnabled(protocol: Cf) -> u8;
        fn SCNetworkProtocolGetConfiguration(protocol: Cf) -> Cf;
        fn SCError() -> i32;
        static kSCNetworkProtocolTypeDNS: Cf;
    }

    struct Owned(Cf);

    impl Owned {
        fn new(value: Cf, what: &str) -> Result<Self, String> {
            if value.is_null() {
                Err(format!("macOS System Configuration returned no {what}"))
            } else {
                Ok(Self(value))
            }
        }
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            // Every value wrapped here is returned by a Create/Copy function.
            unsafe { CFRelease(self.0) }
        }
    }

    fn string(value: Cf, what: &str) -> Result<String, String> {
        if value.is_null() {
            return Err(format!("macOS System Configuration returned no {what}"));
        }
        let mut buffer = [0i8; 1025];
        // The fixed bound rejects unexpectedly large labels instead of truncating
        // an identifier that might later be mistaken for another target.
        if unsafe { CFStringGetCString(value, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) }
            == 0
        {
            return Err(format!("Invalid or oversized macOS {what}"));
        }
        let text = unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_str()
            .map_err(|_| format!("Invalid UTF-8 macOS {what}"))?;
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err(format!("Invalid macOS {what}"));
        }
        Ok(text.to_owned())
    }

    pub fn inspect() -> Result<IdentitySnapshot, String> {
        let name = Owned::new(
            unsafe {
                CFStringCreateWithCString(
                    ptr::null(),
                    c"NAAB DNS read-only identity".as_ptr(),
                    UTF8,
                )
            },
            "process name",
        )?;
        let preferences = Owned::new(
            unsafe { SCPreferencesCreate(ptr::null(), name.0, ptr::null()) },
            "preferences session",
        )?;
        let set = Owned::new(
            unsafe { SCNetworkSetCopyCurrent(preferences.0) },
            "current set",
        )?;
        let current_set_id = string(unsafe { SCNetworkSetGetSetID(set.0) }, "set ID")?;
        let array = Owned::new(unsafe { SCNetworkSetCopyServices(set.0) }, "service list")?;
        let count = unsafe { CFArrayGetCount(array.0) };
        if !(0..=64).contains(&count) {
            return Err("Too many macOS network services".into());
        }
        let mut services = Vec::with_capacity(count as usize);
        for index in 0..count {
            let service = unsafe { CFArrayGetValueAtIndex(array.0, index) };
            if service.is_null() {
                return Err("Missing macOS network service".into());
            }
            let interface = unsafe { SCNetworkServiceGetInterface(service) };
            let device = if interface.is_null() {
                None
            } else {
                let bsd = unsafe { SCNetworkInterfaceGetBSDName(interface) };
                (!bsd.is_null())
                    .then(|| string(bsd, "BSD device"))
                    .transpose()?
            };
            services.push(ServiceIdentity {
                id: string(
                    unsafe { SCNetworkServiceGetServiceID(service) },
                    "service ID",
                )?,
                name: string(unsafe { SCNetworkServiceGetName(service) }, "service name")?,
                enabled: unsafe { SCNetworkServiceGetEnabled(service) } != 0,
                device,
            });
        }
        services.sort_by(|a, b| a.id.cmp(&b.id));
        if services.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err("Duplicate macOS service ID".into());
        }
        Ok(IdentitySnapshot {
            current_set_id,
            services,
        })
    }

    pub fn inspect_dns_protocol(
        expected_set_id: &str,
        expected_service_id: &str,
    ) -> Result<super::DnsProtocolSnapshot, String> {
        let name = Owned::new(
            unsafe {
                CFStringCreateWithCString(
                    ptr::null(),
                    c"NAAB DNS read-only protocol".as_ptr(),
                    UTF8,
                )
            },
            "process name",
        )?;
        let preferences = Owned::new(
            unsafe { SCPreferencesCreate(ptr::null(), name.0, ptr::null()) },
            "preferences session",
        )?;
        let set = Owned::new(
            unsafe { SCNetworkSetCopyCurrent(preferences.0) },
            "current set",
        )?;
        let current_set_id = string(unsafe { SCNetworkSetGetSetID(set.0) }, "set ID")?;
        if current_set_id != expected_set_id {
            return Err("Mac network location changed during preflight".into());
        }
        let services = Owned::new(unsafe { SCNetworkSetCopyServices(set.0) }, "service list")?;
        let count = unsafe { CFArrayGetCount(services.0) };
        if !(0..=64).contains(&count) {
            return Err("Too many macOS network services".into());
        }
        let mut selected = None;
        for index in 0..count {
            let service = unsafe { CFArrayGetValueAtIndex(services.0, index) };
            if service.is_null() {
                return Err("Missing macOS network service".into());
            }
            if string(
                unsafe { SCNetworkServiceGetServiceID(service) },
                "service ID",
            )? == expected_service_id
            {
                if selected.replace(service).is_some() {
                    return Err("Duplicate macOS service ID".into());
                }
            }
        }
        let service = selected.ok_or("Primary service disappeared during preflight")?;
        let protocol = unsafe { SCNetworkServiceCopyProtocol(service, kSCNetworkProtocolTypeDNS) };
        if protocol.is_null() {
            return Ok(super::DnsProtocolSnapshot {
                current_set_id,
                service_id: expected_service_id.to_owned(),
                protocol_present: false,
                protocol_enabled: false,
                configuration_xml: None,
                null_configuration_status: None,
            });
        }
        let protocol = Owned::new(protocol, "DNS protocol")?;
        let configuration = unsafe { SCNetworkProtocolGetConfiguration(protocol.0) };
        let null_configuration_status = configuration.is_null().then(|| unsafe { SCError() });
        let configuration_xml = if configuration.is_null() {
            None
        } else {
            // XML property lists retain every key and type. Do not interpret a
            // missing dictionary as automatic DNS: the API also returns NULL
            // on error, so a future writer must resolve that ambiguity.
            let data = Owned::new(
                unsafe {
                    CFPropertyListCreateData(
                        ptr::null(),
                        configuration,
                        100, // kCFPropertyListXMLFormat_v1_0
                        0,
                        ptr::null_mut(),
                    )
                },
                "DNS protocol property list",
            )?;
            let length = unsafe { CFDataGetLength(data.0) };
            if !(0..=128 * 1024).contains(&length) {
                return Err("Mac DNS protocol configuration is too large".into());
            }
            let bytes = unsafe { CFDataGetBytePtr(data.0) };
            if bytes.is_null() && length != 0 {
                return Err("Mac DNS protocol bytes are unavailable".into());
            }
            Some(if length == 0 {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(bytes, length as usize) }.to_vec()
            })
        };
        Ok(super::DnsProtocolSnapshot {
            current_set_id,
            service_id: expected_service_id.to_owned(),
            protocol_present: true,
            protocol_enabled: unsafe { SCNetworkProtocolGetEnabled(protocol.0) } != 0,
            configuration_xml,
            null_configuration_status,
        })
    }

    fn parse_dns_dictionary(xml: &[u8]) -> Result<Owned, String> {
        if xml.is_empty() || xml.len() > 128 * 1024 {
            return Err("Invalid Mac DNS protocol property-list length".into());
        }
        let data = Owned::new(
            unsafe { CFDataCreate(ptr::null(), xml.as_ptr(), xml.len() as isize) },
            "DNS protocol data",
        )?;
        let value = Owned::new(
            unsafe {
                CFPropertyListCreateWithData(
                    ptr::null(),
                    data.0,
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            },
            "DNS protocol property list",
        )?;
        if unsafe { CFGetTypeID(value.0) } != unsafe { CFDictionaryGetTypeID() } {
            return Err("Mac DNS protocol property list is not a dictionary".into());
        }
        Ok(value)
    }

    pub fn equivalent_dns_configuration(left: &[u8], right: &[u8]) -> Result<bool, String> {
        let left = parse_dns_dictionary(left)?;
        let right = parse_dns_dictionary(right)?;
        Ok(unsafe { CFEqual(left.0, right.0) } != 0)
    }
}

#[cfg(target_os = "macos")]
pub fn inspect() -> Result<IdentitySnapshot, String> {
    native::inspect()
}

#[cfg(target_os = "macos")]
pub fn inspect_dns_protocol(
    expected_set_id: &str,
    expected_service_id: &str,
) -> Result<DnsProtocolSnapshot, String> {
    native::inspect_dns_protocol(expected_set_id, expected_service_id)
}

#[cfg(not(target_os = "macos"))]
pub fn inspect_dns_protocol(_: &str, _: &str) -> Result<DnsProtocolSnapshot, String> {
    Err("Mac DNS protocol inspection is available only on macOS".into())
}

/// Compare complete native DNS dictionaries by value, not serialized byte order.
/// A null configuration must be classified separately before it is comparable.
#[cfg(target_os = "macos")]
pub fn equivalent_dns_configuration(left: &[u8], right: &[u8]) -> Result<bool, String> {
    native::equivalent_dns_configuration(left, right)
}

#[cfg(not(target_os = "macos"))]
pub fn equivalent_dns_configuration(_: &[u8], _: &[u8]) -> Result<bool, String> {
    Err("Mac DNS protocol comparison is available only on macOS".into())
}

#[cfg(not(target_os = "macos"))]
pub fn inspect() -> Result<IdentitySnapshot, String> {
    Err("Mac service identities are available only on macOS".into())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn native_identity_reader_works_without_elevation() {
        let snapshot = super::inspect().expect("read current macOS network configuration");
        assert!(!snapshot.current_set_id.is_empty());
        assert!(snapshot
            .services
            .iter()
            .all(|service| !service.id.is_empty()));
        if let Some(service) = snapshot.services.iter().find(|service| service.enabled) {
            let dns = super::inspect_dns_protocol(&snapshot.current_set_id, &service.id)
                .expect("read macOS DNS protocol without elevation");
            assert_eq!(dns.service_id, service.id);
        }
    }

    #[test]
    fn protocol_dictionary_comparison_ignores_xml_key_order() {
        let left = br#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>ServerAddresses</key><array><string>192.0.2.1</string></array><key>SearchDomains</key><array><string>example.test</string></array></dict></plist>"#;
        let right = br#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>SearchDomains</key><array><string>example.test</string></array><key>ServerAddresses</key><array><string>192.0.2.1</string></array></dict></plist>"#;
        assert!(super::equivalent_dns_configuration(left, right).unwrap());
        assert!(super::equivalent_dns_configuration(left, b"not a plist").is_err());
    }
}

#[cfg(test)]
mod summary_tests {
    use super::DnsProtocolSnapshot;

    #[test]
    fn summary_does_not_expose_protocol_values() {
        let snapshot = DnsProtocolSnapshot {
            current_set_id: "set".into(),
            service_id: "service".into(),
            protocol_present: true,
            protocol_enabled: true,
            configuration_xml: Some(b"secret search domain".to_vec()),
            null_configuration_status: None,
        };
        let json = serde_json::to_string(&snapshot.summary()).unwrap();
        assert!(!json.contains("secret"));
        assert!(json.contains("configurationSha256"));
        assert!(json.contains("configurationBytes"));
    }

    #[test]
    fn null_configuration_keeps_diagnostic_status_without_inferred_mode() {
        let snapshot = DnsProtocolSnapshot {
            current_set_id: "set".into(),
            service_id: "service".into(),
            protocol_present: true,
            protocol_enabled: true,
            configuration_xml: None,
            null_configuration_status: Some(0),
        };
        let summary = snapshot.summary();
        assert!(!summary.configuration_present);
        assert_eq!(summary.null_configuration_status, Some(0));
    }
}
