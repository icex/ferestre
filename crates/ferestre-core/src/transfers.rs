//! A durable install queue. Contains decisions and paths, never licenses or URLs.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const LIMIT: usize = 1024 * 1024;
const MAX_JOBS: usize = 100;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Running,
    Paused,
    Failed,
}
impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Queued",
            Self::Running => "Downloading",
            Self::Paused => "Paused",
            Self::Failed => "Needs attention",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub product_id: String,
    pub name: String,
    pub destination: Option<PathBuf>,
    pub updating: bool,
    pub status: Status,
}
impl Job {
    pub fn new(id: &str, name: &str, destination: Option<&Path>, updating: bool) -> Self {
        Self {
            product_id: id.to_ascii_uppercase(),
            name: name.into(),
            destination: destination.map(Path::to_path_buf),
            updating,
            status: Status::Pending,
        }
    }
    fn valid(&self) -> bool {
        self.product_id.len() == 12
            && self.product_id.bytes().all(|b| b.is_ascii_alphanumeric())
            && !self.name.is_empty()
            && self.name.len() <= 1024
            && self
                .destination
                .as_ref()
                .is_none_or(|p| p.is_absolute() && p.as_os_str().len() <= 4096)
    }
}
#[derive(Default, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Queue {
    schema: u32,
    pub jobs: Vec<Job>,
}
impl Queue {
    pub fn load(state: &Path) -> Result<Self> {
        let path = state.join("install-queue.json");
        let mut queue = match private_read(&path, LIMIT)? {
            None => Self::default(),
            Some(bytes) => {
                serde_json::from_slice::<Self>(&bytes).context("Reading install queue")?
            }
        };
        if queue.schema > 1
            || queue.jobs.len() > MAX_JOBS
            || queue.jobs.iter().any(|job| !job.valid())
        {
            bail!("Invalid install queue");
        }
        let mut ids = std::collections::BTreeSet::new();
        if queue.jobs.iter().any(|j| !ids.insert(j.product_id.clone())) {
            bail!("Duplicate install queue entry");
        }
        // A new window never starts a network transfer on its own. Even queued
        // work waits for Resume after a restart or an unexpected process exit.
        for job in &mut queue.jobs {
            if matches!(job.status, Status::Running | Status::Pending) {
                job.status = Status::Paused;
            }
        }
        Ok(queue)
    }
    pub fn save(&self, state: &Path) -> Result<()> {
        let value = serde_json::json!({"schema":1,"jobs":self.jobs});
        private_write(state, "install-queue.json", &serde_json::to_vec(&value)?)
    }
    pub fn enqueue(&mut self, job: Job) -> Result<bool> {
        if !job.valid() {
            bail!("Invalid install request");
        }
        if self.jobs.iter().any(|j| j.product_id == job.product_id) {
            return Ok(false);
        }
        if self.jobs.len() >= MAX_JOBS {
            bail!("The install queue is full");
        }
        self.jobs.push(job);
        Ok(true)
    }
    pub fn set_status(&mut self, id: &str, status: Status) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.product_id == id) {
            job.status = status;
        }
    }
    /// Continue the existing decision without replacing its destination or kind.
    pub fn resume(&mut self, id: &str) -> bool {
        let Some(job) = self
            .jobs
            .iter_mut()
            .find(|j| j.product_id.eq_ignore_ascii_case(id))
        else {
            return false;
        };
        if job.status != Status::Running {
            job.status = Status::Pending;
        }
        true
    }
    pub fn remove(&mut self, id: &str) {
        self.jobs.retain(|j| j.product_id != id);
    }
    pub fn next(&self) -> Option<Job> {
        self.jobs
            .iter()
            .find(|j| j.status == Status::Pending)
            .cloned()
    }
}

pub(crate) fn private_read(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
        bail!("Unsafe private state file");
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        bail!("Private state file is too large");
    }
    Ok(Some(bytes))
}
pub(crate) fn private_write(state: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() > LIMIT {
        bail!("Private state is too large");
    }
    fs::create_dir_all(state)?;
    if let Ok(meta) = fs::symlink_metadata(state.join(name)) {
        use std::os::unix::fs::MetadataExt;
        if !meta.is_file() || meta.nlink() != 1 || meta.uid() != unsafe { libc::geteuid() } {
            bail!("Unsafe private state file");
        }
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let temp = state.join(format!(".{name}.{}.{nonce}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
        fs::rename(&temp, state.join(name))?;
        fs::File::open(state)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_keeps_order_and_destinations_but_requires_explicit_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let mut q = Queue::default();
        q.enqueue(Job::new(
            "9ZZTESTGAME1",
            "A",
            Some(Path::new("/games/private")),
            true,
        ))
        .unwrap();
        q.enqueue(Job::new("9ZZTESTGAME2", "B", None, false))
            .unwrap();
        q.set_status("9ZZTESTGAME1", Status::Running);
        q.save(tmp.path()).unwrap();
        let mut restored = Queue::load(tmp.path()).unwrap();
        assert!(restored.next().is_none());
        assert_eq!(restored.jobs[0].destination, Some("/games/private".into()));
        assert!(restored.jobs.iter().all(|j| j.status == Status::Paused));
        restored.set_status("9ZZTESTGAME2", Status::Pending);
        assert_eq!(restored.next().unwrap().name, "B");
    }
    #[test]
    fn duplicate_queue_requests_cannot_change_a_running_destination() {
        let mut q = Queue::default();
        q.enqueue(Job::new(
            "9ZZTESTGAME1",
            "A",
            Some(Path::new("/games/original")),
            false,
        ))
        .unwrap();
        assert!(!q
            .enqueue(Job::new(
                "9ZZTESTGAME1",
                "A",
                Some(Path::new("/games/other")),
                false
            ))
            .unwrap());
        assert_eq!(q.jobs[0].destination, Some("/games/original".into()));
    }
    #[test]
    fn resume_preserves_update_destination_and_only_schedules_the_selected_job() {
        let mut queue = Queue::default();
        queue
            .enqueue(Job::new(
                "9ZZTESTGAME1",
                "Update",
                Some(Path::new("/games/custom/title")),
                true,
            ))
            .unwrap();
        queue
            .enqueue(Job::new(
                "9ZZTESTGAME2",
                "Other",
                Some(Path::new("/games/other")),
                true,
            ))
            .unwrap();
        queue.set_status("9ZZTESTGAME1", Status::Paused);
        queue.set_status("9ZZTESTGAME2", Status::Paused);
        assert!(queue.resume("9zztestgame1"));
        let resumed = queue.next().unwrap();
        assert!(resumed.updating);
        assert_eq!(
            resumed.destination,
            Some(PathBuf::from("/games/custom/title"))
        );
        assert_eq!(queue.jobs[1].status, Status::Paused);
        assert!(!queue.resume("9ZZUNKNOWN00"));
        assert_eq!(queue.jobs.len(), 2);
    }
    #[test]
    fn unsafe_or_malformed_queue_is_not_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("keep");
        fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, tmp.path().join("install-queue.json")).unwrap();
        assert!(Queue::load(tmp.path()).is_err());
        assert!(Queue::default().save(tmp.path()).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"keep");
        assert!(Queue::default()
            .enqueue(Job::new("invalid", "A", None, false))
            .is_err());
    }
}
