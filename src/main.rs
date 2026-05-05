use std::{
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{
        Path as AxumPath, Query, State,
        ws::{Message, WebSocketUpgrade},
    },
    http::{HeaderValue, header},
    middleware,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::body::AsyncReadBody;
use axum_extra::extract::Form;
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqlitePool, migrate::MigrateDatabase};
use tokio::{io::ReadBuf, sync::mpsc};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

mod browse;
mod disk_info;
mod erase;
pub mod error;
mod jobs;
mod lsblk;
mod mount;
mod smartctl;
mod users;

use browse::BrowseView;
use disk_info::Disk;
use erase::{EraseJob, EraseType, hdparm::Hdparm};
use error::AppError;
use jobs::{JobInfo, JobManager, JobPage};
use mount::{mount_partition, unmount_partition};
use smartctl::SmartCtl;

const DB_PATH: &str = "./db/db.sqlite";

#[derive(Clone)]
struct AppState {
    db: SqlitePool,
    jinja: Arc<minijinja::Environment<'static>>,
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
        .route("/download/{device}/{*path}", get(download_path))
        .route("/browse/{device}", get(browse_root))
        .route("/browse/{device}/{*path}", get(browse_path))
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

    let template = state
        .jinja
        .get_template("job_detail.html")
        .expect("template is loaded");
    let rendered = template.render(JobDetailView {
        is_admin: current_user.is_admin,
        job,
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

async fn browse_root(
    State(state): State<AppState>,
    current_user: users::CurrentUser,
    AxumPath(device): AxumPath<String>,
) -> Result<Html<String>, AppError> {
    render_browse(&state, &device, "", current_user.is_admin).await
}

async fn browse_path(
    State(state): State<AppState>,
    current_user: users::CurrentUser,
    AxumPath((device, path)): AxumPath<(String, String)>,
) -> Result<Html<String>, AppError> {
    render_browse(&state, &device, &path, current_user.is_admin).await
}

async fn render_browse(
    state: &AppState,
    device: &str,
    path: &str,
    is_admin: bool,
) -> Result<Html<String>, AppError> {
    let view: BrowseView = browse::list(device, path, is_admin).await?;
    let template = state
        .jinja
        .get_template("browse.html")
        .expect("template is loaded");
    let rendered = template.render(view)?;
    Ok(Html(rendered))
}

async fn download_path(
    AxumPath((device, path)): AxumPath<(String, String)>,
) -> Result<Response, AppError> {
    let download = browse::download(&device, &path)
        .await
        .map_err(|err| match err {
            browse::BrowseError::EscapesRoot => {
                AppError::forbidden("Download path escapes the mounted directory.")
            }
            browse::BrowseError::NotDownloadable => AppError::not_found_for(
                "Download",
                format!("No downloadable file or folder exists at {path}."),
            ),
            _ => AppError::from(err),
        })?;
    match download.kind {
        browse::DownloadKind::File => {
            let file = tokio::fs::File::open(&download.path).await?;
            let body = Body::new(AsyncReadBody::new(file));
            let content_disposition = format!(
                "attachment; filename=\"{}\"",
                escape_header_filename(&download.name)
            );

            Ok((
                [
                    (
                        header::CONTENT_TYPE,
                        HeaderValue::from_static(mime::APPLICATION_OCTET_STREAM.as_ref()),
                    ),
                    (
                        header::CONTENT_DISPOSITION,
                        HeaderValue::from_str(&content_disposition)?,
                    ),
                    (
                        header::CONTENT_LENGTH,
                        HeaderValue::from_str(&download.size.to_string())?,
                    ),
                ],
                body,
            )
                .into_response())
        }
        browse::DownloadKind::Folder => {
            let filename = format!("{}.zip", download.name);
            let body = Body::new(AsyncReadBody::new(zip_folder_reader(download.path)));
            let content_disposition = format!(
                "attachment; filename=\"{}\"",
                escape_header_filename(&filename)
            );

            Ok((
                [
                    (
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/zip"),
                    ),
                    (
                        header::CONTENT_DISPOSITION,
                        HeaderValue::from_str(&content_disposition)?,
                    ),
                ],
                body,
            )
                .into_response())
        }
    }
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

fn zip_folder_reader(folder: PathBuf) -> ChannelReader {
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
        let mut buffer = [0; 64 * 1024];
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

struct ChannelReader {
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
