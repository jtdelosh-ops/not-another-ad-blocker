use super::blocklist::{DnsPolicy, RuleSource};
use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DnsConfig {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    pub upstreams: Vec<SocketAddr>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_cache")]
    pub cache_entries: usize,
    #[serde(default = "default_concurrency")]
    pub max_in_flight: usize,
    #[serde(default = "default_activity")]
    pub activity_capacity: usize,
    #[serde(default)]
    pub filter_lists: Vec<FilterFile>,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default)]
    pub blocklist: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterFile {
    pub name: String,
    pub path: PathBuf,
}

fn default_listen() -> SocketAddr {
    "127.0.0.1:5354".parse().unwrap()
}
fn default_timeout() -> u64 {
    2_000
}
fn default_cache() -> usize {
    1_024
}
fn default_concurrency() -> usize {
    64
}
fn default_activity() -> usize {
    300
}

pub fn read_bounded(path: &Path, maximum: usize) -> Result<String, String> {
    let file =
        File::open(path).map_err(|error| format!("Cannot open {}: {error}", path.display()))?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err(format!("{} must be a regular file", path.display()));
    }
    let mut text = String::new();
    file.take(maximum as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("Cannot read UTF-8 file {}: {error}", path.display()))?;
    if text.len() > maximum {
        return Err(format!("{} exceeds {maximum} bytes", path.display()));
    }
    Ok(text)
}

impl DnsConfig {
    pub fn load(path: &Path) -> Result<(Self, DnsPolicy), String> {
        let text = read_bounded(path, 1024 * 1024)?;
        let config: Self = serde_json::from_str(&text)
            .map_err(|error| format!("Invalid DNS configuration: {error}"))?;
        config.validate()?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut sources = Vec::new();
        for file in &config.filter_lists {
            sources.push(RuleSource {
                name: file.name.clone(),
                text: read_bounded(&base.join(&file.path), 8 * 1024 * 1024)?,
            });
        }
        let policy = DnsPolicy::compile(&sources, &config.allowlist, &config.blocklist)?;
        Ok((config, policy))
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.listen.ip().is_loopback() || self.listen.port() < 1024 {
            return Err("The development listener must use a loopback address and an unprivileged port (1024–65535)".into());
        }
        if self.upstreams.is_empty() || self.upstreams.len() > 4 {
            return Err("Configure 1–4 explicit upstream IP:port addresses".into());
        }
        for upstream in &self.upstreams {
            let ip = upstream.ip().to_canonical();
            if upstream.port() == 0
                || ip.is_unspecified()
                || ip.is_multicast()
                || matches!(ip, std::net::IpAddr::V4(v4) if v4.is_broadcast())
            {
                return Err("Upstreams must be unicast addresses with nonzero ports".into());
            }
            if ip.is_loopback() && upstream.port() == self.listen.port() {
                return Err("An upstream cannot point back to the local DNS listener".into());
            }
        }
        if !(100..=10_000).contains(&self.timeout_ms) || !(1..=256).contains(&self.max_in_flight) {
            return Err("timeoutMs must be 100–10000 and maxInFlight must be 1–256".into());
        }
        if self.cache_entries > 4_096 || self.activity_capacity > 1_000 {
            return Err("cacheEntries must be 0–4096 and activityCapacity must be 0–1000".into());
        }
        if self.filter_lists.len() > 2 {
            return Err("At most two filter-list files may be loaded".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> DnsConfig {
        serde_json::from_value(json!({"upstreams":["127.0.0.1:5533"]})).unwrap()
    }

    #[test]
    fn explicit_upstream_defaults_are_local_and_bounded() {
        let config = config();
        config.validate().unwrap();
        assert_eq!(config.listen, "127.0.0.1:5354".parse().unwrap());
        assert_eq!(config.activity_capacity, 300);
        assert!(serde_json::from_str::<DnsConfig>("{}").is_err());
        assert!(serde_json::from_str::<DnsConfig>(r#"{"upstreams":[],"typo":true}"#).is_err());
        assert!(serde_json::from_str::<DnsConfig>(r#"{"upstreams":[],"upstreams":[]}"#).is_err());
    }

    #[test]
    fn listener_upstream_loops_and_resource_limits_are_rejected() {
        for address in [
            "0.0.0.0:5354",
            "192.0.2.1:5354",
            "127.0.0.1:53",
            "[::]:5354",
        ] {
            let mut config = config();
            config.listen = address.parse().unwrap();
            assert!(config.validate().is_err());
        }
        for address in [
            "127.0.0.1:5354",
            "[::1]:5354",
            "[::ffff:127.0.0.1]:5354",
            "0.0.0.0:53",
            "224.0.0.1:53",
            "255.255.255.255:53",
            "1.1.1.1:0",
        ] {
            let mut config = config();
            config.upstreams = vec![address.parse().unwrap()];
            assert!(config.validate().is_err());
        }
        let mut config = config();
        config.cache_entries = 4_097;
        assert!(config.validate().is_err());
        config.cache_entries = 0;
        config.activity_capacity = 1_001;
        assert!(config.validate().is_err());
        config.activity_capacity = 0;
        config.validate().unwrap();
    }
}
