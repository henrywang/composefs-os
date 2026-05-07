use anyhow::{Context, Result};
use std::process::Command;

const REPO: &str = "/sysroot/composefs";

fn base() -> Command {
    let mut cmd = Command::new("cfsctl");
    cmd.arg("--repo").arg(REPO);
    cmd
}

/// Run a cfsctl subcommand, inheriting stdio so progress reaches the terminal.
pub fn run(args: &[&str]) -> Result<()> {
    let status = base().args(args).status().context("spawning cfsctl")?;
    if !status.success() {
        anyhow::bail!("cfsctl {} exited {}", args.join(" "), status);
    }
    Ok(())
}

/// Run a cfsctl subcommand and return its stdout as a string.
pub fn output(args: &[&str]) -> Result<String> {
    let out = base().args(args).output().context("spawning cfsctl")?;
    if !out.status.success() {
        anyhow::bail!(
            "cfsctl {} exited {}: {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    String::from_utf8(out.stdout).context("cfsctl output was not valid UTF-8")
}
