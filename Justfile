set shell := ["bash", "-euo", "pipefail", "-c"]

# Image tags — match what .github/workflows/e2e.yml expects
base_image           := "composefs-os:fedora-44"
base_image_uki       := "composefs-os:fedora-44-uki"
base_image_uki_sb    := "composefs-os:fedora-44-uki-sb"
example_image        := "composefs-os-test:latest"
example_image_uki    := "composefs-os-uki-test:latest"
example_image_uki_sb := "composefs-os-uki-sb-test:latest"

# Ubuntu image tags
base_image_ubuntu           := "composefs-os:ubuntu-26.04"
base_image_ubuntu_uki       := "composefs-os:ubuntu-26.04-uki"
base_image_ubuntu_uki_sb    := "composefs-os:ubuntu-26.04-uki-sb"
example_image_ubuntu        := "composefs-os-ubuntu-test:latest"
example_image_ubuntu_uki    := "composefs-os-ubuntu-uki-test:latest"
example_image_ubuntu_uki_sb := "composefs-os-ubuntu-uki-sb-test:latest"

# AWS image tags
aws_image_fedora := "composefs-os-aws:fedora-44"
aws_image_ubuntu := "composefs-os-aws:ubuntu-26.04"

# Arch image tags (rolling release — no version suffix)
base_image_arch           := "composefs-os:arch-latest"
base_image_arch_uki       := "composefs-os:arch-latest-uki"
base_image_arch_uki_sb    := "composefs-os:arch-latest-uki-sb"
example_image_arch        := "composefs-os-arch-test:latest"
example_image_arch_uki    := "composefs-os-arch-uki-test:latest"
example_image_arch_uki_sb := "composefs-os-arch-uki-sb-test:latest"

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
    podman build --network=host -t {{base_image}} --target grub -f Containerfile.fedora .

# Build the base UKI/systemd-boot image (slow)
build-base-uki:
    podman build --network=host -t {{base_image_uki}} --target uki -f Containerfile.fedora .

# Build the base UKI + Secure Boot image (slow)
build-base-uki-secureboot:
    podman build --network=host -t {{base_image_uki_sb}} --target uki-secureboot -f Containerfile.fedora .

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

# Build the Ubuntu base GRUB image
build-base-ubuntu:
    podman build --network=host -t {{base_image_ubuntu}} --target grub -f Containerfile.ubuntu .

# Build the Ubuntu base UKI/systemd-boot image
build-base-ubuntu-uki:
    podman build --network=host -t {{base_image_ubuntu_uki}} --target uki -f Containerfile.ubuntu .

# Build the Ubuntu base UKI + Secure Boot image
build-base-ubuntu-uki-secureboot:
    podman build --network=host -t {{base_image_ubuntu_uki_sb}} --target uki-secureboot -f Containerfile.ubuntu .

# Build the Ubuntu example GRUB image on top of the base
build-example-ubuntu base=base_image_ubuntu:
    podman build -t {{example_image_ubuntu}} \
        --network=host \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/ubuntu/Containerfile .

# Build the Ubuntu example UKI image
build-example-ubuntu-uki base=base_image_ubuntu_uki:
    podman build -t {{example_image_ubuntu_uki}} \
        --network=host \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/ubuntu/Containerfile .

# Build the Ubuntu example UKI + Secure Boot image
build-example-ubuntu-uki-secureboot base=base_image_ubuntu_uki_sb:
    podman build -t {{example_image_ubuntu_uki_sb}} \
        --network=host \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/ubuntu/Containerfile .

# Build the Fedora AWS image (GRUB base + cloud-init)
build-aws-fedora base=base_image:
    podman build -t {{aws_image_fedora}} \
        --network=host \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/fedora/Containerfile.aws .

# Build the Ubuntu AWS image (GRUB base + cloud-init)
build-aws-ubuntu base=base_image_ubuntu:
    podman build -t {{aws_image_ubuntu}} \
        --network=host \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/ubuntu/Containerfile.aws .

# Build the Arch base GRUB image
build-base-arch:
    podman build --network=host -t {{base_image_arch}} --target grub -f Containerfile.arch .

# Build the Arch base UKI/systemd-boot image
build-base-arch-uki:
    podman build --network=host -t {{base_image_arch_uki}} --target uki -f Containerfile.arch .

# Build the Arch base UKI + Secure Boot image
build-base-arch-uki-secureboot:
    podman build --network=host -t {{base_image_arch_uki_sb}} --target uki-secureboot -f Containerfile.arch .

