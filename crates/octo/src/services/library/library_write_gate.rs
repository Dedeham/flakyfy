//! One writer at a time over the music folder.
//!
//! The jobs that rewrite many files (the library tidy, the genre re-tag, the explicit marking)
//! each take the gate whole: a second job is refused while one runs, and the dashboard says
//! which one. A download being placed and lyrics being written beside a song take it shared:
//! many at once, but never while a job holds it, so a file never lands in a folder that is
//! being moved or gets its tags written twice at once. A job waits for the placements already
//! under way to finish before it starts.

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

/// The gate. Clones share it.
#[derive(Clone, Default)]
pub struct LibraryWriteGate {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    lock: Arc<RwLock<()>>,
    /// The job holding the gate, in the words the dashboard shows.
    job: Mutex<Option<String>>,
}

/// A job's hold on the gate. Dropping it lets the gate go.
pub struct LibraryJob {
    gate: LibraryWriteGate,
    _write: OwnedRwLockWriteGuard<()>,
}

impl Drop for LibraryJob {
    fn drop(&mut self) {
        *self.gate.inner.job.lock() = None;
    }
}

/// A placement's or a lyrics write's share of the gate.
pub type LibraryWrite = OwnedRwLockReadGuard<()>;

impl LibraryWriteGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// The job writing to the library now, if any.
    pub fn job(&self) -> Option<String> {
        self.inner.job.lock().clone()
    }

    /// The refusal for a job that would start while another holds the gate.
    pub fn busy_message(holder: &str) -> String {
        format!("{holder} is changing the music folder right now. Try again when it is done.")
    }

    /// Take the gate for a job named `name`. Refused, with the running job's name, when
    /// another job holds it; otherwise waits for placements under way and holds the gate.
    pub async fn begin_job(&self, name: &str) -> Result<LibraryJob, String> {
        {
            let mut job = self.inner.job.lock();
            if let Some(holder) = job.as_ref() {
                return Err(holder.clone());
            }
            *job = Some(name.to_string());
        }
        let write = Arc::clone(&self.inner.lock).write_owned().await;
        Ok(LibraryJob {
            gate: self.clone(),
            _write: write,
        })
    }

    /// A share of the gate for one placement or one lyrics write: waits while a job holds it.
    pub async fn write(&self) -> LibraryWrite {
        Arc::clone(&self.inner.lock).read_owned().await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_second_job_is_refused_with_the_running_jobs_name() {
        let gate = LibraryWriteGate::new();
        let tidy = gate.begin_job("Tidy artists").await.expect("free");

        let second = tokio::time::timeout(Duration::from_secs(1), gate.begin_job("The genre re-tag"))
            .await
            .expect("refused at once rather than waiting");
        assert_eq!(second.err().as_deref(), Some("Tidy artists"));
        assert_eq!(gate.job().as_deref(), Some("Tidy artists"));

        drop(tidy);
        assert_eq!(gate.job(), None);
        assert!(gate.begin_job("The genre re-tag").await.is_ok());
    }

    #[tokio::test]
    async fn a_placement_waits_while_a_job_writes() {
        let gate = LibraryWriteGate::new();
        let job = gate.begin_job("Tidy artists").await.expect("free");

        let waiting = tokio::time::timeout(Duration::from_millis(30), gate.write()).await;
        assert!(waiting.is_err(), "a placement went ahead during a job");

        drop(job);
        let placed = tokio::time::timeout(Duration::from_secs(1), gate.write()).await;
        assert!(placed.is_ok());
    }

    #[tokio::test]
    async fn a_job_waits_for_placements_under_way() {
        let gate = LibraryWriteGate::new();
        let placing = gate.write().await;
        let other = gate.clone();
        let job = tokio::spawn(async move { other.begin_job("Tidy artists").await.map(|_| ()) });

        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !job.is_finished(),
            "the job started while a download was being placed"
        );
        drop(placing);
        assert!(job.await.expect("joined").is_ok());
    }
}
