use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Subcommand)]
pub enum InstallCommand {
    /// Partition a block device (or file) and install the current container image
    ToDisk(ToDiskOpts),
}

#[derive(Args)]
pub struct ToDiskOpts {
    /// Target block device (e.g. /dev/vda) or file path for loopback install
    device: PathBuf,
    /// Disk size for file-based loopback install (e.g. 10G); required when DEVICE is not a block device
    #[arg(long)]
    size: Option<String>,
    /// Root filesystem type
    #[arg(long, value_name = "TYPE", default_value = "ext4", value_parser = ["ext4", "xfs"])]
    filesystem: String,
    /// Run wipefs on the target before partitioning
    #[arg(long)]
    wipe: bool,
    /// Install pre-signed shim+grub EFI chain instead of grub2-install (required for Secure Boot)
    #[arg(long)]
    secure_boot: bool,
    /// Use systemd-boot + UKI (Unified Kernel Image): FAT32 XBOOTLDR, no grubenv
    #[arg(long)]
    uki: bool,
    /// PEM private key for signing EFI binaries (--uki --secure-boot); generated if omitted
    #[arg(long, value_name = "PATH", requires = "sb_cert")]
    sb_key: Option<PathBuf>,
    /// PEM certificate for signing EFI binaries (--uki --secure-boot)
    #[arg(long, value_name = "PATH", requires = "sb_key")]
    sb_cert: Option<PathBuf>,
}

