# composefs-os / cbootc Design Notes

composefs-os ships bootable Linux OCI images backed by composefs-rs.
cbootc is the embedded upgrade/rollback tool inside those images — a minimal
bootc-like operational layer with no ostree dependency.

## Status

Working implementation. All core commands (install, upgrade, switch, rollback,
status, verify) are functional on x86-64 EFI systems.

## Goals

- Boot and update Linux systems from OCI container images
- Use composefs-rs (via cfsctl) as the only storage and deployment backend
- Stay small enough for one person to maintain (~3-5k LOC total)
- Support Fedora, Ubuntu, and Arch from the same binary
- Keep cbootc itself completely distro-neutral; push distro differences
  into example Containerfiles

## Non-Goals

- Replacing bootc as a general-purpose tool
- Supporting ostree (ever)
- Fleet management, complex policy, integration with vendor support tools
- Layered packages, derived images at runtime
- Anything beyond systemd-based distros
- Backwards compatibility — we move with composefs-rs upstream

## Architecture Overview

```
+--------------------------+
|  cbootc CLI (this tool)  |  thin operational layer
+-----------+--------------+
            |
            v
+--------------------------+
|  cfsctl (composefs-rs)   |  storage primitive: pull, hash, prepare-boot
+-----------+--------------+
            |
            v
+--------------------------+
|  EROFS + overlayfs +     |  kernel features only — no userspace at runtime
|  fs-verity               |
+--------------------------+
```

cbootc shells out to cfsctl (initially) or uses it as a library (eventually).
Everything cbootc does that cfsctl already does is delegated. cbootc's job is
the operational layer above storage: CLI, status, upgrade orchestration,
signature verification, rollback, install, systemd timer integration.

## Key Design Decisions

### 1. composefs-rs only, no backend abstraction

bootc has a storage abstraction so it can support both ostree and (eventually)
composefs-rs. cbootc deliberately does not. This is the single biggest
simplification vs. bootc and the main reason cbootc can stay small.

If we ever need a second backend, that's the point at which cbootc has scope-
crept beyond its goals and the answer is "use bootc instead."

### 2. cfsctl as subprocess initially, library later

For v1, shell out to cfsctl as a subprocess. Parse its output, handle exit
codes, surface errors. This keeps cbootc decoupled from cfsctl's internal API
which is still moving.

Once cfsctl's library API stabilizes (and once we hit a real performance or
error-handling reason to care), migrate to using it as a Rust crate.

### 3. dracut as the only initramfs

All three target distros (Fedora, Ubuntu, Arch) support dracut. The
composefs-rs project ships a dracut module. Standardizing on dracut everywhere
eliminates the need to port the composefs mount logic to initramfs-tools and
mkinitcpio. One place for that logic to live.

Users who want initramfs-tools or mkinitcpio can write their own equivalent
and contribute it. Not in scope for the core tool.

### 4. Distro neutrality in cbootc itself

Zero `if distro == "fedora"` paths in cbootc source. All distro differences
live in the Containerfile examples under `examples/<distro>/`. cbootc only
sees:
- An OCI image reference
- The current cmdline
- The boot directory layout
- systemd journal output

None of which are distro-specific.

### 5. BLS for boot entries

Boot Loader Specification snippets under `/boot/loader/entries/`. Works with
GRUB and systemd-boot, which covers the realistic deployment targets. cfsctl
already writes these; cbootc just triggers it.

### 6. Cosign for image signing

Signature verification before deploy, using `sigstore-rs` or by shelling out
to `cosign verify`. Enforced when a public key is configured; warning-only
when none is. Configuration via `/etc/cbootc/config.toml`.

### 7. Rollback via grubenv next_entry, not state machines

Rather than tracking deployment generations in custom state, lean on the BLS
entries cfsctl writes. Rollback = write `next_entry=<digest>` to
`/boot/grub2/grubenv` via `grub2-editenv`, then reboot. GRUB reads the env,
boots that entry once, and clears it. Simple, debuggable, no custom state to
corrupt.

### 8. Installer in the binary

`cbootc install to-disk <DEVICE>` runs inside the container image and writes a
bootable system to a block device or raw file. It handles partitioning (GPT,
via sfdisk), formatting, composefs repo initialisation, EFI setup, and
shared-var wiring. Running inside the container means the source image is always
the container itself — no separate image reference needed at install time.

Two EFI boot modes are supported via `--secure-boot`:

- **Default:** runs `grub2-install --target=x86_64-efi` to generate a GRUB EFI
  binary. Works on any EFI system; rejected by firmware with Secure Boot enabled.
