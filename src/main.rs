use disk_info::Disk;
use rocket::{fs::FileServer, request::FlashMessage, serde::Serialize};
use rocket_dyn_templates::Template;
use smartctl::SmartCtl;

#[macro_use]
extern crate rocket;

mod disk_info;
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
