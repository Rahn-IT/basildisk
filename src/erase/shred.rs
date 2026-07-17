use super::command_runner::{self, CommandRunnerError};
use crate::jobs::JobLogger;

pub struct Shred;

#[derive(Debug, thiserror::Error)]
pub enum ShredError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("error running shred: {0}")]
    CommandRunner(#[from] CommandRunnerError),
    #[error("shred exited with code {0}")]
    ExitCode(i32),
    #[error("shred terminated without an exit code: {0}")]
    Terminated(std::process::ExitStatus),
}

impl Shred {
    pub async fn override_disk(device: String, logger: &JobLogger) -> Result<(), ShredError> {
        let mut command = tokio::process::Command::new("shred");
        command.arg("-v").arg(format!("/dev/{device}"));

        let output = command_runner::run_and_log(&mut command, logger).await?;
        let status = output.status;
        match status.code() {
            Some(0) => Ok(()),
            Some(code) => Err(ShredError::ExitCode(code)),
            None => Err(ShredError::Terminated(status)),
        }
    }
}
