use std::collections::{BTreeMap, HashMap};

use lsblk::{Disk, IndexDisk, LsBlk};
use rocket::{
    fairing::AdHoc,
    fs::FileServer,
    request::FlashMessage,
    response::{Flash, Redirect},
    serde::Serialize,
    Build, Rocket, State,
};
use rocket_dyn_templates::Template;
use smartctl::SmartCtl;

#[macro_use]
extern crate rocket;

mod lsblk;
mod smartctl;

#[launch]
async fn rocket() -> _ {
    rocket::build()
        .mount("/", routes![index, smart])
        .mount("/static", FileServer::from("templates/static"))
        .attach(Template::fairing())
}

#[derive(Serialize)]
pub struct Index {
    flash: Option<(String, String)>,
    disks: Vec<IndexDisk>,
}

#[get("/")]
async fn index(flash: Option<FlashMessage<'_>>) -> Template {
    let disks = LsBlk::list().await;

    let index = match disks {
        Ok(disks) => Index {
            flash: flash.map(FlashMessage::into_inner),
            disks: disks.iter().map(Disk::format).collect(),
        },
        Err(_err) => Index {
            flash: Some(("error".into(), "Error listing disks".to_string())),
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
