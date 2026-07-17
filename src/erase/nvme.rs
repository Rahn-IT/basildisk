use std::{process::Stdio, string::FromUtf8Error, time::Duration};

use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::broadcast,
    task::JoinSet,
    time::Instant,
};

pub struct Nvme {
    pub format_nvm: bool,
    pub format_crypto_erase: bool,
    pub sanitize_crypto_erase: bool,
    pub sanitize_block_erase: bool,
    pub sanitize_overwrite: bool,
}

#[derive(Debug, Error)]
pub enum NvmeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("error parsing output: {0}")]
    UTF8(#[from] FromUtf8Error),
    #[error("nvme command exited with code {0}")]
    ExitCode(i32),
    #[error("nvme command terminated without an exit code: {0}")]
    Terminated(std::process::ExitStatus),
    #[error("nvme sanitize failed: {0}")]
    SanitizeFailed(String),
    #[error("nvme sanitize did not complete within {0:?}")]
    SanitizeTimeout(Duration),
}

impl Nvme {
    pub async fn get_for_disk(device: &str) -> Result<Self, NvmeError> {
        let controller = controller_for_device(device);
        let output = tokio::process::Command::new("nvme")
            .arg("id-ctrl")
            .arg(format!("/dev/{controller}"))
            .arg("-H")
            .output()
            .await?;

        let output = String::from_utf8(output.stdout)?;

        Ok(Self {
            format_nvm: has_supported_line(&output, "Format NVM Supported"),
            format_crypto_erase: has_supported_line(
                &output,
                "Crypto Erase Supported as part of Secure Erase",
            ),
            sanitize_crypto_erase: has_supported_line(
                &output,
                "Crypto Erase Sanitize Operation Supported",
            ),
            sanitize_block_erase: has_supported_line(
                &output,
                "Block Erase Sanitize Operation Supported",
            ),
            sanitize_overwrite: has_supported_line(
                &output,
                "Overwrite Sanitize Operation Supported",
            ),
        })
    }

    pub async fn sanitize_crypto_erase_disk(
        device: String,
        logger: &broadcast::Sender<String>,
    ) -> Result<(), NvmeError> {
        sanitize_disk(device, logger, "0x04").await
    }

    pub async fn sanitize_block_erase_disk(
        device: String,
        logger: &broadcast::Sender<String>,
    ) -> Result<(), NvmeError> {
        sanitize_disk(device, logger, "0x02").await
    }

    pub async fn sanitize_overwrite_disk(
        device: String,
        logger: &broadcast::Sender<String>,
    ) -> Result<(), NvmeError> {
        sanitize_disk(device, logger, "0x03").await
    }

    pub async fn format_crypto_erase_disk(
        device: String,
        logger: &broadcast::Sender<String>,
    ) -> Result<(), NvmeError> {
        format_disk(device, logger, "2").await
    }

    pub async fn format_user_data_erase_disk(
        device: String,
        logger: &broadcast::Sender<String>,
    ) -> Result<(), NvmeError> {
        format_disk(device, logger, "1").await
    }
}

async fn sanitize_disk(
    device: String,
    logger: &broadcast::Sender<String>,
    sanact: &str,
) -> Result<(), NvmeError> {
    let controller = controller_for_device(&device);
    let mut command = Command::new("nvme");
    command
        .arg("sanitize")
        .arg(format!("/dev/{controller}"))
        .arg(format!("--sanact={sanact}"))
        .arg("--force");

    run_and_log(&mut command, logger).await?;
    poll_sanitize_log(&controller, logger).await
}

async fn format_disk(
    device: String,
    logger: &broadcast::Sender<String>,
    ses: &str,
) -> Result<(), NvmeError> {
    let mut command = Command::new("nvme");
    command
        .arg("format")
        .arg(format!("/dev/{device}"))
        .arg(format!("--ses={ses}"))
        .arg("--force");

    run_and_log(&mut command, logger).await.map(|_| ())
}

async fn poll_sanitize_log(
    controller: &str,
    logger: &broadcast::Sender<String>,
) -> Result<(), NvmeError> {
    let mut delay = Duration::from_secs(5);
    let max_delay = Duration::from_secs(60);
    let timeout = Duration::from_secs(6 * 60 * 60);
    let started_at = Instant::now();

    loop {
        if started_at.elapsed() >= timeout {
            return Err(NvmeError::SanitizeTimeout(timeout));
        }

        tokio::time::sleep(delay).await;

        let mut command = Command::new("nvme");
        command
            .arg("sanitize-log")
            .arg(format!("/dev/{controller}"))
            .arg("-H");
        let output = run_and_log(&mut command, logger).await?;
        let sanitize_log = format!("{}{}", output.stdout, output.stderr);

        match sanitize_status(&sanitize_log) {
            SanitizeStatus::Success => return Ok(()),
            SanitizeStatus::Failed(message) => return Err(NvmeError::SanitizeFailed(message)),
            SanitizeStatus::InProgress | SanitizeStatus::Unknown => {
                delay = delay.saturating_mul(2).min(max_delay);
            }
        }
    }
}

struct LoggedOutput {
    stdout: String,
    stderr: String,
}

