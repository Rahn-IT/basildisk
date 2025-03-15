use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use disk_info::{Disk, DiskType, ListDiskError};
use erase::{hdparm::Hdparm, EraseType};
use jobs::{JobInfo, JobManager};
use rocket::{
    form::Form,
    fs::FileServer,
    request::FlashMessage,
    response::{Flash, Redirect},
    serde::{json::Json, Serialize},
    time::format_description::modifier::UnixTimestamp,
    State,
};
use rocket_dyn_templates::Template;
use smartctl::SmartCtl;
use thiserror::Error;

#[macro_use]
extern crate rocket;

mod disk_info;
mod erase;
mod jobs;
mod lsblk;
mod smartctl;

#[launch]
async fn rocket() -> _ {
    rocket::build()
        .mount(
            "/",
            routes![
                index,
                smart,
                job_list,
                secure_erase_request,
                secure_erase_confirm,
                sleep
            ],
        )
        .mount("/static", FileServer::from("templates/static"))
        .attach(Template::fairing())
        .manage(Arc::new(JobManager::new()))
}

#[derive(Serialize)]
pub struct Index {
    flash: Option<(String, String)>,
    disks: Vec<Disk>,
}

#[get("/")]
async fn index(flash: Option<FlashMessage<'_>>) -> Template {
    let disks = Disk::list().await;

    let index = match disks {
        Ok(disks) => Index {
            flash: flash.map(FlashMessage::into_inner),
            disks,
        },
        Err(err) => Index {
            flash: Some(("error".into(), format!("Error listing disks: {}", err))),
            disks: Vec::new(),
        },
    };

    Template::render("index", &index)
}

#[derive(Serialize)]
pub struct Smart {
    flash: Option<(String, String)>,
    smart: Option<SmartCtl>,
}

#[get("/smart/<device>")]
async fn smart(device: String, flash: Option<FlashMessage<'_>>) -> Template {
    let smart_data = SmartCtl::get(&device).await;

    let smart = match smart_data {
        Ok(smart) => Smart {
            flash: flash.map(FlashMessage::into_inner),
            smart: Some(smart),
        },
        Err(_err) => Smart {
            flash: Some(("error".into(), _err.to_string())),
            smart: None,
        },
    };

    Template::render("smart", &smart)
}

#[derive(Serialize)]
struct Jobs {
    running_jobs: Vec<JobInfo>,
}

#[get("/jobs")]
async fn job_list(manager: &State<Arc<JobManager>>) -> Template {
    let jobs = Jobs {
        running_jobs: manager.list_running_jobs().await,
    };

    Template::render("jobs", &jobs)
}

#[derive(Serialize)]
struct EraseRequestData {
    disk: Option<Disk>,
    timestamp: u64,
    requires_unfreeze: bool,
    flash: Option<(String, String)>,
}

#[get("/secure_erase/<device>")]
async fn secure_erase_request(device: String, flash: Option<FlashMessage<'_>>) -> Template {
    let disks: Result<Vec<Disk>, String> = Disk::list().await.map_err(|err| format!("{:?}", err));
    match disks {
        Ok(disks) => {
            if let Some(disk) = disks.into_iter().find(|disk| disk.device == device) {
                let requires_unfreeze = match disk.erase_type {
                    EraseType::AtaSecureErase | EraseType::AtaEnhancedSecureErase => {
                        Hdparm::get_for_disk(&device)
                            .await
                            .map_err(|err| format!("{:?}", err))
                            .map(|hdparm| hdparm.frozen)
                    }
                    _ => Err(
                        "Secure Erase not yet supported for this disk type or connection"
                            .to_string(),
                    ),
                };

                let mut request = EraseRequestData {
                    disk: Some(disk),
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|time| time.as_secs())
                        .unwrap_or(0),
                    requires_unfreeze: false,
                    flash: flash.map(FlashMessage::into_inner),
                };

                match requires_unfreeze {
                    Ok(requires_unfreeze) => {
                        request.requires_unfreeze = requires_unfreeze;
                    }
                    Err(err) => {
                        request.flash = Some(("error".to_string(), err));
                    }
                }

                Template::render("secure_erase", &request)
            } else {
                let err = EraseRequestData {
                    disk: None,
                    timestamp: 0,
                    requires_unfreeze: false,
                    flash: Some(("error".to_string(), format!("Disk {} not found", device))),
                };
                Template::render("secure_erase", &err)
            }
        }
        Err(err) => {
            let err = EraseRequestData {
                disk: None,
                timestamp: 0,
                requires_unfreeze: false,
                flash: Some(("error".to_string(), err)),
            };
            Template::render("secure_erase", &err)
        }
    }
}

#[derive(FromForm)]
struct ConfirmErase {
    serial: String,
    timestamp: u64,
}

#[post("/secure_erase/<device>", data = "<erase_form>")]
async fn secure_erase_confirm(device: String, erase_form: Form<ConfirmErase>) -> Flash<Redirect> {
    let erase_form = erase_form.into_inner();
    let on_error = Redirect::to(format!("/secure_erase/{}", device));

    let disks = Disk::list().await.map_err(|err| format!("{:?}", err));
    let disks = match disks {
        Ok(disks) => disks,
        Err(err) => return Flash::error(on_error, err.to_string()),
    };

    let now: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    if now - erase_form.timestamp > 60 {
        return Flash::error(on_error, "Confirm Timeout, try again!");
    }

    if let Some(disk) = disks.into_iter().find(|disk| disk.device == device) {
        if disk.serial != Some(erase_form.serial) {
            return Flash::error(
                on_error,
                "Serial number of Disk changed. Did you unplug the disk?",
            );
        }
        Flash::error(on_error, "Todo")
    } else {
        Flash::error(on_error, "Disk not found!")
    }
}

#[post("/sleep")]
async fn sleep() {
    tokio::process::Command::new("rtcwake")
        .arg("-m")
        .arg("mem")
        .arg("-s")
        .arg("5")
        .status()
        .await
        .unwrap();
}
