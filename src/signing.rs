use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    fs, io,
    process::{Command, Stdio},
};

const SIGNING_CONFIG: &str = "/etc/cbootc/signing.toml";

#[derive(Deserialize)]
struct SigningConfig {
    key: String,
}

fn read_key() -> Result<Option<String>> {
    match fs::read_to_string(SIGNING_CONFIG) {
        Ok(raw) => {
            let cfg: SigningConfig =
                toml::from_str(&raw).with_context(|| format!("parsing {SIGNING_CONFIG}"))?;
            Ok(Some(cfg.key))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {SIGNING_CONFIG}")),
    }
}

/// Verify `image_ref`'s signature with the configured cosign public key.
///
/// `strict = true`  → error when no key is configured (used by `verify` command).
/// `strict = false` → warn and continue when no key is configured (used by `upgrade`).
///
/// Returns the OCI manifest digest cosign verified, if one was reported.
/// Note: this is the OCI manifest sha256, not the composefs fs-verity digest —
/// they are different hash systems applied to different content and cannot be
/// compared directly. The manifest digest is recorded in state.json for audit
/// purposes. A TOCTOU window exists between `cfsctl pull` and this call: if the
/// registry tag advances between the two, cosign verifies the new manifest, not
/// what was staged. Closing that gap requires pinning to a manifest digest
/// reference, which needs a cfsctl command to retrieve the pulled manifest digest.
pub fn verify_image(image_ref: &str, strict: bool) -> Result<Option<String>> {
    let key = match read_key()? {
        Some(k) => k,
        None if strict => {
            anyhow::bail!("no signing key configured in {SIGNING_CONFIG}");
        }
        None => {
            eprintln!("warning: no signing key configured; skipping signature verification");
            return Ok(None);
        }
    };

    // Inherit stderr so cosign's progress and error messages reach the user.
    // Capture stdout to parse the verified manifest digest from the JSON result.
    let out = Command::new("cosign")
        .args(["verify", "--key", &key, "-o", "json", image_ref])
        .stderr(Stdio::inherit())
        .output()
        .context("spawning cosign")?;

    if !out.status.success() {
        anyhow::bail!("signature verification failed for {image_ref}");
    }

    Ok(parse_manifest_digest(&out.stdout))
}

/// Extract the OCI manifest digest from cosign's `-o json` output.
///
/// cosign writes a JSON array; each element has the shape:
/// `{"critical": {"image": {"docker-manifest-digest": "sha256:..."}, ...}, ...}`
fn parse_manifest_digest(stdout: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    v.get(0)?
        .get("critical")?
        .get("image")?
        .get("docker-manifest-digest")?
        .as_str()
        .map(str::to_owned)
}
