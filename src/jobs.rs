use std::{collections::BTreeMap, future::Future, sync::Arc};

use serde::Serialize;
use tokio::sync::{broadcast, mpsc, oneshot, watch, TryLockError};

pub struct JobManager {
    running_jobs: std::sync::Mutex<BTreeMap<uuid::Uuid, RunningJob>>,
    disk_locks: std::sync::Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Serialize, Clone)]
pub struct JobInfo {
    id: uuid::Uuid,
    disk: String,
    name: String,
}

pub struct RunningJob {
    info: JobInfo,
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

    pub async fn run_job<J: Job>(self: Arc<Self>, job: J) -> Result<(), TryLockError> {
        let device = job.get_device();

        let lock = self.lock_disk(device)?;

        let (send, recv) = broadcast::channel(3);

        let rjob = RunningJob {
            info: JobInfo {
                id: uuid::Uuid::new_v4(),
                disk: device.to_string(),
                name: job.get_name(),
            },
            incoming: recv,
        };

        let mut running_jobs = self.running_jobs.lock().unwrap();
        running_jobs.insert(rjob.info.id, rjob);

        tokio::spawn(async move {
            let _lock = lock;
            let logger = send;
            let mut log_receiver = logger.subscribe();
            let job = job;

            let job_handle = tokio::spawn(async move { job.run(logger).await });

            let mut log = Vec::new();

            while let Ok(content) = log_receiver.recv().await {
                log.push(content);
            }

            let result = job_handle.await;

            todo!()
        });

        Ok(())
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
