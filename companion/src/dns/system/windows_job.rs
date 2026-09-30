//! A private, non-inheritable job bounds the bridge and every native child it
//! starts. Closing the sole job handle also closes descendants' inherited pipes.
use std::{
    ffi::c_void,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::Child,
};

#[repr(C)]
#[derive(Default)]
struct BasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
#[derive(Default)]
struct ExtendedLimitInformation {
    basic_limit_information: BasicLimitInformation,
    // IO_COUNTERS contains six consecutive ULONGLONG values.
    io_counters: [u64; 6],
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
    fn SetInformationJobObject(
        job: *mut c_void,
        class: i32,
        information: *const c_void,
        length: u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
}

pub(super) struct ProcessJob(OwnedHandle);

impl ProcessJob {
    pub(super) fn new() -> io::Result<Self> {
        // SAFETY: null attributes create a non-inheritable handle; null name
        // creates a fresh private job rather than opening any existing job.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateJobObjectW returned a valid, uniquely owned handle.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
        let information = ExtendedLimitInformation {
            basic_limit_information: BasicLimitInformation {
                limit_flags: 0x2000, // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: repr(C) matches JOBOBJECT_EXTENDED_LIMIT_INFORMATION, including
        // pointer-sized SIZE_T/ULONG_PTR fields, and lives throughout this call.
        // Class 9 is JobObjectExtendedLimitInformation. Breakaway is not enabled.
        if unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                9,
                (&information as *const ExtendedLimitInformation).cast(),
                std::mem::size_of_val(&information) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    pub(super) fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are live for the duration of this call. This job
        // has no UI/security limits and supports nesting in an existing job.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
