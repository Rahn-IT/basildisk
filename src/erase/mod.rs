use std::fmt::Display;

use hdparm::{Hdparm, HdparmError};
use nvme::{Nvme, NvmeError};
use serde::Serialize;
use sha2::{Digest, Sha256};
use shred::Shred;
use thiserror::Error;
use tokio::{process::Command, sync::broadcast};

use crate::{
    disk_info::{ConnectionType, DiskType},
    jobs::{self, Job},
};

pub mod hdparm;
pub mod nvme;
pub mod shred;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EraseType {
    None,
    BlockOverride,
    AtaSecureErase,
    AtaEnhancedSecureErase,
    NvmeSanitizeCryptoErase,
    NvmeSanitizeBlockErase,
    NvmeSanitizeOverwrite,
    NvmeFormatCryptoErase,
    NvmeFormatUserDataErase,
}

impl Serialize for EraseType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let display = format!("{self}");
        serializer.serialize_str(&display)
    }
}

impl Display for EraseType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EraseType::None => write!(f, "None"),
            EraseType::BlockOverride => write!(f, "Block Override"),
            EraseType::AtaSecureErase => write!(f, "ATA Secure Erase"),
            EraseType::AtaEnhancedSecureErase => write!(f, "ATA Enhanced Secure Erase"),
            EraseType::NvmeSanitizeCryptoErase => write!(f, "NVMe Sanitize Crypto Erase"),
            EraseType::NvmeSanitizeBlockErase => write!(f, "NVMe Sanitize Block Erase"),
            EraseType::NvmeSanitizeOverwrite => write!(f, "NVMe Sanitize Overwrite"),
            EraseType::NvmeFormatCryptoErase => write!(f, "NVMe Format Crypto Erase"),
            EraseType::NvmeFormatUserDataErase => write!(f, "NVMe Format User Data Erase"),
        }
    }
}

