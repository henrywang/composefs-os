use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

const CONFIG_PATH: &str = "/etc/cbootc/config.toml";

#[derive(Serialize, Deserialize)]
struct Config {
    image: Option<ImageConfig>,
}

#[derive(Serialize, Deserialize)]
struct ImageConfig {
    #[serde(rename = "ref")]
    image_ref: String,
}

/// Returns the configured image reference, or None if the config file is absent.
/// Errors if the file exists but cannot be parsed.
pub fn image_ref() -> Result<Option<String>> {
    match fs::read_to_string(CONFIG_PATH) {
        Ok(raw) => {
            let config: Config =
                toml::from_str(&raw).with_context(|| format!("parsing {CONFIG_PATH}"))?;
            Ok(config.image.map(|i| i.image_ref))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {CONFIG_PATH}")),
    }
}

/// Like `image_ref`, but errors if no image is configured.
pub fn require_image_ref() -> Result<String> {
    image_ref()?.ok_or_else(|| anyhow::anyhow!("no image configured in {CONFIG_PATH}"))
}

/// Overwrite the tracked image reference in config.toml.
// v1 rewrites the whole file; revisit when more fields land.
pub fn write_image_ref(new_ref: &str) -> Result<()> {
    let config = Config {
        image: Some(ImageConfig {
            image_ref: new_ref.to_owned(),
        }),
    };
    let toml = toml::to_string_pretty(&config).context("serializing config")?;
    let dir = Path::new(CONFIG_PATH).parent().unwrap();
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    fs::write(CONFIG_PATH, toml).with_context(|| format!("writing {CONFIG_PATH}"))
}
