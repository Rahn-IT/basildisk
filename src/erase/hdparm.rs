use std::string::FromUtf8Error;

use thiserror::Error;

pub struct Hdparm {
    pub frozen: bool,
    pub secure_erase: bool,
    pub enhanced_secure_erase: bool,
}

#[derive(Debug, Error)]
pub enum HdparmError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("error parsing output: {0}")]
    UTF8(#[from] FromUtf8Error),
}

impl Hdparm {
    pub async fn get_for_disk(device: &str) -> Result<Self, HdparmError> {
        let output = tokio::process::Command::new("hdparm")
            .arg("-I")
            .arg(format!("/dev/{}", device))
            .output()
            .await?;

        let output = String::from_utf8(output.stdout)?;

        let mut hdparm = Self {
            frozen: false,
            secure_erase: false,
            enhanced_secure_erase: false,
        };

        for line in output
            .lines()
            .skip_while(|line| !line.starts_with("Security:"))
            .skip(1)
        {
            if line.starts_with("Device Sleep:") {
                break;
            }

            let mut line = line.trim();

            let active = !line.starts_with("not");

            if !active {
                line = line[3..].trim();
            }

            match line {
                "frozen" => hdparm.frozen = active,
                "supported: enhanced erase" => hdparm.enhanced_secure_erase = active,
                _ => (),
            };
        }

        Ok(hdparm)
    }
}
