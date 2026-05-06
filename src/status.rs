use anyhow::{Context, Result};
use serde::Deserialize;
use std::{fs, io};

use crate::config;

const CMDLINE: &str = "/proc/cmdline";
const STATE: &str = "/var/lib/cbootc/state.json";

#[derive(Deserialize)]
struct State {
    last_upgrade: Option<String>,
}

fn read_opt(path: &str) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {path}")),
    }
}

fn composefs_digest() -> Option<String> {
    fs::read_to_string(CMDLINE)
        .ok()?
        .split_whitespace()
        .find(|tok| tok.starts_with("composefs="))
        .map(|tok| tok["composefs=".len()..].to_owned())
}

fn last_upgrade() -> Result<Option<String>> {
    let Some(raw) = read_opt(STATE)? else {
        return Ok(None);
    };
    let state: State = serde_json::from_str(&raw).with_context(|| format!("parsing {STATE}"))?;
    Ok(state.last_upgrade)
}

pub fn run() -> Result<()> {
    let image = config::image_ref()?.unwrap_or_else(|| "(not configured)".to_owned());
    let digest = composefs_digest().unwrap_or_else(|| "(not booted from composefs)".to_owned());
    let upgraded = last_upgrade()?.unwrap_or_else(|| "(none recorded)".to_owned());

    println!("Image:        {image}");
    println!("Digest:       {digest}");
    println!("Last upgrade: {upgraded}");
    Ok(())
}
