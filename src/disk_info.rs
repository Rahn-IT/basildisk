use std::{collections::HashMap, fmt::Display};

use serde::Serialize;
use thiserror::Error;
use tokio::task::JoinSet;

use crate::{
    erase::{EraseType, GetEraseTypeError},
    lsblk::{LsBlk, LsBlkError},
    mount,
    smartctl::{SmartCtl, SmartCtlError},
};

#[derive(Serialize)]
pub struct Disk {
    pub model: String,
    pub model_exact: Option<String>,
    pub serial: Option<String>,
    size_formated: String,
    pub device: String,
    removable: bool,
    pub is_mounted: bool,
    pub mount_points_display: String,
    pub partitions: Vec<Partition>,
    pub disk_type: DiskType,
    pub connection_type: ConnectionType,
    pub erase_type: EraseType,
    pub erase_can_run: bool,
}

#[derive(Serialize)]
pub struct Partition {
    pub name: String,
    pub kind: String,
    pub fs_type: Option<String>,
    pub size_formated: String,
    pub has_usage: bool,
    pub usage_display: String,
    pub usage_percent: u8,
    pub depth_class: String,
    pub is_mounted: bool,
    pub mount_points: Vec<String>,
    pub mount_points_display: String,
    pub can_mount: bool,
    pub can_unmount: bool,
    pub can_browse: bool,
    pub browse_url: String,
    pub mount_disabled_reason: String,
    pub unmount_disabled_reason: String,
}

#[derive(Serialize, PartialEq, Eq, Clone, Copy)]
pub enum DiskType {
    Ssd,
    Hdd,
}

impl Display for DiskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hdd => write!(f, "HDD"),
            Self::Ssd => write!(f, "SSD"),
        }
    }
}

#[derive(Clone, Copy)]
pub enum ConnectionType {
    Sata,
    Scsi,
    Nvme,
    Usb,
    Unknown,
}

impl Serialize for ConnectionType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let display = format!("{self}");
        serializer.serialize_str(&display)
    }
}

impl Display for ConnectionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionType::Sata => write!(f, "Serial ATA"),
            ConnectionType::Scsi => write!(f, "SCSI"),
            ConnectionType::Nvme => write!(f, "NVMe"),
            ConnectionType::Usb => write!(f, "USB"),
            ConnectionType::Unknown => write!(f, "Unknown"),
        }
    }
}

