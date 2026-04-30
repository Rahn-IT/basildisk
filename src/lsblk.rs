use serde::Deserialize;
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
            .arg("-o")
            .arg("HOTPLUG,MODEL,NAME,ROTA,SERIAL,SIZE,TRAN,TYPE,FSTYPE,FSUSED,FSAVAIL,FSUSE%,MOUNTPOINTS")
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
    #[serde(default)]
    pub hotplug: bool,
    pub model: Option<String>,
    pub name: String,
    #[serde(default)]
    pub rota: bool,
    pub serial: Option<String>,
    #[serde(default)]
    pub size: u64,
    pub tran: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: String,
    pub fstype: Option<String>,
    pub fsused: Option<u64>,
    pub fsavail: Option<u64>,
    #[serde(rename = "fsuse%")]
    pub fsuse_percent: Option<String>,
    #[serde(default)]
    pub mountpoints: Vec<Option<String>>,
    #[serde(default)]
    pub children: Vec<LsBlkDisk>,
}

impl LsBlkDisk {
    pub fn mount_points(&self) -> Vec<String> {
        let mut mount_points = Vec::new();
        self.collect_mount_points(&mut mount_points);
        mount_points.sort();
        mount_points.dedup();
        mount_points
    }

    fn collect_mount_points(&self, mount_points: &mut Vec<String>) {
        mount_points.extend(
            self.mountpoints
                .iter()
                .filter_map(|mountpoint| mountpoint.as_deref())
                .map(str::trim)
                .filter(|mountpoint| !mountpoint.is_empty())
                .map(ToOwned::to_owned),
        );

        for child in &self.children {
            child.collect_mount_points(mount_points);
        }
    }

    pub fn partitions(&self) -> Vec<LsBlkPartition> {
        let mut partitions = Vec::new();
        self.collect_partitions(0, &mut partitions);
        partitions
    }

    fn collect_partitions(&self, depth: usize, partitions: &mut Vec<LsBlkPartition>) {
        for child in &self.children {
            let mount_points = child.direct_mount_points();
            let mount_points_display = mount_points.join(", ");
            partitions.push(LsBlkPartition {
                name: child.name.clone(),
                kind: child.kind.clone(),
                fs_type: child.fstype.clone(),
                fs_used: child.fsused,
                fs_available: child.fsavail,
                fs_use_percent: child.fsuse_percent.clone(),
                size: child.size,
                depth,
                is_mounted: !mount_points.is_empty(),
                mount_points,
                mount_points_display,
            });
            child.collect_partitions(depth + 1, partitions);
        }
    }

    fn direct_mount_points(&self) -> Vec<String> {
        let mut mount_points: Vec<String> = self
            .mountpoints
            .iter()
            .filter_map(|mountpoint| mountpoint.as_deref())
            .map(str::trim)
            .filter(|mountpoint| !mountpoint.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        mount_points.sort();
        mount_points.dedup();
        mount_points
    }
}

pub struct LsBlkPartition {
    pub name: String,
    pub kind: String,
    pub fs_type: Option<String>,
    pub fs_used: Option<u64>,
    pub fs_available: Option<u64>,
    pub fs_use_percent: Option<String>,
    pub size: u64,
    pub depth: usize,
    pub is_mounted: bool,
    pub mount_points: Vec<String>,
    pub mount_points_display: String,
}
