use std::path::Path;

use axum::{
    Router,
    extract::{Path as AxumPath, State},
    response::{Html, Redirect},
    routing::get,
};
use axum_extra::extract::Form;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::process::Command;

use crate::{
    AppState,
    disk_info::Disk,
    erase::command_runner,
    error::AppError,
    jobs::{Job, JobLogger},
    mount::{self, MountAccess},
    users::CurrentUser,
};

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

#[derive(Debug, Deserialize)]
struct StartPhotoRecForm {
    recovery_name: String,
    recup_target: String,
}

struct PhotoRecJob {
    device: String,
    recovery_name: String,
    recovery_target: String,
    recovery_dir: String,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/photorec/{device}", get(photorec_get).post(photorec_post))
}

async fn photorec_get(
    State(state): State<AppState>,
    current_user: CurrentUser,
    AxumPath(device): AxumPath<String>,
) -> Result<Html<String>, AppError> {
    let disks = Disk::list().await?;
    let source_mounted = source_is_mounted(&disks, &device);
    let Some(source_mounted) = source_mounted else {
        return Err(AppError::not_found_for(
            "Device or partition",
            format!("No device or partition exists for {device}"),
        ));
    };
    if source_mounted {
        return Err(AppError::conflict(
            "Unmount the recovery source before starting PhotoRec.",
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

async fn photorec_post(
    State(state): State<AppState>,
    AxumPath(device): AxumPath<String>,
    Form(form): Form<StartPhotoRecForm>,
) -> Result<Redirect, AppError> {
    let disks = Disk::list().await?;
    let source_mounted = source_is_mounted(&disks, &device);
    let Some(source_mounted) = source_mounted else {
        return Err(AppError::not_found_for(
            "Device or partition",
            format!("No device or partition exists for {device}"),
        ));
    };
    if source_mounted {
        return Err(AppError::conflict(
            "Unmount the recovery source before starting PhotoRec.",
        ));
    }

    let recovery_name = sanitize_recovery_name(&form.recovery_name);
    if recovery_name.is_empty() {
        return Err(AppError::conflict(
            "Enter a recovery name using letters, numbers, spaces, hyphens, or underscores.",
        ));
    }

    let target_is_mounted = disks
        .iter()
        .flat_map(|disk| &disk.partitions)
        .any(|partition| {
            mount::mount_point_under_mnt(&partition.mount_points).as_deref()
                == Some(form.recup_target.as_str())
        });
    if !target_is_mounted {
        return Err(AppError::conflict(
            "The selected recovery destination is no longer mounted under /mnt.",
        ));
    }

    let recovery_dir_name = recovery_directory_name(&recovery_name, OffsetDateTime::now_utc());
    let recovery_dir = Path::new(&form.recup_target).join(recovery_dir_name);

    let job = PhotoRecJob {
        device,
        recovery_name,
        recovery_target: form.recup_target,
        recovery_dir: recovery_dir.to_string_lossy().into_owned(),
    };
    let id = state
        .job_manager
        .run_job(job, state.db.clone())
        .await
        .map_err(|_| AppError::conflict("A job is already running for this disk."))?;

    Ok(Redirect::to(&format!("/jobs/{id}")))
}

fn sanitize_recovery_name(name: &str) -> String {
    name.chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ' ' | '-' | '_')
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn source_is_mounted(disks: &[Disk], device: &str) -> Option<bool> {
    disks.iter().find_map(|disk| {
        if disk.device == device {
            Some(disk.is_mounted)
        } else {
            disk.partitions
                .iter()
                .find(|partition| partition.name == device)
                .map(|partition| partition.is_mounted)
        }
    })
}

fn recovery_directory_name(recovery_name: &str, now: OffsetDateTime) -> String {
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}-{recovery_name}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
    )
}

impl Job for PhotoRecJob {
    fn get_device(&self) -> &str {
        &self.device
    }

    fn get_name(&self) -> String {
        format!("Photo recovery: {}", self.recovery_name)
    }

    async fn run(self, logger: JobLogger) -> Result<(), Box<dyn std::error::Error + Send>> {
        logger.write(format!(
            "\nStarting PhotoRec recovery\n=================================================\nSource: /dev/{}\nDestination: {}\n=================================================\n",
            self.device, self.recovery_dir
        ));

        mount::remount_partition(&self.recovery_target, MountAccess::ReadWrite)
            .await
            .map_err(|err| -> Box<dyn std::error::Error + Send> { Box::new(err) })?;
        logger.write(format!(
            "Recovery destination remounted read-write: {}\n",
            self.recovery_target
        ));

        let mut command = Command::new("photorec");
        command
            .arg("/log")
            .arg("/d")
            .arg(&self.recovery_dir)
            .arg("/cmd")
            .arg(Path::new("/dev").join(&self.device))
            .arg("search");
        let recovery_result = command_runner::run_and_log(&mut command, &logger)
            .await
            .map_err(|err| -> Box<dyn std::error::Error + Send> { Box::new(err) })
            .and_then(|output| {
                if output.status.success() {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("PhotoRec exited with {}", output.status).into())
                }
            });

        logger.write(format!(
            "Remounting recovery destination read-only: {}\n",
            self.recovery_target
        ));
        let remount_result =
            mount::remount_partition(&self.recovery_target, MountAccess::Read).await;

        if let Err(err) = remount_result {
            logger.write(format!(
                "Could not remount recovery destination read-only: {err}\n"
            ));
            if recovery_result.is_ok() {
                return Err(Box::new(err));
            }
        }

        recovery_result?;
        logger.write("\n=================================================\nPhotoRec recovery completed successfully.\n=================================================\n");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::{recovery_directory_name, sanitize_recovery_name};

    #[test]
    fn sanitizes_recovery_folder_name() {
        assert_eq!(
            sanitize_recovery_name(" Phone SD-card! / 2026 "),
            "Phone SD-card  2026"
        );
        assert_eq!(sanitize_recovery_name("..."), "");
    }

    #[test]
    fn prefixes_recovery_folder_name_with_datetime() {
        assert_eq!(
            recovery_directory_name(
                "Phone SD-card",
                OffsetDateTime::from_unix_timestamp(0).unwrap()
            ),
            "19700101-000000-Phone SD-card"
        );
    }
}
