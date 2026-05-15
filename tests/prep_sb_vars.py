#!/usr/bin/env python3
"""
Add the composefs-os Secure Boot signing cert to an OVMF_VARS file so that
QEMU boots the UKI+SB disk image with Secure Boot enforcement without any
manual key enrollment.

Reads the DER cert from <disk>.sb.cer (written by cbootc install to-disk
alongside the disk image), then uses virt-fw-vars to add it to the UEFI
Signature Database (db) in a copy of OVMF_VARS.

Requires: python3-virt-firmware  (dnf install python3-virt-firmware)

Usage:
    python3 tests/prep_sb_vars.py disk-uki-sb.raw ovmf-vars-uki-sb.fd
    python3 tests/prep_sb_vars.py disk-uki-sb.raw ovmf-vars-uki-sb.fd \\
        --base-vars /usr/share/edk2/ovmf/OVMF_VARS.secboot.fd
"""

import argparse
import os
import sys

OVMF_VARS_CANDIDATES = [
    "/usr/share/edk2/ovmf/OVMF_VARS.secboot.fd",  # Fedora
    "/usr/share/OVMF/OVMF_VARS_4M.ms.fd",          # Ubuntu 24.04
    "/usr/share/OVMF/OVMF_VARS.secboot.fd",         # Ubuntu older
]


def find_base_vars():
    for p in OVMF_VARS_CANDIDATES:
        if os.path.exists(p):
            return p
    return None


def main():
    parser = argparse.ArgumentParser(
        description="Prepare OVMF_VARS with the composefs-os SB cert enrolled"
    )
    parser.add_argument("disk_image", help="Path to disk-uki-sb.raw")
    parser.add_argument("output_vars", help="Path for the output OVMF_VARS file")
    parser.add_argument("--base-vars", help="Base OVMF_VARS file to start from")
    args = parser.parse_args()

    # Locate base OVMF_VARS
    base_vars = args.base_vars or find_base_vars()
    if not base_vars:
        print(
            "ERROR: OVMF_VARS.secboot.fd not found. "
            "Install edk2-ovmf (Fedora) or ovmf (Ubuntu), "
            "or pass --base-vars."
        )
        sys.exit(1)

    # The cert is written by cbootc alongside the disk image as <disk>.sb.cer
    cert_path = args.disk_image + ".sb.cer"
    if not os.path.exists(cert_path):
        print(
            f"ERROR: SB cert not found at {cert_path}\n"
            "       Run 'just install-disk-uki-secureboot' first — cbootc writes\n"
            "       <disk>.sb.cer alongside the disk image during install."
        )
        sys.exit(1)

    try:
        import virt.firmware.vars as varstore  # noqa: F401 — check import before subprocess
    except ImportError:
        print(
            "ERROR: python3-virt-firmware not installed.\n"
            "       dnf install python3-virt-firmware"
        )
        sys.exit(1)

    import subprocess

    print(f"Base OVMF_VARS : {base_vars}")
    print(f"SB cert        : {cert_path}")
    print(f"Output VARS    : {args.output_vars}")

    # The owner GUID identifies who added this db entry; value is arbitrary for
    # custom keys — use a stable composefs-os namespace UUID.
    OWNER_GUID = "a3d08522-5ba0-4c92-b57c-c98eb7ab2f8b"

    result = subprocess.run(
        [
            "virt-fw-vars",
            "--input", base_vars,
            "--output", args.output_vars,
            "--add-db", OWNER_GUID, cert_path,
        ],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(f"ERROR: virt-fw-vars failed:\n{result.stderr}")
        sys.exit(1)

    print(f"OK — {args.output_vars} ready for 'just e2e-uki-secureboot'")


if __name__ == "__main__":
    main()
