#!/usr/bin/env python3
"""
End-to-end tests for composefs-os.

Boots a pre-built disk image in QEMU and verifies cbootc behaviour over the
serial console.  Requires:
  - qemu-system-x86_64
  - OVMF firmware (edk2-ovmf or ovmf package)
  - pexpect  (pip install pexpect)
  - A disk image built from examples/fedora/Containerfile (has serial autologin)

Usage:
  python3 tests/e2e.py disk.raw
  python3 tests/e2e.py --ovmf /path/to/OVMF_CODE.fd disk.raw
  python3 tests/e2e.py --secure-boot disk-sb.raw
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import pexpect

# Matches ANSI CSI sequences, OSC sequences, and bare control characters.
_ANSI_RE = re.compile(
    r'\x1b(?:'
    r'\[[0-9;:]*[mABCDEFGHJKLMPSTfhil]'
    r'|][^\x07\x1b]*(?:\x07|\x1b\\)'
    r'|[()#%A-Za-z]'
    r')'
    r'|[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]'
)


def strip_ansi(text):
    return _ANSI_RE.sub('', text)


class ConsoleLog:
    """Tees pexpect output to a file; keeps stdout clean for test results."""

    def __init__(self, path):
        self.path = path
        self._fh = open(path, 'w', encoding='utf-8', errors='replace')

    def write(self, data):
        self._fh.write(data)
        self._fh.flush()

    def flush(self):
        self._fh.flush()

    def close(self):
        self._fh.close()

    def tail(self, n=40):
        """Return the last n non-empty lines of the log, stripped of ANSI."""
        try:
            with open(self.path, encoding='utf-8', errors='replace') as f:
                raw = f.read()
            lines = [l for l in strip_ansi(raw).splitlines() if l.strip()]
            return '\n'.join(lines[-n:])
        except OSError:
            return ''


def _print_console_tail(log, n=40):
    if log is None:
        return
    tail = log.tail(n)
    if tail:
        print("        ── last console output ──────────────────────────────")
        for line in tail.splitlines():
            print(f"        {line}")
        print(f"        ── full log: {log.path} ──────────────────────────")


PROMPT = r"(?:\[root@[^\]]+\]#|root@[^:]+:[^#]*#)"
TIMEOUT_BOOT = 180
TIMEOUT_CMD = 30
TIMEOUT_UPGRADE = 300  # image pull over loopback can be slow for large OS images

REGISTRY_PORT = 5000

# QEMU user-mode networking (SLIRP): the host is always reachable from the
# guest at 10.0.2.2.  SLIRP forwards guest TCP connections to 10.0.2.2 to
# the host's loopback (127.0.0.1), so the registry container bound to
# 0.0.0.0:5000 (via --network=host) is reachable at 10.0.2.2:5000 without
# any TAP setup or firewall rules.
SLIRP_GATEWAY = "10.0.2.2"
REGISTRY_HOST = SLIRP_GATEWAY

OVMF_CANDIDATES = [
    "/usr/share/edk2/ovmf/OVMF_CODE.fd",       # Fedora
    "/usr/share/OVMF/OVMF_CODE.fd",             # Ubuntu ≤23.10
    "/usr/share/OVMF/OVMF_CODE_4M.fd",          # Ubuntu 24.04+
    "/usr/share/ovmf/OVMF_CODE.fd",
]

OVMF_SECBOOT_CODE_CANDIDATES = [
    "/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd",   # Fedora
    "/usr/share/OVMF/OVMF_CODE_4M.ms.fd",           # Ubuntu 24.04
    "/usr/share/OVMF/OVMF_CODE.secboot.fd",         # Ubuntu older
]

OVMF_SECBOOT_VARS_CANDIDATES = [
    "/usr/share/edk2/ovmf/OVMF_VARS.secboot.fd",   # Fedora
    "/usr/share/OVMF/OVMF_VARS_4M.ms.fd",           # Ubuntu 24.04
    "/usr/share/OVMF/OVMF_VARS.secboot.fd",         # Ubuntu older
]


def find_ovmf():
    for path in OVMF_CANDIDATES:
        if os.path.exists(path):
            return path
    return None


def find_ovmf_secboot():
    code = next((p for p in OVMF_SECBOOT_CODE_CANDIDATES if os.path.exists(p)), None)
    vars_ = next((p for p in OVMF_SECBOOT_VARS_CANDIDATES if os.path.exists(p)), None)
    return code, vars_


class LocalRegistry:
    """Starts a local OCI registry and pushes a v2 test image into it."""

    def __init__(self, source_image, port=REGISTRY_PORT):
        self.port = port
        self.container_name = "cbootc-test-registry"
        self.source_image = source_image
        self.image_ref = f"localhost:{port}/test-image:latest"

    def __enter__(self):
        dockerfile = (
            f"FROM {self.source_image}\n"
            "RUN echo v2 > /usr/lib/cbootc-test-version\n"
        )
        subprocess.run(
            ["podman", "build", "-t", "cbootc-test-v2:latest", "-f", "-", "."],
            input=dockerfile.encode(),
            check=True,
        )
        subprocess.run(
            ["podman", "rm", "-f", self.container_name],
            check=False,
            capture_output=True,
        )
        # --network=host: registry binds directly to 0.0.0.0:5000 in the host
        # network namespace — no container NAT or FORWARD rules needed.
        subprocess.run(
            [
                "podman", "run", "-d", "--rm",
                "--network=host",
                "--name", self.container_name,
                "-e", f"REGISTRY_HTTP_ADDR=0.0.0.0:{self.port}",
                "docker.io/library/registry:2",
            ],
            check=True,
        )
        time.sleep(2)
        subprocess.run(
            [
                "skopeo", "copy",
                "--dest-tls-verify=false",
                "containers-storage:localhost/cbootc-test-v2:latest",
                f"docker://{self.image_ref}",
            ],
            check=True,
        )
        return self

    def __exit__(self, *_):
        subprocess.run(
            ["podman", "stop", self.container_name],
            check=False,
            capture_output=True,
        )




def boot(disk_image, ovmf_code, ovmf_vars=None, log=None):
    if ovmf_vars:
        # Secure Boot OVMF requires q35 + SMM for the firmware's variable-locking
        # code; without smm=on the firmware stalls silently before any serial output.
        machine = "-machine q35,smm=on -global driver=cfi.pflash01,property=secure,value=on"
        pflash = (
            f"-drive if=pflash,format=raw,readonly=on,file={ovmf_code} "
            f"-drive if=pflash,format=raw,file={ovmf_vars}"
        )
    else:
        machine = ""
        pflash = f"-drive if=pflash,format=raw,readonly=on,file={ovmf_code}"
    cmd = (
        f"qemu-system-x86_64 -enable-kvm -m 2048 "
        f"{machine} "
        f"-drive file={disk_image},if=virtio,snapshot=on "
        f"{pflash} "
        f"-nographic -no-reboot"
    )
    child = pexpect.spawn(cmd, timeout=TIMEOUT_BOOT, encoding="utf-8")
    child.logfile_read = log
    return child


def boot_with_network(disk_image, ovmf_code, ovmf_vars=None, log=None):
    """Boot with QEMU user-mode networking (SLIRP) and a snapshot overlay.

    SLIRP requires no host-side TAP or firewall setup: the guest reaches the
    host at 10.0.2.2 and QEMU forwards those connections to host loopback.
    snapshot=on,format=raw keeps the overlay in QEMU without copying the full
    image; writes persist across guest reboots within the same QEMU process.
    Omits -no-reboot so the VM can reboot within a single QEMU session.
    """
    if ovmf_vars:
        machine = "-machine q35,smm=on -global driver=cfi.pflash01,property=secure,value=on"
        pflash = (
            f"-drive if=pflash,format=raw,readonly=on,file={ovmf_code} "
            f"-drive if=pflash,format=raw,file={ovmf_vars}"
        )
    else:
        machine = ""
        pflash = f"-drive if=pflash,format=raw,readonly=on,file={ovmf_code}"
    cmd = (
        f"qemu-system-x86_64 -enable-kvm -m 2048 "
        f"{machine} "
        f"-drive file={disk_image},if=virtio,snapshot=on,format=raw "
        f"{pflash} "
        f"-netdev user,id=net0 "
        f"-device virtio-net-pci,netdev=net0 "
        f"-nographic"
    )
    child = pexpect.spawn(cmd, timeout=TIMEOUT_BOOT, encoding="utf-8")
    child.logfile_read = log
    return child


def wait_for_shell(child):
    child.expect(PROMPT, timeout=TIMEOUT_BOOT)


def reboot_and_wait_for_shell(child):
    """Send 'systemctl reboot' and wait for the next boot's shell prompt.

    'systemctl reboot' returns to the shell briefly before the reboot
    actually starts, causing the shell to emit a spurious prompt.
    We skip it by waiting for an early BIOS/kernel marker — which only
    appears during a fresh boot — before looking for the shell prompt.
    """
    child.sendline("systemctl reboot")
    # "BdsDxe:" is the OVMF/UEFI firmware pre-boot message.
    # "Booting paravirtualized kernel" appears early in the Linux kernel log.
    # Either marker confirms we are past the stale Boot-N shell prompt.
    child.expect(r"BdsDxe:|Booting paravirtualized kernel", timeout=TIMEOUT_BOOT)
    wait_for_shell(child)


def run_cmd(child, cmd, timeout=TIMEOUT_CMD):
    """Run a shell command; return (exit_code, output_before_exit_marker)."""
    marker = "__E2E_EXIT__"
    child.sendline(f"{cmd}; echo {marker}$?")
    child.expect(rf"{marker}(\d+)", timeout=timeout)
    exit_code = int(child.match.group(1))
    output = strip_ansi(child.before)
    child.expect(PROMPT, timeout=timeout)
    return exit_code, output


# ---------------------------------------------------------------------------
# Individual tests
# ---------------------------------------------------------------------------

def test_secure_boot_enabled(child):
    """SecureBoot EFI variable must report enabled (last attribute byte = 1)."""
    rc, out = run_cmd(
        child,
        "od -An -t u1 "
        "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c "
        "2>/dev/null | tr -s ' ' '\\n' | tail -1"
    )
    assert rc == 0, "SecureBoot EFI variable not readable"
    m = re.search(r"(\d+)\s*$", out)
    assert m and m.group(1) == "1", f"Secure Boot not active: {out.strip()!r}"


def test_status(child):
    """cbootc status: Digest populated, Image configured, no error trace."""
    rc, out = run_cmd(child, "cbootc status")
    assert rc == 0, f"cbootc status exited {rc}"
    assert re.search(r"Digest:\s+\S", out), f"Digest field missing or empty:\n{out}"
    assert "(not booted from composefs)" not in out, "not running from composefs"
    assert "Image:" in out, f"Image field missing:\n{out}"


def test_bls_title(child):
    """BLS entry title must not be the cfsctl placeholder 'todoOS'."""
    rc, out = run_cmd(child, "grep '^title ' /boot/loader/entries/*.conf")
    assert rc == 0, "no BLS entries found"
    assert "todoOS" not in out, f"BLS title still 'todoOS':\n{out}"


def test_rollback_no_previous(child):
    """cbootc rollback with a single BLS entry must fail with a clear message."""
    rc, out = run_cmd(child, "cbootc rollback")
    assert rc != 0, "expected non-zero exit when no previous deployment exists"
    assert "no previous" in out.lower(), f"unexpected error output:\n{out}"


def test_grubenv_exists(child):
    """/boot/grub2/grubenv or /boot/grub/grubenv must exist (written at install time)."""
    rc, _ = run_cmd(
        child,
        "test -f /boot/grub2/grubenv || test -f /boot/grub/grubenv",
    )
    assert rc == 0, "grubenv not found at /boot/grub2/grubenv or /boot/grub/grubenv"


def test_uki_efi_linux(child):
    """/boot/efi/EFI/Linux must contain exactly one .efi UKI after install."""
    rc, _ = run_cmd(
        child,
        "test $(ls /boot/efi/EFI/Linux/*.efi 2>/dev/null | wc -l) -eq 1",
    )
    assert rc == 0, "expected exactly 1 UKI .efi in /boot/efi/EFI/Linux"


def test_no_grubenv(child):
    """grubenv must NOT exist on a systemd-boot system."""
    rc, _ = run_cmd(child, "test -f /boot/grub2/grubenv || test -f /boot/grub/grubenv")
    assert rc != 0, "grubenv should not exist in UKI mode"


def test_var_config(child):
    """/var/lib/cbootc/config.toml must exist and contain the image ref."""
    rc, out = run_cmd(child, "cat /var/lib/cbootc/config.toml")
    assert rc == 0, "config.toml not found"
    assert "ref" in out, f"image ref missing from config.toml:\n{out}"


# ---------------------------------------------------------------------------
# Upgrade / switch / rollback helpers
# ---------------------------------------------------------------------------

def get_current_digest(child):
    """Extract the composefs digest from /proc/cmdline (strips leading '?')."""
    rc, out = run_cmd(child, "cat /proc/cmdline")
    assert rc == 0, "could not read /proc/cmdline"
    m = re.search(r"composefs=\??(\S+)", out)
    assert m, f"No composefs= token in /proc/cmdline:\n{out}"
    return m.group(1)


def configure_insecure_registry(child, port=REGISTRY_PORT):
    """Write a registries.conf.d drop-in allowing plain HTTP to the test registry."""
    rc, out = run_cmd(
        child,
        "mkdir -p /etc/containers/registries.conf.d && "
        "{ "
        'echo "[[registry]]"; '
        f'echo \'location = "{REGISTRY_HOST}:{port}"\'; '
        'echo "insecure = true"; '
        "} > /etc/containers/registries.conf.d/local-test.conf",
    )
    assert rc == 0, f"failed to configure insecure registry:\n{out}"


SLIRP_GUEST_IP = "10.0.2.100"   # static IP we assign inside the VM
SLIRP_CIDR = "24"               # 10.0.2.0/24 — puts host (10.0.2.2) on same link


def configure_guest_network(child):
    """Assign a static IP in the QEMU SLIRP subnet (10.0.2.0/24).

    Uses a static address rather than waiting for DHCP so the test works
    regardless of how the image's network manager is configured.  Once
    10.0.2.100/24 is assigned, 10.0.2.2 (the SLIRP host) is reachable via
    the automatic link-local subnet route — no explicit default route needed.
    Re-reads the interface name on each iteration to survive udev renames.
    """
    rc, out = run_cmd(
        child,
        "ok=0; "
        "for i in $(seq 1 30); do "
        "  iface=$(ip -br link show | awk '!/^lo/{print $1}' | head -1); "
        "  [ -z \"$iface\" ] && { sleep 1; continue; }; "
        "  nmcli dev set \"$iface\" managed no 2>/dev/null; "
        "  ip addr flush dev \"$iface\" 2>/dev/null; "
        f"  ip addr add {SLIRP_GUEST_IP}/{SLIRP_CIDR} dev \"$iface\" 2>/dev/null "
        f"  && ip link set \"$iface\" up && ok=1 && break; "
        "  sleep 1; "
        "done; "
        "[ $ok -eq 1 ]",
        timeout=35,
    )
    assert rc == 0, f"failed to configure guest network:\n{out}"


def wait_for_network(child):
    """Wait until the registry TCP port is reachable from the guest."""
    rc, out = run_cmd(
        child,
        # Test actual TCP connectivity to the registry rather than ICMP ping:
        # the host firewall may pass ICMP but still block TCP.
        f"ok=0; "
        f"for i in $(seq 1 30); do "
        f"  timeout 2 bash -c 'echo >/dev/tcp/{REGISTRY_HOST}/{REGISTRY_PORT}' "
        f"  2>/dev/null && ok=1 && break; "
        f"  sleep 1; "
        f"done; "
        f"[ $ok -eq 1 ]",
        timeout=35,
    )
    assert rc == 0, (
        f"Registry at {REGISTRY_HOST}:{REGISTRY_PORT} not reachable from guest:\n{out}"
    )


def test_switch(child, image_ref):
    """cbootc switch must exit 0 and pull the new image."""
    rc, out = run_cmd(child, f"cbootc switch {image_ref}", timeout=TIMEOUT_UPGRADE)
    assert rc == 0, f"cbootc switch failed:\n{out}"


def test_new_grub_entry_created(child):
    """Two or more BLS entries must exist after upgrade."""
    rc, out = run_cmd(
        child, "echo BLSCOUNT:$(ls /boot/loader/entries/*.conf 2>/dev/null | wc -l)"
    )
    assert rc == 0, "could not count BLS entries"
    m = re.search(r"BLSCOUNT:(\d+)", out)
    assert m, f"could not parse BLS count from output:\n{out!r}"
    count = int(m.group(1))
    assert count >= 2, f"expected ≥2 BLS entries after upgrade, got {count}"


def test_new_uki_entry_created(child):
    """Two or more UKI .efi files must exist after upgrade."""
    rc, out = run_cmd(
        child, "echo UKICOUNT:$(ls /boot/efi/EFI/Linux/*.efi 2>/dev/null | wc -l)"
    )
    assert rc == 0, "could not count UKI entries"
    m = re.search(r"UKICOUNT:(\d+)", out)
    assert m, f"could not parse UKI count from output:\n{out!r}"
    count = int(m.group(1))
    assert count >= 2, f"expected ≥2 UKI entries after upgrade, got {count}"


def test_upgraded_digest_active(child, previous_digest):
    """After reboot, /proc/cmdline must show a different digest."""
    current = get_current_digest(child)
    assert current != previous_digest, (
        f"Digest unchanged after upgrade reboot: {current!r} "
        "(system may have booted the old entry)"
    )


def test_rollback_succeeds(child):
    """cbootc rollback must exit 0 when a previous deployment exists."""
    rc, out = run_cmd(child, "cbootc rollback")
    assert rc == 0, f"cbootc rollback failed unexpectedly:\n{out}"


def test_grubenv_next_entry_set(child):
    """After rollback, grubenv must have a non-empty next_entry."""
    rc, out = run_cmd(
        child,
        "grub2-editenv /boot/grub2/grubenv list 2>/dev/null || "
        "grub-editenv /boot/grub/grubenv list 2>/dev/null",
    )
    assert rc == 0, "grubenv not readable after rollback"
    assert re.search(r"next_entry=\S+", out), (
        f"next_entry not set in grubenv:\n{out!r}"
    )


def test_loader_conf_default_set(child):
    """After rollback, /boot/efi/loader/loader.conf must have a 'default' entry."""
    rc, out = run_cmd(child, "cat /boot/efi/loader/loader.conf 2>/dev/null || true")
    assert rc == 0, "could not read loader.conf"
    # Use \b instead of ^ — Ubuntu's bash emits OSC shell-integration sequences
    # (e.g. \x1b]3008;...\x1b\\) immediately before command output, which are
    # not newlines, so re.MULTILINE ^ never matches at that position.
    assert re.search(r"\bdefault\s+\S", out), (
        f"no 'default' line in /boot/efi/loader/loader.conf:\n{out!r}"
    )


def test_rolled_back_digest_active(child, expected_digest):
    """After rollback reboot, /proc/cmdline must show the original digest."""
    current = get_current_digest(child)
    assert current == expected_digest, (
        f"Expected original digest {expected_digest!r} after rollback, got {current!r}"
    )


def run_upgrade_sequence(disk_image, ovmf_code, registry, uki=False,
                         secure_boot=False, ovmf_vars=None, log=None):
    """Three-boot upgrade → rollback sequence on a sparse copy of disk_image.

    Boot 1: configure insecure registry, cbootc switch to v2, verify new entry,
            optionally verify Secure Boot still active, then reboot.
    Boot 2: verify new digest is active, cbootc rollback, verify next-boot
            selection set, optionally verify Secure Boot still active, then reboot.
    Boot 3: verify original digest is active, optionally verify Secure Boot.

    Returns (passed, failed) counts.
    """
    passed = failed = 0

    def step(name, fn, *args):
        nonlocal passed, failed
        try:
            fn(*args)
            print(f"  PASS  {name}")
            passed += 1
        except AssertionError as e:
            print(f"  FAIL  {name}: {e}")
            _print_console_tail(log)
            failed += 1

    # boot_with_network uses snapshot=on so the original disk is never
    # modified; the QEMU-managed qcow2 overlay persists across guest
    # reboots within the same QEMU process.
    child = boot_with_network(disk_image, ovmf_code, ovmf_vars, log=log)
    try:
        wait_for_shell(child)
        print("==> Boot 1 OK\n")

        original_digest = get_current_digest(child)

        step("configure_insecure_registry", configure_insecure_registry, child)
        step("configure_guest_network", configure_guest_network, child)
        step("wait_for_network", wait_for_network, child)

        image_ref = (
            f"docker://{REGISTRY_HOST}:{registry.port}/test-image:latest"
        )
        step("switch_to_v2", test_switch, child, image_ref)

        if uki:
            step("new_uki_entry_created", test_new_uki_entry_created, child)
        else:
            step("new_grub_entry_created", test_new_grub_entry_created, child)

        if secure_boot:
            step("secure_boot_enabled_boot1", test_secure_boot_enabled, child)

        reboot_and_wait_for_shell(child)
        print("\n==> Boot 2 OK\n")

        step("upgraded_digest_active",
             test_upgraded_digest_active, child, original_digest)

        if secure_boot:
            step("secure_boot_enabled_boot2", test_secure_boot_enabled, child)

        step("rollback_succeeds", test_rollback_succeeds, child)

        if uki:
            step("loader_conf_default_set", test_loader_conf_default_set, child)
        else:
            step("grubenv_next_entry_set", test_grubenv_next_entry_set, child)

        reboot_and_wait_for_shell(child)
        print("\n==> Boot 3 OK\n")

        step("rolled_back_digest_active",
             test_rolled_back_digest_active, child, original_digest)

        if secure_boot:
            step("secure_boot_enabled_boot3", test_secure_boot_enabled, child)

    except pexpect.TIMEOUT:
        print("\nFAIL: timed out waiting for output")
        _print_console_tail(log, n=50)
        failed += 1
    finally:
        child.sendline("poweroff -f")
        child.expect(pexpect.EOF, timeout=30)

    return passed, failed


# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

GRUB_TESTS = [
    test_status,
    test_bls_title,
    test_rollback_no_previous,
    test_grubenv_exists,
    test_var_config,
]

UKI_TESTS = [
    test_status,
    test_uki_efi_linux,
    test_rollback_no_previous,
    test_no_grubenv,
    test_var_config,
]


def main():
    parser = argparse.ArgumentParser(description="composefs-os e2e tests")
    parser.add_argument("disk_image", help="Path to disk.raw")
    parser.add_argument("--ovmf", help="Path to OVMF CODE flash file")
    parser.add_argument("--ovmf-vars", help="Path to OVMF VARS flash file (Secure Boot mode)")
    parser.add_argument(
        "--secure-boot",
        action="store_true",
        help="Boot with Secure Boot enforcement (requires OVMF secboot firmware)",
    )
    parser.add_argument(
        "--uki",
        action="store_true",
        help="Test a UKI/systemd-boot disk image instead of a GRUB image",
    )
    parser.add_argument(
        "--uki-secureboot",
        action="store_true",
        help="Test a UKI + Secure Boot disk image (requires secboot OVMF with cert enrolled in db)",
    )
    parser.add_argument(
        "--upgrade",
        action="store_true",
        help=(
            "Run a 3-boot upgrade/switch/rollback sequence instead of static "
            "post-install tests. Requires --source-image and a running podman daemon."
        ),
    )
    parser.add_argument(
        "--source-image",
        metavar="IMAGE",
        help="Local podman image to build the v2 upgrade image from (required with --upgrade)",
    )
    args = parser.parse_args()

    log_path = os.path.join(os.getcwd(), "e2e-console.log")
    log = ConsoleLog(log_path)
    print(f"Console log: {log_path}")
    print(f"  (run 'tail -f {log_path}' in another terminal to follow boot output)")

    ovmf_vars_tmp = None
    try:
        if args.secure_boot:
            sb_code, sb_vars = find_ovmf_secboot()
            ovmf_code = args.ovmf or sb_code
            ovmf_vars_src = args.ovmf_vars or sb_vars
            if not ovmf_code or not ovmf_vars_src:
                print(
                    "ERROR: Secure Boot OVMF firmware not found. "
                    "Install edk2-ovmf (Fedora) or ovmf (Ubuntu), "
                    "or pass --ovmf / --ovmf-vars."
                )
                sys.exit(1)
            fd, ovmf_vars_tmp = tempfile.mkstemp(suffix=".fd")
            os.close(fd)
            shutil.copy2(ovmf_vars_src, ovmf_vars_tmp)
            tests = [test_secure_boot_enabled] + GRUB_TESTS
        elif args.uki_secureboot:
            sb_code, sb_vars = find_ovmf_secboot()
            ovmf_code = args.ovmf or sb_code
            ovmf_vars_src = args.ovmf_vars or sb_vars
            if not ovmf_code or not ovmf_vars_src:
                print(
                    "ERROR: Secure Boot OVMF firmware not found. "
                    "Install edk2-ovmf (Fedora) or ovmf (Ubuntu), "
                    "or pass --ovmf / --ovmf-vars."
                )
                sys.exit(1)
            fd, ovmf_vars_tmp = tempfile.mkstemp(suffix=".fd")
            os.close(fd)
            shutil.copy2(ovmf_vars_src, ovmf_vars_tmp)
            # test_secure_boot_enabled requires the signing cert pre-enrolled in
            # OVMF_VARS db — use prep_sb_vars.py or pass a prepared --ovmf-vars.
            tests = [test_secure_boot_enabled] + UKI_TESTS
        elif args.uki:
            ovmf_code = args.ovmf or find_ovmf()
            if not ovmf_code:
                print("ERROR: OVMF firmware not found. Install edk2-ovmf or pass --ovmf.")
                sys.exit(1)
            if args.ovmf_vars:
                fd, ovmf_vars_tmp = tempfile.mkstemp(suffix=".fd")
                os.close(fd)
                shutil.copy2(args.ovmf_vars, ovmf_vars_tmp)
            tests = UKI_TESTS
        else:
            ovmf_code = args.ovmf or find_ovmf()
            if not ovmf_code:
                print("ERROR: OVMF firmware not found. Install edk2-ovmf or pass --ovmf.")
                sys.exit(1)
            tests = GRUB_TESTS

        if args.upgrade:
            if not args.source_image:
                print("ERROR: --source-image is required with --upgrade")
                sys.exit(1)
            uki = args.uki or args.uki_secureboot
            secure_boot = args.secure_boot or args.uki_secureboot
            with LocalRegistry(args.source_image) as registry:
                passed, failed = run_upgrade_sequence(
                    args.disk_image, ovmf_code, registry,
                    uki=uki, secure_boot=secure_boot, ovmf_vars=ovmf_vars_tmp,
                    log=log,
                )
            print(f"\n{passed} passed, {failed} failed")
            sys.exit(0 if failed == 0 else 1)

        print(f"Booting {args.disk_image} ...")
        child = boot(args.disk_image, ovmf_code, ovmf_vars_tmp, log=log)

        try:
            wait_for_shell(child)
            print("==> Boot OK\n")

            passed = failed = 0
            for test_fn in tests:
                name = test_fn.__name__
                try:
                    test_fn(child)
                    print(f"  PASS  {name}")
                    passed += 1
                except AssertionError as e:
                    print(f"  FAIL  {name}: {e}")
                    _print_console_tail(log)
                    failed += 1

            print(f"\n{passed} passed, {failed} failed")
            sys.exit(0 if failed == 0 else 1)

        except pexpect.TIMEOUT:
            print("\nFAIL: timed out waiting for output")
            _print_console_tail(log, n=50)
            sys.exit(1)
        finally:
            child.sendline("poweroff -f")
            child.expect(pexpect.EOF, timeout=30)
    finally:
        log.close()
        if ovmf_vars_tmp and os.path.exists(ovmf_vars_tmp):
            os.unlink(ovmf_vars_tmp)


if __name__ == "__main__":
    main()
