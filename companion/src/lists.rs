//! Fixed-source downloads and immutable, bounded native-messaging snapshots.

use crate::subscription_rules;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub const MAX_LIST_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_LIST_LINES: usize = 300_000;
pub const MAX_PAGE_ITEMS: usize = 128;
pub const MAX_PAGE_BYTES: usize = 512 * 1024;
const MAX_INDEX_BYTES: usize = 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
const KEEP_SNAPSHOTS: usize = 3;
const MAX_NETWORK_BUDGET: usize = 29_800;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
struct Source {
    id: &'static str,
    title: &'static str,
    url: &'static str,
}

const SOURCES: [Source; 2] = [
    Source {
        id: "easylist",
        title: "EasyList",
        url: "https://easylist.to/easylist/easylist.txt",
    },
    Source {
        id: "easyprivacy",
        title: "EasyPrivacy",
        url: "https://easylist.to/easylist/easyprivacy.txt",
    },
];

#[derive(Debug)]
pub struct ListError {
    pub code: &'static str,
    pub message: String,
}

impl ListError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

type Result<T> = std::result::Result<T, ListError>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefreshRequest {
    pub ids: Vec<String>,
    pub network_budget: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PageKind {
    Network,
    Cosmetic,
    Diagnostics,
}

impl PageKind {
    fn label(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Cosmetic => "cosmetic",
            Self::Diagnostics => "diagnostics",
        }
    }
    fn field(self) -> &'static str {
        match self {
            Self::Network => "networkRules",
            Self::Cosmetic => "cosmeticRules",
            Self::Diagnostics => "diagnostics",
        }
    }
    fn maximum(self) -> usize {
        match self {
            Self::Network => MAX_NETWORK_BUDGET,
            Self::Cosmetic => 10_000,
            Self::Diagnostics => 200,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageRequest {
    pub snapshot_id: String,
    pub kind: PageKind,
    pub offset: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageIndex {
    kind: PageKind,
    offset: usize,
    total: usize,
    next_offset: Option<usize>,
    sha256: String,
    bytes: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotIndex {
    version: u32,
    manifest_sha256: String,
    pages: Vec<PageIndex>,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_refresh(request: &RefreshRequest) -> Result<Vec<Source>> {
    if request.ids.is_empty()
        || request.ids.len() > SOURCES.len()
        || request.network_budget > MAX_NETWORK_BUDGET
    {
        return Err(ListError::new(
            "INVALID_REQUEST",
            "Select one or both supported lists and a network budget from 0 to 29800",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut selected = vec![];
    for id in &request.ids {
        let source = SOURCES
            .iter()
            .find(|source| source.id == id)
            .ok_or_else(|| {
                ListError::new(
                    "INVALID_REQUEST",
                    "Only easylist and easyprivacy subscriptions are supported",
                )
            })?;
        if !seen.insert(id) {
            return Err(ListError::new(
                "INVALID_REQUEST",
                "Subscription IDs must be unique",
            ));
        }
        selected.push(*source);
    }
    selected.sort_by_key(|source| source.id);
    Ok(selected)
}

/// This environment override is intended for tests and portable installations.
/// Native requests never accept a directory, file name, or URL.
pub fn cache_root() -> Result<PathBuf> {
    if let Some(path) = env::var_os("NAAB_DATA_DIR").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    #[cfg(target_os = "windows")]
    let root = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("NotAnotherAdBlocker"));
    #[cfg(target_os = "macos")]
    let root = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join("Library/Application Support/NotAnotherAdBlocker"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let root = env::var_os("XDG_DATA_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|path| path.join(".local/share"))
        })
        .map(|path| path.join("not-another-ad-blocker"));
    root.ok_or_else(|| {
        ListError::new(
            "CACHE_UNAVAILABLE",
            "The user-local data directory is unavailable",
        )
    })
}

pub fn refresh(request: RefreshRequest) -> Result<Value> {
    validate_refresh(&request)?;
    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(25))
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("NotAnotherAdBlocker/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| {
            ListError::new("LIST_FETCH_FAILED", "Could not initialize the HTTPS client")
        })?;
    let root = cache_root()?;
    let fetched_at = OffsetDateTime::now_utc().format(&Rfc3339).map_err(|_| {
        ListError::new(
            "CACHE_UNAVAILABLE",
            "Could not determine the current UTC time",
        )
    })?;
    refresh_with(&root, &request, &fetched_at, |source| {
        let response = client
            .get(source.url)
            .header(reqwest::header::ACCEPT, "text/plain")
            .send()
            .map_err(|_| {
                format!(
                    "{} could not be downloaded over HTTPS within 25 seconds",
                    source.title
                )
            })?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(format!(
                "{} returned HTTP {}; previous rules were kept",
                source.title,
                response.status().as_u16()
            ));
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_LIST_BYTES as u64)
        {
            return Err(format!("{} exceeds the 8 MiB download limit", source.title));
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_LIST_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| format!("{} download was interrupted or timed out", source.title))?;
        Ok(bytes)
    })
}

fn validate_list(bytes: Vec<u8>, source: Source) -> Result<String> {
    if bytes.len() > MAX_LIST_BYTES {
        return Err(ListError::new(
            "INVALID_LIST",
            format!("{} exceeds the 8 MiB list limit", source.title),
        ));
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        ListError::new(
            "INVALID_LIST",
            format!("{} is not valid UTF-8", source.title),
        )
    })?;
    let first = text
        .trim_start_matches('\u{feff}')
        .lines()
        .next()
        .unwrap_or("")
        .trim();
    if !(first == "[Adblock]" || (first.starts_with("[Adblock Plus ") && first.ends_with(']'))) {
        return Err(ListError::new(
            "INVALID_LIST",
            format!("{} is missing an Adblock list header", source.title),
        ));
    }
    let mut meaningful = false;
    for (index, line) in text.lines().enumerate() {
        if index >= MAX_LIST_LINES {
            return Err(ListError::new(
                "INVALID_LIST",
                format!("{} exceeds the 300000-line limit", source.title),
            ));
        }
        let line = line.trim();
        if line.contains('\0') {
            return Err(ListError::new(
                "INVALID_LIST",
                format!("{} contains invalid NUL characters", source.title),
            ));
        }
        if index > 0 && !line.is_empty() && !line.starts_with('!') && !line.starts_with('[') {
            meaningful = true;
        }
    }
    if !meaningful {
        return Err(ListError::new(
            "INVALID_LIST",
            format!("{} contains no filtering rules", source.title),
        ));
    }
    Ok(text)
}

fn header_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .take(100)
        .filter_map(|line| line.trim().strip_prefix('!'))
        .filter_map(|line| line.trim().split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| {
            let mut clean: String = value
                .trim()
                .chars()
                .filter(|character| !character.is_control())
                .take(128)
                .collect();
            while clean.len() > 128 {
                clean.pop();
            }
            clean
        })
}

fn refresh_with(
    root: &Path,
    request: &RefreshRequest,
    fetched_at: &str,
    mut fetch: impl FnMut(Source) -> std::result::Result<Vec<u8>, String>,
) -> Result<Value> {
    let sources = validate_refresh(request)?;
    let mut raw = vec![];
    let mut metadata = vec![];
    // Nothing on disk is touched until every selected source is downloaded,
    // validated, and compiled. The caller's installed rules remain authoritative.
    for source in sources {
        let bytes =
            fetch(source).map_err(|message| ListError::new("LIST_FETCH_FAILED", message))?;
        let text = validate_list(bytes, source)?;
        metadata.push(json!({"id":source.id,"title":header_value(&text, "Title").filter(|value| !value.is_empty()).unwrap_or_else(|| source.title.into()),"url":source.url,"fetchedAt":fetched_at,"sha256":digest(text.as_bytes()),"bytes":text.len(),"version":header_value(&text,"Version").unwrap_or_default()}));
        raw.push((source.id.to_owned(), text));
    }
    let inputs: Vec<_> = raw
        .iter()
        .map(|(id, text)| (id.as_str(), text.as_str()))
        .collect();
    let compiled = subscription_rules::compile_subscriptions(&inputs, request.network_budget)
        .map_err(|message| ListError::new("COMPILATION_FAILED", message))?;
    if compiled["coverage"]["networkSupported"]
        .as_u64()
        .unwrap_or(0)
        == 0
        && compiled["stats"]["network"].as_u64().unwrap_or(0) == 0
        && compiled["stats"]["cosmetic"].as_u64().unwrap_or(0) == 0
    {
        return Err(ListError::new(
            "INVALID_LIST",
            "The selected lists contain no supported filtering rules; previous rules were kept",
        ));
    }
    persist_snapshot(root, fetched_at, &metadata, &raw, &compiled)
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| {
            ListError::new(
                "CACHE_UNAVAILABLE",
                "Could not create a subscription cache file",
            )
        })?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| {
            ListError::new(
                "CACHE_UNAVAILABLE",
                "Could not save a complete subscription cache file",
            )
        })
}

