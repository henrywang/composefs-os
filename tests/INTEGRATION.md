# cbootc Integration Test Checklist

Run these tests in a QEMU VM booted from a composefs-rs image. Do not run on
a host machine — several steps write to `/boot`, `/etc/cbootc`, and
`/var/lib/cbootc`.

## Prerequisites

Before starting, build the binary and get it into the VM:

```sh
# On the host
cargo build --release
# Copy binary into image or scp after boot
scp target/release/cbootc root@<vm-ip>:/usr/bin/cbootc
```

Ensure `cfsctl` is installed in the VM and the composefs repo is at
`/composefs`.

---

## 1. Fresh boot — no config

```sh
cbootc status
```

Expected output (no errors, graceful "not configured" messages):

```
Image:        (not configured)
Digest:       <sha256:...>   # must be non-empty; read from /proc/cmdline
Last upgrade: (none recorded)
```

- [ ] Command exits 0
- [ ] Digest field shows the composefs hash from `/proc/cmdline`
- [ ] No panic or error trace

---

## 2. `switch` — adopt an image reference

```sh
cbootc switch docker://registry.example.com/my-image:latest
```

- [ ] `/etc/cbootc/config.toml` created; contains `ref = "docker://..."`
- [ ] Pull output from cfsctl visible on stdout
- [ ] Boot entry written to `/boot/loader/entries/`; file is named
      `<kver>.conf` (e.g. `6.12.3-200.fc41.x86_64.conf`)
- [ ] `/var/lib/cbootc/state.json` created with `last_upgrade` RFC3339 timestamp
      and `last_known_good_digest`
- [ ] `cbootc status` now shows the image ref and the new digest

---

## 3. `upgrade` — pull a newer tag

Push a new image to the registry, then:

```sh
cbootc upgrade
```

- [ ] Pull completes without error
- [ ] New digest differs from previous; `state.json` updated
- [ ] New `.conf` entry in `/boot/loader/entries/`; old entry still present
- [ ] Message: `Run 'systemctl reboot' to apply, or pass --reboot.`

### 3a. `upgrade --reboot`

```sh
cbootc upgrade --reboot
```

- [ ] System reboots
- [ ] After reboot, `cbootc status` shows the new digest in Digest field

---

## 4. `rollback` — revert to previous deployment

After step 3a, run:

```sh
cbootc rollback
```

- [ ] `grub2-reboot` (or `grub-reboot`) called with the previous entry's stem
      (verify with `journalctl -t cbootc` or strace if needed)
- [ ] Message: `Next boot will use <prev-digest>.`
- [ ] After `systemctl reboot`, system boots the previous digest
- [ ] `cbootc status` shows old digest in Digest field

### Rollback with only one entry

Remove all but one `.conf` from `/boot/loader/entries/`, then:

```sh
cbootc rollback
```

- [ ] Exits non-zero with: `no previous composefs deployment found in /boot/loader/entries`

---

## 5. `verify` — signature check

### 5a. No signing key configured

```sh
cbootc verify
```

- [ ] Exits non-zero with: `no signing key configured in /etc/cbootc/signing.toml`

### 5b. Valid key

```sh
# Generate a test keypair
cosign generate-key-pair

cat > /etc/cbootc/signing.toml <<'EOF'
key = "/etc/cbootc/cosign.pub"
EOF
cp cosign.pub /etc/cbootc/cosign.pub

# Sign the running image (must have access to the registry)
cosign sign --key cosign.key docker://registry.example.com/my-image@<digest>

cbootc verify
```

- [ ] Exits 0
- [ ] Prints: `Verified: docker://...`

### 5c. Mismatched key

Replace `/etc/cbootc/cosign.pub` with a different public key:

```sh
cbootc verify
```

- [ ] Exits non-zero with: `signature verification failed for ...`

### 5d. Manifest digest recorded in state.json

After a successful `upgrade` or `verify` with a signing key configured,
`state.json` gains a `last_verified_manifest` field:

```json
{
  "last_upgrade": "2026-04-28T...",
  "last_known_good_digest": "sha256:...",
  "last_verified_manifest": "sha256:..."
}
```

- [ ] `last_verified_manifest` present and is a valid `sha256:` string
- [ ] `cbootc verify` prints `Manifest: sha256:...` on stdout
- [ ] `cbootc upgrade` prints `Verified manifest: sha256:...` when a key is configured

**Known limitation (TOCTOU):** cosign verifies whatever manifest the registry
tag resolves to at verify time. If the tag advances between `cfsctl pull` and
the `cosign verify` call, cosign verifies the newer manifest, not what was
staged. Closing that gap requires passing a digest-pinned reference
(`image@sha256:...`) to cosign, which in turn requires cfsctl to expose the
OCI manifest digest of the pulled image. Track as a follow-on enhancement.

---

## 6. Automatic update timer

```sh
cp units/cbootc-update.{service,timer} /etc/systemd/system/
systemctl enable --now cbootc-update.timer
systemctl list-timers cbootc-update.timer
```

- [ ] Timer shows `NEXT` timestamp roughly 24 h out (± 1 h random delay)
- [ ] Manually trigger: `systemctl start cbootc-update.service`
- [ ] Service exits 0; `state.json` `last_upgrade` timestamp updated

### Timer no-op when config absent

```sh
mv /etc/cbootc/config.toml /tmp/config.toml.bak
systemctl start cbootc-update.service
```

- [ ] Service exits 0 immediately (ConditionPathExists skips ExecStart)

Restore: `mv /tmp/config.toml.bak /etc/cbootc/config.toml`

---

## 7. Dracut / initramfs (prerequisite — separate issue)

The Fedora Containerfile at `examples/fedora/Containerfile` has a `TODO` at
step 3: the composefs dracut module from `composefs-rs/dracut/` must be
dropped into the image before `dracut` is run, otherwise the initramfs cannot
mount the composefs root.

Until that module is vendored or fetched, the image will not boot. Track as a
separate issue; the tests above assume a working boot.

---

## Observations log

| Date | Test | Result | Notes |
|------|------|--------|-------|
|      |      |        |       |
