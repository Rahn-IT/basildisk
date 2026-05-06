use axum::{
    Router,
    extract::{Path as AxumPath, State},
    response::Html,
    routing::get,
};
use serde::Serialize;

use crate::{AppState, disk_info::Disk, error::AppError, smartctl::SmartCtl, users};

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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(root))
        .route("/smart/{device}", get(smart))
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
