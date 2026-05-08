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
        "--entry-id",
        &digest,
        "--cmdline",
        &cmdline,
        &image_ref,
    ])?;

    patch_bls_entry(Path::new(BOOT_DIR), &digest, &image_ref)?;

    // Wire the new deployment's var to the shared /sysroot/state/var so
    // /var content survives upgrades.
    let deploy_var = PathBuf::from("/sysroot/state/deploy")
        .join(digest.trim())
        .join("var");
    if deploy_var.is_dir() {
        fs::remove_dir(&deploy_var).context("removing new deployment var dir")?;
        symlink("../../var", &deploy_var).context("creating shared var symlink")?;
    }

    carry_forward_etc(&digest)?;

    write_state(&digest, manifest_digest.as_deref())?;
    println!("Boot entry written.");

    if reboot {
        trigger_reboot()
    } else {
        println!("Run 'systemctl reboot' to apply, or pass --reboot.");
        Ok(())
    }
}

/// Rewrite the title and version lines in the BLS entry so the GRUB menu
/// shows something useful instead of the hardcoded "todoOS / 0-todo" from cfsctl.
pub fn patch_bls_entry(bootdir: &Path, digest: &str, image_ref: &str) -> Result<()> {
    let entry_path = bootdir
        .join("loader/entries")
        .join(format!("{digest}.conf"));
    if !entry_path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&entry_path)
        .with_context(|| format!("reading {}", entry_path.display()))?;

    // "docker://ghcr.io/user/image:tag" → "image:tag"
    let short = image_ref
        .rsplit("://")
        .next()
        .unwrap_or(image_ref)
        .rsplit('/')
        .next()
        .unwrap_or(image_ref);
    let digest_short = &digest[..digest.len().min(12)];
    let date = Utc::now().format("%Y-%m-%d").to_string();

    let patched = content
        .lines()
        .map(|line| {
            if line.starts_with("title ") {
                format!("title {short} {date} ({digest_short})")
            } else if line.starts_with("version ") {
                format!("version {digest_short}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";

    fs::write(&entry_path, patched).with_context(|| format!("writing {}", entry_path.display()))
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

/// Extract the composefs=<hash> token from /proc/cmdline, if present.
fn current_composefs_digest() -> Option<String> {
    let raw = fs::read_to_string("/proc/cmdline").ok()?;
    raw.split_whitespace()
        .find_map(|tok| tok.strip_prefix("composefs=").map(str::to_owned))
}

/// Carry /etc changes from the current deployment's overlayfs upper directory
/// into the new deployment's upper directory, so user edits survive upgrades.
///
/// The overlayfs upper IS the diff between the image /etc and the running /etc,
/// so copying it forward gives three-way-merge semantics:
///   - user-modified file → in new upper → user version persists
///   - image-only change  → not in upper → new image version shows through
///   - conflict           → user version wins (same default as bootc)
fn carry_forward_etc(new_digest: &str) -> Result<()> {
    let Some(current_digest) = current_composefs_digest() else {
        return Ok(());
    };
    if current_digest == new_digest {
        return Ok(());
    }

    let old_upper = PathBuf::from("/sysroot/state/deploy")
        .join(&current_digest)
        .join("etc/upper");
    let new_upper = PathBuf::from("/sysroot/state/deploy")
        .join(new_digest)
        .join("etc/upper");

    if !old_upper.exists() || !new_upper.exists() {
        return Ok(());
    }

    println!("Carrying forward /etc changes ...");
    // -a preserves xattrs (overlayfs opaque markers) and device files (whiteouts)
    let src = format!("{}/.", old_upper.display());
    let status = Command::new("cp")
        .args(["-a", &src, new_upper.to_str().unwrap()])
        .status()
        .context("cp etc/upper")?;
    if !status.success() {
        anyhow::bail!("failed to carry forward /etc changes");
    }
    Ok(())
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
