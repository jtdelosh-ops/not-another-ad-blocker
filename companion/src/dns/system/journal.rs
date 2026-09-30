//! Bounded, exclusively locked local storage. Caller supplies a private, trusted
//! directory; this is not a privileged helper accepting user-controlled paths.
use super::{Journal, RecoveryRecord, Report};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_BYTES: u64 = 128 * 1024;

pub struct FileJournal {
    directory: PathBuf,
    // Never unlink the lock file: another process may already have opened it.
    _lock: File,
}

fn regular_or_missing(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err("Journal entries must be regular files, not links or directories".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

impl FileJournal {
    pub fn open(directory: &Path) -> Result<Self, String> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(directory).map_err(|e| e.to_string())?;
        if !fs::symlink_metadata(directory)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_dir()
        {
            return Err("Journal directory must not be a symlink".into());
        }
        let directory = directory.canonicalize().map_err(|e| e.to_string())?;
        let path = directory.join("controller.lock");
        regular_or_missing(&path)?;
        let lock = private_options()
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|e| format!("DNS journal is in use or cannot be locked: {e}"))?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }

    fn sync_directory(&self) -> Result<(), String> {
        #[cfg(unix)]
        File::open(&self.directory)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
        // Windows file contents are flushed below. Directory-entry durability
        // across power loss must be verified with the production platform helper.
        Ok(())
    }

    fn write_json(&self, name: &str, value: &impl serde::Serialize) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("DNS journal entry exceeds size limit".into());
        }
        let destination = self.directory.join(name);
        let temporary = self.directory.join(format!("{name}.pending"));
        regular_or_missing(&destination)?;
        regular_or_missing(&temporary)?;
        let mut file = private_options()
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        durable_rename(&temporary, &destination)?;
        self.sync_directory()
    }
}

#[cfg(not(windows))]
fn durable_rename(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn durable_rename(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<_> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both buffers are valid NUL-terminated UTF-16 paths for this call.
    // REPLACE_EXISTING | WRITE_THROUGH: keep the prior committed file on failure
    // and wait for the move to be flushed before changing operating-system DNS.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0x1 | 0x8) } == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

impl Journal for FileJournal {
    fn load(&mut self) -> Result<Option<RecoveryRecord>, String> {
        let path = self.directory.join("recovery.json");
        if !regular_or_missing(&path)? {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|e| e.to_string())?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("DNS journal entry exceeds size limit".into());
        }
        let record: RecoveryRecord =
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid recovery record: {e}"))?;
        record.validate()?;
        Ok(Some(record))
    }

    fn create(&mut self, record: &RecoveryRecord) -> Result<(), String> {
        record.validate()?;
        if regular_or_missing(&self.directory.join("recovery.json"))? {
            return Err("Recover the existing DNS session before enabling a new one".into());
        }
        self.write_json("recovery.json", record)
    }

    fn clear(&mut self) -> Result<(), String> {
        let path = self.directory.join("recovery.json");
        if regular_or_missing(&path)? {
            fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        self.sync_directory()
    }

    fn save_report(&mut self, report: &Report) -> Result<(), String> {
        // Successful later polls must not erase the most recent failure evidence.
        if report.is_incident() {
            self.write_json("last-incident.json", report)?;
        }
        self.write_json("last-report.json", report)
    }
}