# Build the Arch example GRUB image on top of the base
build-example-arch base=base_image_arch:
    podman build -t {{example_image_arch}} \
        --network=host \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/arch/Containerfile .

# Build the Arch example UKI image
build-example-arch-uki base=base_image_arch_uki:
    podman build -t {{example_image_arch_uki}} \
        --network=host \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/arch/Containerfile .

# Build the Arch example UKI + Secure Boot image
build-example-arch-uki-secureboot base=base_image_arch_uki_sb:
    podman build -t {{example_image_arch_uki_sb}} \
        --network=host \
        --build-arg BASE_IMAGE={{base}} \
        -f examples/arch/Containerfile .

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

# Create a Fedora AWS raw disk image for import into EC2 (requires sudo)
install-disk-aws-fedora image=aws_image_fedora disk="disk-aws-fedora.raw" size="10G":
    sudo podman run --rm --privileged \
        -v "$(pwd)":/output \
        -v /var/lib/containers:/var/lib/containers \
        -v /var/tmp:/var/tmp \
        {{image}} \
        cbootc install to-disk /output/{{disk}} --size {{size}}

# Create an Ubuntu AWS raw disk image for import into EC2 (requires sudo)
install-disk-aws-ubuntu image=aws_image_ubuntu disk="disk-aws-ubuntu.raw" size="10G":
    sudo podman run --rm --privileged \
        -v "$(pwd)":/output \
        -v /var/lib/containers:/var/lib/containers \
        -v /var/tmp:/var/tmp \
        {{image}} \
        cbootc install to-disk /output/{{disk}} --size {{size}}

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

# ── Upgrade / Switch / Rollback e2e tests ────────────────────────────────────

# Run upgrade/switch/rollback e2e against a GRUB disk image (requires sudo for podman)
e2e-upgrade image=example_image disk="disk.raw":
    sudo python3 tests/e2e.py --upgrade --source-image {{image}} {{disk}}

# Run upgrade/switch/rollback e2e against a GRUB + Secure Boot disk image
e2e-upgrade-secureboot image=example_image disk="disk-sb.raw":
    sudo python3 tests/e2e.py --upgrade --secure-boot --source-image {{image}} {{disk}}

# Run upgrade/switch/rollback e2e against a UKI/systemd-boot disk image
e2e-uki-upgrade image=example_image_uki disk="disk-uki.raw" ovmf_vars="":
    v="{{ovmf_vars}}"; sudo python3 tests/e2e.py --upgrade --uki --source-image {{image}} ${v:+--ovmf-vars "$v"} {{disk}}

# Run upgrade/switch/rollback e2e against a UKI + Secure Boot disk image
e2e-uki-upgrade-secureboot image=example_image_uki_sb disk="disk-uki-sb.raw" ovmf_vars="":
    #!/usr/bin/env bash
    set -euo pipefail
    vars="{{ovmf_vars}}"
    if [ -z "$vars" ]; then
        just prep-sb-vars {{disk}} ovmf-vars-uki-sb.fd
        vars=ovmf-vars-uki-sb.fd
    fi
    sudo python3 tests/e2e.py --upgrade --uki-secureboot --source-image {{image}} --ovmf-vars "$vars" {{disk}}

# Arch upgrade variants
e2e-upgrade-arch image=example_image_arch disk="disk-arch.raw":
    sudo python3 tests/e2e.py --upgrade --source-image {{image}} {{disk}}

e2e-uki-upgrade-arch image=example_image_arch_uki disk="disk-arch-uki.raw" ovmf_vars="":
    v="{{ovmf_vars}}"; sudo python3 tests/e2e.py --upgrade --uki --source-image {{image}} ${v:+--ovmf-vars "$v"} {{disk}}

e2e-uki-upgrade-secureboot-arch image=example_image_arch_uki_sb disk="disk-arch-uki-sb.raw" ovmf_vars="":
    #!/usr/bin/env bash
    set -euo pipefail
    vars="{{ovmf_vars}}"
    if [ -z "$vars" ]; then
        just prep-sb-vars {{disk}} ovmf-vars-arch-uki-sb.fd
        vars=ovmf-vars-arch-uki-sb.fd
    fi
    sudo python3 tests/e2e.py --upgrade --uki-secureboot --source-image {{image}} --ovmf-vars "$vars" {{disk}}

