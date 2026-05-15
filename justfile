set shell := ["bash", "-euo", "pipefail", "-c"]

# Image tags — match what .github/workflows/e2e.yml expects
base_image        := "composefs-os:fedora-44"
base_image_uki    := "composefs-os:fedora-44-uki"
example_image     := "composefs-os-test:latest"
example_image_uki := "composefs-os-uki-test:latest"

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
    podman build -t {{base_image}} --target grub -f Containerfile.base .

# Build the base UKI/systemd-boot image (slow)
build-base-uki:
    podman build -t {{base_image_uki}} --target uki -f Containerfile.base .

# Build the example GRUB image on top of the base (fast)
build-example base=base_image:
    podman build -t {{example_image}} \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/fedora/Containerfile .

# Build the example UKI image on top of the UKI base (fast)
build-example-uki base=base_image_uki:
    podman build -t {{example_image_uki}} \
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

# ── Cleanup ───────────────────────────────────────────────────────────────────

# Remove Rust build artifacts
clean:
    cargo clean
