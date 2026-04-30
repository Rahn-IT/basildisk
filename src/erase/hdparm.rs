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

            if line.contains("min for SECURITY ERASE UNIT") {
                hdparm.secure_erase = true;
                continue;
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

    pub async fn ata_secure_erase_disk(
        device: String,
        logger: &broadcast::Sender<String>,
        enhanced: bool,
    ) -> Result<(), AtaSecureEraseError> {
        // Set Device Password
        let mut command = tokio::process::Command::new("hdparm");
        command
            .arg("--user-master")
            .arg("--security-set-pass")
            .arg("p")
            .arg(format!("/dev/{device}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let std_cmd = command.as_std();

        let run_log = format!(
            "> {} {}\n",
            std_cmd.get_program().to_string_lossy(),
            std_cmd
                .get_args()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
        );
        let _ = logger.send(run_log);
        let output = command.output().await?;

        let error = String::from_utf8(output.stderr)?;
        let output = String::from_utf8(output.stdout)?;

        let _ = logger.send(output);
        let _ = logger.send(error);

        // Erase Device

        let erase_arg = if enhanced {
            "--security-erase-enhanced"
        } else {
            "--security-erase"
        };

        let mut command = tokio::process::Command::new("hdparm");
        command
            .arg("--user-master")
            .arg(erase_arg)
            .arg("p")
            .arg(format!("/dev/{device}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let std_cmd = command.as_std();

        let run_log = format!(
            "> {} {}\n",
            std_cmd.get_program().to_string_lossy(),
            std_cmd
                .get_args()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
        );
        let _ = logger.send(run_log);

        let mut child = command.spawn()?;

        let mut joinset = JoinSet::new();

        let stdout = child.stdout.take().unwrap();
        let logger2 = logger.clone();
        joinset.spawn(async move {
            let mut stdout = BufReader::new(stdout).lines();
            while let Ok(Some(mut line)) = stdout.next_line().await {
                line.push('\n');
                let _ = logger2.send(line);
            }
        });

        let stderr = child.stderr.take().unwrap();
        let logger2 = logger.clone();
        joinset.spawn(async move {
            let mut stderr = BufReader::new(stderr).lines();
            while let Ok(Some(mut line)) = stderr.next_line().await {
                line.push('\n');
                let _ = logger2.send(line);
            }
        });

        joinset.join_all().await;

        if let Some(code) = child.wait().await?.code() {
            if code == 0 {
                Ok(())
            } else {
                Err(AtaSecureEraseError::ExitCode(code))
            }
        } else {
            Ok(())
        }
    }
}
