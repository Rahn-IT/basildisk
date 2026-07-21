use std::{path::Path, sync::Arc};

use axum::{Router, middleware};
use sqlx::{Sqlite, SqlitePool, migrate::MigrateDatabase};

mod browse;
mod disk_info;
mod disk_routes;
mod erase;
mod erase_routes;
pub mod error;
mod export;
mod job_download;
mod job_import;
mod job_routes;
mod jobs;
mod lsblk;
mod mount;
mod partition_routes;
mod photorec;
mod shutdown;
mod smartctl;
mod static_assets;
mod timestamp;
mod users;
mod zip_download;

use jobs::JobManager;

const DB_PATH: &str = "./db/db.sqlite";
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) db: SqlitePool,
    pub(crate) jinja: Arc<minijinja::Environment<'static>>,
    pub(crate) job_manager: Arc<JobManager>,
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
    Router::new()
        .merge(browse::router())
        .merge(disk_routes::router())
        .merge(erase_routes::router())
        .merge(export::router())
        .merge(job_download::router())
        .merge(job_import::router())
        .merge(job_routes::router())
        .merge(partition_routes::router())
        .merge(photorec::router())
        .merge(shutdown::router())
        .merge(static_assets::router())
        .merge(users::router())
}
