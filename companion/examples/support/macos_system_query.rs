//! Native system-resolver evidence for the disposable Mac fixture only.
//! Unlike dns-sd's CLI output, the callback distinguishes a negative answer
//! (-65554) from timeout (-65568). Neither changes network settings.
use std::{
    ffi::{c_char, c_void, CString},
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

// Apple's Clients/Makefile documents that these functions are re-exported by
// libSystem on macOS, which Rust already links. -ldns_sd is for other platforms.
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

struct Context {
    name: CString,
    result: Option<Result<(), String>>,
}

unsafe extern "C" fn reply(
    _: Service,
    flags: u32,
    _: u32,
    error: i32,
    _: *const c_char,
    _: u16,
    _: u16,
    _: u16,
    _: *const c_void,
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
        state.result = Some(if error == -65554 {
            Ok(())
        } else {
            Err(format!("System DNS callback error {error}"))
        });
        return;
    }
    if flags & 2 == 0 {
        return;
    } // Ignore remove events.
    state.result = Some(Err(
        "Expected a negative system DNS answer, got a positive answer".into(),
    ));
}

pub(super) fn query_negative(
    name: &str,
    mut guard: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let mut context = Context {
        name: CString::new(name).map_err(|e| e.to_string())?,
        result: None,
    };
    let mut service = std::ptr::null_mut();
    // ReturnIntermediates delivers negative answers; Timeout remains an error.
    // Interface 0 lets macOS select the resolver, as ordinary apps do.
    let status = unsafe {
        DNSServiceQueryRecord(
            &mut service,
            0x1000 | 0x10000,
            0,
            context.name.as_ptr(),
            1, // A
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
    fn positive_answers_are_not_negative_evidence() {
        for kind in [1, 16] {
            let mut context = Context {
                name: CString::new("unique.example.com.").unwrap(),
                result: None,
            };
            let name = context.name.clone();
            let data = [192, 0, 2, 1];
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
            assert!(context.result.unwrap().is_err());
        }
    }
}
