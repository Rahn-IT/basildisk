use std::{collections::BTreeMap, future::Future, sync::Arc, time::Duration};

use diesel::{
    prelude::{Insertable, Queryable},
    QueryDsl, RunQueryDsl,
};
use rocket::fairing::AdHoc;
use serde::Serialize;
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch, TryLockError},
    task,
};
use uuid::Uuid;

use crate::{schema::jobs, DbConn};

pub struct JobManager {
    running_jobs: std::sync::Mutex<BTreeMap<String, RunningJob>>,
    disk_locks: std::sync::Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Serialize, Clone, Queryable, Insertable)]
#[diesel(table_name = jobs)]
pub struct JobInfo {
    pub id: String,
    pub disk: String,
    pub name: String,
}

pub struct RunningJob {
    info: JobInfo,
    log: Arc<std::sync::Mutex<String>>,
    incoming: broadcast::Receiver<String>,
}

#[derive(Queryable, Insertable)]
#[diesel(table_name = jobs)]
pub struct SavedJob {
    #[diesel(embed)]
    pub info: JobInfo,
    pub log: String,
}

impl JobManager {
    pub fn new() -> Self {
        Self {
            running_jobs: std::sync::Mutex::new(BTreeMap::new()),
            disk_locks: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    pub async fn list_running_jobs(&self) -> Vec<JobInfo> {
        self.running_jobs
            .lock()
            .unwrap()
            .values()
            .map(|job| &job.info)
            .cloned()
            .collect()
    }

    pub async fn list_finished_jobs(
        &self,
        conn: &DbConn,
    ) -> diesel::result::QueryResult<Vec<JobInfo>> {
        conn.run(|conn| {
            jobs::table
                .select((jobs::id, jobs::disk, jobs::name))
                .load::<JobInfo>(conn)
        })
        .await
    }

    pub async fn get_job_info(
        &self,
        id: String,
        conn: &DbConn,
    ) -> diesel::result::QueryResult<JobInfo> {
        if let Some(job) = self.running_jobs.lock().unwrap().get(&id) {
            return Ok(job.info.clone());
        }

        conn.run(|conn| {
            jobs::table
                .select((jobs::id, jobs::disk, jobs::name))
                .find(id)
                .first::<JobInfo>(conn)
        })
        .await
    }

    pub async fn subscribe_log(
        &self,
        id: String,
        conn: &DbConn,
    ) -> diesel::result::QueryResult<(String, Option<broadcast::Receiver<String>>)> {
        {
            let lock = self.running_jobs.lock().unwrap();

            if let Some(job) = lock.get(&id) {
                let lock = job.log.lock().unwrap();

                let subscriber = job.incoming.resubscribe();
                return Ok((lock.clone(), Some(subscriber)));
            }
        }

        let log = conn
            .run(|conn| jobs::table.select(jobs::log).find(id).first::<String>(conn))
            .await?;

        Ok((log, None))
    }

    fn lock_disk(&self, device: &str) -> Result<tokio::sync::OwnedMutexGuard<()>, TryLockError> {
        let mut guard = self.disk_locks.lock().unwrap();

        let mutex = if let Some(mutex) = guard.get(device) {
            mutex.clone()
        } else {
            let mutex = Arc::new(tokio::sync::Mutex::new(()));
            guard.insert(device.to_string(), mutex.clone());
            mutex
        };

        mutex.try_lock_owned()
    }

    pub async fn run_job<J: Job>(
        self: &Arc<Self>,
        job: J,
        conn: DbConn,
    ) -> Result<String, TryLockError> {
        let device = job.get_device();

        let drive_lock = self.lock_disk(device)?;

        let (send, recv) = broadcast::channel(3);

        let log = Arc::new(std::sync::Mutex::new(String::new()));

        let rjob = RunningJob {
            info: JobInfo {
                id: Uuid::new_v4().to_string(),
                disk: device.to_string(),
                name: job.get_name(),
            },
            incoming: recv,
            log: log.clone(),
        };

        let id = rjob.info.id.clone();
        let id2 = id.clone();
        let mut running_jobs = self.running_jobs.lock().unwrap();
        running_jobs.insert(rjob.info.id.clone(), rjob);

        let self2 = self.clone();

        task::spawn(async move {
            let _drive_lock = drive_lock;
            let logger = send;
            let mut log_receiver = logger.subscribe();
            let job = job;

            let job_handle = tokio::spawn(async move { job.run(logger).await });

            while let Ok(content) = log_receiver.recv().await {
                log.lock().unwrap().push_str(&content);
            }

            let result = job_handle.await;

            match result {
                Err(err1) => {}
                Ok(Err(err2)) => {}
                Ok(Ok(_)) => {}
            }

            let rjob = {
                let mut running_jobs = self2.running_jobs.lock().unwrap();
                running_jobs.remove(&id).unwrap()
            };

            let job = SavedJob {
                info: rjob.info,
                log: rjob.log.lock().unwrap().clone(),
            };

            if let Err(err) = conn
                .run(move |conn| diesel::insert_into(jobs::table).values(&job).execute(conn))
                .await
            {
                println!("Error: {err}")
            }
            println!("Job Finished!")
        });

        Ok(id2)
    }
}

pub trait Job: Send + Sync + 'static {
    fn get_device(&self) -> &str;
    fn get_name(&self) -> String;
    fn run(
        self,
        logger: broadcast::Sender<String>,
    ) -> impl Future<Output = Result<(), Box<dyn std::error::Error + Send>>> + Send;
}

pub struct TestJob {
    pub device: String,
}

impl Job for TestJob {
    fn get_device(&self) -> &str {
        &self.device
    }

    fn get_name(&self) -> String {
        "Test-Job".to_string()
    }

    async fn run(
        self,
        logger: broadcast::Sender<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send>> {
        for num in 0..100 {
            let _ = logger.send(format!("Test {num}\n"));
            tokio::time::sleep(Duration::from_secs(1)).await
        }
        Ok(())
    }
}
