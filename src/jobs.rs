use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use sqlx::SqlitePool;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
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
pub const JOB_PAGE_SIZE: i64 = 20;
pub(crate) type FinalLogSuccessDataFn =
    fn(String) -> Pin<Box<dyn Future<Output = Option<FinalLogSuccessData>> + Send>>;

pub(crate) struct FinalLogSuccessData {
    pub log: String,
    pub timestamp_request: Option<Vec<u8>>,
    pub timestamp_response: Option<Vec<u8>>,
}

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
    timestamp_request: Arc<std::sync::Mutex<Option<Vec<u8>>>>,
    timestamp_response: Arc<std::sync::Mutex<Option<Vec<u8>>>>,
    incoming: broadcast::Receiver<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobPage {
    pub jobs: Vec<JobInfo>,
    pub page: i64,
    pub page_size: i64,
    pub total_jobs: i64,
    pub total_pages: i64,
    pub has_previous: bool,
    pub has_next: bool,
    pub previous_page: i64,
    pub next_page: i64,
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

    pub async fn list_running_jobs_filtered(&self, search: Option<&str>) -> Vec<JobInfo> {
        let search = search.map(str::trim).filter(|value| !value.is_empty());

        self.running_jobs
            .lock()
            .unwrap()
            .values()
            .filter(|job| {
                let Some(search) = search else {
                    return true;
                };

                job.info.name.contains(search)
                    || job.info.disk.contains(search)
                    || job.log.lock().unwrap().contains(search)
            })
            .map(|job| &job.info)
            .cloned()
            .collect()
    }

    pub async fn list_finished_jobs_page(
        &self,
        db: &SqlitePool,
        page: i64,
        search: Option<&str>,
    ) -> Result<JobPage, sqlx::Error> {
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        let total_jobs = if let Some(search) = search {
            sqlx::query_scalar!(
                r#"
                SELECT COUNT(*) as "count!: i64"
                FROM jobs
                WHERE name LIKE '%' || $1 || '%'
                    OR disk LIKE '%' || $1 || '%'
                    OR log LIKE '%' || $1 || '%'
                "#,
                search
            )
            .fetch_one(db)
            .await?
        } else {
            sqlx::query_scalar!("SELECT COUNT(*) as \"count!: i64\" FROM jobs")
                .fetch_one(db)
                .await?
        };

        let total_pages = total_jobs.saturating_add(JOB_PAGE_SIZE - 1) / JOB_PAGE_SIZE;
        let page = page.clamp(1, total_pages.max(1));
        let offset = (page - 1) * JOB_PAGE_SIZE;

        let jobs = if let Some(search) = search {
            sqlx::query_as!(
                JobInfo,
                r#"
                SELECT id, disk, name
                FROM jobs
                WHERE name LIKE '%' || $1 || '%'
                    OR disk LIKE '%' || $1 || '%'
                    OR log LIKE '%' || $1 || '%'
                ORDER BY rowid DESC
                LIMIT $2 OFFSET $3
                "#,
                search,
                JOB_PAGE_SIZE,
                offset
            )
            .fetch_all(db)
            .await?
        } else {
            sqlx::query_as!(
                JobInfo,
                r#"
                SELECT id, disk, name
                FROM jobs
                ORDER BY rowid DESC
                LIMIT $1 OFFSET $2
                "#,
                JOB_PAGE_SIZE,
                offset
            )
            .fetch_all(db)
            .await?
        };

        Ok(Self::build_job_page(jobs, page, total_jobs))
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

    pub async fn get_timestamp_files(
        &self,
        id: &str,
        db: &SqlitePool,
    ) -> Result<Option<JobTimestampFiles>, sqlx::Error> {
        {
            let lock = self.running_jobs.lock().unwrap();

            if let Some(job) = lock.get(id) {
                return Ok(Some(JobTimestampFiles {
                    request: job.timestamp_request.lock().unwrap().clone(),
                    response: job.timestamp_response.lock().unwrap().clone(),
                }));
            }
        }

        sqlx::query_as!(
            JobTimestampFiles,
            r#"
            SELECT timestamp_request as "request?", timestamp_response as "response?"
            FROM jobs
            WHERE id = $1
            LIMIT 1
            "#,
            id
        )
        .fetch_optional(db)
        .await
    }

    fn build_job_page(jobs: Vec<JobInfo>, page: i64, total_jobs: i64) -> JobPage {
        let total_pages = total_jobs.saturating_add(JOB_PAGE_SIZE - 1) / JOB_PAGE_SIZE;
        let total_pages = total_pages.max(1);

        JobPage {
            jobs,
            page,
            page_size: JOB_PAGE_SIZE,
            total_jobs,
            total_pages,
            has_previous: page > 1,
            has_next: page < total_pages,
            previous_page: page.saturating_sub(1).max(1),
            next_page: (page + 1).min(total_pages),
        }
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
        let timestamp_request = Arc::new(std::sync::Mutex::new(None));
        let timestamp_response = Arc::new(std::sync::Mutex::new(None));

        let rjob = RunningJob {
            info: JobInfo {
                id: Uuid::new_v4().to_string(),
                disk: device.to_string(),
                name: job.get_name(),
            },
            incoming: recv,
            log: log.clone(),
            timestamp_request: timestamp_request.clone(),
            timestamp_response: timestamp_response.clone(),
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
            let final_log_success_data = job.final_log_success_data();

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
                Ok(Ok(_)) => {
                    if let Some(final_log_success_data) = final_log_success_data {
                        let extra = {
                            let log = log.lock().unwrap();
                            final_log_success_data(log.clone())
                        };
                        if let Some(extra) = extra.await {
                            log.lock().unwrap().push_str(&extra.log);
                            *timestamp_request.lock().unwrap() = extra.timestamp_request;
                            *timestamp_response.lock().unwrap() = extra.timestamp_response;
                        }
                    }
                }
            }

            let rjob = {
                let mut running_jobs = self2.running_jobs.lock().unwrap();
                running_jobs.remove(&id).unwrap()
            };

            let log = rjob.log.lock().unwrap().clone();
            let timestamp_request = rjob.timestamp_request.lock().unwrap().clone();
            let timestamp_response = rjob.timestamp_response.lock().unwrap().clone();

            if let Err(err) = sqlx::query!(
                "INSERT INTO jobs (id, disk, name, log, timestamp_request, timestamp_response) VALUES ($1, $2, $3, $4, $5, $6)",
                rjob.info.id,
                rjob.info.disk,
                rjob.info.name,
                log,
                timestamp_request,
                timestamp_response
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

#[derive(Debug, Clone)]
pub(crate) struct JobTimestampFiles {
    pub request: Option<Vec<u8>>,
    pub response: Option<Vec<u8>>,
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn format_unix_timestamp(timestamp: i64) -> String {
    OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()
        .and_then(|datetime| datetime.format(&Rfc3339).ok())
        .unwrap_or_else(|| timestamp.to_string())
}

pub trait Job: Send + Sync + 'static {
    fn get_device(&self) -> &str;
    fn get_name(&self) -> String;
    fn final_log_success_data(&self) -> Option<FinalLogSuccessDataFn> {
        None
    }
    fn run(
        self,
        logger: broadcast::Sender<String>,
    ) -> impl Future<Output = Result<(), Box<dyn std::error::Error + Send>>> + Send;
}
