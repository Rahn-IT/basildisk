use std::{collections::BTreeMap, future::Future, sync::Arc};

use serde::Serialize;
use sqlx::SqlitePool;
use tokio::{
    sync::{TryLockError, broadcast},
    task,
};
use uuid::Uuid;

const BANNER: &str = r"
__________               __ __       _____        __    
\______   \____    ________|  |   __| _/__| ______  | __
  |   |  _/__  \  /  ___/  |  |  / __ ||  |/  ___/  |/ /
  |   |   \/ __ \_\___ \|  |  |__ /_/ ||  |\___ \|    \ 
 /______  /____  /____  \__|____/____ ||__|____  \__|_ \
        \/     \/     \/             \/        \/     \/
";

pub struct JobManager {
    running_jobs: std::sync::Mutex<BTreeMap<String, RunningJob>>,
    disk_locks: std::sync::Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Debug, Serialize, Clone)]
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

    pub async fn list_finished_jobs(&self, db: &SqlitePool) -> Result<Vec<JobInfo>, sqlx::Error> {
        sqlx::query_as!(
            JobInfo,
            r#"
            SELECT id, disk, name
            FROM jobs
            ORDER BY rowid DESC
            "#
        )
        .fetch_all(db)
        .await
    }

    pub async fn get_job_info(
        &self,
        id: &str,
        db: &SqlitePool,
    ) -> Result<Option<JobInfo>, sqlx::Error> {
        if let Some(job) = self.running_jobs.lock().unwrap().get(id) {
            return Ok(Some(job.info.clone()));
        }

        sqlx::query_as!(
            JobInfo,
            r#"
            SELECT id, disk, name
            FROM jobs
            WHERE id = $1
            LIMIT 1
            "#,
            id
        )
        .fetch_optional(db)
        .await
    }

    pub async fn subscribe_log(
        &self,
        id: &str,
        db: &SqlitePool,
    ) -> Result<Option<(String, Option<broadcast::Receiver<String>>)>, sqlx::Error> {
        {
            let lock = self.running_jobs.lock().unwrap();

            if let Some(job) = lock.get(id) {
                let lock = job.log.lock().unwrap();

                let subscriber = job.incoming.resubscribe();
                return Ok(Some((lock.clone(), Some(subscriber))));
            }
        }

        let log = sqlx::query_scalar!("SELECT log FROM jobs WHERE id = $1 LIMIT 1", id)
            .fetch_optional(db)
            .await?;

        Ok(log.map(|log| (log, None)))
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
        db: SqlitePool,
    ) -> Result<String, TryLockError> {
        let device = job.get_device();

        let drive_lock = self.lock_disk(device)?;

        let (send, recv) = broadcast::channel(3);

        let log = Arc::new(std::sync::Mutex::new(BANNER.to_string()));

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
                Err(err) => log
                    .lock()
                    .unwrap()
                    .push_str(&format!("Job task failed: {err}\n")),
                Ok(Err(err)) => log
                    .lock()
                    .unwrap()
                    .push_str(&format!("Job failed: {err}\n")),
                Ok(Ok(_)) => {}
            }

            let rjob = {
                let mut running_jobs = self2.running_jobs.lock().unwrap();
                running_jobs.remove(&id).unwrap()
            };

            let log = rjob.log.lock().unwrap().clone();

            if let Err(err) = sqlx::query!(
                "INSERT INTO jobs (id, disk, name, log) VALUES ($1, $2, $3, $4)",
                rjob.info.id,
                rjob.info.disk,
                rjob.info.name,
                log
            )
            .execute(&db)
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