async fn run_and_log(
    command: &mut Command,
    logger: &broadcast::Sender<String>,
) -> Result<LoggedOutput, NvmeError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

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
    joinset.spawn(read_and_log(stdout, logger.clone(), StreamKind::Stdout));

    let stderr = child.stderr.take().unwrap();
    joinset.spawn(read_and_log(stderr, logger.clone(), StreamKind::Stderr));

    let mut stdout = String::new();
    let mut stderr = String::new();
    while let Some(result) = joinset.join_next().await {
        match result {
            Ok(StreamOutput {
                kind: StreamKind::Stdout,
                output,
            }) => stdout.push_str(&output),
            Ok(StreamOutput {
                kind: StreamKind::Stderr,
                output,
            }) => stderr.push_str(&output),
            Err(err) => stderr.push_str(&format!("log reader task failed: {err}\n")),
        }
    }

    let status = child.wait().await?;
    match status.code() {
        Some(0) => {}
        Some(code) => return Err(NvmeError::ExitCode(code)),
        None => return Err(NvmeError::Terminated(status)),
    }

    Ok(LoggedOutput { stdout, stderr })
}

#[derive(Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
}

struct StreamOutput {
    kind: StreamKind,
    output: String,
}

async fn read_and_log<R>(
    reader: R,
    logger: broadcast::Sender<String>,
    kind: StreamKind,
) -> StreamOutput
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader).lines();
    let mut output = String::new();

    while let Ok(Some(mut line)) = reader.next_line().await {
        line.push('\n');
        output.push_str(&line);
        let _ = logger.send(line);
    }

    StreamOutput { kind, output }
}

#[derive(Debug, PartialEq, Eq)]
enum SanitizeStatus {
    InProgress,
    Success,
    Failed(String),
    Unknown,
}

fn sanitize_status(output: &str) -> SanitizeStatus {
    if let Some(sstat) = sanitize_sstat(output) {
        return match sstat & 0x7 {
            0x1 | 0x4 => SanitizeStatus::Success,
            0x2 => SanitizeStatus::InProgress,
            0x3 => SanitizeStatus::Failed(format!("SSTAT reports failure: 0x{sstat:x}")),
            _ => SanitizeStatus::Unknown,
        };
    }

    let normalized = output.to_ascii_lowercase();
    if normalized.contains("success") || normalized.contains("completed successfully") {
        return SanitizeStatus::Success;
    }

    if normalized.contains("failed")
        || normalized.contains("failure")
        || normalized.contains("unsuccessful")
    {
        return SanitizeStatus::Failed("sanitize-log reports failure".to_string());
    }

    if normalized.contains("in progress")
        || normalized.contains("progress")
        || normalized.contains("sprog")
    {
        return SanitizeStatus::InProgress;
    }

    SanitizeStatus::Unknown
}

fn sanitize_sstat(output: &str) -> Option<u16> {
    output.lines().find_map(|line| {
        if !line.to_ascii_lowercase().contains("sstat") {
            return None;
        }

        let value = line.split(':').nth(1)?.split_whitespace().next()?;
        if let Some(hex) = value.strip_prefix("0x") {
            u16::from_str_radix(hex, 16).ok()
        } else {
            value.parse().ok()
        }
    })
}

fn controller_for_device(device: &str) -> String {
    device
        .rsplit_once('n')
        .filter(|(_, namespace)| {
            !namespace.is_empty()
                && namespace
                    .chars()
                    .all(|character| character.is_ascii_digit())
        })
        .map(|(controller, _)| controller)
        .filter(|controller| controller.starts_with("nvme") && !controller.is_empty())
        .unwrap_or(device)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_for_namespace_device() {
        assert_eq!(controller_for_device("nvme0n1"), "nvme0");
        assert_eq!(controller_for_device("nvme10n2"), "nvme10");
        assert_eq!(controller_for_device("nvme10n12"), "nvme10");
    }

    #[test]
    fn controller_for_controller_device_is_unchanged() {
        assert_eq!(controller_for_device("nvme0"), "nvme0");
        assert_eq!(controller_for_device("nvme10"), "nvme10");
    }

    #[test]
    fn non_nvme_device_is_unchanged() {
        assert_eq!(controller_for_device("sda"), "sda");
        assert_eq!(controller_for_device("mapper/data"), "mapper/data");
    }

    #[test]
    fn malformed_namespace_suffix_is_unchanged() {
        assert_eq!(controller_for_device("nvme0n"), "nvme0n");
        assert_eq!(controller_for_device("nvme0nx"), "nvme0nx");
    }

    #[test]
    fn parses_sanitize_sstat_hex() {
        assert_eq!(
            sanitize_sstat("Sanitize Status                        (SSTAT) :  0x101"),
            Some(0x101)
        );
        assert_eq!(
            sanitize_sstat("Sanitize Status                        (SSTAT) :  0x2"),
            Some(0x2)
        );
    }

    #[test]
    fn parses_sanitize_sstat_decimal() {
        assert_eq!(
            sanitize_sstat("Sanitize Status                        (SSTAT) :  2"),
            Some(2)
        );
    }

    #[test]
    fn parses_sanitize_status_from_low_sstat_bits() {
        assert_eq!(
            sanitize_status("Sanitize Status                        (SSTAT) :  0x101"),
            SanitizeStatus::Success
        );
        assert_eq!(
            sanitize_status("Sanitize Status                        (SSTAT) :  0x2"),
            SanitizeStatus::InProgress
        );
        assert_eq!(
            sanitize_status("Sanitize Status                        (SSTAT) :  0x3"),
            SanitizeStatus::Failed("SSTAT reports failure: 0x3".to_string())
        );
    }
}

fn has_supported_line(output: &str, label: &str) -> bool {
    output
        .lines()
        .filter(|line| line.contains(label))
        .any(|line| {
            let line = line.trim();
            (line.contains(": 0x1") || line.contains(": 1")) && !line.contains("Not Supported")
        })
}
