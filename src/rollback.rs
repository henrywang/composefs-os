use anyhow::{Context, Result, bail};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

const ENTRIES_DIR: &str = "/boot/loader/entries";

struct BLSEntry {
    path: PathBuf,
    composefs_digest: String,
    mtime: SystemTime,
}

fn current_digest() -> Option<String> {
    fs::read_to_string("/proc/cmdline")
        .ok()?
        .split_whitespace()
        .find(|tok| tok.starts_with("composefs="))
        // Strip optional '?' insecure-mode prefix so GRUB and UKI comparisons agree
        .map(|tok| tok["composefs=".len()..].trim_start_matches('?').to_owned())
}

fn load_entries() -> Result<Vec<BLSEntry>> {
    let dir = Path::new(ENTRIES_DIR);
    let mut entries = Vec::new();

    for item in fs::read_dir(dir).with_context(|| format!("reading {ENTRIES_DIR}"))? {
        let item = item.with_context(|| format!("iterating {ENTRIES_DIR}"))?;
        let path = item.path();
        if path.extension().and_then(|e| e.to_str()) != Some("conf") {
            continue;
        }
        let meta = fs::metadata(&path).with_context(|| format!("stat {}", path.display()))?;
        let mtime = meta
            .modified()
            .with_context(|| format!("mtime {}", path.display()))?;
        let content =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;

        if let Some(digest) = parse_composefs_digest(&content) {
            entries.push(BLSEntry {
                path,
                composefs_digest: digest,
                mtime,
            });
        }
    }

    Ok(entries)
}

fn parse_composefs_digest(content: &str) -> Option<String> {
    content
        .lines()
        .find(|l| l.starts_with("options "))?
        .split_whitespace()
        .find(|tok| tok.starts_with("composefs="))
        .map(|tok| tok["composefs=".len()..].trim_start_matches('?').to_owned())
}

fn entry_id(path: &Path) -> &str {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("")
}

const EFI_LINUX_DIR: &str = "/boot/efi/EFI/Linux";

fn grubenv_path() -> Option<&'static str> {
    ["/boot/grub2/grubenv", "/boot/grub/grubenv"]
        .into_iter()
        .find(|&p| Path::new(p).exists())
}

fn set_next_entry(id: &str) -> Result<()> {
    let grubenv =
        grubenv_path().context("grubenv not found at /boot/grub2/grubenv or /boot/grub/grubenv")?;
    let next = format!("next_entry={id}");
    for cmd in &["grub2-editenv", "grub-editenv"] {
        match Command::new(cmd).args([grubenv, "set", &next]).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => bail!("{cmd}: exited {status}"),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("spawning {cmd}")),
        }
    }
    bail!("neither grub2-editenv nor grub-editenv found in PATH")
}

/// Return the GRUB menuentry index (0-based, newest-first) of the BLS entry
/// whose file stem equals `target_id`.
///
/// Ubuntu's GRUB does not ship blscfg.mod, so cbootc generates a traditional
/// grub.cfg via write_grub_menuentry_cfg, sorted newest-first by mtime.
/// Ubuntu's GRUB does not reliably match `set default=<128-char-id>` against
/// `menuentry --id <id>`, so we write the numeric index instead.
fn grub_numeric_index(target_id: &str) -> Result<usize> {
    let dir = Path::new(ENTRIES_DIR);
    let mut all: Vec<(SystemTime, String)> = Vec::new();
    for item in fs::read_dir(dir).with_context(|| format!("reading {ENTRIES_DIR}"))? {
        let item = item.with_context(|| format!("iterating {ENTRIES_DIR}"))?;
        let path = item.path();
        if path.extension().and_then(|e| e.to_str()) != Some("conf") {
            continue;
        }
        let mtime = item
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_owned();
        all.push((mtime, stem));
    }
    // Newest-first — same order write_grub_menuentry_cfg uses (index 0 = default).
    all.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    all.iter()
        .position(|(_, stem)| stem == target_id)
        .with_context(|| format!("BLS entry {target_id} not found in {ENTRIES_DIR}"))
}

fn load_uki_entries() -> Result<Vec<BLSEntry>> {
    let dir = Path::new(EFI_LINUX_DIR);
    let mut entries = Vec::new();

    for item in fs::read_dir(dir).with_context(|| format!("reading {EFI_LINUX_DIR}"))? {
        let item = item.with_context(|| format!("iterating {EFI_LINUX_DIR}"))?;
        let path = item.path();
        if path.extension().and_then(|e| e.to_str()) != Some("efi") {
            continue;
        }
        let meta = fs::metadata(&path).with_context(|| format!("stat {}", path.display()))?;
        let mtime = meta
            .modified()
            .with_context(|| format!("mtime {}", path.display()))?;
        // ukify names the .efi after the composefs digest (set via --entry-id)
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_owned();
        entries.push(BLSEntry {
            path,
            composefs_digest: stem,
            mtime,
        });
    }

    Ok(entries)
}

fn set_next_entry_bootctl(id: &str) -> Result<()> {
    let status = Command::new("bootctl")
        .args(["set-next", id])
        .status()
        .context("spawning bootctl set-next")?;
    if !status.success() {
        bail!("bootctl set-next failed: {status}");
    }
    Ok(())
}

fn use_systemd_boot() -> bool {
    Path::new(EFI_LINUX_DIR).exists() && grubenv_path().is_none()
}

pub fn run() -> Result<()> {
    let current = current_digest();
    let systemd_boot = use_systemd_boot();

    let mut entries = if systemd_boot {
        load_uki_entries()?
    } else {
        load_entries()?
    };

    // Keep only entries that are not the currently booted deployment.
    entries.retain(|e| Some(&e.composefs_digest) != current.as_ref());

    if entries.is_empty() {
        bail!("no previous composefs deployment found");
    }

    // Most-recently written entry = the last prepare-boot ran before this one.
    entries.sort_by_key(|e| e.mtime);
    let previous = entries.last().unwrap();

    let id = entry_id(&previous.path);
    if systemd_boot {
        set_next_entry_bootctl(id)?;
    } else if crate::install::has_grub2() {
        // Fedora/RHEL: grub2's blscfg.mod reads BLS entries natively and
        // matches next_entry=<digest> against each entry's --id.
        set_next_entry(id)?;
    } else {
        // Ubuntu/Debian: no blscfg.mod; grub.cfg is generated by
        // write_grub_menuentry_cfg, sorted newest-first.  GRUB does not
        // reliably match set default=<128-char-digest> against --id, so
        // write the numeric position instead.
        let index = grub_numeric_index(id)?;
        set_next_entry(&index.to_string())?;
    }

    println!(
        "Next boot will use deployment {}.",
        previous.composefs_digest
    );
    println!("Run 'systemctl reboot' to apply.");
    Ok(())
}
