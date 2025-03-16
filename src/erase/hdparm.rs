use std::{process::Stdio, string::FromUtf8Error};

use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::broadcast,
    task::JoinSet,
};

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

#[derive(Debug, Error)]
pub enum AtaSecureEraseError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("error parsing output: {0}")]
    UTF8(#[from] FromUtf8Error),
    #[error("hdparm exited with code {0}")]
    ExitCode(i32),
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

    pub async fn ata_secure_erase_disk_enhanced(
        device: String,
        logger: &broadcast::Sender<String>,
    ) -> Result<(), AtaSecureEraseError> {
        // Set Device Password
        let command = "hdparm";
        let args = ["--user-master", "u", "--security-set-pass", "p"];
        let device_path = format!("/dev/{device}");
        let output = tokio::process::Command::new(command)
            .args(args)
            .arg(&device_path)
            .output()
            .await?;

        let run_log = format!("> {} {} {}", command, args.join(" "), &device_path);
        logger.send(run_log);

        let error = String::from_utf8(output.stderr)?;
        let output = String::from_utf8(output.stdout)?;

        logger.send(output);
        logger.send(error);

        // Erase Device

        let command = "hdparm";
        let args = ["--user-master", "u", "--security-erase-enhanced", "p"];
        let device_path = format!("/dev/{device}");
        let mut child = tokio::process::Command::new(command)
            .args(args)
            .arg(&device_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let run_log = format!("> {} {} {}", command, args.join(" "), &device_path);
        logger.send(run_log);

        let mut joinset = JoinSet::new();

        let stdout = child.stdout.take().unwrap();
        let logger2 = logger.clone();
        joinset.spawn(async move {
            let mut stdout = BufReader::new(stdout);
            let mut buf = Vec::new();
            while let Ok(bytes) = stdout.read_until(b'\n', &mut buf).await {
                if bytes == 0 {
                    break;
                }

                logger2.send(String::from_utf8_lossy(&buf[..bytes]).to_string());
            }
        });

        let stderr = child.stderr.take().unwrap();
        let logger2 = logger.clone();
        joinset.spawn(async move {
            let mut stdout = BufReader::new(stderr);
            let mut buf = Vec::new();
            while let Ok(bytes) = stdout.read_until(b'\n', &mut buf).await {
                if bytes == 0 {
                    break;
                }

                logger2.send(String::from_utf8_lossy(&buf[..bytes]).to_string());
            }
        });

        joinset.join_all().await;

        if let Some(code) = child.wait().await?.code() {
            Err(AtaSecureEraseError::ExitCode(code))
        } else {
            Ok(())
        }
    }
}
