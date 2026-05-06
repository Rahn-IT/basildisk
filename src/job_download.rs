use axum::{
    Router,
    extract::{Path as AxumPath, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};

use crate::{AppState, error::AppError, export};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs/{id}/log.txt", get(job_log_download))
        .route(
            "/jobs/{id}/timestamp-request.tsq",
            get(job_timestamp_request_download),
        )
        .route(
            "/jobs/{id}/timestamp-response.tsr",
            get(job_timestamp_response_download),
        )
}

async fn job_log_download(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, AppError> {
    let job = state
        .job_manager
        .get_job_info(&id, &state.db)
        .await?
        .ok_or_else(|| AppError::not_found_for("Job", format!("No job exists for id: {id}")))?;
    let (log, _) = state
        .job_manager
        .subscribe_log(&id, &state.db)
        .await?
        .ok_or_else(|| AppError::not_found_for("Job", format!("No job exists for id: {id}")))?;
    let timestamp_files = state
        .job_manager
        .get_timestamp_files(&id, &state.db)
        .await?
        .ok_or_else(|| AppError::not_found_for("Job", format!("No job exists for id: {id}")))?;
    let log = export::build_txt_log(&log, &timestamp_files);
    let filename = format!("secure-erase-protocol-{}-{}.txt", job.disk, job.id);
    let content_disposition = format!(
        "attachment; filename=\"{}\"",
        escape_header_filename(&filename)
    );

    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&content_disposition)?,
            ),
        ],
        log,
    )
        .into_response())
}

async fn job_timestamp_request_download(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, AppError> {
    job_timestamp_download(
        &state,
        &id,
        "application/timestamp-query",
        "tsq",
        TimestampFileKind::Request,
    )
    .await
}

async fn job_timestamp_response_download(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, AppError> {
    job_timestamp_download(
        &state,
        &id,
        "application/timestamp-reply",
        "tsr",
        TimestampFileKind::Response,
    )
    .await
}

async fn job_timestamp_download(
    state: &AppState,
    id: &str,
    content_type: &'static str,
    extension: &str,
    kind: TimestampFileKind,
) -> Result<Response, AppError> {
    let job = state
        .job_manager
        .get_job_info(id, &state.db)
        .await?
        .ok_or_else(|| AppError::not_found_for("Job", format!("No job exists for id: {id}")))?;
    let timestamp_files = state
        .job_manager
        .get_timestamp_files(id, &state.db)
        .await?
        .ok_or_else(|| AppError::not_found_for("Job", format!("No job exists for id: {id}")))?;
    let data = match kind {
        TimestampFileKind::Request => timestamp_files.request,
        TimestampFileKind::Response => timestamp_files.response,
    }
    .ok_or_else(|| {
        AppError::not_found_for(
            "Timestamp",
            "This job log does not contain that timestamp file yet.",
        )
    })?;
    let filename = format!(
        "secure-erase-protocol-{}-{}.{}",
        job.disk, job.id, extension
    );
    let content_disposition = format!(
        "attachment; filename=\"{}\"",
        escape_header_filename(&filename)
    );

    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&content_disposition)?,
            ),
            (
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&data.len().to_string())?,
            ),
        ],
        data,
    )
        .into_response())
}

enum TimestampFileKind {
    Request,
    Response,
}

fn escape_header_filename(filename: &str) -> String {
    filename
        .chars()
        .filter(|character| character.is_ascii() && !character.is_control())
        .flat_map(|character| match character {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect(),
            _ => vec![character],
        })
        .collect()
}
