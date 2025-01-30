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
    model_exact: Option<String>,
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

            joinset.spawn(async move { (SmartCtl::get(&device).await, device) });
        }

        let mut smartctl = HashMap::new();

        while let Some(result) = joinset.join_next().await {
            let (smart_result, device) = result?;
            let smart_data = smart_result?;

            smartctl.insert(device, smart_data);
        }

        let disks = lsblk
            .into_iter()
            .map(|lsblk_info| {
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
                    if model_display.contains("Samsung based") {
                        model_display = lsblk_info.model.unwrap_or_default()
                    }

                    let mut connection_type = ConnectionType::Unknown;

                    if let Some(device) = &smart.device {
                        connection_type = match device.protocol.as_str() {
                            "SCSI" => ConnectionType::SCSI,
                            "ATA" => ConnectionType::SATA,
                            _ => ConnectionType::Unknown,
                        }
                    }

                    Disk {
                        model: model_display,
                        model_exact,
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
}