fn persist_snapshot(
    root: &Path,
    fetched_at: &str,
    metadata: &[Value],
    raw: &[(String, String)],
    compiled: &Value,
) -> Result<Value> {
    let snapshots = root.join("snapshots");
    fs::create_dir_all(&snapshots).map_err(|_| {
        ListError::new(
            "CACHE_UNAVAILABLE",
            "Could not create the subscription cache directory",
        )
    })?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = Staging(snapshots.join(format!(
        ".staging-{}-{stamp}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir(&staging.0).map_err(|_| {
        ListError::new(
            "CACHE_UNAVAILABLE",
            "Could not create a subscription staging directory",
        )
    })?;
    let mut total_bytes = 0usize;
    let mut counts = serde_json::Map::new();
    let mut pages = vec![];
    for kind in [PageKind::Network, PageKind::Cosmetic, PageKind::Diagnostics] {
        let items = compiled[kind.field()].as_array().ok_or_else(|| {
            ListError::new(
                "COMPILATION_FAILED",
                "The compiler returned an invalid snapshot",
            )
        })?;
        if items.len() > kind.maximum() {
            return Err(ListError::new(
                "COMPILATION_FAILED",
                "The compiler exceeded a snapshot item limit",
            ));
        }
        counts.insert(kind.label().into(), json!(items.len()));
        let mut offset = 0;
        loop {
            let mut end = (offset + MAX_PAGE_ITEMS).min(items.len());
            let bytes = loop {
                let next = (end < items.len()).then_some(end);
                let page = json!({"kind":kind,"offset":offset,"total":items.len(),"items":&items[offset..end],"nextOffset":next});
                let bytes = serde_json::to_vec(&page).map_err(|_| {
                    ListError::new("COMPILATION_FAILED", "Could not serialize compiled rules")
                })?;
                // Leave room for snapshotId and the native protocol envelope.
                if bytes.len() + 256 <= MAX_PAGE_BYTES {
                    break bytes;
                }
                if end <= offset + 1 {
                    return Err(ListError::new(
                        "COMPILATION_FAILED",
                        "A compiled rule exceeds the 512 KiB page limit",
                    ));
                }
                end = offset + (end - offset) / 2;
            };
            total_bytes += bytes.len();
            if total_bytes > MAX_SNAPSHOT_BYTES {
                return Err(ListError::new(
                    "COMPILATION_FAILED",
                    "Compiled subscription snapshot exceeds 64 MiB",
                ));
            }
            write_new(
                &staging.0.join(format!("{}-{offset}.json", kind.label())),
                &bytes,
            )?;
            pages.push(PageIndex {
                kind,
                offset,
                total: items.len(),
                next_offset: (end < items.len()).then_some(end),
                sha256: digest(&bytes),
                bytes: bytes.len(),
            });
            if end == items.len() {
                break;
            }
            offset = end;
        }
    }
    for (id, text) in raw {
        // IDs originate only from the fixed source table, never raw caller text.
        if !SOURCES.iter().any(|source| source.id == id) {
            return Err(ListError::new(
                "COMPILATION_FAILED",
                "Invalid raw source ID",
            ));
        }
        total_bytes += text.len();
        if total_bytes > MAX_SNAPSHOT_BYTES {
            return Err(ListError::new(
                "COMPILATION_FAILED",
                "Subscription snapshot exceeds 64 MiB",
            ));
        }
        write_new(&staging.0.join(format!("{id}.txt")), text.as_bytes())?;
    }
    let mut manifest = json!({"fetchedAt":fetched_at,"lists":metadata,"counts":counts,"stats":compiled["stats"],"coverage":compiled["coverage"],"listStats":compiled["listStats"],"unsupportedReasons":compiled["unsupportedReasons"]});
    let manifest_bytes = serde_json::to_vec(&manifest).map_err(|_| {
        ListError::new(
            "COMPILATION_FAILED",
            "Could not serialize subscription metadata",
        )
    })?;
    if manifest_bytes.len() > MAX_PAGE_BYTES {
        return Err(ListError::new(
            "COMPILATION_FAILED",
            "Subscription metadata exceeds its size limit",
        ));
    }
    write_new(&staging.0.join("manifest.json"), &manifest_bytes)?;
    let index = SnapshotIndex {
        version: 1,
        manifest_sha256: digest(&manifest_bytes),
        pages,
    };
    let index_bytes = serde_json::to_vec(&index).map_err(|_| {
        ListError::new(
            "COMPILATION_FAILED",
            "Could not serialize the subscription index",
        )
    })?;
    if index_bytes.len() > MAX_INDEX_BYTES
        || total_bytes + manifest_bytes.len() + index_bytes.len() > MAX_SNAPSHOT_BYTES
    {
        return Err(ListError::new(
            "COMPILATION_FAILED",
            "Subscription snapshot metadata exceeds its size limit",
        ));
    }
    write_new(&staging.0.join("index.json"), &index_bytes)?;
    let snapshot_id = digest(&index_bytes);
    let destination = snapshots.join(&snapshot_id);
    if destination.exists() {
        // Identical timestamps/content can share an existing immutable snapshot.
        let existing = read_bounded(&destination.join("index.json"), MAX_INDEX_BYTES)?;
        if existing != index_bytes {
            return Err(ListError::new(
                "SNAPSHOT_CORRUPT",
                "An existing subscription snapshot is corrupt",
            ));
        }
    } else {
        fs::rename(&staging.0, &destination).map_err(|_| {
            ListError::new(
                "CACHE_UNAVAILABLE",
                "Could not commit the subscription snapshot",
            )
        })?;
    }
    manifest["snapshotId"] = json!(snapshot_id);
    prune_snapshots(&snapshots, &snapshot_id);
    Ok(manifest)
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        ListError::new(
            "SNAPSHOT_UNAVAILABLE",
            "The subscription snapshot is unavailable; refresh the lists again",
        )
    })?;
    if !metadata.file_type().is_file() || metadata.len() > limit as u64 {
        return Err(ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription snapshot contains an invalid or oversized file",
        ));
    }
    let mut bytes = vec![];
    File::open(path)
        .and_then(|file| file.take(limit as u64 + 1).read_to_end(&mut bytes))
        .map_err(|_| {
            ListError::new(
                "SNAPSHOT_UNAVAILABLE",
                "Could not read the subscription snapshot",
            )
        })?;
    if bytes.len() > limit {
        return Err(ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription snapshot exceeds its file size limit",
        ));
    }
    Ok(bytes)
}

