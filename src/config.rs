use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

const CONFIG_PATH: &str = "/var/lib/cbootc/config.toml";

#[derive(Serialize, Deserialize, Default)]
struct Config {
    image: Option<ImageConfig>,
    secureboot: Option<SecureBootConfig>,
}

#[derive(Serialize, Deserialize)]
struct ImageConfig {
    #[serde(rename = "ref")]
    image_ref: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SecureBootConfig {
    pub key: String,
    pub cert: String,
}

fn read_config() -> Result<Option<Config>> {
    match fs::read_to_string(CONFIG_PATH) {
        Ok(raw) => {
            let config: Config =
                toml::from_str(&raw).with_context(|| format!("parsing {CONFIG_PATH}"))?;
            Ok(Some(config))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {CONFIG_PATH}")),
    }
}

fn write_config(config: &Config) -> Result<()> {
    let toml = toml::to_string_pretty(config).context("serializing config")?;
    let dir = Path::new(CONFIG_PATH).parent().unwrap();
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    fs::write(CONFIG_PATH, toml).with_context(|| format!("writing {CONFIG_PATH}"))
}

/// Returns the configured image reference, or None if the config file is absent.
/// Errors if the file exists but cannot be parsed.
pub fn image_ref() -> Result<Option<String>> {
    Ok(read_config()?.and_then(|c| c.image).map(|i| i.image_ref))
}

/// Returns the Secure Boot signing config, or None if not configured.
pub fn secureboot() -> Result<Option<SecureBootConfig>> {
    Ok(read_config()?.and_then(|c| c.secureboot))
}

/// Like `image_ref`, but errors if no image is configured.
pub fn require_image_ref() -> Result<String> {
    image_ref()?.ok_or_else(|| anyhow::anyhow!("no image configured in {CONFIG_PATH}"))
}

/// Overwrite the tracked image reference in config.toml (preserves other fields).
pub fn write_image_ref(new_ref: &str) -> Result<()> {
    let mut config = read_config()?.unwrap_or_default();
    config.image = Some(ImageConfig {
        image_ref: new_ref.to_owned(),
    });
    write_config(&config)
}
