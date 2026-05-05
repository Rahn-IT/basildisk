use std::{
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    extract::{
        Path as AxumPath, Query, State,
        ws::{Message, WebSocketUpgrade},
    },
    http::{HeaderValue, header},
    middleware,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::Form;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqlitePool, migrate::MigrateDatabase};

mod browse;
mod disk_info;
mod erase;
pub mod error;
mod jobs;
mod lsblk;
mod mount;
mod smartctl;
mod timestamp;
mod users;
mod zip_download;

use disk_info::Disk;
use erase::{EraseJob, EraseType, hdparm::Hdparm};
use error::AppError;
use jobs::{JobInfo, JobManager, JobPage};
use mount::{mount_partition, unmount_partition};
use smartctl::SmartCtl;

const DB_PATH: &str = "./db/db.sqlite";
#[derive(Clone)]
pub(crate) struct AppState {
    db: SqlitePool,
    pub(crate) jinja: Arc<minijinja::Environment<'static>>,
    job_manager: Arc<JobManager>,
}

#[derive(Serialize)]
struct DiskListView {
    is_admin: bool,
    disks: Vec<Disk>,
    error_message: Option<String>,
}

#[derive(Serialize)]
struct SmartView {
    is_admin: bool,
    smart: SmartCtl,
}

#[derive(Serialize)]
struct EraseRequestView {
    is_admin: bool,
    disk: Disk,
    timestamp: u64,
    requires_unfreeze: bool,
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConfirmErase {
    serial: String,
    timestamp: u64,
}

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

#[tokio::main]
async fn main() {
    if !tokio::fs::try_exists(DB_PATH).await.unwrap() {
        tokio::fs::create_dir_all(Path::new(DB_PATH).parent().unwrap())
            .await
            .unwrap();
        Sqlite::create_database(DB_PATH).await.unwrap();
    }

    let db = SqlitePool::connect(DB_PATH).await.unwrap();
    sqlx::migrate!("./migrations").run(&db).await.unwrap();

    let mut jinja = minijinja::Environment::new();
    minijinja_embed::load_templates!(&mut jinja);

    let state = AppState {
        db: db.clone(),
        jinja: Arc::new(jinja),
        job_manager: Arc::new(JobManager::new()),
    };

    tokio::spawn(async move {
        users::run_session_gc_scheduler(db).await;
    });

    // build our application with a route
    let app = router()
        .layer(middleware::from_fn_with_state(
            state.clone(),
            users::auth_middleware,
        ))
        .with_state(state);

    // run our app with hyper, listening globally on port 3000
    let addr = "0.0.0.0:5080";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("Starting webserver on: http://{}", addr);
    axum::serve(listener, app).await.unwrap();
}

fn router() -> Router<AppState> {
    let admin_routes = Router::new()
        .route("/users", get(users::index).post(users::create_post))
        .route(
            "/users/{id}/delete",
            get(users::delete_get).post(users::delete_post),
        )
        .route("/erase/{device}", get(erase_get).post(erase_post))
        .route_layer(middleware::from_extractor::<users::RequireAdmin>());

    Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        .route("/smart/{device}", get(smart))
        .route("/jobs", get(jobs_index))
        .route("/jobs/{id}", get(job_detail))
        .route("/jobs/{id}/log", get(job_log))
        .route("/jobs/{id}/log.txt", get(job_log_download))
        .route(
            "/jobs/{id}/timestamp-request.tsq",
            get(job_timestamp_request_download),
        )
        .route(
            "/jobs/{id}/timestamp-response.tsr",
            get(job_timestamp_response_download),
        )
        .route("/partitions/{device}/mount", post(partition_mount_post))
        .route("/partitions/{device}/unmount", post(partition_unmount_post))
        .route("/setup", get(users::setup_get).post(users::setup_post))
        .route("/login", get(users::login_get).post(users::login_post))
        .route("/logout", post(users::logout_post))
        .route(
            "/static/style.css",
            get((
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(mime::TEXT_CSS_UTF_8.as_ref()),
                )],
                include_bytes!("../assets/static/style.css"),
            )),
        )
        .route(
            "/static/script.js",
            get((
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(mime::APPLICATION_JAVASCRIPT_UTF_8.as_ref()),
                )],
                include_bytes!("../assets/static/script.js"),
            )),
        )
        .merge(browse::router())
        .merge(admin_routes)
}

async fn root(
    State(state): State<AppState>,
    current_user: users::CurrentUser,
) -> Result<Html<String>, AppError> {
    let (disks, error_message) = match Disk::list().await {
        Ok(disks) => (disks, None),
        Err(err) => (Vec::new(), Some(format!("Error listing disks: {err}"))),
    };

    let template = state
        .jinja
        .get_template("home.html")
        .expect("template is loaded");
    let rendered = template.render(DiskListView {
        is_admin: current_user.is_admin,
        disks,
        error_message,
    })?;
    Ok(Html(rendered))
}

