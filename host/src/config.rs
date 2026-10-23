//! Host configuration file (TOML). Holds the host identity and paired device keys.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::net::crypto::random_array;
use crate::net::ratelimit::HARD_CAP_KBPS;

pub const MIN_KEEPALIVE_SECS: u16 = 15;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read or write {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid config file {path}: {source}")]
    Parse { path: PathBuf, source: Box<toml::de::Error> },
    #[error("cannot serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
    #[error("no home directory found for the config file")]
    NoHome,
}

/// Fixed-size byte string stored as lowercase hex in the TOML file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Hex<const N: usize>(pub [u8; N]);

impl<const N: usize> fmt::Debug for Hex<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hex({})", hex::encode(self.0))
    }
}

impl<const N: usize> Serialize for Hex<N> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(self.0))
    }
}

impl<'de, const N: usize> Deserialize<'de> for Hex<N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        let arr: [u8; N] =
            bytes.try_into().map_err(|_| serde::de::Error::custom(format!("expected {N} bytes of hex")))?;
        Ok(Self(arr))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub host_name: String,
    pub host_id: Hex<16>,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub devices: Vec<PairedDevice>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct NetworkConfig {
    pub bind: IpAddr,
    pub discovery_port: u16,
    pub control_port: u16,
    pub audio_port: u16,
    pub budget_kbps: u32,
    pub keepalive_secs: u16,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            discovery_port: 47860,
            control_port: 47861,
            audio_port: 47862,
            budget_kbps: HARD_CAP_KBPS,
            keepalive_secs: MIN_KEEPALIVE_SECS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PairedDevice {
    pub id: Hex<16>,
    pub name: String,
    pub key: Hex<32>,
    /// Unix seconds.
    pub paired_at: u64,
}

impl Config {
    pub fn new_default() -> Self {
        let host_name =
            std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "PC".to_string());
        Self {
            host_name: crate::net::codec::truncate_utf8(&host_name, 32).to_string(),
            host_id: Hex(random_array()),
            network: NetworkConfig::default(),
            devices: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let n = &self.network;
        if self.host_name.is_empty() || self.host_name.len() > 32 {
            return Err(ConfigError::Invalid("host_name must be 1-32 bytes".into()));
        }
        if n.budget_kbps == 0 || n.budget_kbps > HARD_CAP_KBPS {
            return Err(ConfigError::Invalid(format!(
                "network.budget_kbps must be 1-{HARD_CAP_KBPS} (802.11b budget)"
            )));
        }
        if n.keepalive_secs < MIN_KEEPALIVE_SECS || n.keepalive_secs > 600 {
            return Err(ConfigError::Invalid(format!("network.keepalive_secs must be {MIN_KEEPALIVE_SECS}-600")));
        }
        let ports = [n.discovery_port, n.control_port, n.audio_port];
        if ports.contains(&0) {
            return Err(ConfigError::Invalid("ports must be non-zero".into()));
        }
        // Discovery and audio are both UDP; control is TCP, so it may share a number with either.
        if n.discovery_port == n.audio_port {
            return Err(ConfigError::Invalid("discovery_port and audio_port must differ".into()));
        }
        for (i, d) in self.devices.iter().enumerate() {
            if self.devices[..i].iter().any(|o| o.id == d.id) {
                return Err(ConfigError::Invalid(format!("device {} is listed twice", hex::encode(d.id.0))));
            }
        }
        Ok(())
    }

    pub fn device(&self, id: &[u8; 16]) -> Option<&PairedDevice> {
        self.devices.iter().find(|d| &d.id.0 == id)
    }

    /// Adds or replaces the device with this id.
    pub fn upsert_device(&mut self, device: PairedDevice) {
        self.devices.retain(|d| d.id != device.id);
        self.devices.push(device);
    }

    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let cfg: Config = toml::from_str(&text)
                    .map_err(|e| ConfigError::Parse { path: path.to_path_buf(), source: Box::new(e) })?;
                cfg.validate()?;
                Ok(cfg)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let cfg = Self::new_default();
                cfg.save(path)?;
                Ok(cfg)
            }
            Err(e) => Err(ConfigError::Io { path: path.to_path_buf(), source: e }),
        }
    }

    /// Writes to a temporary file first so a crash never leaves a half-written key store.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        self.validate()?;
        let io_err = |source| ConfigError::Io { path: path.to_path_buf(), source };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io_err)?;
        }
        let text = format!(
            "# PSP Companion host config. Device keys below are secrets: do not share this file.\n{}",
            toml::to_string_pretty(self)?
        );
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(io_err)?;
        std::fs::rename(&tmp, path).map_err(io_err)
    }
}

pub struct Paths {
    pub config_file: PathBuf,
    pub log_dir: PathBuf,
}

impl Paths {
    pub fn discover(config_override: Option<PathBuf>) -> Result<Self, ConfigError> {
        let dirs = directories::ProjectDirs::from("", "", "psp-companion").ok_or(ConfigError::NoHome)?;
        Ok(Self {
            config_file: config_override.unwrap_or_else(|| dirs.config_dir().join("config.toml")),
            log_dir: dirs.data_local_dir().join("logs"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pspc-config-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("config.toml")
    }

    #[test]
    fn creates_saves_and_reloads() {
        let path = temp_path("roundtrip");
        let mut cfg = Config::load_or_create(&path).unwrap();
        assert!(path.exists());
        cfg.upsert_device(PairedDevice { id: Hex([1; 16]), name: "PSP".into(), key: Hex([2; 32]), paired_at: 5 });
        cfg.upsert_device(PairedDevice { id: Hex([1; 16]), name: "PSP2".into(), key: Hex([3; 32]), paired_at: 6 });
        cfg.save(&path).unwrap();
        let back = Config::load_or_create(&path).unwrap();
        assert_eq!(back, cfg);
        assert_eq!(back.devices.len(), 1);
        assert_eq!(back.device(&[1; 16]).unwrap().key.0, [3; 32]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn rejects_budget_above_cap_and_fast_keepalive() {
        let mut cfg = Config::new_default();
        cfg.validate().unwrap();
        cfg.network.budget_kbps = 301;
        assert!(cfg.validate().is_err());
        cfg.network.budget_kbps = 300;
        cfg.network.keepalive_secs = 5;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_malformed_file() {
        let path = temp_path("malformed");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "host_name = \"x\"\nhost_id = \"abcd\"\n").unwrap();
        assert!(matches!(Config::load_or_create(&path), Err(ConfigError::Parse { .. })));
        std::fs::write(&path, "host_name = \"x\"\nhost_id = \"000102030405060708090a0b0c0d0e0f\"\nextra = 1\n")
            .unwrap();
        assert!(matches!(Config::load_or_create(&path), Err(ConfigError::Parse { .. })));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
