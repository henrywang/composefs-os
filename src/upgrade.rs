use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{fs, os::unix::fs::symlink, path::Path, path::PathBuf, process::Command};

use crate::{cfsctl, config, signing};

const BOOT_DIR: &str = "/boot";
const STATE_PATH: &str = "/var/lib/cbootc/state.json";

#[derive(Serialize, Deserialize)]
struct State {
    last_upgrade: String,
    last_known_good_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_verified_manifest: Option<String>,
}

/// Add `docker://` transport prefix if the ref has no explicit transport.
pub fn normalize_ref(image_ref: &str) -> String {
    if image_ref.contains("://") {
        image_ref.to_owned()
    } else {
        format!("docker://{image_ref}")
    }
}

pub fn run(reboot: bool) -> Result<()> {
    let image_ref = normalize_ref(&config::require_image_ref()?);

    println!("Pulling {image_ref} ...");
    cfsctl::run(&["oci", "pull", &image_ref])?;

    let digest = cfsctl::output(&["oci", "compute-id", "--bootable", &image_ref])?;
    let digest = digest.trim().to_owned();
    println!("Digest: {digest}");

    let manifest_digest = signing::verify_image(&image_ref, false)?;
    if let Some(ref m) = manifest_digest {
        println!("Verified manifest: {m}");
    }

    println!("Preparing boot entry ...");
    let cmdline = current_cmdline()?;
    cfsctl::run(&[
        "oci",
        "prepare-boot",
        "--bootdir",
        BOOT_DIR,
        "--cmdline",
        &cmdline,
        &image_ref,
    ])?;

    // Wire the new deployment's var to the shared /sysroot/state/var so
    // /var content survives upgrades.
    let deploy_var = PathBuf::from("/sysroot/state/deploy")
        .join(digest.trim())
        .join("var");
    if deploy_var.is_dir() {
        fs::remove_dir(&deploy_var).context("removing new deployment var dir")?;
        symlink("../../var", &deploy_var).context("creating shared var symlink")?;
    }

    write_state(&digest, manifest_digest.as_deref())?;
    println!("Boot entry written.");

    if reboot {
        trigger_reboot()
    } else {
        println!("Run 'systemctl reboot' to apply, or pass --reboot.");
        Ok(())
    }
}

/// Read the running kernel cmdline, stripping the composefs= token so
/// cfsctl can append the new one for the upgraded image.
fn current_cmdline() -> Result<String> {
    let raw = fs::read_to_string("/proc/cmdline").context("reading /proc/cmdline")?;
    let filtered = raw
        .split_whitespace()
        .filter(|tok| !tok.starts_with("composefs="))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(filtered)
}

fn write_state(digest: &str, manifest_digest: Option<&str>) -> Result<()> {
    let state = State {
        last_upgrade: Utc::now().to_rfc3339(),
        last_known_good_digest: digest.to_owned(),
        last_verified_manifest: manifest_digest.map(str::to_owned),
    };
    let dir = Path::new(STATE_PATH).parent().unwrap();
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let json = serde_json::to_string_pretty(&state).context("serializing state")?;
    fs::write(STATE_PATH, json).with_context(|| format!("writing {STATE_PATH}"))
}

fn trigger_reboot() -> Result<()> {
    let status = Command::new("systemctl")
        .arg("reboot")
        .status()
        .context("spawning systemctl reboot")?;
    if !status.success() {
        anyhow::bail!("systemctl reboot exited {status}");
    }
    Ok(())
}