pub fn page(request: PageRequest) -> Result<Value> {
    // Validate the identifier before constructing or accessing a cache path.
    if !valid_digest(&request.snapshot_id) {
        return Err(ListError::new(
            "INVALID_REQUEST",
            "snapshotId must be 64 lowercase hexadecimal characters",
        ));
    }
    page_at(&cache_root()?, &request)
}

fn page_at(root: &Path, request: &PageRequest) -> Result<Value> {
    if !valid_digest(&request.snapshot_id) {
        return Err(ListError::new(
            "INVALID_REQUEST",
            "snapshotId must be 64 lowercase hexadecimal characters",
        ));
    }
    let snapshot = root.join("snapshots").join(&request.snapshot_id);
    let directory = fs::symlink_metadata(&snapshot).map_err(|_| {
        ListError::new(
            "SNAPSHOT_UNAVAILABLE",
            "The subscription snapshot expired or is unavailable; refresh the lists again",
        )
    })?;
    if !directory.file_type().is_dir() {
        return Err(ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription snapshot directory is invalid",
        ));
    }
    let bytes = read_bounded(&snapshot.join("index.json"), MAX_INDEX_BYTES)?;
    if digest(&bytes) != request.snapshot_id {
        return Err(ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription snapshot index failed its integrity check",
        ));
    }
    let index: SnapshotIndex = serde_json::from_slice(&bytes).map_err(|_| {
        ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription snapshot index is invalid",
        )
    })?;
    if index.version != 1 || index.pages.len() > 40_003 || !valid_digest(&index.manifest_sha256) {
        return Err(ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription snapshot index is invalid",
        ));
    }
    let manifest = read_bounded(&snapshot.join("manifest.json"), MAX_PAGE_BYTES)?;
    if digest(&manifest) != index.manifest_sha256 {
        return Err(ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription metadata failed its integrity check",
        ));
    }
    let selected = index
        .pages
        .iter()
        .find(|page| page.kind == request.kind && page.offset == request.offset)
        .ok_or_else(|| {
            ListError::new(
                "INVALID_REQUEST",
                "offset must match a stored page boundary",
            )
        })?;
    if selected.total > request.kind.maximum()
        || selected.bytes + 256 > MAX_PAGE_BYTES
        || !valid_digest(&selected.sha256)
    {
        return Err(ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription page index exceeds its limits",
        ));
    }
    let bytes = read_bounded(
        &snapshot.join(format!("{}-{}.json", request.kind.label(), request.offset)),
        MAX_PAGE_BYTES,
    )?;
    if bytes.len() != selected.bytes || digest(&bytes) != selected.sha256 {
        return Err(ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription page failed its integrity check",
        ));
    }
    let mut page: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ListError::new("SNAPSHOT_CORRUPT", "The subscription page is invalid"))?;
    let items = page["items"].as_array().ok_or_else(|| {
        ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription page items are invalid",
        )
    })?;
    let end = request.offset.checked_add(items.len()).ok_or_else(|| {
        ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription page offset is invalid",
        )
    })?;
    if items.len() > MAX_PAGE_ITEMS
        || end > selected.total
        || (items.is_empty() && selected.total != 0)
        || page["kind"] != json!(request.kind)
        || page["offset"] != json!(request.offset)
        || page["total"] != json!(selected.total)
        || page["nextOffset"] != json!(selected.next_offset)
        || selected.next_offset != (end < selected.total).then_some(end)
    {
        return Err(ListError::new(
            "SNAPSHOT_CORRUPT",
            "The subscription page boundaries are invalid",
        ));
    }
    page["snapshotId"] = json!(request.snapshot_id);
    Ok(page)
}

