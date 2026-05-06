use axum::{Router, extract::State, middleware, response::Html, routing::get};
use serde::Serialize;
use tokio::process::Command;

use crate::{
    AppState,
    error::AppError,
    jobs::JobInfo,
    users::{self, CurrentUser},
};

#[derive(Serialize)]
struct ShutdownConfirmView {
    is_admin: bool,
    running_jobs: Vec<JobInfo>,
    has_running_jobs: bool,
}

#[derive(Serialize)]
struct ShutdownRequestedView {
    is_admin: bool,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/shutdown", get(shutdown_get).post(shutdown_post))
        .route_layer(middleware::from_extractor::<users::RequireAdmin>())
}

async fn shutdown_get(
    State(state): State<AppState>,
    current_user: CurrentUser,
) -> Result<Html<String>, AppError> {
    let running_jobs = state.job_manager.list_running_jobs().await;
    let template = state
        .jinja
        .get_template("shutdown.html")
        .expect("template is loaded");
    let rendered = template.render(ShutdownConfirmView {
        is_admin: current_user.is_admin,
        has_running_jobs: !running_jobs.is_empty(),
        running_jobs,
    })?;
    Ok(Html(rendered))
}

async fn shutdown_post(
    State(state): State<AppState>,
    current_user: CurrentUser,
) -> Result<Html<String>, AppError> {
    let output = Command::new("shutdown")
        .arg("-h")
        .arg("now")
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let message = [stderr.trim(), stdout.trim()]
            .into_iter()
            .find(|value| !value.is_empty())
            .unwrap_or("shutdown command failed");
        return Err(AppError::internal(anyhow::anyhow!(message.to_string())));
    }

    let template = state
        .jinja
        .get_template("shutdown_requested.html")
        .expect("template is loaded");
    let rendered = template.render(ShutdownRequestedView {
        is_admin: current_user.is_admin,
    })?;
    Ok(Html(rendered))
}
