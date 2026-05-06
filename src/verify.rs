use anyhow::{bail, Result};
use std::fs;

use crate::{config, signing};

pub fn run() -> Result<()> {
    // Confirm we're running from a composefs deployment before trying to verify it.
    let booted = fs::read_to_string("/proc/cmdline")
        .ok()
        .and_then(|c| {
            c.split_whitespace()
                .find(|tok| tok.starts_with("composefs="))
                .map(|tok| tok["composefs=".len()..].to_owned())
        });

    if booted.is_none() {
        bail!("not running from a composefs deployment; nothing to verify");
    }

    let image_ref = config::require_image_ref()?;
    let manifest_digest = signing::verify_image(&image_ref, true)?;
    println!("Verified: {image_ref}");
    if let Some(m) = manifest_digest {
        println!("Manifest: {m}");
    }
    Ok(())
}