# Ubuntu upgrade variants
e2e-upgrade-ubuntu image=example_image_ubuntu disk="disk-ubuntu.raw":
    sudo python3 tests/e2e.py --upgrade --source-image {{image}} {{disk}}

e2e-upgrade-secureboot-ubuntu image=example_image_ubuntu disk="disk-ubuntu-sb.raw":
    sudo python3 tests/e2e.py --upgrade --secure-boot --source-image {{image}} {{disk}}

e2e-uki-upgrade-ubuntu image=example_image_ubuntu_uki disk="disk-ubuntu-uki.raw" ovmf_vars="":
    v="{{ovmf_vars}}"; sudo python3 tests/e2e.py --upgrade --uki --source-image {{image}} ${v:+--ovmf-vars "$v"} {{disk}}

e2e-uki-upgrade-secureboot-ubuntu image=example_image_ubuntu_uki_sb disk="disk-ubuntu-uki-sb.raw" ovmf_vars="":
    #!/usr/bin/env bash
    set -euo pipefail
    vars="{{ovmf_vars}}"
    if [ -z "$vars" ]; then
        just prep-sb-vars {{disk}} ovmf-vars-ubuntu-uki-sb.fd
        vars=ovmf-vars-ubuntu-uki-sb.fd
    fi
    sudo python3 tests/e2e.py --upgrade --uki-secureboot --source-image {{image}} --ovmf-vars "$vars" {{disk}}

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

# Full Arch GRUB workflow (no Secure Boot — shim-signed not in official repos)
ci-arch-grub: build-base-arch (build-example-arch base_image_arch)
    just install-disk {{example_image_arch}} disk-arch.raw 5G
    just e2e disk-arch.raw

# Full Arch UKI workflow
ci-arch-uki: build-base-arch-uki (build-example-arch-uki base_image_arch_uki)
    #!/usr/bin/env bash
    set -euo pipefail
    just install-disk-uki {{example_image_arch_uki}} disk-arch-uki.raw 5G
    ovmf_vars=$(find /usr/share -name 'OVMF_VARS.fd' ! -name '*.secboot*' ! -name '*.ms.*' 2>/dev/null | head -1)
    just e2e-uki disk-arch-uki.raw "$ovmf_vars"

# Full Arch UKI + Secure Boot workflow
ci-arch-uki-secureboot: build-base-arch-uki-secureboot (build-example-arch-uki-secureboot base_image_arch_uki_sb)
    just install-disk-uki-secureboot {{example_image_arch_uki_sb}} disk-arch-uki-sb.raw 5G
    just e2e-uki-secureboot disk-arch-uki-sb.raw

# Full Ubuntu GRUB workflow
ci-ubuntu-grub: build-base-ubuntu (build-example-ubuntu base_image_ubuntu)
    just install-disk {{example_image_ubuntu}} disk-ubuntu.raw 5G
    just e2e disk-ubuntu.raw

# Full Ubuntu UKI workflow
ci-ubuntu-uki: build-base-ubuntu-uki (build-example-ubuntu-uki base_image_ubuntu_uki)
    just install-disk-uki {{example_image_ubuntu_uki}} disk-ubuntu-uki.raw 5G
    just e2e-uki disk-ubuntu-uki.raw

# Full Ubuntu UKI + Secure Boot workflow
ci-ubuntu-uki-secureboot: build-base-ubuntu-uki-secureboot (build-example-ubuntu-uki-secureboot base_image_ubuntu_uki_sb)
    just install-disk-uki-secureboot {{example_image_ubuntu_uki_sb}} disk-ubuntu-uki-sb.raw 5G
    just e2e-uki-secureboot disk-ubuntu-uki-sb.raw

# Full Fedora AWS image build: base → aws image → raw disk
aws-fedora: build-base (build-aws-fedora base_image)
    just install-disk-aws-fedora

# Full Ubuntu AWS image build: base → aws image → raw disk
aws-ubuntu: build-base-ubuntu (build-aws-ubuntu base_image_ubuntu)
    just install-disk-aws-ubuntu

# Full GRUB upgrade/switch/rollback workflow (Fedora)
ci-grub-upgrade: build-base (build-example base_image)
    just install-disk
    just e2e-upgrade

# Full UKI upgrade/switch/rollback workflow (Fedora)
ci-uki-upgrade: build-base-uki (build-example-uki base_image_uki)
    just install-disk-uki
    just e2e-uki-upgrade

# ── Cleanup ───────────────────────────────────────────────────────────────────

# Remove Rust build artifacts
clean:
    cargo clean