#[derive(Debug, Error)]
pub enum ListDiskError {
    #[error("error finding disks with lsblk: {0}")]
    LsBlk(#[from] LsBlkError),
    #[error("join error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("error during smartctl: {0}")]
    SmartCtl(#[from] SmartCtlError),
    #[error("error while getting erase type: {0}")]
    EraseTypeError(#[from] GetEraseTypeError),
}

impl Disk {
    pub async fn list() -> Result<Vec<Disk>, ListDiskError> {
        let lsblk = LsBlk::list().await?;

        let mut joinset = JoinSet::new();

        for disk in &lsblk {
            let device = disk.name.clone();

            joinset.spawn(async move { (SmartCtl::get(&device).await, device) });
        }

        let mut smartctl = HashMap::new();

        while let Some(result) = joinset.join_next().await {
            let (smart_result, device) = result?;
            let smart_data = smart_result?;

            smartctl.insert(device, smart_data);
        }

        let mut disks = Vec::new();

        for lsblk_info in lsblk {
            if let Some(smart) = smartctl.remove(&lsblk_info.name) {
                let model_exact: Option<String> = smart.model_name;
                let model_family: Option<String> = smart.model_family;

                let mut model_display = model_family
                    .unwrap_or_else(|| {
                        model_exact.clone().unwrap_or_else(|| {
                            lsblk_info
                                .model
                                .clone()
                                .unwrap_or_else(|| "Unknown Disk Model".to_string())
                        })
                    })
                    .trim()
                    .to_string();

                // Samsung just writes junk into the model family :(
                if model_display.contains("based") {
                    model_display = lsblk_info.model.clone().unwrap_or_default()
                }

                let connection_type = if let Some(tran) = &lsblk_info.tran {
                    match tran.as_str() {
                        "sata" => ConnectionType::Sata,
                        "scsi" => ConnectionType::Scsi,
                        "nvme" => ConnectionType::Nvme,
                        "usb" => ConnectionType::Usb,
                        _ => ConnectionType::Unknown,
                    }
                } else {
                    ConnectionType::Unknown
                };

                let disk_type = if lsblk_info.rota {
                    DiskType::Hdd
                } else {
                    DiskType::Ssd
                };

                let erase_type =
                    EraseType::get_for_disk(&lsblk_info.name, connection_type, disk_type).await?;

                let mount_points = lsblk_info.mount_points();
                let partitions = lsblk_info
                    .partitions()
                    .into_iter()
                    .map(|partition| {
                        let supported = partition
                            .fs_type
                            .as_deref()
                            .is_some_and(mount::is_supported_filesystem);
                        let can_mount = supported && !partition.is_mounted;
                        let can_unmount = partition.is_mounted
                            && mount::mount_point_under_mnt(&partition.mount_points).is_some();
                        let can_browse = partition.is_mounted
                            && mount::mount_point_under_mnt(&partition.mount_points).is_some();
                        let mount_disabled_reason = if supported {
                            "Partition is already mounted.".to_string()
                        } else {
                            "Unsupported filesystem.".to_string()
                        };
                        let unmount_disabled_reason = if partition.is_mounted {
                            "Partition is not mounted under /mnt.".to_string()
                        } else {
                            "Partition is not mounted.".to_string()
                        };

                        let browse_url = format!("/browse/{}", partition.name);

                        Partition {
                            name: partition.name,
                            kind: partition.kind,
                            fs_type: partition.fs_type,
                            size_formated: Self::format_size(partition.size),
                            has_usage: partition.fs_used.is_some(),
                            usage_display: Self::format_usage(
                                partition.fs_used,
                                partition.fs_available,
                            ),
                            usage_percent: Self::usage_percent(
                                partition.fs_use_percent.as_deref(),
                                partition.fs_used,
                                partition.fs_available,
                            ),
                            depth_class: format!("partition-depth-{}", partition.depth.min(4)),
                            is_mounted: partition.is_mounted,
                            mount_points: partition.mount_points,
                            mount_points_display: partition.mount_points_display,
                            can_mount,
                            can_unmount,
                            can_browse,
                            browse_url,
                            mount_disabled_reason,
                            unmount_disabled_reason,
                        }
                    })
                    .collect();
                let disk = Disk {
                    model: model_display,
                    model_exact,
                    serial: lsblk_info.serial,
                    size_formated: Self::format_size(lsblk_info.size),
                    device: lsblk_info.name,
                    removable: lsblk_info.hotplug,
                    is_mounted: !mount_points.is_empty(),
                    mount_points_display: mount_points.join(", "),
                    partitions,
                    connection_type,
                    disk_type,
                    erase_type,
                    erase_can_run: erase_type.can_run(),
                };

                disks.push(disk);
            } else {
                panic!("dammit");
            }
        }

        Ok(disks)
    }

    fn format_size(size: u64) -> String {
        let mut size_formatted = if size > 1_000_000_000_000 {
            format!("{} TB", size / 1_000_000_000_000)
        } else if size > 1_000_000_000 {
            format!("{} GB", size / 1_000_000_000)
        } else if size > 1_000_000 {
            format!("{} MB", size / 1_000_000)
        } else if size > 1000 {
            format!("{} KB", size / 1000)
        } else {
            format!("{} B", size)
        };

        let size_bin = if size > (1024u64).pow(4) {
            format!("{:.2} TiB", size as f64 / 1024f64.powi(4))
        } else if size > (1024u64).pow(3) {
            format!("{:.2} GiB", size as f64 / 1024f64.powi(3))
        } else if size > (1024u64).pow(2) {
            format!("{:.2} MiB", size as f64 / 1024f64.powi(2))
        } else if size > 1024 {
            format!("{:.2} KiB", size as f64 / 1024f64)
        } else {
            "".to_string()
        };

        if !size_bin.is_empty() {
            size_formatted = format!("{} / {}", size_formatted, size_bin);
        }

        size_formatted
    }

    fn format_usage(used: Option<u64>, available: Option<u64>) -> String {
        let Some(used) = used else {
            return String::new();
        };

        if let Some(available) = available {
            format!(
                "{} used / {} total",
                Self::format_size(used),
                Self::format_size(used.saturating_add(available))
            )
        } else {
            format!("{} used", Self::format_size(used))
        }
    }

    fn usage_percent(fsuse_percent: Option<&str>, used: Option<u64>, available: Option<u64>) -> u8 {
        if let Some(percent) =
            fsuse_percent.and_then(|value| value.trim().trim_end_matches('%').parse::<u8>().ok())
        {
            return percent.min(100);
        }

        let (Some(used), Some(available)) = (used, available) else {
            return 0;
        };

        let total = used.saturating_add(available);
        used.saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0)
            .min(100) as u8
    }
}
