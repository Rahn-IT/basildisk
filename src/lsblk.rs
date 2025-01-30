use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::process::Command;

#[derive(Deserialize)]
pub struct LsBlk {
    blockdevices: Vec<LsBlkDisk>,
}

#[derive(Debug, Error)]
pub enum LsBlkError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Decode(#[from] serde_json::Error),
}

impl LsBlk {
    pub async fn list() -> Result<Vec<LsBlkDisk>, LsBlkError> {
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

#[derive(Deserialize)]
pub struct LsBlkDisk {
    pub hotplug: bool,
    pub model: Option<String>,
    pub name: String,
    pub rota: bool,
    pub serial: Option<String>,
    pub size: u64,
    pub tran: Option<String>,
    pub vendor: Option<String>,
    pub wwn: Option<String>,
}
