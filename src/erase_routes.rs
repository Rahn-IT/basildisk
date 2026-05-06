use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    extract::{Path as AxumPath, State},
    middleware,
    response::{Html, Redirect},
    routing::get,
};
use axum_extra::extract::Form;
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    disk_info::Disk,
    erase::{EraseJob, EraseType, hdparm::Hdparm},
    error::AppError,
    users::{self, CurrentUser},
};

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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/erase/{device}", get(erase_get).post(erase_post))
        .route_layer(middleware::from_extractor::<users::RequireAdmin>())
}

async fn erase_get(
    State(state): State<AppState>,
    current_user: CurrentUser,
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

async fn find_disk(device: &str) -> Result<Disk, AppError> {
    Disk::list()
        .await?
        .into_iter()
        .find(|disk| disk.device == device)
        .ok_or_else(|| AppError::not_found_for("Disk", format!("No disk exists for {device}")))
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or(0)
}
