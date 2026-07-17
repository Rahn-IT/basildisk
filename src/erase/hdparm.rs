use std::string::FromUtf8Error;

use thiserror::Error;
use tokio::sync::broadcast;

use super::command_runner::{self, CommandRunnerError};

pub struct Hdparm {
    pub frozen: bool,
    pub security_enabled: bool,
    pub security_locked: bool,
    pub secure_erase: bool,
    pub enhanced_secure_erase: bool,
}

#[derive(Debug, Error)]
pub enum HdparmError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("error parsing output: {0}")]
    UTF8(#[from] FromUtf8Error),
    #[error("hdparm exited with code {0}")]
    ExitCode(i32),
    #[error("hdparm terminated without an exit code: {0}")]
    Terminated(std::process::ExitStatus),
    #[error("hdparm reported an SG_IO diagnostic: {0}")]
    SgIoDiagnostic(String),
}

#[derive(Debug, Error)]
pub enum AtaSecureEraseError {
    #[error("error running hdparm: {0}")]
    CommandRunner(#[from] CommandRunnerError),
    #[error("hdparm exited with code {0}")]
    ExitCode(i32),
    #[error("hdparm terminated without an exit code: {0}")]
    Terminated(std::process::ExitStatus),
    #[error("error checking ATA security state: {0}")]
    SecurityState(#[from] HdparmError),
    #[error("hdparm reported an SG_IO diagnostic: {0}")]
    SgIoDiagnostic(String),
    #[error("unexpected ATA security state after {0}")]
    UnexpectedSecurityState(&'static str),
}

impl Hdparm {
    pub async fn get_for_disk(device: &str) -> Result<Self, HdparmError> {
        let output = tokio::process::Command::new("hdparm")
            .arg("-I")
            .arg(format!("/dev/{}", device))
            .output()
            .await?;

        if !output.status.success() {
            return Err(hdparm_status_error(output.status));
        }

        let stderr = String::from_utf8(output.stderr)?;
        let output = String::from_utf8(output.stdout)?;
        reject_sg_io_diagnostic(&output, &stderr)?;

        Ok(Self::parse_identify_output(&output))
    }

    fn parse_identify_output(output: &str) -> Self {
        let mut hdparm = Self {
            frozen: false,
            security_enabled: false,
            security_locked: false,
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
                "enabled" => hdparm.security_enabled = active,
                "locked" => hdparm.security_locked = active,
                "supported: enhanced erase" => hdparm.enhanced_secure_erase = active,
                _ => (),
            };
        }

        hdparm
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
            .arg(format!("/dev/{device}"));

        let output = command_runner::run_and_log(&mut command, logger).await?;
        ensure_ata_command_succeeded(output.status)?;
        reject_ata_sg_io_diagnostic(&output.stdout, &output.stderr)?;

        let security = Self::get_for_disk(&device).await?;
        if !security.security_enabled || security.security_locked {
            return Err(AtaSecureEraseError::UnexpectedSecurityState(
                "setting the security password",
            ));
        }

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
            .arg(format!("/dev/{device}"));

        let output = command_runner::run_and_log(&mut command, logger).await?;
        ensure_ata_command_succeeded(output.status)?;
        reject_ata_sg_io_diagnostic(&output.stdout, &output.stderr)?;

        let security = Self::get_for_disk(&device).await?;
        if security.security_enabled || security.security_locked {
            return Err(AtaSecureEraseError::UnexpectedSecurityState("secure erase"));
        }

        Ok(())
    }
}

fn hdparm_status_error(status: std::process::ExitStatus) -> HdparmError {
    match status.code() {
        Some(code) => HdparmError::ExitCode(code),
        None => HdparmError::Terminated(status),
    }
}

fn ensure_ata_command_succeeded(
    status: std::process::ExitStatus,
) -> Result<(), AtaSecureEraseError> {
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(AtaSecureEraseError::ExitCode(code)),
        None => Err(AtaSecureEraseError::Terminated(status)),
    }
}

fn reject_sg_io_diagnostic(output: &str, stderr: &str) -> Result<(), HdparmError> {
    match sg_io_diagnostic(output, stderr) {
        Some(diagnostic) => Err(HdparmError::SgIoDiagnostic(diagnostic)),
        None => Ok(()),
    }
}

fn reject_ata_sg_io_diagnostic(output: &str, stderr: &str) -> Result<(), AtaSecureEraseError> {
    match sg_io_diagnostic(output, stderr) {
        Some(diagnostic) => Err(AtaSecureEraseError::SgIoDiagnostic(diagnostic)),
        None => Ok(()),
    }
}

fn sg_io_diagnostic(output: &str, stderr: &str) -> Option<String> {
    output
        .lines()
        .chain(stderr.lines())
        .find(|line| line.contains("SG_IO:"))
        .map(|line| line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ata_security_state() {
        let output = r#"
Security:
		supported
		enabled
		not	locked
		not	frozen
		2min for SECURITY ERASE UNIT. 2min for ENHANCED SECURITY ERASE UNIT.
		supported: enhanced erase
Device Sleep:
"#;

        let hdparm = Hdparm::parse_identify_output(output);

        assert!(hdparm.security_enabled);
        assert!(!hdparm.security_locked);
        assert!(!hdparm.frozen);
        assert!(hdparm.secure_erase);
        assert!(hdparm.enhanced_secure_erase);
    }

    #[test]
    fn parses_security_disabled_after_erase() {
        let output = r#"
Security:
		not	enabled
		not	locked
Device Sleep:
"#;

        let hdparm = Hdparm::parse_identify_output(output);

        assert!(!hdparm.security_enabled);
        assert!(!hdparm.security_locked);
    }

    #[test]
    fn detects_sg_io_diagnostic() {
        assert_eq!(
            sg_io_diagnostic("", "SG_IO: bad/missing sense data"),
            Some("SG_IO: bad/missing sense data".to_string())
        );
        assert_eq!(sg_io_diagnostic("completed", ""), None);
    }
}
