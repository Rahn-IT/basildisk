use std::fmt::Display;

use hdparm::{Hdparm, HdparmError};
use serde::Serialize;
use shred::Shred;
use thiserror::Error;
use tokio::sync::broadcast;

use crate::{
    disk_info::{ConnectionType, DiskType},
    jobs::Job,
};

pub mod hdparm;
pub mod shred;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EraseType {
    None,
    BlockOverride,
    AtaSecureErase,
    AtaEnhancedSecureErase,
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
        }
    }
}

#[derive(Debug, Error)]
pub enum GetEraseTypeError {
    #[error("error during hdparm: {0}")]
    Hdparm(#[from] HdparmError),
}

impl EraseType {
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

    async fn run(
        self,
        logger: broadcast::Sender<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send>> {
        let intro = format!(
            "
Starting Secure Disk Erasure
=================================================
Model: {}
Serial: {}
Device Name: {}
=================================================
Connected via: {}
Detected Disk Type: {}
Selected Erasure Method: {}
=================================================
",
            self.model,
            self.serial,
            self.device,
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
            EraseType::None => Ok(()),
        };

        let outro = if let Err(err) = &result {
            format!(
                "
=================================================
Errors detected during secure erase!
=================================================
{err}
"
            )
        } else {
            "
=================================================
Secure Erase was successful!
=================================================
            "
            .to_string()
        };

        logger.send(outro).unwrap();

        result
    }
}
