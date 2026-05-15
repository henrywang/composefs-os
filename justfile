set shell := ["bash", "-euo", "pipefail", "-c"]

# Image tags — match what .github/workflows/e2e.yml expects
base_image           := "composefs-os:fedora-44"
base_image_uki       := "composefs-os:fedora-44-uki"
base_image_uki_sb    := "composefs-os:fedora-44-uki-sb"
example_image        := "composefs-os-test:latest"
example_image_uki    := "composefs-os-uki-test:latest"
example_image_uki_sb := "composefs-os-uki-sb-test:latest"

# List available recipes
default:
    @just --list

# ── Rust ─────────────────────────────────────────────────────────────────────

# Compile cbootc (output: target/release/cbootc)
build:
    cargo build --release

# Run unit tests
test:
    cargo test

# Check formatting and lints without modifying files (safe for CI)
check:
    cargo fmt --check
    cargo clippy -- -D warnings

# Reformat source code
fmt:
    cargo fmt

# ── Container images ──────────────────────────────────────────────────────────

# Build the base GRUB image (slow — runs dnf + dracut inside the container)
build-base:
    podman build --network=host -t {{base_image}} --target grub -f Containerfile.base .

# Build the base UKI/systemd-boot image (slow)
build-base-uki:
    podman build --network=host -t {{base_image_uki}} --target uki -f Containerfile.base .

# Build the base UKI + Secure Boot image (slow)
build-base-uki-secureboot:
    podman build --network=host -t {{base_image_uki_sb}} --target uki-secureboot -f Containerfile.base .

# Build the example GRUB image on top of the base (fast)
build-example base=base_image:
    podman build -t {{example_image}} \
        --network=host \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/fedora/Containerfile .

# Build the example UKI image on top of the UKI base (fast)
build-example-uki base=base_image_uki:
    podman build -t {{example_image_uki}} \
        --network=host \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/fedora/Containerfile .

# Build the example UKI + Secure Boot image on top of the UKI-SB base (fast)
build-example-uki-secureboot base=base_image_uki_sb:
    podman build -t {{example_image_uki_sb}} \
        --network=host \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/fedora/Containerfile .

# ── Disk install ──────────────────────────────────────────────────────────────

# Create a bootable GRUB raw disk image (requires sudo)
install-disk image=example_image disk="disk.raw" size="10G":
    sudo podman run --rm --privileged \
        -v "$(pwd)":/output \
        -v /var/lib/containers:/var/lib/containers \
        -v /var/tmp:/var/tmp \
        {{image}} \
        cbootc install to-disk /output/{{disk}} --size {{size}}

# Create a Secure Boot disk image (requires sudo)
install-disk-secureboot image=example_image disk="disk-sb.raw" size="10G":
    sudo podman run --rm --privileged \
        -v "$(pwd)":/output \
        -v /var/lib/containers:/var/lib/containers \
        -v /var/tmp:/var/tmp \
        {{image}} \
        cbootc install to-disk /output/{{disk}} --size {{size}} --secure-boot

# Create a UKI/systemd-boot disk image (requires sudo)
install-disk-uki image=example_image_uki disk="disk-uki.raw" size="10G":
    sudo podman run --rm --privileged \
        -v "$(pwd)":/output \
        -v /var/lib/containers:/var/lib/containers \
        -v /var/tmp:/var/tmp \
        {{image}} \
        cbootc install to-disk /output/{{disk}} --size {{size}} --uki

# Create a UKI + Secure Boot disk image (requires sudo)
install-disk-uki-secureboot image=example_image_uki_sb disk="disk-uki-sb.raw" size="10G":
    sudo podman run --rm --privileged \
        -v "$(pwd)":/output \
        -v /var/lib/containers:/var/lib/containers \
        -v /var/tmp:/var/tmp \
        {{image}} \
        cbootc install to-disk /output/{{disk}} --size {{size}} --uki --secure-boot

# ── End-to-end tests ──────────────────────────────────────────────────────────

# Run e2e tests against a GRUB disk image
e2e disk="disk.raw":
    python3 tests/e2e.py {{disk}}

# Run e2e tests against a UKI/systemd-boot disk image
# Pass ovmf_vars to enable Q35/SMM machine type (required by some firmware)
e2e-uki disk="disk-uki.raw" ovmf_vars="":
    v="{{ovmf_vars}}"; python3 tests/e2e.py --uki ${v:+--ovmf-vars "$v"} {{disk}}

# Run e2e tests with Secure Boot enforcement
e2e-secureboot disk="disk-sb.raw":
    python3 tests/e2e.py --secure-boot {{disk}}

# Enroll the SB cert from <disk>.sb.cer into a fresh OVMF_VARS copy
prep-sb-vars disk="disk-uki-sb.raw" vars_out="ovmf-vars-uki-sb.fd":
    python3 tests/prep_sb_vars.py {{disk}} {{vars_out}}

# Run e2e tests against a UKI + Secure Boot disk image
# Prepares a custom OVMF_VARS with the cert enrolled automatically if --ovmf-vars is not given
e2e-uki-secureboot disk="disk-uki-sb.raw" ovmf_vars="":
    #!/usr/bin/env bash
    set -euo pipefail
    vars="{{ovmf_vars}}"
    if [ -z "$vars" ]; then
        just prep-sb-vars {{disk}} ovmf-vars-uki-sb.fd
        vars=ovmf-vars-uki-sb.fd
    fi
    python3 tests/e2e.py --uki-secureboot --ovmf-vars "$vars" {{disk}}

# ── Convenience combos ────────────────────────────────────────────────────────

# Rust checks only — fast, no containers needed
ci: check test build

# Full GRUB workflow: build images → create disk → run e2e
ci-grub: build-base (build-example base_image)
    just install-disk
    just e2e

# Full Secure Boot workflow: build images → create disk → run e2e
ci-secureboot: build-base (build-example base_image)
    just install-disk-secureboot
    just e2e-secureboot

# Full UKI workflow: build images → create disk → run e2e
ci-uki: build-base-uki (build-example-uki base_image_uki)
    just install-disk-uki
    just e2e-uki

# Full UKI + Secure Boot workflow: build images → create disk → run e2e
ci-uki-secureboot: build-base-uki-secureboot (build-example-uki-secureboot base_image_uki_sb)
    just install-disk-uki-secureboot
    just e2e-uki-secureboot

# ── Cleanup ───────────────────────────────────────────────────────────────────

# Remove Rust build artifacts
clean:
    cargo clean
