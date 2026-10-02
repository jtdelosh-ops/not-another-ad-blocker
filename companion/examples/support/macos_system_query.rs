//! Native system-resolver evidence for the disposable Mac fixture only.
//! Unlike dns-sd's CLI output, the callback distinguishes a negative answer
//! (-65554) from timeout (-65568). Neither changes network settings.
use std::{
    ffi::{c_char, c_void, CStr, CString},
    time::{Duration, Instant},
};

type Service = *mut c_void;
type Reply = unsafe extern "C" fn(
    Service,
    u32,
    u32,
    i32,
    *const c_char,
    u16,
    u16,
    u16,
    *const c_void,
    u32,
    *mut c_void,
);

#[link(name = "dns_sd")]
unsafe extern "C" {
    fn DNSServiceQueryRecord(
        service: *mut Service,
        flags: u32,
        interface: u32,
        name: *const c_char,
        kind: u16,
        class: u16,
        callback: Reply,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceRefSockFD(service: Service) -> i32;
    fn DNSServiceProcessResult(service: Service) -> i32;
    fn DNSServiceRefDeallocate(service: Service);
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
unsafe extern "C" {
    fn poll(fds: *mut PollFd, count: u32, timeout: i32) -> i32;
}

struct Owned(Service);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { DNSServiceRefDeallocate(self.0) };
    }
}

pub(super) enum Expected<'a> {
    Token(&'a str),
    NoRecord,
}

struct Context {
    name: CString,
    token: Option<String>,
    result: Option<Result<(), String>>,
}

unsafe extern "C" fn reply(
    _: Service,
    flags: u32,
    _: u32,
    error: i32,
    name: *const c_char,
    kind: u16,
    class: u16,
    length: u16,
    data: *const c_void,
    _: u32,
    context: *mut c_void,
) {
    // Callback is invoked synchronously by ProcessResult on the query thread.
    // Context outlives the service and is not touched concurrently.
    let state = unsafe { &mut *context.cast::<Context>() };
    if state.result.is_some() {
        return;
    }
    if error != 0 {
        // All other callback arguments are undefined for errors; do not read
        // them. Only this exact code is valid negative-answer evidence.
        state.result = Some(if error == -65554 && state.token.is_none() {
            Ok(())
        } else {
            Err(format!("System DNS callback error {error}"))
        });
        return;
    }
    if flags & 2 == 0 {
        return;
    } // Ignore remove events.
    let valid = match &state.token {
        Some(token) => {
            !name.is_null()
                && !data.is_null()
                && kind == 16
                && class == 1
                && unsafe {
                    CStr::from_ptr(name)
                        .to_bytes()
                        .eq_ignore_ascii_case(state.name.as_bytes())
                }
                && usize::from(length) == token.len() + 1
                && {
                    let bytes = unsafe {
                        std::slice::from_raw_parts(data.cast::<u8>(), usize::from(length))
                    };
                    bytes.first().copied() == Some(token.len() as u8)
                        && &bytes[1..] == token.as_bytes()
                }
        }
        None => false,
    };
    state.result = Some(if valid {
        Ok(())
    } else {
        Err("Unexpected system DNS name, type, or answer data".into())
    });
}

pub(super) fn query(
    name: &str,
    expected: Expected<'_>,
    mut guard: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let mut context = Context {
        name: CString::new(name).map_err(|e| e.to_string())?,
        token: match expected {
            Expected::Token(value) => Some(value.to_owned()),
            Expected::NoRecord => None,
        },
        result: None,
    };
    let kind = if context.token.is_some() { 16 } else { 1 };
    let mut service = std::ptr::null_mut();
    // ReturnIntermediates delivers negative answers; Timeout remains an error.
    // Interface 0 lets macOS select the resolver, as ordinary apps do.
    let status = unsafe {
        DNSServiceQueryRecord(
            &mut service,
            0x1000 | 0x10000,
            0,
            context.name.as_ptr(),
            kind,
            1,
            reply,
            (&mut context as *mut Context).cast(),
        )
    };
    if status != 0 || service.is_null() {
        return Err(format!("System DNS query registration failed: {status}"));
    }
    // Dropped before context on every return, preventing callbacks after free.
    let service = Owned(service);
    let fd = unsafe { DNSServiceRefSockFD(service.0) };
    if fd < 0 {
        return Err("System DNS query has no event socket".into());
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        guard()?;
        let mut ready = PollFd {
            fd,
            events: 1,
            revents: 0,
        }; // POLLIN
        let status = unsafe { poll(&mut ready, 1, 100) };
        if status < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.to_string());
        }
        if ready.revents & !1 != 0 {
            return Err("System DNS query event socket failed".into());
        }
        if ready.revents & 1 != 0 {
            let status = unsafe { DNSServiceProcessResult(service.0) };
            if status != 0 {
                return Err(format!("System DNS event processing failed: {status}"));
            }
            if let Some(result) = context.result.take() {
                return result;
            }
        }
    }
    Err("System DNS query exceeded its 15-second deadline".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_is_never_a_negative_answer() {
        for (error, pass) in [(-65554, true), (-65568, false), (-65563, false)] {
            let mut context = Context {
                name: CString::new("unique.example.com.").unwrap(),
                token: None,
                result: None,
            };
            // Nonzero-error callback parameters are deliberately null/undefined.
            unsafe {
                reply(
                    std::ptr::null_mut(),
                    0,
                    0,
                    error,
                    std::ptr::null(),
                    0,
                    0,
                    0,
                    std::ptr::null(),
                    0,
                    (&mut context as *mut Context).cast(),
                )
            };
            assert_eq!(context.result.unwrap().is_ok(), pass);
        }
    }

    #[test]
    fn token_requires_exact_name_type_and_data() {
        for (name, kind, token, pass) in [
            ("fresh.naab-health.invalid.", 16, "token", true),
            ("other.naab-health.invalid.", 16, "token", false),
            ("fresh.naab-health.invalid.", 1, "token", false),
            ("fresh.naab-health.invalid.", 16, "wrong", false),
        ] {
            let mut context = Context {
                name: CString::new("fresh.naab-health.invalid.").unwrap(),
                token: Some("token".into()),
                result: None,
            };
            let name = CString::new(name).unwrap();
            let mut data = vec![token.len() as u8];
            data.extend_from_slice(token.as_bytes());
            unsafe {
                reply(
                    std::ptr::null_mut(),
                    2,
                    0,
                    0,
                    name.as_ptr(),
                    kind,
                    1,
                    data.len() as u16,
                    data.as_ptr().cast(),
                    0,
                    (&mut context as *mut Context).cast(),
                )
            };
            assert_eq!(context.result.unwrap().is_ok(), pass);
        }
    }
}
