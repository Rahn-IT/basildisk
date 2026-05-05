use std::{
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};

use axum::body::Bytes;
use tokio::{io::ReadBuf, sync::mpsc};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

pub fn folder_reader(folder: PathBuf) -> ChannelReader {
    let (sender, receiver) = mpsc::channel(8);
    let error_sender = sender.clone();

    tokio::task::spawn_blocking(move || {
        if let Err(err) = write_folder_zip(&folder, ChannelWriter { sender }) {
            let _ = error_sender.blocking_send(Err(io::Error::other(err.to_string())));
        }
    });

    ChannelReader::new(receiver)
}

fn write_folder_zip(folder: &Path, writer: ChannelWriter) -> anyhow::Result<()> {
    let root = std::fs::canonicalize(folder)?;
    let archive_root = root.parent().unwrap_or(&root).to_path_buf();
    let mut zip = ZipWriter::new_stream(writer).set_auto_large_file();
    add_path_to_zip(&mut zip, &root, &archive_root, &root)?;
    zip.finish()?;
    Ok(())
}

fn add_path_to_zip(
    zip: &mut ZipWriter<zip::write::StreamWriter<ChannelWriter>>,
    path: &Path,
    archive_root: &Path,
    allowed_root: &Path,
) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }

    let canonical_path = std::fs::canonicalize(path)?;
    if !canonical_path.starts_with(allowed_root) {
        return Ok(());
    }

    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(3))
        .unix_permissions(if metadata.is_dir() { 0o755 } else { 0o644 });
    let archive_name = zip_archive_name(path, archive_root)?;

    if metadata.is_dir() {
        zip.add_directory(format!("{archive_name}/"), options)?;
        let mut entries = std::fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            add_path_to_zip(zip, &entry.path(), archive_root, allowed_root)?;
        }
    } else if metadata.is_file() {
        zip.start_file(archive_name, options)?;
        let mut file = std::fs::File::open(path)?;
        let mut buffer = [0; 1024 * 1024];
        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            zip.write_all(&buffer[..bytes_read])?;
        }
    }

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

struct ChannelWriter {
    sender: mpsc::Sender<io::Result<Bytes>>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }

        self.sender
            .blocking_send(Ok(Bytes::copy_from_slice(buffer)))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "download stream closed"))?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct ChannelReader {
    receiver: mpsc::Receiver<io::Result<Bytes>>,
    current: Cursor<Bytes>,
}

impl ChannelReader {
    fn new(receiver: mpsc::Receiver<io::Result<Bytes>>) -> Self {
        Self {
            receiver,
            current: Cursor::new(Bytes::new()),
        }
    }
}

impl tokio::io::AsyncRead for ChannelReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let remaining = self.current.get_ref().len() as u64 - self.current.position();
            if remaining > 0 {
                let bytes_to_copy = remaining.min(output.remaining() as u64) as usize;
                if bytes_to_copy == 0 {
                    return Poll::Ready(Ok(()));
                }

                let position = self.current.position() as usize;
                output.put_slice(&self.current.get_ref()[position..position + bytes_to_copy]);
                self.current.set_position((position + bytes_to_copy) as u64);
                return Poll::Ready(Ok(()));
            }

            match Pin::new(&mut self.receiver).poll_recv(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    self.current = Cursor::new(bytes);
                }
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Err(err)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
