use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{fs, os::unix::fs::symlink, path::Path, path::PathBuf, process::Command};

use crate::{cfsctl, config, signing};

const EFI_ESP: &str = "/boot/efi";
// UKIs live on the ESP, not XBOOTLDR — systemd-boot always scans its own partition.
const EFI_LINUX_DIR: &str = "/boot/efi/EFI/Linux";

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
    if Path::new(EFI_LINUX_DIR).exists() {
        println!("Building UKI ...");
        build_uki(Path::new(BOOT_DIR), Path::new(EFI_ESP), &digest)?;
        if let Some(sb) = config::secureboot()? {
            let uki_path = Path::new(EFI_LINUX_DIR).join(format!("{digest}.efi"));
            println!("Signing UKI ...");
            crate::install::sign_efi(&uki_path, Path::new(&sb.key), Path::new(&sb.cert))?;
        }
    } else {
        patch_bls_entry(Path::new(BOOT_DIR), &digest, &image_ref)?;
        if !crate::install::has_grub2() {
            // Ubuntu: regenerate menuentry-based grub.cfg so the new deployment
            // appears in the menu (blscfg.mod is not available on Ubuntu).
            write_grub_menuentry_cfg(Path::new(BOOT_DIR), crate::install::grub_dir())?;
        }
    }

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

/// Convert a Type 1 BLS entry (written by `prepare-boot` on the XBOOTLDR/`bootdir`)
/// into a UKI .efi on the ESP (`esp`), where systemd-boot always scans.
/// Reads the full `options` line — including the `composefs=?<hash>` token that
/// `prepare-boot` prepended — embeds it in the UKI cmdline, then removes the
/// now-redundant Type 1 .conf and extracted kernel/initramfs directory.
pub fn build_uki(bootdir: &Path, esp: &Path, digest: &str) -> Result<()> {
    let conf_path = bootdir
        .join("loader/entries")
        .join(format!("{digest}.conf"));
    let conf = fs::read_to_string(&conf_path)
        .with_context(|| format!("reading BLS entry {}", conf_path.display()))?;

    let cmdline = conf
        .lines()
        .find(|l| l.starts_with("options "))
        .map(|l| l["options ".len()..].trim())
        .context("no 'options' line in BLS entry written by prepare-boot")?;

    let vmlinuz = bootdir.join(digest).join("vmlinuz");
    let initramfs = bootdir.join(digest).join("initramfs.img");
    let efi_linux = esp.join("EFI/Linux");
    fs::create_dir_all(&efi_linux).context("creating EFI/Linux on ESP")?;
    let output = efi_linux.join(format!("{digest}.efi"));

    let status = Command::new("ukify")
        .args([
            "build",
            &format!("--linux={}", vmlinuz.display()),
            &format!("--initrd={}", initramfs.display()),
            &format!("--cmdline={cmdline}"),
            &format!("--output={}", output.display()),
        ])
        .status()
        .context("spawning ukify")?;
    if !status.success() {
        anyhow::bail!("ukify failed: {status}");
    }

    // Type 1 artifacts are now redundant — the UKI is self-contained.
    let _ = fs::remove_file(&conf_path);
    let t1_dir = bootdir.join(digest);
    if t1_dir.exists() {
        fs::remove_dir_all(&t1_dir).context("removing Type 1 kernel directory")?;
    }

    Ok(())
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

/// Generate a traditional GRUB menuentry config from all BLS entries in
/// `bootdir/loader/entries/`.  Used on distros (Ubuntu/Debian) that do not
/// ship `blscfg.mod` in their GRUB package.
///
/// Entries are sorted newest-first; index 0 is the default boot entry.
/// rollback.rs writes `next_entry=<numeric-index>` (not the digest) for
/// Ubuntu because GRUB does not reliably match long --id values.
pub fn write_grub_menuentry_cfg(bootdir: &Path, grub_subdir: &str) -> Result<()> {
    let entries_dir = bootdir.join("loader/entries");
    let mut bls: Vec<(std::time::SystemTime, String, String, String)> = Vec::new();

    if entries_dir.exists() {
        for item in fs::read_dir(&entries_dir).context("reading BLS entries")? {
            let item = item?;
            let path = item.path();
            if path.extension().and_then(|e| e.to_str()) != Some("conf") {
                continue;
            }
            let mtime = item
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            let content =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            let digest = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_owned();
            let title = content
                .lines()
                .find(|l| l.starts_with("title "))
                .map(|l| l["title ".len()..].trim().to_owned())
                .unwrap_or_else(|| digest[..digest.len().min(12)].to_owned());
            let options = content
                .lines()
                .find(|l| l.starts_with("options "))
                .map(|l| l["options ".len()..].trim().to_owned())
                .unwrap_or_default();
            bls.push((mtime, title, digest, options));
        }
    }

    // Newest first → index 0 is the default boot entry.
    bls.sort_by_key(|b| std::cmp::Reverse(b.0));

    let grub_boot_dir = bootdir.join(grub_subdir);
    fs::create_dir_all(&grub_boot_dir).context("creating grub boot dir")?;

    let mut cfg = String::from(
        "serial --unit=0 --speed=115200\n\
         terminal_input serial console\n\
         terminal_output serial console\n\
         load_env\n\
         if [ \"${next_entry}\" ] ; then\n\
           set default=\"${next_entry}\"\n\
           set next_entry=\n\
           save_env next_entry\n\
         fi\n\
         set timeout=3\n\n",
    );

    for (_mtime, title, digest, options) in &bls {
        cfg.push_str(&format!(
            "menuentry \"{title}\" --id {digest} {{\n\
             \tsearch --no-floppy --label --set=root boot\n\
             \tlinux /{digest}/vmlinuz {options}\n\
             \tinitrd /{digest}/initramfs.img\n\
             }}\n\n"
        ));
    }

    let cfg_path = grub_boot_dir.join("grub.cfg");
    fs::write(&cfg_path, &cfg).with_context(|| format!("writing {}", cfg_path.display()))
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
        .find_map(|tok| tok.strip_prefix("composefs="))
        .map(|v| v.trim_start_matches('?').to_owned())
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