pub fn run(command: InstallCommand) -> Result<()> {
    match command {
        InstallCommand::ToDisk(opts) => run_to_disk(opts),
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn run_to_disk(opts: ToDiskOpts) -> Result<()> {
    check_root()?;
    let image_ref = detect_image_ref()?;
    println!("==> Source image: {image_ref}");

    let (dev, loop_dev) = prepare_device(&opts)?;

    let mnt = tempfile::TempDir::new().context("failed to create mount root")?;
    let mnt_path = mnt.path().to_path_buf();
    let mut mounts: Vec<PathBuf> = Vec::new();

    let sb_cert_out = if loop_dev.is_some() && opts.uki && opts.secure_boot {
        // Derive <disk>.sb.cer next to the disk image so the host can enroll
        // the cert into OVMF_VARS without mounting the image.
        let mut p = opts.device.clone();
        let stem = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        p.set_file_name(format!("{stem}.sb.cer"));
        Some(p)
    } else {
        None
    };
    let result = install_inner(
        &dev,
        &image_ref,
        &InstallOpts {
            filesystem: &opts.filesystem,
            secure_boot: opts.secure_boot,
            uki: opts.uki,
            sb_key: opts.sb_key.as_deref(),
            sb_cert: opts.sb_cert.as_deref(),
            sb_cert_out,
        },
        &mnt_path,
        &mut mounts,
    );

    // Always clean up, regardless of result
    for m in mounts.iter().rev() {
        let _ = Command::new("umount").arg(m).status();
    }
    if let Some(ref ld) = loop_dev {
        let _ = Command::new("losetup").arg("-d").arg(ld).status();
    }

    result?;

    println!("==> Done");
    if loop_dev.is_some() {
        let output = opts.device.display();
        println!();
        println!("Boot it with:");
        println!("  qemu-system-x86_64 -enable-kvm -m 4096 \\");
        println!("      -drive file={output},if=virtio \\");
        if opts.secure_boot {
            println!("      -machine q35,smm=on \\");
            println!("      -global driver=cfi.pflash01,property=secure,value=on \\");
            println!(
                "      -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd \\"
            );
            println!("      -drive if=pflash,format=raw,file=/tmp/OVMF_VARS.secboot.fd \\");
            println!("      -nographic");
            println!();
            println!(
                "(copy /usr/share/edk2/ovmf/OVMF_VARS.secboot.fd to /tmp/OVMF_VARS.secboot.fd — QEMU needs a writable VARS file)"
            );
        } else {
            println!(
                "      -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.fd \\"
            );
            println!("      -nographic");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Device setup
// ---------------------------------------------------------------------------

fn prepare_device(opts: &ToDiskOpts) -> Result<(PathBuf, Option<String>)> {
    if opts.device.exists() && opts.device.metadata()?.file_type().is_block_device() {
        if opts.wipe {
            println!("==> Wiping {}", opts.device.display());
            run_cmd("wipefs", &["-a", opts.device.to_str().unwrap()])?;
        }
        return Ok((opts.device.clone(), None));
    }

    let size = opts
        .size
        .as_deref()
        .context("--size is required for file-based install (e.g. --size 10G)")?;
    println!("==> Creating {} image at {}", size, opts.device.display());
    run_cmd(
        "truncate",
        &[&format!("--size={size}"), opts.device.to_str().unwrap()],
    )?;
    println!("==> Attaching loop device");
    let ld = run_cmd_output(
        "losetup",
        &[
            "--find",
            "--show",
            "--partscan",
            opts.device.to_str().unwrap(),
        ],
    )?;
    println!("    {ld}");
    Ok((PathBuf::from(&ld), Some(ld)))
}

// ---------------------------------------------------------------------------
// Core installation
// ---------------------------------------------------------------------------

struct InstallOpts<'a> {
    filesystem: &'a str,
    secure_boot: bool,
    uki: bool,
    sb_key: Option<&'a Path>,
    sb_cert: Option<&'a Path>,
    /// For loopback installs: write a DER copy of the SB cert here so the host
    /// can enroll it into OVMF_VARS without having to mount the disk image.
    sb_cert_out: Option<PathBuf>,
}

fn install_inner(
    dev: &Path,
    image_ref: &str,
    opts: &InstallOpts<'_>,
    mnt_path: &Path,
    mounts: &mut Vec<PathBuf>,
) -> Result<()> {
    let InstallOpts {
        filesystem,
        secure_boot,
        uki,
        sb_key,
        sb_cert,
        ref sb_cert_out,
    } = *opts;
    let dev_s = dev.to_str().unwrap();
    let efi_p = part(dev, 1);
    let boot_p = part(dev, 2);
    let root_p = part(dev, 3);

    println!("==> Partitioning {dev_s}");
    sfdisk_gpt(dev)?;
    // Update kernel's partition table view; udevadm settle handles bare-metal
    // but inside a container the host's udev never writes to the container's
    // private /dev.  ensure_partition_nodes() fills the gap via sysfs + mknod.
    let _ = Command::new("partx").args(["-u", dev_s]).status();
    let _ = Command::new("udevadm")
        .args(["settle", "--timeout=5"])
        .status();
    ensure_partition_nodes(dev, 3)?;

    println!("==> Formatting filesystems");
    run_cmd("mkfs.fat", &["-F32", "-n", "EFI", efi_p.to_str().unwrap()])?;
    // UKI: XBOOTLDR must be FAT so systemd-boot (a UEFI app) can read EFI/Linux/
    if uki {
        run_cmd(
            "mkfs.fat",
            &["-F32", "-n", "BOOT", boot_p.to_str().unwrap()],
        )?;
    } else {
        run_cmd("mkfs.ext4", &["-F", "-L", "boot", boot_p.to_str().unwrap()])?;
    }
    match filesystem {
        "xfs" => run_cmd("mkfs.xfs", &["-f", "-L", "root", root_p.to_str().unwrap()])?,
        // Linux 7.0 regression: overlayfs checks trusted.overlay.verity xattrs
        // unconditionally at switch_root, failing when objects have fs-verity.
        // Omit -O verity so FS_IOC_ENABLE_VERITY fails silently in insecure
        // mode, leaving objects without verity and erofs without the xattrs.
        _ => run_cmd("mkfs.ext4", &["-F", "-L", "root", root_p.to_str().unwrap()])?,
    }

    let root_uuid = blkid_uuid(root_p.to_str().unwrap())?;
    let boot_uuid = blkid_uuid(boot_p.to_str().unwrap())?;
    let efi_uuid = blkid_uuid(efi_p.to_str().unwrap())?;

    println!("==> Mounting");
    run_cmd(
        "mount",
        &[root_p.to_str().unwrap(), mnt_path.to_str().unwrap()],
    )?;
    mounts.push(mnt_path.to_path_buf());

    let boot_mnt = mnt_path.join("boot");
    fs::create_dir_all(&boot_mnt)?;
    run_cmd(
        "mount",
        &[boot_p.to_str().unwrap(), boot_mnt.to_str().unwrap()],
    )?;
    mounts.push(boot_mnt.clone());

    let efi_mnt = mnt_path.join("boot/efi");
    fs::create_dir_all(&efi_mnt)?;
    run_cmd(
        "mount",
        &[efi_p.to_str().unwrap(), efi_mnt.to_str().unwrap()],
    )?;
    mounts.push(efi_mnt.clone());

    // cfsctl (via containers-image) needs /var/tmp for blob staging.
    // The composefs image layout leaves /var empty; create the dir here.
    fs::create_dir_all("/var/tmp").context("create /var/tmp")?;

    println!("==> Initializing composefs repo");
    let cfs_repo = mnt_path.join("composefs");
    fs::create_dir_all(&cfs_repo)?;
    // Linux 7.0 kernel regression (bootc#2174): overlayfs fsverity enforcement
    // fails at switch_root with "has no fs-verity digest". Pass --insecure as a
    // global flag so prepare-boot writes composefs=?<hash> (with '?' prefix) in
    // the BLS entry, causing composefs-setup-root to skip verity=require.
    run_cmd(
        "cfsctl",
        &["--insecure", "--repo", cfs_repo.to_str().unwrap(), "init"],
    )?;

    println!("==> Pulling image: {image_ref}");
    run_cmd(
        "cfsctl",
        &[
            "--insecure",
            "--repo",
            cfs_repo.to_str().unwrap(),
            "oci",
            "pull",
            image_ref,
        ],
    )?;

    println!("==> Preparing boot entries");
    let digest = run_cmd_output(
        "cfsctl",
        &[
            "--insecure",
            "--repo",
            cfs_repo.to_str().unwrap(),
            "oci",
            "compute-id",
            "--bootable",
            image_ref,
        ],
    )?;
    // prepare-boot prepends composefs=?<hash> to whatever --cmdline we supply.
    // UKI uses root=LABEL=root (stable label); GRUB uses root=UUID=... (stable uuid).
    let cmdline = if uki {
        format!("root=LABEL=root rootfstype={filesystem} rw console=ttyS0,115200")
    } else {
        format!("root=UUID={root_uuid} rootfstype={filesystem} rw console=ttyS0,115200")
    };
    run_cmd(
        "cfsctl",
        &[
            "--insecure",
            "--repo",
            cfs_repo.to_str().unwrap(),
            "oci",
            "prepare-boot",
            "--bootdir",
            boot_mnt.to_str().unwrap(),
            "--entry-id",
            &digest,
            "--cmdline",
            cmdline.as_str(),
            image_ref,
        ],
    )?;
    if uki {
        println!("==> Building UKI");
        crate::upgrade::build_uki(&boot_mnt, &efi_mnt, &digest)?;
    } else {
        crate::upgrade::patch_bls_entry(&boot_mnt, &digest, image_ref)?;
    }

    // Resolve SB keys early so the TempDir (if we generated them) outlives all
    // subsequent uses: UKI signing, systemd-boot signing, and key persistence.
    let _sb_tmpdir: Option<tempfile::TempDir>;
    let sb_signing: Option<(PathBuf, PathBuf)> = if uki && secure_boot {
        let (key, cert, tmp) = resolve_sb_keys(sb_key, sb_cert)?;
        _sb_tmpdir = tmp;
        Some((key, cert))
    } else {
        _sb_tmpdir = None;
        None
    };

    if let Some((ref key, ref cert)) = sb_signing {
        let uki_path = efi_mnt.join("EFI/Linux").join(format!("{digest}.efi"));
        println!("==> Signing UKI");
        sign_efi(&uki_path, key, cert)?;
    }

    // cfsctl oci prepare-boot creates state/deploy/<id>/etc/upper/ as the
    // overlayfs upperdir for /etc. Files placed there are visible in the
    // running system's /etc. The ext4 root's /etc is the lowerdir for the
    // composefs image's /etc, not for the overlay, so writing there has no
    // effect on what the booted system sees.
    let deploy_dir = find_deploy_dir(mnt_path)?;
    let etc_upper = deploy_dir.join("etc/upper");

    println!("==> Writing fstab");
    fs::write(
        etc_upper.join("fstab"),
        if uki {
            format!(
                "UUID={root_uuid}  /          {filesystem}  ro        0 1\n\
                 UUID={boot_uuid}  /boot      vfat  umask=0077,shortname=winnt  0 2\n\
                 UUID={efi_uuid}   /boot/efi  vfat  umask=0077,shortname=winnt  0 2\n"
            )
        } else {
            format!(
                "UUID={root_uuid}  /          {filesystem}  ro        0 1\n\
                 UUID={boot_uuid}  /boot      ext4          defaults  0 2\n\
                 UUID={efi_uuid}   /boot/efi  vfat          umask=0077,shortname=winnt  0 2\n"
            )
        },
    )?;

    // Replace the per-deployment var with a symlink to a shared state/var so
    // /var content (databases, logs, cbootc state) survives upgrades.
    fs::remove_dir(deploy_dir.join("var")).context("removing placeholder var dir")?;
    symlink("../../var", deploy_dir.join("var")).context("creating shared var symlink")?;

    if uki && secure_boot {
        let (key, cert) = sb_signing.as_ref().unwrap();
        println!("==> Installing Secure Boot EFI chain (shim → signed systemd-boot)");
        install_systemd_boot_secureboot(&efi_mnt, key, cert)?;
    } else if uki {
        println!("==> Installing systemd-boot");
        install_systemd_boot(&efi_mnt)?;
    } else if secure_boot {
        println!("==> Installing Secure Boot EFI chain (shim → grub)");
        install_shim_efi(&efi_mnt, &boot_uuid, grub_dir())?;
    } else if has_grub2() {
        // Fedora: grub2-install reliably embeds the boot partition UUID.
        println!("==> Installing GRUB (grub2-install)");
        let efi_dir_arg = format!("--efi-directory={}", efi_mnt.display());
        let boot_dir_arg = format!("--boot-directory={}", boot_mnt.display());
        run_cmd(
            "grub2-install",
            &[
                "--target=x86_64-efi",
                efi_dir_arg.as_str(),
                boot_dir_arg.as_str(),
                "--bootloader-id=cbootc",
                "--removable",
                "--no-nvram",
                "--force",
            ],
        )?;
    } else {
        // Ubuntu/Debian: grub-install can't reliably detect the boot partition
        // UUID inside a container.  Use grub-mkstandalone to build a
        // self-contained EFI binary with a label-based search embedded, then
        // copy the grub module tree to the boot partition manually.
        println!("==> Installing GRUB (grub-mkstandalone)");
        install_grub_standalone(&efi_mnt, &boot_mnt, grub_dir())?;
    }

    if !uki {
        println!("==> Writing grub.cfg");
        let grub_subdir = grub_dir();
        let grub_boot_dir = boot_mnt.join(grub_subdir);
        fs::create_dir_all(&grub_boot_dir)?;
        if has_grub2() {
            // Fedora: grub2 has blscfg.mod — BLS entries are scanned natively.
            fs::write(
                grub_boot_dir.join("grub.cfg"),
                "serial --unit=0 --speed=115200\n\
                 terminal_input serial console\n\
                 terminal_output serial console\n\
                 load_env\n\
                 if [ \"${next_entry}\" ] ; then\n\
                   set default=\"${next_entry}\"\n\
                   set next_entry=\n\
                   save_env next_entry\n\
                 fi\n\
                 set timeout=3\n\
                 insmod ext2\n\
                 function load_video { true; }\n\
                 insmod blscfg\n\
                 blscfg\n",
            )?;
        } else {
            // Ubuntu/Debian: no blscfg.mod — generate traditional menuentry blocks.
            crate::upgrade::write_grub_menuentry_cfg(&boot_mnt, grub_subdir)?;
        }

        println!("==> Creating grubenv");
        grub_editenv_create(grub_boot_dir.join("grubenv").to_str().unwrap())?;
    }

    println!("==> Populating /var from image");
    let shared_var = mnt_path.join("state/var");
    fs::create_dir_all(&shared_var)?;
    run_cmd("cp", &["-ax", "/var/.", shared_var.to_str().unwrap()])?;

    println!("==> Writing cbootc config");
    let cbootc_dir = shared_var.join("lib/cbootc");
    fs::create_dir_all(&cbootc_dir)?;
    fs::write(
        cbootc_dir.join("config.toml"),
        format!("[image]\nref = \"{image_ref}\"\n"),
    )?;

    if let Some((ref key, ref cert)) = sb_signing {
        println!("==> Persisting Secure Boot keys");
        fs::copy(key, cbootc_dir.join("sb.key")).context("copying sb.key")?;
        fs::copy(cert, cbootc_dir.join("sb.crt")).context("copying sb.crt")?;
        // Append [secureboot] section so `cbootc upgrade` can re-sign future UKIs.
        let config_path = cbootc_dir.join("config.toml");
        let mut config_txt = fs::read_to_string(&config_path)?;
        config_txt.push_str(
            "\n[secureboot]\nkey = \"/var/lib/cbootc/sb.key\"\ncert = \"/var/lib/cbootc/sb.crt\"\n",
        );
        fs::write(&config_path, config_txt)
            .with_context(|| format!("updating {}", config_path.display()))?;

        // For loopback installs, export a DER copy of the cert next to the disk
        // image so the host can enroll it into OVMF_VARS without mounting.
        if let Some(cert_out) = sb_cert_out {
            run_cmd(
                "openssl",
                &[
                    "x509",
                    "-in",
                    cert.to_str().unwrap(),
                    "-outform",
                    "DER",
                    "-out",
                    cert_out.to_str().unwrap(),
                ],
            )?;
            println!("    SB cert exported to: {}", cert_out.display());
        }
    }

    println!("==> Syncing");
    Command::new("sync")
        .status()
        .context("failed to run sync")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_deploy_dir(mnt_path: &Path) -> Result<PathBuf> {
    let state_deploy = mnt_path.join("state/deploy");
    let mut entries: Vec<_> = fs::read_dir(&state_deploy)
        .with_context(|| format!("reading {}", state_deploy.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    if entries.len() != 1 {
        bail!(
            "expected exactly one deployment in state/deploy, found {}",
            entries.len()
        );
    }
    Ok(entries.remove(0).path())
}

fn detect_image_ref() -> Result<String> {
    let content = fs::read_to_string("/run/.containerenv")
        .context("not running inside a container (no /run/.containerenv) — pass the image via cbootc switch first or run from within the container image")?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("image=") {
            let image = rest.trim_matches('"');
            if image.is_empty() {
                bail!("image= field in /run/.containerenv is empty");
            }
            return Ok(if image.contains("://") {
                image.to_string()
            } else {
                format!("containers-storage:{image}")
            });
        }
    }
    bail!("could not find image= field in /run/.containerenv")
}

fn check_root() -> Result<()> {
    let status = fs::read_to_string("/proc/self/status").context("read /proc/self/status")?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:\t") {
            let uid: u32 = rest
                .split_whitespace()
                .next()
                .unwrap_or("1")
                .parse()
                .unwrap_or(1);
            if uid != 0 {
                bail!("cbootc install to-disk must run as root");
            }
            return Ok(());
        }
    }
    bail!("could not determine UID from /proc/self/status")
}

fn part(dev: &Path, n: u8) -> PathBuf {
    let s = dev.to_str().unwrap();
    // Devices ending in a digit (e.g. nvme0n1, loop0) need a 'p' separator
    if s.chars()
        .last()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        PathBuf::from(format!("{s}p{n}"))
    } else {
        PathBuf::from(format!("{s}{n}"))
    }
}

// Inside a container the host udev never writes to the container's private /dev.
// sysfs IS global (all mounts see the same kernel state), so we can read the
// major:minor numbers from there and create the nodes ourselves with mknod.
fn ensure_partition_nodes(dev: &Path, count: u8) -> Result<()> {
    let dev_name = dev.file_name().unwrap().to_str().unwrap();
    for i in 1..=count {
        let part_dev = part(dev, i);
        if part_dev.exists() {
            continue;
        }
        let part_name = part_dev.file_name().unwrap().to_str().unwrap();
        let sys_dev = format!("/sys/block/{dev_name}/{part_name}/dev");
        let dev_nums = fs::read_to_string(&sys_dev).with_context(|| {
            format!(
                "sysfs entry for {part_name} not found — partition table re-read may have failed"
            )
        })?;
        let (major, minor) = dev_nums
            .trim()
            .split_once(':')
            .context("unexpected format in sysfs dev file")?;
        run_cmd("mknod", &[part_dev.to_str().unwrap(), "b", major, minor])?;
    }
    Ok(())
}

fn sfdisk_gpt(dev: &Path) -> Result<()> {
    let mut child = Command::new("sfdisk")
        .arg(dev)
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to spawn sfdisk")?;
    child.stdin.as_mut().unwrap().write_all(
        b"label: gpt\n\
          - : size=512MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=\"EFI\"\n\
          - : size=1GiB,   type=BC13C2FF-59E6-4262-A352-B275FD6F7172, name=\"boot\"\n\
          - :               type=4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709, name=\"root\"\n",
    )?;
    let status = child.wait()?;
    if !status.success() {
        bail!("sfdisk failed with {status}");
    }
    Ok(())
}

fn blkid_uuid(dev: &str) -> Result<String> {
    run_cmd_output("blkid", &["-s", "UUID", "-o", "value", dev])
}

fn install_systemd_boot(efi_mnt: &Path) -> Result<()> {
    // systemd-boot-unsigned installs the EFI binary at this fixed path.
    // Copy it directly instead of running bootctl install, which has version-
    // dependent flags and requires efivarfs (not available in containers).
    let src = Path::new("/usr/lib/systemd/boot/efi/systemd-bootx64.efi");
    if !src.exists() {
        anyhow::bail!(
            "systemd-boot EFI binary not found at {}. \
             The image must be built with --target uki (systemd-boot-unsigned package).",
            src.display()
        );
    }

    let systemd_dir = efi_mnt.join("EFI/systemd");
    fs::create_dir_all(&systemd_dir)?;
    fs::copy(src, systemd_dir.join("systemd-bootx64.efi"))
        .context("copying systemd-boot to EFI/systemd/")?;

    // UEFI fallback: without NVRAM entries (QEMU, VMs, removable media)
    // firmware looks for EFI/BOOT/BOOTx64.EFI on the ESP.
    let boot_dir = efi_mnt.join("EFI/BOOT");
    fs::create_dir_all(&boot_dir)?;
    fs::copy(src, boot_dir.join("BOOTx64.EFI"))
        .context("copying systemd-boot to EFI/BOOT/BOOTx64.EFI")?;

    Ok(())
}

fn install_grub_standalone(efi_mnt: &Path, boot_mnt: &Path, grub_subdir: &str) -> Result<()> {
    // Build a self-contained EFI binary: embed a tiny config that searches the
    // boot partition by label ("boot") so no UUID detection is needed at
    // install time.  Include the modules needed so the EFI binary can load the
    // main grub.cfg (with blscfg/load_env) from the boot partition directly.
    let stub = format!(
        "search --no-floppy --label --set=root boot\n\
         set prefix=($root)/{grub_subdir}\n\
         configfile $prefix/grub.cfg\n"
    );
    let stub_path = "/tmp/cbootc-grub-stub.cfg";
    fs::write(stub_path, &stub).context("writing grub stub config")?;

    let boot_dir = efi_mnt.join("EFI/BOOT");
    fs::create_dir_all(&boot_dir)?;
    let efi_out = boot_dir.join("BOOTx64.EFI");
    run_cmd(
        "grub-mkstandalone",
        &[
            "--format=x86_64-efi",
            // Modules needed by the embedded stub AND by the boot-partition
            // grub.cfg that configfile loads: serial/terminal for console,
            // loadenv/normal for load_env+menuentry, linux for kernel loading.
            "--modules=ext2 part_gpt fat search search_label \
                       configfile loadenv normal linux serial terminal",
            &format!("--output={}", efi_out.display()),
            &format!("boot/grub/grub.cfg={stub_path}"),
        ],
    )?;

    // Copy the full grub module tree to the boot partition so that
    // `insmod` calls in grub.cfg (locale files, extra modules) can resolve.
    let grub_boot_dir = boot_mnt.join(grub_subdir);
    fs::create_dir_all(&grub_boot_dir)?;
    run_cmd(
        "cp",
        &[
            "-r",
            "/usr/lib/grub/x86_64-efi",
            grub_boot_dir.to_str().unwrap(),
        ],
    )?;

    Ok(())
}

fn find_shim_dir() -> Result<PathBuf> {
    let efi_dir = Path::new("/usr/share/efi/EFI");
    for entry in fs::read_dir(efi_dir).context("reading /usr/share/efi/EFI")? {
        let e = entry?;
        if e.file_type()?.is_dir() && e.path().join("shimx64.efi").exists() {
            return Ok(e.path());
        }
    }
    bail!(
        "Secure Boot EFI binaries not found in /usr/share/efi/EFI/*/shimx64.efi. \
         The image must be built from a Containerfile with the grub target \
         (shim-signed/grub-efi binaries preserved at that path)."
    )
}

fn install_shim_efi(efi_mnt: &Path, boot_uuid: &str, grub_subdir: &str) -> Result<()> {
    let src = find_shim_dir()?;

    let boot_dst = efi_mnt.join("EFI/BOOT");
    fs::create_dir_all(&boot_dst)?;

    fs::copy(src.join("shimx64.efi"), boot_dst.join("BOOTx64.EFI"))
        .context("copy shimx64.efi → EFI/BOOT/BOOTx64.EFI")?;
    fs::copy(src.join("grubx64.efi"), boot_dst.join("grubx64.efi"))
        .context("copy grubx64.efi → EFI/BOOT/grubx64.efi")?;
    let mm = src.join("mmx64.efi");
    if mm.exists() {
        fs::copy(&mm, boot_dst.join("mmx64.efi")).context("copy mmx64.efi")?;
    }

    // Also populate EFI/<distro>/ for NVRAM-based boot entries and as a fallback
    // for grub binaries that have EFI/<distro>/ compiled as their search prefix
    // (e.g. EFI/fedora for Fedora, EFI/ubuntu for Ubuntu).
    let distro_name = src.file_name().unwrap().to_string_lossy();
    let distro_dst = efi_mnt.join(format!("EFI/{distro_name}"));
    fs::create_dir_all(&distro_dst)?;
    fs::copy(src.join("shimx64.efi"), distro_dst.join("shimx64.efi"))
        .with_context(|| format!("copy shimx64.efi → EFI/{distro_name}/"))?;
    fs::copy(src.join("grubx64.efi"), distro_dst.join("grubx64.efi"))
        .with_context(|| format!("copy grubx64.efi → EFI/{distro_name}/"))?;

    // The signed grubx64.efi has the distro prefix compiled in so it reads
    // EFI/<distro>/grub.cfg from the ESP.
    //
    // Fedora's grub2 has blscfg.mod: write the full BLS-scanning config here.
    // Ubuntu's grub does not ship blscfg.mod: write a redirect stub to the
    // boot partition's menuentry-based grub.cfg (generated separately).
    let esp_grub_cfg = if has_grub2() {
        format!(
            "serial --unit=0 --speed=115200\n\
             terminal_input serial console\n\
             terminal_output serial console\n\
             search --no-floppy --fs-uuid --set=root {boot_uuid}\n\
             set prefix=($root)/{grub_subdir}\n\
             load_env\n\
             if [ \"${{next_entry}}\" ] ; then\n\
               set default=\"${{next_entry}}\"\n\
               set next_entry=\n\
               save_env next_entry\n\
             fi\n\
             set timeout=3\n\
             insmod ext2\n\
             function load_video {{ true; }}\n\
             insmod blscfg\n\
             blscfg\n"
        )
    } else {
        // grubx64.efi.signed (Ubuntu) has ext2 and common modules built in.
        // Redirect to the boot partition's menuentry-based grub.cfg so that
        // upgrades only need to regenerate one file rather than touching the ESP.
        format!(
            "serial --unit=0 --speed=115200\n\
             terminal_input serial console\n\
             terminal_output serial console\n\
             search --no-floppy --fs-uuid --set=root {boot_uuid}\n\
             set prefix=($root)/{grub_subdir}\n\
             configfile $prefix/grub.cfg\n"
        )
    };
    fs::write(boot_dst.join("grub.cfg"), &esp_grub_cfg)?;
    fs::write(distro_dst.join("grub.cfg"), &esp_grub_cfg)?;

    Ok(())
}

pub(crate) fn has_grub2() -> bool {
    Command::new("grub2-install")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// The boot-partition subdirectory where grub writes its modules and config.
// grub2-install (Fedora) uses "grub2"; grub-install (Ubuntu/Debian) uses "grub".
pub(crate) fn grub_dir() -> &'static str {
    if has_grub2() { "grub2" } else { "grub" }
}

fn grub_editenv_create(path: &str) -> Result<()> {
    for cmd in &["grub2-editenv", "grub-editenv"] {
        match Command::new(cmd).args([path, "create"]).status() {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => bail!("{cmd} create: exited {s}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("spawning {cmd}")),
        }
    }
    bail!("neither grub2-editenv nor grub-editenv found in PATH")
}

fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to run {program}"))?;
    if !status.success() {
        bail!("{program} failed with {status}");
    }
    Ok(())
}

fn run_cmd_output(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    if !out.status.success() {
        bail!("{program} failed with {}", out.status);
    }
    Ok(String::from_utf8(out.stdout)
        .context("non-UTF-8 output")?
        .trim()
        .to_string())
}

/// Resolve Secure Boot signing key/cert.
///
/// If both are provided they are returned as-is (validated to exist).
/// If neither is provided, a self-signed 2048-bit RSA pair is generated
/// inside a temporary directory; the caller must keep the returned
/// `Option<TempDir>` alive until the files have been copied to persistent
/// storage.
fn resolve_sb_keys(
    key: Option<&Path>,
    cert: Option<&Path>,
) -> Result<(PathBuf, PathBuf, Option<tempfile::TempDir>)> {
    match (key, cert) {
        (Some(k), Some(c)) => {
            if !k.exists() {
                bail!("--sb-key path does not exist: {}", k.display());
            }
            if !c.exists() {
                bail!("--sb-cert path does not exist: {}", c.display());
            }
            Ok((k.to_path_buf(), c.to_path_buf(), None))
        }
        _ => {
            println!("==> Generating Secure Boot signing key pair");
            let tmp = tempfile::TempDir::new().context("creating temp dir for SB keys")?;
            let key_path = tmp.path().join("sb.key");
            let cert_path = tmp.path().join("sb.crt");
            run_cmd(
                "openssl",
                &[
                    "req",
                    "-newkey",
                    "rsa:2048",
                    "-nodes",
                    "-keyout",
                    key_path.to_str().unwrap(),
                    "-new",
                    "-x509",
                    "-sha256",
                    "-days",
                    "3650",
                    "-subj",
                    "/CN=composefs-os Secure Boot/",
                    "-out",
                    cert_path.to_str().unwrap(),
                ],
            )?;
            Ok((key_path, cert_path, Some(tmp)))
        }
    }
}

/// Sign an EFI binary in-place with `sbsign`.
pub(crate) fn sign_efi(target: &Path, key: &Path, cert: &Path) -> Result<()> {
    run_cmd(
        "sbsign",
        &[
            "--key",
            key.to_str().unwrap(),
            "--cert",
            cert.to_str().unwrap(),
            "--output",
            target.to_str().unwrap(),
            target.to_str().unwrap(),
        ],
    )
}

/// Sign `src` EFI binary, writing the signed output to `dst`.
fn sign_efi_to(src: &Path, dst: &Path, key: &Path, cert: &Path) -> Result<()> {
    run_cmd(
        "sbsign",
        &[
            "--key",
            key.to_str().unwrap(),
            "--cert",
            cert.to_str().unwrap(),
            "--output",
            dst.to_str().unwrap(),
            src.to_str().unwrap(),
        ],
    )
}

/// Install signed systemd-boot for the UKI + Secure Boot path.
///
/// No shim: the firmware verifies systemd-boot and the UKI directly against
/// its Signature Database (db).  The signing cert must be enrolled in the db
/// once (via the UEFI setup menu or efi-updatevar) before Secure Boot is
/// enforced.
///
/// Layout on the ESP:
///   EFI/BOOT/BOOTx64.EFI            ← signed systemd-boot (UEFI fallback path)
///   EFI/systemd/systemd-bootx64.efi ← same binary (NVRAM boot entry path)
///   EFI/BOOT/composefs-os-sb.cer    ← DER cert for one-time db enrollment
fn install_systemd_boot_secureboot(efi_mnt: &Path, key: &Path, cert: &Path) -> Result<()> {
    let sdboot_src = Path::new("/usr/lib/systemd/boot/efi/systemd-bootx64.efi");
    if !sdboot_src.exists() {
        bail!(
            "systemd-boot EFI binary not found at {}. \
             The image must be built with --target uki-secureboot.",
            sdboot_src.display()
        );
    }

    // Sign systemd-boot into a temp file, then place it on the ESP.
    let signed_tmp = tempfile::Builder::new()
        .suffix(".efi")
        .tempfile()
        .context("creating temp file for signed systemd-boot")?;
    sign_efi_to(sdboot_src, signed_tmp.path(), key, cert)?;

    let boot_dir = efi_mnt.join("EFI/BOOT");
    fs::create_dir_all(&boot_dir)?;
    // Firmware's UEFI fallback path — loaded when no NVRAM boot entry exists.
    fs::copy(signed_tmp.path(), boot_dir.join("BOOTx64.EFI"))
        .context("copying signed systemd-boot → EFI/BOOT/BOOTx64.EFI")?;

    let systemd_dir = efi_mnt.join("EFI/systemd");
    fs::create_dir_all(&systemd_dir)?;
    fs::copy(signed_tmp.path(), systemd_dir.join("systemd-bootx64.efi"))
        .context("copying signed systemd-boot → EFI/systemd/")?;

    // Export DER cert so the user can enroll it into the UEFI db once:
    //   Via UEFI setup menu: Secure Boot → Key Management → DB → Add from File
    //   Via efi-updatevar:   efi-updatevar -a -c /boot/efi/EFI/BOOT/composefs-os-sb.cer db
    run_cmd(
        "openssl",
        &[
            "x509",
            "-in",
            cert.to_str().unwrap(),
            "-outform",
            "DER",
            "-out",
            boot_dir.join("composefs-os-sb.cer").to_str().unwrap(),
        ],
    )?;

    println!(
        "    Enroll the signing cert into the UEFI Signature Database (db) once:\n\
         \t  UEFI setup menu → Secure Boot → Key Management → DB → Add from File\n\
         \t    select EFI/BOOT/composefs-os-sb.cer on the ESP\n\
         \t  or from a running system:\n\
         \t    efi-updatevar -a -c /boot/efi/EFI/BOOT/composefs-os-sb.cer db"
    );

    Ok(())
}
