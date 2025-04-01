use std::{process::Stdio, string::FromUtf8Error};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    sync::broadcast,
    task::JoinSet,
};

pub struct Shred;

#[derive(Debug, thiserror::Error)]
pub enum ShredError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("error parsing output: {0}")]
    UTF8(#[from] FromUtf8Error),
    #[error("shred exited with code {0}")]
    ExitCode(i32),
}

impl Shred {
    pub async fn override_disk(
        device: String,
        logger: &broadcast::Sender<String>,
    ) -> Result<(), ShredError> {
        let mut command = tokio::process::Command::new("shred");
        command
            .arg("-v")
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
        logger.send(run_log);

        let mut child = command.spawn()?;

        let mut joinset = JoinSet::new();

        let stdout = child.stdout.take().unwrap();
        let logger2 = logger.clone();
        joinset.spawn(async move {
            let mut stdout = BufReader::new(stdout).lines();
            while let Ok(Some(mut line)) = stdout.next_line().await {
                line.push('\n');
                logger2.send(line);
            }
        });

        let stderr = child.stderr.take().unwrap();
        let logger2 = logger.clone();
        joinset.spawn(async move {
            let mut stderr = BufReader::new(stderr).lines();
            while let Ok(Some(mut line)) = stderr.next_line().await {
                line.push('\n');
                logger2.send(line);
            }
        });

        joinset.join_all().await;

        if let Some(code) = child.wait().await?.code() {
            if code == 0 {
                Ok(())
            } else {
                Err(ShredError::ExitCode(code))
            }
        } else {
            Ok(())
        }
    }
}
