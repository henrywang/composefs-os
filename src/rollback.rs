use anyhow::{bail, Context, Result};
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
        .map(|tok| tok["composefs=".len()..].to_owned())
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
        let mtime = meta.modified().with_context(|| format!("mtime {}", path.display()))?;
        let content =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;

        if let Some(digest) = parse_composefs_digest(&content) {
            entries.push(BLSEntry { path, composefs_digest: digest, mtime });
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
        .map(|tok| tok["composefs=".len()..].to_owned())
}

fn entry_id(path: &Path) -> &str {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("")
}

fn grub_reboot(id: &str) -> Result<()> {
    for cmd in &["grub2-reboot", "grub-reboot"] {
        match Command::new(cmd).arg(id).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => bail!("{cmd} {id}: exited {status}"),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("spawning {cmd}")),
        }
    }
    bail!("neither grub2-reboot nor grub-reboot found in PATH")
}

pub fn run() -> Result<()> {
    let current = current_digest();

    let mut entries = load_entries()?;

    // Keep only entries that are not the currently booted deployment.
    entries.retain(|e| Some(&e.composefs_digest) != current.as_ref());

    if entries.is_empty() {
        bail!("no previous composefs deployment found in {ENTRIES_DIR}");
    }

    // Most-recently written entry = what the last prepare-boot produced.
    entries.sort_by_key(|e| e.mtime);
    let previous = entries.last().unwrap();

    let id = entry_id(&previous.path);
    // cfsctl names BLS entries after the kernel version (e.g. "6.12.3-200.fc41.x86_64.conf"),
    // so the file stem is what grub2-reboot/grub-reboot expects as its entry identifier.
    grub_reboot(id)?;

    println!("Next boot will use {}.", previous.composefs_digest);
    println!("Run 'systemctl reboot' to apply.");
    Ok(())
}
