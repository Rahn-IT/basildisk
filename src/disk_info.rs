use std::collections::HashMap;

use serde::Serialize;
use thiserror::Error;
use tokio::task::JoinSet;

use crate::{
    lsblk::{LsBlk, LsBlkError},
    smartctl::{SmartCtl, SmartCtlError},
};

#[derive(Serialize)]
pub struct Disk {
    model: String,
    serial: Option<String>,
    size_formated: String,
    device: String,
    removable: bool,
    disk_type: DiskType,
    connection_type: ConnectionType,
}

#[derive(Serialize)]
pub enum DiskType {
    SSD,
    HDD,
    USB,
    Virtual,
}

#[derive(Serialize)]
pub enum ConnectionType {
    SATA,
    SCSI,
    NVMe,
    Unknown,
}

#[derive(Debug, Error)]
pub enum ListDiskError {
    #[error("error finding disks with lsblk: {0}")]
    LsBlk(#[from] LsBlkError),
    #[error("join error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("error during smartctl: {0}")]
    SmartCtl(#[from] SmartCtlError),
}

impl Disk {
    pub async fn list() -> Result<Vec<Disk>, ListDiskError> {
        let lsblk = LsBlk::list().await?;

        let mut joinset = JoinSet::new();

        for disk in &lsblk {
            let device = disk.name.clone();

            joinset.spawn(async move { SmartCtl::get(&device).await });
        }

        let mut smartctl = HashMap::new();

        while let Some(result) = joinset.join_next().await {
            let smart_data = result??;

            // Smartctl outputs the path, not just the device name, so we just truncate the /dev/
            let device = &smart_data.device.name[5..];

            smartctl.insert(device.to_string(), smart_data);
        }

        let disks = lsblk
            .into_iter()
            .map(|lsblk_info| {
                if let Some(smart) = smartctl.remove(&lsblk_info.name) {
                    let model = if let Some(vendor) = &lsblk_info.vendor {
                        if let Some(model) = &lsblk_info.model {
                            format!("{}: {}", vendor.trim(), model.trim())
                        } else {
                            format!("{}: unknown disk model", vendor.trim())
                        }
                    } else if let Some(model) = &lsblk_info.model {
                        model.trim().to_string()
                    } else {
                        "unknown disk model".to_string()
                    };

                    let connection_type = match smart.device.protocol.as_str() {
                        "SCSI" => ConnectionType::SCSI,
                        _ => ConnectionType::Unknown,
                    };

                    Disk {
                        model,
                        serial: lsblk_info.serial,
                        size_formated: Self::format_size(lsblk_info.size),
                        device: lsblk_info.name,
                        removable: lsblk_info.hotplug,
                        connection_type,
                        disk_type: DiskType::HDD,
                    }
                } else {
                    panic!("dammit");
                }
            })
            .collect();

        Ok(disks)
    }

    fn format_size(size: u64) -> String {
        let mut size_formatted = if size > 1000_000_000_000 {
            format!("{} TB", size / 1000_000_000_000)
        } else if size > 1000_000_000 {
            format!("{} GB", size / 1000_000_000)
        } else if size > 1000_000 {
            format!("{} MB", size / 1000_000)
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
}
