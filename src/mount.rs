use std::{
    path::{Path, PathBuf},
    string::FromUtf8Error,
};

use thiserror::Error;
use tokio::process::Command;

const MOUNT_ROOT: &str = "/mnt";

#[derive(Clone, Copy)]
pub enum MountAccess {
    Read,
    ReadWrite,
}

impl MountAccess {
    fn mount_option(self) -> &'static str {
        match self {
            Self::Read => "ro",
            Self::ReadWrite => "rw",
        }
    }
}

#[derive(Debug, Error)]
pub enum MountError {
    #[error("unsupported filesystem: {0}")]
    UnsupportedFilesystem(String),
    #[error("device name contains unsupported path characters: {0}")]
    InvalidDevice(String),
    #[error("partition is not mounted under /mnt")]
    NotMountedUnderMnt,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("error parsing output: {0}")]
    UTF8(#[from] FromUtf8Error),
    #[error("{command} failed with code {code}: {stderr}")]
    CommandFailed {
        command: String,
        code: i32,
        stderr: String,
    },
}

pub fn is_supported_filesystem(fs_type: &str) -> bool {
    matches!(
        fs_type,
        "ext2" | "ext3" | "ext4" | "btrfs" | "xfs" | "vfat" | "exfat" | "ntfs" | "ntfs3"
    )
}

pub fn mount_point_for_device(device: &str) -> Result<PathBuf, MountError> {
    validate_device_name(device)?;
    Ok(Path::new(MOUNT_ROOT).join(device))
}

pub fn mount_point_under_mnt(mount_points: &[String]) -> Option<String> {
    mount_points
        .iter()
        .find(|mount_point| is_under_mnt(mount_point))
        .cloned()
}

pub async fn mount_partition(device: &str, fs_type: &str) -> Result<PathBuf, MountError> {
    if !is_supported_filesystem(fs_type) {
        return Err(MountError::UnsupportedFilesystem(fs_type.to_string()));
    }

    validate_device_name(device)?;
    let device_path = Path::new("/dev").join(device);
    let mount_point = mount_point_for_device(device)?;

    tokio::fs::create_dir_all(&mount_point).await?;

    run_command(
        Command::new("mount")
            .arg("-o")
            .arg(MountAccess::Read.mount_option())
            .arg(device_path)
            .arg(&mount_point),
    )
    .await?;

    Ok(mount_point)
}

pub async fn remount_partition(
    device: &str,
    mount_point: &str,
    access: MountAccess,
) -> Result<(), MountError> {
    validate_device_name(device)?;
    if !is_under_mnt(mount_point) {
        return Err(MountError::NotMountedUnderMnt);
    }

    run_command(Command::new("umount").arg(mount_point)).await?;

    let mount_result = run_command(
        Command::new("mount")
            .arg("-o")
            .arg(access.mount_option())
            .arg(Path::new("/dev").join(device))
            .arg(mount_point),
    )
    .await;

    if mount_result.is_err() && matches!(access, MountAccess::ReadWrite) {
        let _ = run_command(
            Command::new("mount")
                .arg("-o")
                .arg(MountAccess::Read.mount_option())
                .arg(Path::new("/dev").join(device))
                .arg(mount_point),
        )
        .await;
    }

    mount_result
}

pub async fn unmount_partition(mount_points: &[String]) -> Result<(), MountError> {
    let mount_point = mount_point_under_mnt(mount_points).ok_or(MountError::NotMountedUnderMnt)?;

    run_command(Command::new("umount").arg(mount_point)).await
}

fn validate_device_name(device: &str) -> Result<(), MountError> {
    if device.is_empty() || device.contains('/') || device.contains("..") {
        return Err(MountError::InvalidDevice(device.to_string()));
    }

    Ok(())
}

fn is_under_mnt(path: &str) -> bool {
    let path = Path::new(path);
    path == Path::new(MOUNT_ROOT) || path.starts_with(MOUNT_ROOT)
}

async fn run_command(command: &mut Command) -> Result<(), MountError> {
    let command_display = format!("{:?}", command.as_std());
    let output = command.output().await?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8(output.stderr)?;
    Err(MountError::CommandFailed {
        command: command_display,
        code: output.status.code().unwrap_or(-1),
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::MountAccess;

    #[test]
    fn maps_mount_access_to_linux_mount_options() {
        assert_eq!(MountAccess::Read.mount_option(), "ro");
        assert_eq!(MountAccess::ReadWrite.mount_option(), "rw");
    }
}
