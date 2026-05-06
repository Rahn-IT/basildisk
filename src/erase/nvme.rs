use std::string::FromUtf8Error;

use thiserror::Error;

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