#[derive(Debug, Error)]
pub enum GetEraseTypeError {
    #[error("error during hdparm: {0}")]
    Hdparm(#[from] HdparmError),
    #[error("error during nvme: {0}")]
    Nvme(#[from] NvmeError),
}

#[derive(Debug, Error)]
enum EraseRunError {
    #[error("{0}")]
    Unsupported(String),
}

impl EraseType {
    pub fn can_run(self) -> bool {
        matches!(
            self,
            Self::BlockOverride | Self::AtaSecureErase | Self::AtaEnhancedSecureErase
        )
    }

    pub async fn get_for_disk(
        device: &str,
        connection_type: ConnectionType,
        disk_type: DiskType,
    ) -> Result<Self, GetEraseTypeError> {
        match connection_type {
            ConnectionType::Sata => match disk_type {
                DiskType::Hdd => Ok(EraseType::BlockOverride),
                DiskType::Ssd => {
                    let hdparm = Hdparm::get_for_disk(device).await?;

                    Ok(if hdparm.enhanced_secure_erase {
                        EraseType::AtaEnhancedSecureErase
                    } else if hdparm.secure_erase {
                        EraseType::AtaSecureErase
                    } else {
                        EraseType::None
                    })
                }
            },
            ConnectionType::Usb => match disk_type {
                DiskType::Ssd => Ok(EraseType::None),
                DiskType::Hdd => Ok(EraseType::BlockOverride),
            },
            ConnectionType::Nvme => {
                let nvme = Nvme::get_for_disk(device).await?;

                Ok(if nvme.sanitize_crypto_erase {
                    EraseType::NvmeSanitizeCryptoErase
                } else if nvme.sanitize_block_erase {
                    EraseType::NvmeSanitizeBlockErase
                } else if nvme.sanitize_overwrite {
                    EraseType::NvmeSanitizeOverwrite
                } else if nvme.format_crypto_erase {
                    EraseType::NvmeFormatCryptoErase
                } else if nvme.format_nvm {
                    EraseType::NvmeFormatUserDataErase
                } else {
                    EraseType::None
                })
            }
            _ => Ok(EraseType::None),
        }
    }
}

pub struct EraseJob {
    pub device: String,
    pub disk_type: DiskType,
    pub connection_type: ConnectionType,
    pub erase_type: EraseType,
    pub model: String,
    pub serial: String,
}

impl Job for EraseJob {
    fn get_device(&self) -> &str {
        &self.device
    }

    fn get_name(&self) -> String {
        format!("{} for {}: {}", self.erase_type, self.model, self.serial)
    }

    fn final_log_success_data(&self) -> Option<fn(&str) -> Option<String>> {
        Some(final_log_success_data)
    }

    async fn run(
        self,
        logger: broadcast::Sender<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send>> {
        let device = self.device.clone();
        let started_at = jobs::format_unix_timestamp(jobs::unix_now());
        let intro = format!(
            "
Starting Secure Disk Erasure
=================================================
Model: {}
Serial: {}
Device Name: {}
Started at: {}
=================================================
Connected via: {}
Detected Disk Type: {}
Selected Erasure Method: {}
=================================================
",
            self.model,
            self.serial,
            self.device,
            started_at,
            self.connection_type,
            self.disk_type,
            self.erase_type
        );

        logger.send(intro).unwrap();

        let result = match self.erase_type {
            EraseType::AtaEnhancedSecureErase => {
                Hdparm::ata_secure_erase_disk(self.device, &logger, true)
                    .await
                    .map_err(|err| {
                        let b: Box<dyn std::error::Error + Send> = Box::new(err);
                        b
                    })
            }
            EraseType::AtaSecureErase => Hdparm::ata_secure_erase_disk(self.device, &logger, false)
                .await
                .map_err(|err| {
                    let b: Box<dyn std::error::Error + Send> = Box::new(err);
                    b
                }),
            EraseType::BlockOverride => {
                Shred::override_disk(self.device, &logger)
                    .await
                    .map_err(|err| {
                        let b: Box<dyn std::error::Error + Send> = Box::new(err);
                        b
                    })
            }
            EraseType::NvmeSanitizeCryptoErase
            | EraseType::NvmeSanitizeBlockErase
            | EraseType::NvmeSanitizeOverwrite
            | EraseType::NvmeFormatCryptoErase
            | EraseType::NvmeFormatUserDataErase => {
                let err: Box<dyn std::error::Error + Send> = Box::new(EraseRunError::Unsupported(
                    "NVMe erase execution is not implemented yet.".to_string(),
                ));
                Err(err)
            }
            EraseType::None => Ok(()),
        };

        let finished_at = jobs::format_unix_timestamp(jobs::unix_now());
        let outro = if let Err(err) = &result {
            format!(
                "
=================================================
Errors detected during secure erase!
Finished at: {finished_at}
=================================================
{err}
"
            )
        } else {
            format!(
                "
=================================================
Secure Erase was successful!
Finished at: {finished_at}
================================================="
            )
        };

        logger.send(outro).unwrap();

        if result.is_ok() {
            refresh_partition_table(&device).await;
        }

        result
    }
}

async fn refresh_partition_table(device: &str) {
    match Command::new("partprobe").arg(device).status().await {
        Ok(status) if status.success() => {}
        Ok(status) => println!("partprobe failed for {device}: {status}"),
        Err(err) => println!("partprobe failed for {device}: {err}"),
    }
}

fn final_log_success_data(log: &str) -> Option<String> {
    let hash = hash_signed_log_content(log)?;
    Some(format!("\nSHA256: {hash}\n"))
}

fn hash_signed_log_content(log: &str) -> Option<String> {
    let first_separator = log.find('=')?;
    let last_separator = log.rfind('=')?;
    if first_separator > last_separator {
        return None;
    }

    let signed_content = &log.as_bytes()[first_separator..=last_separator];
    let digest = Sha256::digest(signed_content);
    Some(format!("{digest:x}"))
}