- **`--secure-boot`:** copies the pre-signed shim (`shimx64.efi`) and Fedora-signed
  GRUB (`grubx64.efi`) from `/usr/share/efi/` (preserved in the base image at
  build time) to the ESP. The full GRUB config is written directly to the ESP so
  GRUB can resolve BLS entries without crossing partition boundaries under Secure
  Boot lockdown. No custom key enrollment required — the Microsoft-signed shim
  trusts the Fedora-signed GRUB out of the box.

## Command Surface

```
cbootc upgrade          Pull latest image, prepare boot entry, optionally reboot
cbootc status           Show current digest, tracked image, last upgrade time
cbootc rollback         Mark previous deployment as next boot
cbootc switch <ref>     Change tracked image reference
cbootc verify           Verify current image's signature against configured key
```

That's it. Five commands. Compare to bootc's ~15.

## File Layout on Target System

```
/sysroot/composefs/                  cfsctl repo (objects, images, streams)
/boot/                               kernel + initramfs + BLS entries
/boot/loader/entries/<digest>.conf   BLS snippet with composefs=<digest>
/boot/grub2/grubenv                  GRUB env block (next_entry for rollback)
/var/lib/cbootc/config.toml          tracked image reference
/var/lib/cbootc/state.json           last-upgrade time, last-known-good digest
/usr/lib/systemd/system/cbootc-*     timer + service for auto-updates (optional)
```

State is minimal. config.toml is user-managed; state.json is cbootc-managed
and can be regenerated from cfsctl + journal if lost.

## Implementation Plan

Rough order of work, each step independently testable:

1. **Project skeleton.** `cargo init`, clap-based CLI with stub commands that
   print "not implemented." Minimal `cbootc --help` works.

2. **`cbootc status`.** Read `/proc/cmdline`, extract `composefs=<digest>`,
   shell out to cfsctl to get tracked image, format as text or JSON.
   Read-only; no risk; good first end-to-end test.

3. **`cbootc upgrade`.** Shell out to `cfsctl oci pull` + `prepare-boot`,
   update state.json, optionally trigger reboot. The core operation.

4. **`cbootc rollback`.** Shell out to `grub2-reboot` (or systemd-boot
   equivalent) targeting the previous BLS entry. Trivial.

5. **`cbootc switch`.** Edit config.toml, then call upgrade logic with the
   new ref.

6. **Cosign verification.** Wrap in a `verify_image()` helper that the upgrade
   path calls before prepare-boot. Configurable strictness.

7. **systemd timer.** Ship `cbootc-update.timer` + `.service` units that run
   `cbootc upgrade --no-reboot` daily, with proper backoff and journal
   logging. Optional; user enables manually.

8. **Integration tests.** Build images with the example Containerfiles, run
   in QEMU via the build-disk script, exercise upgrade/rollback in the VM.

## Multi-Distro Strategy

One cbootc binary. Three Containerfile examples. Same disk-builder script for
all three. The contract an image must meet:

- `/usr/lib/modules/<kver>/vmlinuz` — kernel
- `/usr/lib/modules/<kver>/initramfs.img` — initramfs with composefs dracut module
- `/usr/etc/` — factory `/etc` content
- `/usr/share/factory/var/` — factory `/var` content
- tmpfiles.d snippets for runtime population
- systemd as PID 1
- `containers.bootc=1` label on the image

If your image meets that contract, cbootc works. Distro doesn't matter.

## Out of Scope (Things to Say No To)

- ostree compatibility
- Layered packages
- Custom bootloaders beyond GRUB / systemd-boot
- Non-systemd init
- Online package management of any kind
- Fleet management protocols
- Migration from existing bootc/ostree systems

If a feature request fits one of these, the answer is "use bootc instead."

## Open Questions

- Exact cfsctl subcommand names — they've moved before, will move again.
  Wrap in a `cfsctl_cmd()` helper to centralize the version dependency.
- Whether to use sigstore-rs as a library or shell out to cosign. Library is
  cleaner; cosign is more battle-tested. Lean library, fall back to cosign
  if integration is painful.
- How to handle SELinux relabeling in the image build pipeline. Probably a
  step in the build-disk script that runs `setfiles` against the unpacked
  rootfs before cfsctl pulls it.
- Whether `cbootc upgrade` should reboot by default or require `--reboot`.
  Lean toward requiring `--reboot` for safety; have the systemd timer pass it.

## References

- composefs-rs: https://github.com/containers/composefs-rs
- bootc: https://github.com/bootc-dev/bootc
- BLS spec: https://uapi-group.org/specifications/specs/boot_loader_specification/
- cosign: https://github.com/sigstore/cosign
