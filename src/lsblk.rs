use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::process::Command;

#[derive(Deserialize)]
pub struct LsBlk {
    blockdevices: Vec<Disk>,
}

#[derive(Debug, Error)]
pub enum LsBlkError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Decode(#[from] serde_json::Error),
}

impl LsBlk {
    pub async fn list() -> Result<Vec<Disk>, LsBlkError> {
        let output = Command::new("lsblk")
            .arg("-d")
            .arg("-o")
            .arg("HOTPLUG,MODEL,NAME,ROTA,SERIAL,SIZE,TRAN,VENDOR,WWN")
            .arg("--bytes")
            .arg("--json")
            .output()
            .await?;

        let mut lsblk: LsBlk = serde_json::from_slice(&output.stdout)?;

        lsblk.blockdevices.retain(|disk| disk.serial.is_some());

        Ok(lsblk.blockdevices)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Disk {
    hotplug: bool,
    model: Option<String>,
    name: String,
    rota: bool,
    serial: Option<String>,
    size: u64,
    tran: Option<String>,
    vendor: Option<String>,
    wwn: Option<String>,
}

impl Disk {
    pub fn format(&self) -> IndexDisk {
        let mut size = if self.size > 1000_000_000_000 {
            format!("{} TB", self.size / 1000_000_000_000)
        } else if self.size > 1000_000_000 {
            format!("{} GB", self.size / 1000_000_000)
        } else if self.size > 1000_000 {
            format!("{} MB", self.size / 1000_000)
        } else if self.size > 1000 {
            format!("{} KB", self.size / 1000)
        } else {
            format!("{} B", self.size)
        };

        let size_bin = if self.size > (1024u64).pow(4) {
            format!("{:.2} TiB", self.size as f64 / 1024f64.powi(4))
        } else if self.size > (1024u64).pow(3) {
            format!("{:.2} GiB", self.size as f64 / 1024f64.powi(3))
        } else if self.size > (1024u64).pow(2) {
            format!("{:.2} MiB", self.size as f64 / 1024f64.powi(2))
        } else if self.size > 1024 {
            format!("{:.2} KiB", self.size as f64 / 1024f64)
        } else {
            "".to_string()
        };

        if !size_bin.is_empty() {
            size = format!("{} / {}", size, size_bin);
        }

        let disk_type = match self.tran.as_ref().map(|s| s.as_str()) {
            Some("nvme") => DiskType::NVMe,
            _ => DiskType::HDD,
        };

        IndexDisk {
            model: self.model.clone().unwrap(),
            serial: self.serial.clone().unwrap(),
            size,
            device: self.name.clone(),
            disk_type,
        }
    }
}

#[derive(Serialize)]
pub struct IndexDisk {
    model: String,
    serial: String,
    size: String,
    device: String,
    disk_type: DiskType,
}

#[derive(Serialize)]
pub enum DiskType {
    SSD,
    HDD,
    USB,
    NVMe,
}
