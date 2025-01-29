use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Serialize, Deserialize)]
pub struct SmartCtl {
    device: Device,
    model_name: String,
    serial_number: String,
    firmware_version: String,
    nvme_smart_health_information_log: HealthInformation,
}

#[derive(Serialize, Deserialize)]
pub struct Device {
    #[serde(rename = "type")]
    _type: String,
    protocol: String,
}

#[derive(Serialize, Deserialize)]
pub struct HealthInformation {
    critical_warning: u64,
    temperature: u64,
    available_spare: u64,
    available_spare_threshold: u64,
    percentage_used: u64,
    data_units_read: u64,
    data_units_written: u64,
    host_reads: u64,
    host_writes: u64,
    controller_busy_time: u64,
    power_cycles: u64,
    power_on_hours: u64,
    unsafe_shutdowns: u64,
    media_errors: u64,
    num_err_log_entries: u64,
    warning_temp_time: u64,
    critical_comp_time: u64,
    temperature_sensors: Vec<u64>,
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
            .arg(format!("/dev/{}", device))
            .output()
            .await?;

        println!("{}", String::from_utf8_lossy(&output.stdout));

        if !output.status.success() {
            return Err(SmartCtlError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "smartctl failed",
            )));
        }

        Ok(serde_json::from_slice(&output.stdout)?)
    }
}
