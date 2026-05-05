use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::mount;

#[derive(Debug, Error)]
pub enum BrowseError {
    #[error("path escapes mount root")]
    EscapesRoot,
    #[error("path is not a file or folder")]
    NotDownloadable,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("mount error: {0}")]
    Mount(#[from] mount::MountError),
}

#[derive(Debug, Serialize)]
pub struct BrowseView {
    pub is_admin: bool,
    pub device: String,
    pub path: String,
    pub parent_url: Option<String>,
    pub entries: Vec<BrowseEntry>,
}

#[derive(Debug, Serialize)]
pub struct BrowseEntry {
    pub name: String,
    pub href: String,
    pub download_href: String,
    pub is_dir: bool,
    pub is_downloadable: bool,
    pub kind: String,
    pub size_display: String,
}

#[derive(Debug)]
pub struct DownloadEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: DownloadKind,
    pub size: u64,
}

#[derive(Debug)]
pub enum DownloadKind {
    File,
    Folder,
}

pub async fn list(
    device: &str,
    relative_path: &str,
    is_admin: bool,
) -> Result<BrowseView, BrowseError> {
    let root = mount::mount_point_for_device(device)?;
    let root = tokio::fs::canonicalize(root).await?;
    let target = resolve_child_path(&root, relative_path).await?;

    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(&target).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        let file_name = entry.file_name().to_string_lossy().to_string();
        let metadata = tokio::fs::symlink_metadata(entry.path()).await?;
        let file_type = metadata.file_type();
        let is_dir = metadata.is_dir();
        let is_file = metadata.is_file();
        let is_symlink = file_type.is_symlink();
        let entry_relative_path = append_relative_path(relative_path, &file_name);

        entries.push(BrowseEntry {
            name: file_name,
            href: browse_url(device, &entry_relative_path),
            download_href: download_url(device, &entry_relative_path),
            is_dir,
            is_downloadable: is_dir || is_file,
            kind: entry_kind(is_dir, is_file, is_symlink).to_string(),
            size_display: if is_dir {
                String::new()
            } else {
                format_size(metadata.len())
            },
        });
    }

    entries.sort_by(|left, right| {
        let left_rank = if left.kind == "Folder" { 0 } else { 1 };
        let right_rank = if right.kind == "Folder" { 0 } else { 1 };
        left_rank
            .cmp(&right_rank)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });

    Ok(BrowseView {
        is_admin,
        device: device.to_string(),
        path: display_path(device, relative_path),
        parent_url: parent_url(device, relative_path),
        entries,
    })
}

pub async fn download(device: &str, relative_path: &str) -> Result<DownloadEntry, BrowseError> {
    let root = mount::mount_point_for_device(device)?;
    let root = tokio::fs::canonicalize(root).await?;
    let target = resolve_child_path(&root, relative_path).await?;
    let metadata = tokio::fs::metadata(&target).await?;

    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| device.to_string());

    let kind = if metadata.is_file() {
        DownloadKind::File
    } else if metadata.is_dir() {
        DownloadKind::Folder
    } else {
        return Err(BrowseError::NotDownloadable);
    };

    Ok(DownloadEntry {
        path: target,
        name,
        kind,
        size: metadata.len(),
    })
}

async fn resolve_child_path(root: &Path, relative_path: &str) -> Result<PathBuf, BrowseError> {
    let target = if relative_path.trim_matches('/').is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative_path.trim_matches('/'))
    };

    let target = tokio::fs::canonicalize(target).await?;
    if !target.starts_with(root) {
        return Err(BrowseError::EscapesRoot);
    }

    Ok(target)
}

fn append_relative_path(current: &str, child: &str) -> String {
    let current = current.trim_matches('/');
    if current.is_empty() {
        child.to_string()
    } else {
        format!("{current}/{child}")
    }
}

fn parent_url(device: &str, relative_path: &str) -> Option<String> {
    let relative_path = relative_path.trim_matches('/');
    if relative_path.is_empty() {
        return None;
    }

    let parent = Path::new(relative_path).parent()?;
    let parent = parent.to_string_lossy();
    if parent.is_empty() {
        Some(format!("/browse/{}", percent_encode_path_segment(device)))
    } else {
        Some(browse_url(device, &parent))
    }
}

fn browse_url(device: &str, relative_path: &str) -> String {
    entry_url("browse", device, relative_path)
}

fn download_url(device: &str, relative_path: &str) -> String {
    entry_url("download", device, relative_path)
}

fn entry_url(route: &str, device: &str, relative_path: &str) -> String {
    let encoded_device = percent_encode_path_segment(device);
    let encoded_path = percent_encode_relative_path(relative_path);
    if encoded_path.is_empty() {
        format!("/{route}/{encoded_device}")
    } else {
        format!("/{route}/{encoded_device}/{encoded_path}")
    }
}

fn percent_encode_relative_path(path: &str) -> String {
    path.trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(percent_encode_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_encode_path_segment(segment: &str) -> String {
    segment
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn entry_kind(is_dir: bool, is_file: bool, is_symlink: bool) -> &'static str {
    if is_symlink {
        "Symlink"
    } else if is_dir {
        "Folder"
    } else if is_file {
        "File"
    } else {
        "Other"
    }
}

fn display_path(device: &str, relative_path: &str) -> String {
    let relative_path = relative_path.trim_matches('/');
    if relative_path.is_empty() {
        format!("/mnt/{device}")
    } else {
        format!("/mnt/{device}/{relative_path}")
    }
}

fn format_size(size: u64) -> String {
    if size > 1_000_000_000 {
        format!("{} GB", size / 1_000_000_000)
    } else if size > 1_000_000 {
        format!("{} MB", size / 1_000_000)
    } else if size > 1000 {
        format!("{} KB", size / 1000)
    } else {
        format!("{} B", size)
    }
}