async fn smart(
    State(state): State<AppState>,
    current_user: users::CurrentUser,
    AxumPath(device): AxumPath<String>,
) -> Result<Html<String>, AppError> {
    let smart = SmartCtl::get(&device).await?;
    let template = state
        .jinja
        .get_template("smart.html")
        .expect("template is loaded");
    let rendered = template.render(SmartView {
        is_admin: current_user.is_admin,
        smart,
    })?;
    Ok(Html(rendered))
}

async fn erase_get(
    State(state): State<AppState>,
    current_user: users::CurrentUser,
    AxumPath(device): AxumPath<String>,
) -> Result<Html<String>, AppError> {
    let disk = find_disk(&device).await?;
    let (requires_unfreeze, error_message) = match disk.erase_type {
        _ if disk.is_mounted => (
            false,
            Some(format!(
                "Secure erase is disabled because this disk is mounted at {}.",
                disk.mount_points_display
            )),
        ),
        EraseType::AtaSecureErase | EraseType::AtaEnhancedSecureErase => {
            match Hdparm::get_for_disk(&device).await {
                Ok(hdparm) => (hdparm.frozen, None),
                Err(err) => (
                    false,
                    Some(format!("Error checking drive security state: {err}")),
                ),
            }
        }
        EraseType::BlockOverride => (false, None),
        EraseType::None => (
            false,
            Some("Secure erase is not supported for this disk.".to_string()),
        ),
        _ => (
            false,
            Some(format!(
                "{} is detected but not implemented yet.",
                disk.erase_type
            )),
        ),
    };

    let template = state
        .jinja
        .get_template("erase.html")
        .expect("template is loaded");
    let rendered = template.render(EraseRequestView {
        is_admin: current_user.is_admin,
        disk,
        timestamp: unix_timestamp(),
        requires_unfreeze,
        error_message,
    })?;
    Ok(Html(rendered))
}

async fn erase_post(
    State(state): State<AppState>,
    AxumPath(device): AxumPath<String>,
    Form(form): Form<ConfirmErase>,
) -> Result<Redirect, AppError> {
    let disk = find_disk(&device).await?;
    let now = unix_timestamp();

    if now.saturating_sub(form.timestamp) > 60 {
        return Err(AppError::conflict("Confirm timeout, try again."));
    }

    if disk.serial.as_deref() != Some(form.serial.as_str()) {
        return Err(AppError::conflict(
            "Serial number changed. Did you unplug the disk?",
        ));
    }

    if disk.erase_type == EraseType::None {
        return Err(AppError::conflict(
            "Secure erase is not supported for this disk.",
        ));
    }

    if !disk.erase_can_run {
        return Err(AppError::conflict(format!(
            "{} is detected but not implemented yet.",
            disk.erase_type
        )));
    }

    if disk.is_mounted {
        return Err(AppError::conflict(format!(
            "Secure erase is disabled because this disk is mounted at {}.",
            disk.mount_points_display
        )));
    }

    let job = EraseJob {
        device,
        connection_type: disk.connection_type,
        disk_type: disk.disk_type,
        erase_type: disk.erase_type,
        model: disk.model,
        serial: form.serial,
    };

    let id = state
        .job_manager
        .run_job(job, state.db.clone())
        .await
        .map_err(|_| AppError::conflict("A job is already running for this disk."))?;

    Ok(Redirect::to(&format!("/jobs/{id}")))
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
        })
        .into_response())
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
    let log = build_txt_log_download(&log, &timestamp_files);
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

fn build_txt_log_download(log: &str, timestamp_files: &jobs::JobTimestampFiles) -> String {
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

async fn partition_mount_post(AxumPath(device): AxumPath<String>) -> Result<Redirect, AppError> {
    let partition = find_partition(&device).await?;
    let fs_type = partition
        .fs_type
        .as_deref()
        .ok_or_else(|| AppError::conflict("Partition does not report a filesystem type."))?;

    mount_partition(&device, fs_type).await?;

    Ok(Redirect::to("/"))
}

async fn partition_unmount_post(AxumPath(device): AxumPath<String>) -> Result<Redirect, AppError> {
    let partition = find_partition(&device).await?;

    unmount_partition(&partition.mount_points).await?;

    Ok(Redirect::to("/"))
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

async fn find_disk(device: &str) -> Result<Disk, AppError> {
    Disk::list()
        .await?
        .into_iter()
        .find(|disk| disk.device == device)
        .ok_or_else(|| AppError::not_found_for("Disk", format!("No disk exists for {device}")))
}

async fn find_partition(device: &str) -> Result<disk_info::Partition, AppError> {
    Disk::list()
        .await?
        .into_iter()
        .flat_map(|disk| disk.partitions)
        .find(|partition| partition.name == device)
        .ok_or_else(|| {
            AppError::not_found_for("Partition", format!("No partition exists for {device}"))
        })
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or(0)
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
