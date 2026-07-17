use std::process::{ExitStatus, Stdio};

use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
    task::JoinSet,
};

use crate::jobs::JobLogger;

#[derive(Debug, Error)]
pub enum CommandRunnerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("log reader task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("command output pipe was not available")]
    MissingOutputPipe,
}

pub struct LoggedCommandOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

pub async fn run_and_log(
    command: &mut Command,
    logger: &JobLogger,
) -> Result<LoggedCommandOutput, CommandRunnerError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    log_command(command, logger);

    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or(CommandRunnerError::MissingOutputPipe)?;
    let stderr = child
        .stderr
        .take()
        .ok_or(CommandRunnerError::MissingOutputPipe)?;

    let mut readers = JoinSet::new();
    readers.spawn(read_and_log(stdout, logger.clone(), OutputStream::Stdout));
    readers.spawn(read_and_log(stderr, logger.clone(), OutputStream::Stderr));

    let mut stdout = String::new();
    let mut stderr = String::new();
    while let Some(result) = readers.join_next().await {
        let (stream, output) = result??;
        match stream {
            OutputStream::Stdout => stdout.push_str(&output),
            OutputStream::Stderr => stderr.push_str(&output),
        }
    }

    Ok(LoggedCommandOutput {
        status: child.wait().await?,
        stdout,
        stderr,
    })
}

fn log_command(command: &Command, logger: &JobLogger) {
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
    logger.write(run_log);
}

#[derive(Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

async fn read_and_log<R>(
    reader: R,
    logger: JobLogger,
    stream: OutputStream,
) -> Result<(OutputStream, String), std::io::Error>
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut output = String::new();
    let mut bytes = Vec::new();

    while reader.read_until(b'\n', &mut bytes).await? != 0 {
        let chunk = String::from_utf8_lossy(&bytes).into_owned();
        output.push_str(&chunk);
        logger.write(chunk);
        bytes.clear();
    }

    Ok((stream, output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn logs_and_collects_both_output_streams() {
        let logger = JobLogger::new(String::new());
        let mut receiver = logger.subscribe();
        let mut command = Command::new("sh");
        command.args(["-c", "printf stdout; printf stderr >&2"]);

        let output = run_and_log(&mut command, &logger).await.unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, "stdout");
        assert_eq!(output.stderr, "stderr");
        assert!(receiver.recv().await.unwrap().starts_with("> sh -c"));
        let first_output = receiver.recv().await.unwrap();
        let second_output = receiver.recv().await.unwrap();
        assert!(matches!(first_output.as_str(), "stdout" | "stderr"));
        assert!(matches!(second_output.as_str(), "stdout" | "stderr"));
        assert_ne!(first_output, second_output);
    }
}
