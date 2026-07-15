use std::{
    fmt::Display,
    path::{Path, PathBuf},
};

use hdparm::{Hdparm, HdparmError};
use nvme::{Nvme, NvmeError};
use serde::Serialize;
use sha2::{Digest, Sha256};
use shred::Shred;
use thiserror::Error;
use tokio::{process::Command, sync::broadcast};

use crate::{
    disk_info::{ConnectionType, DiskType},
    jobs::{self, FinalLogSuccessData, FinalLogSuccessDataFn, Job},
    timestamp,
};

pub mod hdparm;
pub mod nvme;
pub mod shred;

pub(crate) const SECURE_ERASE_SIGNATURE_EXPLANATION: &str = r#"
Signature explanation:
The SHA256 value above is calculated over the bytes from the first '=' character in this log through the last '=' character in the secure erase footer. This includes the secure erase header, all recorded erase output, and the success footer. It excludes the Basildisk ASCII banner above the header and excludes the SHA256 line, timestamp availability line, TimestampRequestBase64, TimestampResponseBase64, and this explanation text.

The timestamp request is an RFC 3161 TimeStampRequest for the SHA256 digest above. The timestamp response is an RFC 3161 TimeStampResponse from FreeTSA for that request. To verify it manually, decode TimestampRequestBase64 to a .tsq file and TimestampResponseBase64 to a .tsr file, then verify the .tsr against the .tsq with FreeTSA's CA certificate and TSA certificate from https://freetsa.org/. The same two files can also be uploaded to FreeTSA's online verifier.
"#;

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

impl EraseType {
    pub fn can_run(self) -> bool {
        matches!(
            self,
            Self::BlockOverride
                | Self::AtaSecureErase
                | Self::AtaEnhancedSecureErase
                | Self::NvmeSanitizeCryptoErase
                | Self::NvmeSanitizeBlockErase
                | Self::NvmeSanitizeOverwrite
                | Self::NvmeFormatCryptoErase
                | Self::NvmeFormatUserDataErase
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

    fn final_log_success_data(&self) -> Option<FinalLogSuccessDataFn> {
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
            EraseType::NvmeSanitizeCryptoErase => {
                Nvme::sanitize_crypto_erase_disk(self.device, &logger)
                    .await
                    .map_err(|err| {
                        let b: Box<dyn std::error::Error + Send> = Box::new(err);
                        b
                    })
            }
            EraseType::NvmeSanitizeBlockErase => {
                Nvme::sanitize_block_erase_disk(self.device, &logger)
                    .await
                    .map_err(|err| {
                        let b: Box<dyn std::error::Error + Send> = Box::new(err);
                        b
                    })
            }
            EraseType::NvmeSanitizeOverwrite => Nvme::sanitize_overwrite_disk(self.device, &logger)
                .await
                .map_err(|err| {
                    let b: Box<dyn std::error::Error + Send> = Box::new(err);
                    b
                }),
            EraseType::NvmeFormatCryptoErase => {
                Nvme::format_crypto_erase_disk(self.device, &logger)
                    .await
                    .map_err(|err| {
                        let b: Box<dyn std::error::Error + Send> = Box::new(err);
                        b
                    })
            }
            EraseType::NvmeFormatUserDataErase => {
                Nvme::format_user_data_erase_disk(self.device, &logger)
                    .await
                    .map_err(|err| {
                        let b: Box<dyn std::error::Error + Send> = Box::new(err);
                        b
                    })
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
Basildisk was created by Rahn-IT (https://it-rahn.de)
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
Basildisk was created by Rahn-IT (https://it-rahn.de)
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
    let device_path = device_path(device);
    match Command::new("partprobe").arg(&device_path).output().await {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!(
                "partprobe failed for {}: {}{}",
                device_path.display(),
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            );
        }
        Err(err) => println!("partprobe failed for {device}: {err}"),
    }
}

fn device_path(device: &str) -> PathBuf {
    let path = Path::new(device);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new("/dev").join(device)
    }
}

fn final_log_success_data(
    log: String,
) -> std::pin::Pin<Box<dyn Future<Output = Option<FinalLogSuccessData>> + Send>> {
    Box::pin(async move {
        let hash = hash_signed_log_content(&log)?;
        let mut data = format!("\nSHA256: {hash}\n");
        let mut timestamp_request = None;
        let mut timestamp_response = None;

        match timestamp::request_sha256_timestamp(&hash).await {
            Ok(timestamp) => {
                data.push_str("Timestamp: RFC3161 FreeTSA response available\n");
                timestamp_request = Some(timestamp.request);
                timestamp_response = Some(timestamp.response);
            }
            Err(err) => data.push_str(&format!("Timestamp: unavailable ({err})\n")),
        }

        Some(FinalLogSuccessData {
            log: data,
            timestamp_request,
            timestamp_response,
        })
    })
}

fn hash_signed_log_content(log: &str) -> Option<String> {
    let first_separator = log.find('=')?;
    let last_separator = log.rfind('=')?;
    if first_separator > last_separator {
        return None;
    }

    let signed_content = &log.as_bytes()[first_separator..=last_separator];
    let digest = Sha256::digest(signed_content);
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}
