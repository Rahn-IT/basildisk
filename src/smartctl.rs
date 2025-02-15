use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Serialize, Deserialize)]
pub struct SmartCtl {
    pub device: Option<Device>,
    pub model_name: Option<String>,
    pub model_family: Option<String>,
    pub serial_number: Option<String>,
    pub firmware_version: Option<String>,
    pub nvme_smart_health_information_log: Option<NvmeHealthInformation>,
    pub ata_smart_attributes: Option<AtaSmartAttributes>,
}

#[derive(Serialize, Deserialize)]
pub struct Device {
    #[serde(rename = "type")]
    pub _type: String,
    pub protocol: String,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct NvmeHealthInformation {
    pub critical_warning: u64,
    pub temperature: u64,
    pub available_spare: u64,
    pub available_spare_threshold: u64,
    pub percentage_used: u64,
    pub data_units_read: u64,
    pub data_units_written: u64,
    pub host_reads: u64,
    pub host_writes: u64,
    pub controller_busy_time: u64,
    pub power_cycles: u64,
    pub power_on_hours: u64,
    pub unsafe_shutdowns: u64,
    pub media_errors: u64,
    pub num_err_log_entries: u64,
    pub warning_temp_time: u64,
    pub critical_comp_time: u64,
    pub temperature_sensors: Vec<u64>,
}

#[derive(Serialize, Deserialize)]
pub struct AtaSmartAttributes {
    pub revision: u8,
    pub table: Vec<AtaAttribute>,
}

#[derive(Serialize, Deserialize)]
pub struct AtaAttribute {
    id: u16,
    name: String,
    value: u8,
    worst: u8,
    thresh: u8,
    raw: RawValue,
}

#[derive(Serialize, Deserialize)]
pub struct RawValue {
    value: u32,
    string: String,
}

#[derive(Debug, Error)]
pub enum SmartCtlError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Decode(#[from] serde_json::Error),
}

impl SmartCtl {
    pub async fn get(device: &str) -> Result<SmartCtl, SmartCtlError> {
        let output = tokio::process::Command::new("smartctl")
            .arg("-a")
            .arg("-j")
            .arg("-T")
            .arg("verypermissive")
            .arg(format!("/dev/{}", device))
            .output()
            .await?;

        println!("{}", String::from_utf8_lossy(&output.stdout));

        Ok(serde_json::from_slice(&output.stdout)?)
    }
}
