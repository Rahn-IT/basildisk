use std::collections::BTreeMap;

use serde::Serialize;
use tokio::sync::watch;

pub struct JobManager {
    running_jobs: BTreeMap<uuid::Uuid, RunningJob>,
}

#[derive(Serialize, Clone)]
pub struct JobInfo {
    id: uuid::Uuid,
    disk: String,
    name: String,
}

pub struct RunningJob {
    info: JobInfo,
    log: watch::Sender<String>,
}

impl JobManager {
    pub fn new() -> Self {
        Self {
            running_jobs: BTreeMap::new(),
        }
    }

    pub async fn list_running_jobs(&self) -> Vec<JobInfo> {
        let mut jobs = Vec::new();

        for job in self.running_jobs.values() {
            jobs.push(job.info.clone());
        }

        jobs
    }
}
