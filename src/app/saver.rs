//! Writes files on a background thread, so saving a large store never
//! stalls drawing, input or the web UI.
//!
//! The event loop hands over a snapshot and a function that encodes it;
//! the worker encodes and writes. One worker writes in order, so an older
//! snapshot never replaces a newer one, and when several saves of the same
//! file are waiting only the newest is written.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;

use anyhow::Result;

type Encode = Box<dyn FnOnce() -> Result<Vec<u8>> + Send>;

enum Job {
    Write { path: PathBuf, what: String, encode: Encode },
    Remove(PathBuf),
}

impl Job {
    fn path(&self) -> &PathBuf {
        match self {
            Job::Write { path, .. } | Job::Remove(path) => path,
        }
    }
}

pub struct Saver {
    jobs: Option<mpsc::Sender<Job>>,
    failures: mpsc::Receiver<String>,
    worker: Option<JoinHandle<()>>,
}

impl Saver {
    pub fn new() -> Self {
        let (jobs, queue) = mpsc::channel::<Job>();
        let (report, failures) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("rettui-saver".into())
            .spawn(move || {
                while let Ok(first) = queue.recv() {
                    // Everything already waiting, keeping the newest job per file.
                    let mut batch = vec![first];
                    batch.extend(queue.try_iter());
                    let mut latest: Vec<Job> = Vec::with_capacity(batch.len());
                    for job in batch {
                        latest.retain(|queued| queued.path() != job.path());
                        latest.push(job);
                    }
                    for job in latest {
                        if let Err(e) = run(job) {
                            let _ = report.send(e);
                        }
                    }
                }
            })
            .expect("spawning the save thread");
        Self {
            jobs: Some(jobs),
            failures,
            worker: Some(worker),
        }
    }

    /// Encode and write `path` in the background; `what` names it in errors.
    pub fn write(&self, path: PathBuf, what: impl Into<String>, encode: impl FnOnce() -> Result<Vec<u8>> + Send + 'static) {
        self.send(Job::Write {
            path,
            what: what.into(),
            encode: Box::new(encode),
        });
    }

    /// Delete `path` after any writes to it that are already waiting.
    pub fn remove(&self, path: PathBuf) {
        self.send(Job::Remove(path));
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            // The worker only stops when `jobs` is dropped.
            let _ = jobs.send(job);
        }
    }

    /// Errors from finished saves, for the log.
    pub fn failures(&self) -> Vec<String> {
        self.failures.try_iter().collect()
    }

    /// Wait for every waiting save to be written (before exiting).
    pub fn finish(&mut self) {
        self.jobs = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        self.finish();
    }
}

fn run(job: Job) -> Result<(), String> {
    match job {
        Job::Write { path, what, encode } => encode()
            .and_then(|bytes| crate::config::write_atomic(&path, &bytes))
            .map_err(|e| format!("Could not save {what}: {e}")),
        Job::Remove(path) => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("Could not delete {}: {e}", path.display())),
            _ => Ok(()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_in_order_and_keeps_the_newest() {
        let dir = std::env::temp_dir().join(format!("rettui-saver-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        let mut saver = Saver::new();
        for n in 0..50 {
            saver.write(file.clone(), "a", move || Ok(n.to_string().into_bytes()));
        }
        saver.finish();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "49");

        let mut saver = Saver::new();
        saver.write(file.clone(), "a", || Ok(b"again".to_vec()));
        saver.remove(file.clone());
        saver.write(dir.join("b.txt"), "b", || anyhow::bail!("no"));
        saver.finish();
        assert!(!file.exists());
        assert_eq!(saver.failures(), vec!["Could not save b: no".to_string()]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
