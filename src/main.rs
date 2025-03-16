use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use diesel::{backend::Backend, deserialize, serialize, sql_types::VarChar};
use disk_info::{Disk, DiskType, ListDiskError};
use erase::{hdparm::Hdparm, EraseType};
use jobs::{JobInfo, JobManager, TestJob};
use rocket::{
    fairing::{AdHoc, Fairing},
    form::Form,
    fs::FileServer,
    request::FlashMessage,
    response::{Flash, Redirect},
    serde::{json::Json, Serialize},
    time::format_description::modifier::UnixTimestamp,
    Build, Rocket, State,
};
use rocket_dyn_templates::Template;
use rocket_sync_db_pools::database;
use smartctl::SmartCtl;
use thiserror::Error;

#[macro_use]
extern crate rocket;

mod disk_info;
mod erase;
mod jobs;
mod lsblk;
mod schema;
mod smartctl;

#[database("sqlite_database")]
pub struct DbConn(diesel::SqliteConnection);

#[launch]
async fn rocket() -> _ {
    rocket::build()
        .mount(
            "/",
            routes![
                index,
                smart,
                job_list,
                job_detail,
                secure_erase_request,
                secure_erase_confirm,
                sleep,
            ],
        )
        .mount("/static", FileServer::from("templates/static"))
        .attach(Template::fairing())
        .attach(DbConn::fairing())
        .attach(AdHoc::on_ignite("Run Migrations", run_migrations))
        .manage(Arc::new(JobManager::new()))
}

async fn run_migrations(rocket: Rocket<Build>) -> Rocket<Build> {
    use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};

    const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

    DbConn::get_one(&rocket)
        .await
        .expect("database connection")
        .run(|conn| {
            conn.run_pending_migrations(MIGRATIONS)
                .expect("diesel migrations");
        })
        .await;

    rocket
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
    finished_jobs: Vec<JobInfo>,
}

#[get("/jobs")]
async fn job_list(manager: &State<Arc<JobManager>>, conn: DbConn) -> Template {
    let jobs = Jobs {
        running_jobs: manager.list_running_jobs().await,
        finished_jobs: manager.list_finished_jobs(&conn).await.unwrap(),
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
async fn secure_erase_confirm(
    device: String,
    erase_form: Form<ConfirmErase>,
    job_manager: &State<Arc<JobManager>>,
    conn: DbConn,
) -> Flash<Redirect> {
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

        let id = match job_manager.run_job(TestJob { device }, conn).await {
            Ok(id) => id,
            Err(err) => return Flash::error(on_error, format!("{err}")),
        };

        Flash::success(Redirect::to(format!("/jobs/{id}")), "Secure Erase started!")
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

#[derive(Serialize)]
struct JobDetail {
    job: JobInfo,
    flash: Option<(String, String)>,
}

#[get("/jobs/<id>")]
async fn job_detail(
    job_manager: &State<Arc<JobManager>>,
    id: String,
    conn: DbConn,
    flash: Option<FlashMessage<'_>>,
) -> Template {
    let job = job_manager.inner().get_job_info(id, &conn).await.unwrap();

    let detail = JobDetail {
        job,
        flash: flash.map(FlashMessage::into_inner),
    };

    Template::render("job_detail", &detail)
}
