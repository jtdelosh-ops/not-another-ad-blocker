//! Read-only identities from macOS System Configuration. Service display names
//! and BSD device names are diagnostics; the set/service IDs identify settings.
use serde::Serialize;

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
}

#[cfg(target_os = "macos")]
pub fn inspect() -> Result<IdentitySnapshot, String> {
    native::inspect()
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
    }
}
