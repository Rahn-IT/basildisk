use async_zip::{Compression, ZipEntryBuilder, base::write::ZipFileWriter};
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sqlx::SqlitePool;
use tokio::io::DuplexStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;
use tokio_util::io::ReaderStream;

use crate::{AppState, erase, error::AppError, jobs, zip_download};

#[derive(Debug)]
struct ExportJob {
    id: String,
    log: String,
    timestamp_request: Option<Vec<u8>>,
    timestamp_response: Option<Vec<u8>>,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/jobs/export.zip", get(export_jobs_zip))
}

pub fn build_txt_log(log: &str, timestamp_files: &jobs::JobTimestampFiles) -> String {
    let mut log = log.to_string();
    if let (Some(request), Some(response)) = (&timestamp_files.request, &timestamp_files.response) {
        log.push_str(&format!(
            "\nTimestampRequestBase64: {}\nTimestampResponseBase64: {}\n",
            STANDARD.encode(request),
            STANDARD.encode(response)
        ));
        log.push_str(erase::SECURE_ERASE_SIGNATURE_EXPLANATION);
    }

    log
}

async fn export_jobs_zip(State(state): State<AppState>) -> Result<Response, AppError> {
    let stream = ReaderStream::with_capacity(
        jobs_zip_reader(state.db.clone()),
        zip_download::STREAM_BUFFER_SIZE,
    );
    let body = Body::from_stream(stream);

    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/zip"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment; filename=\"basildisk-job-logs.zip\""),
            ),
        ],
        body,
    )
        .into_response())
}

fn jobs_zip_reader(db: SqlitePool) -> DuplexStream {
    let (reader, writer) = tokio::io::duplex(zip_download::STREAM_BUFFER_SIZE);

    tokio::spawn(async move {
        if let Err(err) = write_jobs_zip(db, writer).await {
            eprintln!("Error streaming job log export ZIP: {err}");
        }
    });

    reader
}

async fn write_jobs_zip(db: SqlitePool, writer: DuplexStream) -> anyhow::Result<()> {
    let jobs = sqlx::query_as!(
        ExportJob,
        r#"
        SELECT
            id,
            log,
            timestamp_request as "timestamp_request?",
            timestamp_response as "timestamp_response?"
        FROM jobs
        ORDER BY rowid ASC
        "#
    )
    .fetch_all(&db)
    .await?;

    write_jobs_to_zip(jobs, writer).await
}

async fn write_jobs_to_zip(jobs: Vec<ExportJob>, writer: DuplexStream) -> anyhow::Result<()> {
    let writer = writer.compat_write();
    let mut zip = ZipFileWriter::new(writer);
    for job in jobs {
        add_job_to_zip(&mut zip, job).await?;
    }
    zip.close().await?;

    Ok(())
}

async fn add_job_to_zip(
    zip: &mut ZipFileWriter<tokio_util::compat::Compat<DuplexStream>>,
    job: ExportJob,
) -> anyhow::Result<()> {
    let timestamp_files = jobs::JobTimestampFiles {
        request: job.timestamp_request,
        response: job.timestamp_response,
    };
    let log = build_txt_log(&job.log, &timestamp_files);
    let entry = ZipEntryBuilder::new(format!("{}.txt", job.id).into(), Compression::Stored);
    zip.write_entry_whole(entry, log.as_bytes()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use async_zip::base::read::mem::ZipFileReader;
    use tokio::io::AsyncReadExt;

    use super::*;

    async fn zip_bytes_for_jobs(jobs: Vec<ExportJob>) -> Vec<u8> {
        let (mut reader, writer) = tokio::io::duplex(zip_download::STREAM_BUFFER_SIZE);
        let writer_task = tokio::spawn(async move { write_jobs_to_zip(jobs, writer).await });

        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        writer_task.await.unwrap().unwrap();
        bytes
    }

    #[tokio::test]
    async fn empty_export_is_valid_empty_zip() {
        let bytes = zip_bytes_for_jobs(Vec::new()).await;
        let zip = ZipFileReader::new(bytes).await.unwrap();

        assert!(zip.file().entries().is_empty());
    }

    #[tokio::test]
    async fn export_uses_uuid_txt_names() {
        let bytes = zip_bytes_for_jobs(vec![ExportJob {
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            log: "Finished\n".to_string(),
            timestamp_request: None,
            timestamp_response: None,
        }])
        .await;
        let zip = ZipFileReader::new(bytes).await.unwrap();

        let entries = zip.file().entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].filename().as_str().unwrap(),
            "550e8400-e29b-41d4-a716-446655440000.txt"
        );
    }
}
