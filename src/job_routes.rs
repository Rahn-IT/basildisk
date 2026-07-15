use axum::{
    Router,
    extract::{
        Path as AxumPath, Query, State,
        ws::{Message, WebSocketUpgrade},
    },
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    error::AppError,
    jobs::{JobInfo, JobPage},
    users,
};

#[derive(Serialize)]
struct JobsView {
    is_admin: bool,
    running_jobs: Vec<JobInfo>,
    finished_jobs: JobPage,
    query: String,
    has_query: bool,
    previous_url: String,
    next_url: String,
}

#[derive(Serialize)]
struct JobDetailView {
    is_admin: bool,
    job: JobInfo,
    has_timestamp_files: bool,
}

#[derive(Debug, Deserialize)]
struct JobsQuery {
    page: Option<i64>,
    q: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(jobs_index))
        .route("/jobs/{id}", get(job_detail))
        .route("/jobs/{id}/log", get(job_log))
}

async fn jobs_index(
    State(state): State<AppState>,
    current_user: users::CurrentUser,
    Query(query): Query<JobsQuery>,
) -> Result<Html<String>, AppError> {
    let query_text = query.q.unwrap_or_default();
    let page = query.page.unwrap_or(1);
    let running_jobs = state
        .job_manager
        .list_running_jobs_filtered(Some(&query_text))
        .await;
    let finished_jobs = state
        .job_manager
        .list_finished_jobs_page(&state.db, page, Some(&query_text))
        .await?;
    let has_query = !query_text.trim().is_empty();

    let template = state
        .jinja
        .get_template("jobs.html")
        .expect("template is loaded");
    let previous_url = jobs_page_url(finished_jobs.previous_page, &query_text);
    let next_url = jobs_page_url(finished_jobs.next_page, &query_text);
    let rendered = template.render(JobsView {
        is_admin: current_user.is_admin,
        running_jobs,
        finished_jobs,
        query: query_text,
        has_query,
        previous_url,
        next_url,
    })?;
    Ok(Html(rendered))
}

async fn job_detail(
    State(state): State<AppState>,
    current_user: users::CurrentUser,
    AxumPath(id): AxumPath<String>,
) -> Result<Html<String>, AppError> {
    let job = state
        .job_manager
        .get_job_info(&id, &state.db)
        .await?
        .ok_or_else(|| AppError::not_found_for("Job", format!("No job exists for id: {id}")))?;
    let timestamp_files = state
        .job_manager
        .get_timestamp_files(&id, &state.db)
        .await?
        .ok_or_else(|| AppError::not_found_for("Job", format!("No job exists for id: {id}")))?;

    let template = state
        .jinja
        .get_template("job_detail.html")
        .expect("template is loaded");
    let rendered = template.render(JobDetailView {
        is_admin: current_user.is_admin,
        job,
        has_timestamp_files: timestamp_files.request.is_some()
            && timestamp_files.response.is_some(),
    })?;
    Ok(Html(rendered))
}

async fn job_log(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let log = state
        .job_manager
        .subscribe_log(&id, &state.db)
        .await?
        .ok_or_else(|| AppError::not_found_for("Job", format!("No job exists for id: {id}")))?;

    Ok(ws
        .on_upgrade(move |mut socket| async move {
            let (current, subscriber) = log;

            if socket.send(Message::Text(current.into())).await.is_err() {
                return;
            }

            if let Some(mut subscriber) = subscriber {
                while let Ok(content) = subscriber.recv().await {
                    if socket.send(Message::Text(content.into())).await.is_err() {
                        break;
                    }
                }
            }

            // Wait for WS to close - ensures it's not dropped before the log is received by the browser.
            while socket.recv().await.is_some() {}
        })
        .into_response())
}

fn jobs_page_url(page: i64, query: &str) -> String {
    if query.trim().is_empty() {
        format!("/jobs?page={page}")
    } else {
        format!("/jobs?page={page}&q={}", form_encode(query))
    }
}

fn form_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}