fn prune_snapshots(snapshots: &Path, current: &str) {
    let Ok(entries) = fs::read_dir(snapshots) else {
        return;
    };
    let mut complete = vec![];
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.file_type().is_dir() {
            continue;
        }
        if valid_digest(&name) {
            complete.push((
                name == current,
                metadata.modified().unwrap_or(UNIX_EPOCH),
                entry.path(),
            ));
        } else if name.starts_with(".staging-")
            && metadata
                .modified()
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(600))
        {
            // Only old, uncommitted directories beneath the fixed cache root.
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    complete.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| b.2.cmp(&a.2))
    });
    for (_, _, path) in complete.into_iter().skip(KEEP_SNAPSHOTS) {
        let _ = fs::remove_dir_all(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            // Keep disposable test data inside this crate in every checkout.
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/list-tests")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
                ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn request() -> RefreshRequest {
        RefreshRequest {
            ids: vec!["easyprivacy".into(), "easylist".into()],
            network_budget: 1000,
        }
    }
    fn fixture(source: Source) -> Vec<u8> {
        format!(
            "[Adblock Plus 2.0]\n! Title: {}\n! Version: 2026092101\n||{}.example^\n##.ad\n",
            source.title, source.id
        )
        .into_bytes()
    }
    fn refresh_fixture(temp: &Temp, stamp: &str) -> Value {
        refresh_with(&temp.0, &request(), stamp, |source| Ok(fixture(source))).unwrap()
    }
    fn page_request(manifest: &Value, kind: PageKind, offset: usize) -> PageRequest {
        PageRequest {
            snapshot_id: manifest["snapshotId"].as_str().unwrap().into(),
            kind,
            offset,
        }
    }

    #[test]
    fn refresh_preserves_metadata_raw_sources_and_pages_across_independent_calls() {
        let temp = Temp::new();
        let manifest = refresh_fixture(&temp, "2026-09-21T12:00:00Z");
        assert!(valid_digest(manifest["snapshotId"].as_str().unwrap()));
        assert_eq!(manifest["lists"][0]["id"], "easylist");
        assert_eq!(manifest["lists"][0]["version"], "2026092101");
        assert_eq!(manifest["lists"][0]["url"], SOURCES[0].url);
        let raw = fs::read(
            temp.0
                .join("snapshots")
                .join(manifest["snapshotId"].as_str().unwrap())
                .join("easylist.txt"),
        )
        .unwrap();
        assert_eq!(manifest["lists"][0]["sha256"], digest(&raw));
        assert_eq!(manifest["lists"][0]["bytes"], raw.len());
        let page = page_at(&temp.0, &page_request(&manifest, PageKind::Network, 0)).unwrap();
        assert_eq!(page["snapshotId"], manifest["snapshotId"]);
        assert_eq!(page["total"], manifest["counts"]["network"]);
        assert_eq!(page["nextOffset"], Value::Null);
    }

    #[test]
    fn selected_sources_must_all_succeed_without_replacing_previous_snapshot() {
        let temp = Temp::new();
        let old = refresh_fixture(&temp, "2026-09-21T12:00:00Z");
        let error = refresh_with(&temp.0, &request(), "later", |source| {
            if source.id == "easyprivacy" {
                Err("offline".into())
            } else {
                Ok(fixture(source))
            }
        })
        .unwrap_err();
        assert_eq!(error.code, "LIST_FETCH_FAILED");
        assert!(page_at(&temp.0, &page_request(&old, PageKind::Network, 0)).is_ok());
        assert_eq!(fs::read_dir(temp.0.join("snapshots")).unwrap().count(), 1);
    }

    #[test]
    fn compilation_failure_preserves_previous_snapshot() {
        let temp = Temp::new();
        let old = refresh_fixture(&temp, "2026-09-21T12:00:00Z");
        let mut input = request();
        input.network_budget = 0;
        let error = refresh_with(&temp.0, &input, "later", |_| {
            Ok(b"[Adblock Plus 2.0]\n@@||allowed.example^\n".to_vec())
        })
        .unwrap_err();
        assert_eq!(error.code, "COMPILATION_FAILED");
        assert!(page_at(&temp.0, &page_request(&old, PageKind::Network, 0)).is_ok());
        assert_eq!(fs::read_dir(temp.0.join("snapshots")).unwrap().count(), 1);
    }

    #[test]
    fn malformed_downloads_are_rejected_before_any_cache_write() {
        let temp = Temp::new();
        for bytes in [
            b"<html>server error</html>".to_vec(),
            b"[Adblock]\n! empty".to_vec(),
            vec![0xff],
            vec![b'a'; MAX_LIST_BYTES + 1],
            b"[Adblock]\n||a.example^\0".to_vec(),
        ] {
            let error =
                refresh_with(&temp.0, &request(), "unused", |_| Ok(bytes.clone())).unwrap_err();
            assert_eq!(error.code, "INVALID_LIST");
            assert!(!temp.0.join("snapshots").exists());
        }
        let too_many = format!("[Adblock]\n{}||ads.example^", "\n".repeat(MAX_LIST_LINES));
        assert!(validate_list(too_many.into_bytes(), SOURCES[0]).is_err());
    }

    #[test]
    fn invalid_sources_budgets_and_snapshot_paths_are_rejected() {
        for request in [
            RefreshRequest {
                ids: vec![],
                network_budget: 1,
            },
            RefreshRequest {
                ids: vec!["easylist".into(), "easylist".into()],
                network_budget: 1,
            },
            RefreshRequest {
                ids: vec!["https://evil.test".into()],
                network_budget: 1,
            },
            RefreshRequest {
                ids: vec!["easylist".into()],
                network_budget: 29_801,
            },
        ] {
            assert_eq!(
                validate_refresh(&request).err().unwrap().code,
                "INVALID_REQUEST"
            );
        }
        let temp = Temp::new();
        for id in [
            "../index.json".to_owned(),
            "a".repeat(63),
            "A".repeat(64),
            "g".repeat(64),
        ] {
            let error = page_at(
                &temp.0,
                &PageRequest {
                    snapshot_id: id,
                    kind: PageKind::Network,
                    offset: 0,
                },
            )
            .unwrap_err();
            assert_eq!(error.code, "INVALID_REQUEST");
        }
        let missing = PageRequest {
            snapshot_id: "0".repeat(64),
            kind: PageKind::Network,
            offset: 0,
        };
        assert_eq!(
            page_at(&temp.0, &missing).unwrap_err().code,
            "SNAPSHOT_UNAVAILABLE"
        );
    }

    fn synthetic(items: Vec<Value>) -> Value {
        json!({"networkRules":items,"cosmeticRules":[],"diagnostics":[],"stats":{"network":0,"cosmetic":0,"unsupported":0,"ignored":0},"coverage":{},"listStats":[],"unsupportedReasons":[]})
    }

    #[test]
    fn paging_obeys_item_and_byte_limits_and_rejects_nonboundary_offsets() {
        let temp = Temp::new();
        let compiled = synthetic(
            (0..257)
                .map(|id| json!({"id":id,"padding":"x".repeat(5000)}))
                .collect(),
        );
        let manifest = persist_snapshot(&temp.0, "time", &[], &[], &compiled).unwrap();
        let mut offset = 0;
        let mut collected = 0;
        loop {
            let page =
                page_at(&temp.0, &page_request(&manifest, PageKind::Network, offset)).unwrap();
            let items = page["items"].as_array().unwrap();
            assert!(items.len() <= MAX_PAGE_ITEMS);
            assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_PAGE_BYTES);
            collected += items.len();
            match page["nextOffset"].as_u64() {
                Some(next) => {
                    assert!(next as usize > offset);
                    offset = next as usize;
                }
                None => break,
            }
        }
        assert_eq!(collected, 257);
        assert_eq!(
            page_at(&temp.0, &page_request(&manifest, PageKind::Network, 1))
                .unwrap_err()
                .code,
            "INVALID_REQUEST"
        );
        let empty = page_at(&temp.0, &page_request(&manifest, PageKind::Cosmetic, 0)).unwrap();
        assert_eq!(empty["items"], json!([]));
        assert!(empty["nextOffset"].is_null());
    }

    #[test]
    fn corrupt_index_page_and_metadata_are_detected() {
        for file in ["index.json", "network-0.json", "manifest.json"] {
            let temp = Temp::new();
            let manifest = refresh_fixture(&temp, "2026-09-21T12:00:00Z");
            let path = temp
                .0
                .join("snapshots")
                .join(manifest["snapshotId"].as_str().unwrap())
                .join(file);
            fs::write(path, b"{}").unwrap();
            assert_eq!(
                page_at(&temp.0, &page_request(&manifest, PageKind::Network, 0))
                    .unwrap_err()
                    .code,
                "SNAPSHOT_CORRUPT"
            );
        }
    }

    #[test]
    fn retention_is_bounded_and_an_expired_snapshot_has_a_clear_error() {
        let temp = Temp::new();
        let mut manifests = vec![];
        for index in 0..5 {
            manifests.push(refresh_fixture(
                &temp,
                &format!("2026-09-21T12:00:0{index}Z"),
            ));
        }
        assert_eq!(
            fs::read_dir(temp.0.join("snapshots")).unwrap().count(),
            KEEP_SNAPSHOTS
        );
        let unavailable = manifests
            .iter()
            .filter(|manifest| {
                page_at(&temp.0, &page_request(manifest, PageKind::Network, 0)).is_err()
            })
            .count();
        assert_eq!(unavailable, 2);
        assert!(page_at(
            &temp.0,
            &page_request(manifests.last().unwrap(), PageKind::Network, 0)
        )
        .is_ok());
    }

    #[test]
    fn oversize_compiled_item_cleans_staging_without_damaging_previous_snapshot() {
        let temp = Temp::new();
        let old = refresh_fixture(&temp, "2026-09-21T12:00:00Z");
        let error = persist_snapshot(
            &temp.0,
            "new",
            &[],
            &[],
            &synthetic(vec![json!({"padding":"x".repeat(MAX_PAGE_BYTES)})]),
        )
        .unwrap_err();
        assert_eq!(error.code, "COMPILATION_FAILED");
        assert_eq!(fs::read_dir(temp.0.join("snapshots")).unwrap().count(), 1);
        assert!(page_at(&temp.0, &page_request(&old, PageKind::Network, 0)).is_ok());
    }
}
