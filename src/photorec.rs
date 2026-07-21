use axum::{
    Router,
    extract::{Path as AxumPath, State},
    response::Html,
    routing::get,
};
use serde::Serialize;

use crate::{AppState, disk_info::Disk, error::AppError, mount, users::CurrentUser};

#[derive(Serialize)]
struct PhotoRecView {
    is_admin: bool,
    source: String,
    recovery_targets: Vec<RecoveryTarget>,
}

#[derive(Serialize)]
struct RecoveryTarget {
    device: String,
    mount_point: String,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/photorec/{device}", get(photorec_get))
}

async fn photorec_get(
    State(state): State<AppState>,
    current_user: CurrentUser,
    AxumPath(device): AxumPath<String>,
) -> Result<Html<String>, AppError> {
    let disks = Disk::list().await?;
    let source_exists = disks.iter().any(|disk| {
        disk.device == device
            || disk
                .partitions
                .iter()
                .any(|partition| partition.name == device)
    });

    if !source_exists {
        return Err(AppError::not_found_for(
            "Device or partition",
            format!("No device or partition exists for {device}"),
        ));
    }

    let recovery_targets = disks
        .into_iter()
        .flat_map(|disk| disk.partitions)
        .filter_map(|partition| {
            mount::mount_point_under_mnt(&partition.mount_points).map(|mount_point| {
                RecoveryTarget {
                    device: partition.name,
                    mount_point,
                }
            })
        })
        .collect();

    let template = state
        .jinja
        .get_template("photorec.html")
        .expect("template is loaded");
    let rendered = template.render(PhotoRecView {
        is_admin: current_user.is_admin,
        source: device,
        recovery_targets,
    })?;

    Ok(Html(rendered))
}
