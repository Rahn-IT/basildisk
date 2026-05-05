use std::path::{Path, PathBuf};

use async_zip::{
    Compression, DeflateOption, ZipEntryBuilder, base::write::ZipFileWriter,
    tokio::write::ZipFileWriter as TokioZipFileWriter,
};
use tokio::io::{BufReader, DuplexStream};
use tokio_util::compat::FuturesAsyncWriteCompatExt;

pub const STREAM_BUFFER_SIZE: usize = 1024 * 1024;

pub fn folder_reader(folder: PathBuf) -> DuplexStream {
    let (reader, writer) = tokio::io::duplex(STREAM_BUFFER_SIZE);

    tokio::spawn(async move {
        if let Err(err) = write_folder_zip(&folder, writer).await {
            eprintln!(
                "Error streaming ZIP download for {}: {err}",
                folder.display()
            );
        }
    });

    reader
}

async fn write_folder_zip(folder: &Path, writer: DuplexStream) -> anyhow::Result<()> {
    let root = tokio::fs::canonicalize(folder).await?;
    let archive_root = root.parent().unwrap_or(&root).to_path_buf();
    let mut zip = ZipFileWriter::with_tokio(writer).force_zip64();

    add_paths_to_zip(&mut zip, &root, &archive_root, &root).await?;
    zip.close().await?;

    Ok(())
}

async fn add_paths_to_zip(
    zip: &mut TokioZipFileWriter<DuplexStream>,
    root: &Path,
    archive_root: &Path,
    allowed_root: &Path,
) -> anyhow::Result<()> {
    let mut pending = vec![root.to_path_buf()];

    while let Some(path) = pending.pop() {
        let metadata = tokio::fs::symlink_metadata(&path).await?;
        if metadata.file_type().is_symlink() {
            continue;
        }

        let canonical_path = tokio::fs::canonicalize(&path).await?;
        if !canonical_path.starts_with(allowed_root) {
            continue;
        }

        let archive_name = zip_archive_name(&path, archive_root)?;
        if metadata.is_dir() {
            add_directory_to_zip(zip, &archive_name).await?;

            let mut children = Vec::new();
            let mut entries = tokio::fs::read_dir(&path).await?;
            while let Some(entry) = entries.next_entry().await? {
                children.push(entry.path());
            }
            children.sort_by_key(|path| path.file_name().map(|name| name.to_os_string()));
            pending.extend(children.into_iter().rev());
        } else if metadata.is_file() {
            add_file_to_zip(zip, &path, &archive_name).await?;
        }
    }

    Ok(())
}

async fn add_directory_to_zip(
    zip: &mut TokioZipFileWriter<DuplexStream>,
    archive_name: &str,
) -> anyhow::Result<()> {
    let entry = ZipEntryBuilder::new(format!("{archive_name}/").into(), Compression::Stored);
    zip.write_entry_whole(entry, &[]).await?;
    Ok(())
}

async fn add_file_to_zip(
    zip: &mut TokioZipFileWriter<DuplexStream>,
    path: &Path,
    archive_name: &str,
) -> anyhow::Result<()> {
    let entry = ZipEntryBuilder::new(archive_name.to_string().into(), Compression::Deflate)
        .deflate_option(DeflateOption::Fast);
    let mut entry_writer = zip.write_entry_stream(entry).await?;
    let file = tokio::fs::File::open(path).await?;
    let mut reader = BufReader::with_capacity(STREAM_BUFFER_SIZE, file);
    let mut writer = (&mut entry_writer).compat_write();
    tokio::io::copy_buf(&mut reader, &mut writer).await?;
    tokio::io::AsyncWriteExt::shutdown(&mut writer).await?;

    entry_writer.close().await?;
    Ok(())
}

fn zip_archive_name(path: &Path, archive_root: &Path) -> anyhow::Result<String> {
    let relative_path = path.strip_prefix(archive_root)?;
    Ok(relative_path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}
