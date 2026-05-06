use axum::{Router, extract::Path as AxumPath, response::Redirect, routing::post};

use crate::{
    AppState,
    disk_info::{Disk, Partition},
    error::AppError,
    mount::{mount_partition, unmount_partition},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/partitions/{device}/mount", post(partition_mount_post))
        .route("/partitions/{device}/unmount", post(partition_unmount_post))
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

async fn find_partition(device: &str) -> Result<Partition, AppError> {
    Disk::list()
        .await?
        .into_iter()
        .flat_map(|disk| disk.partitions)
        .find(|partition| partition.name == device)
        .ok_or_else(|| {
            AppError::not_found_for("Partition", format!("No partition exists for {device}"))
        })
}
