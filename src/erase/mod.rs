use std::fmt::{write, Display};

use hdparm::{Hdparm, HdparmError};
use rocket_sync_db_pools::diesel::sql_types::BigInt;
use serde::Serialize;
use thiserror::Error;
use tokio::sync::broadcast;

use crate::{
    disk_info::{ConnectionType, Disk, DiskType},
    jobs::Job,
};

pub mod hdparm;

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
            ConnectionType::Sata => {
                if disk_type != DiskType::Ssd {
                    return Ok(EraseType::None);
                }

                let hdparm = Hdparm::get_for_disk(device).await?;

                Ok(if hdparm.secure_erase {
                    EraseType::AtaSecureErase
                } else if hdparm.enhanced_secure_erase {
                    EraseType::AtaEnhancedSecureErase
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
                Hdparm::ata_secure_erase_disk_enhanced(self.device, &logger)
                    .await
                    .map_err(|err| {
                        let b: Box<dyn std::error::Error + Send> = Box::new(err);
                        b
                    })
            }
            _ => todo!(),
        };

        let outro = if result.is_ok() {
            "
=================================================
Secure Erase was successful!
=================================================
            "
            .to_string()
        } else {
            "
=================================================
Errors detected during secure erase!
=================================================
            "
            .to_string()
        };

        logger.send(outro).unwrap();

        result
    }
}
