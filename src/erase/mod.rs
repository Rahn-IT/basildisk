use hdparm::{Hdparm, HdparmError};
use serde::Serialize;
use thiserror::Error;

use crate::disk_info::{ConnectionType, Disk, DiskType};

mod hdparm;

#[derive(Serialize)]
pub enum EraseType {
    None,
    BlockOverride,
    AtaSecureErase,
    AtaEnhancedSecureErase,
}

#[derive(Debug, Error)]
pub enum GetEraseTypeError {
    #[error("error during hdparm: {0}")]
    Hdparm(#[from] HdparmError),
    #[error("Not implemented yet")]
    Todo,
}

impl EraseType {
    pub async fn get_for_disk(
        device: &str,
        connection_type: ConnectionType,
        disk_type: DiskType,
    ) -> Result<Self, GetEraseTypeError> {
        match connection_type {
            ConnectionType::SATA => {
                if disk_type != DiskType::SSD {
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
