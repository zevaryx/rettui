//! Writes files on a background thread, so saving a large store never
//! stalls drawing, input or the web UI.
//!
//! The event loop hands over a snapshot and a function that encodes it;
//! the worker encodes and writes. One worker writes in order, so an older
//! snapshot never replaces a newer one, and when several saves of the same
//! file are waiting only the newest is written. Appends (the message
//! archive) and other tasks (keeping it within the storage limit) all run,
//! in order.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;

use anyhow::Result;

type Encode = Box<dyn FnOnce() -> Result<Vec<u8>> + Send>;
type Task = Box<dyn FnOnce() -> Result<Vec<String>> + Send>;

enum Job {
    Write {
        path: PathBuf,
        what: String,
        encode: Encode,
    },
    /// Added to the end of the file; never replaced by a later job.
    Append {
        path: PathBuf,
        what: String,
        encode: Encode,
    },
    Remove(PathBuf),
    /// Anything else, in turn; what it returns goes to the log.
    Run {
        what: String,
        task: Task,
    },
}

impl Job {
    /// The file a later write or removal of it makes this one pointless for.
    fn replaceable(&self) -> Option<&PathBuf> {
        match self {
            Job::Write { path, .. } | Job::Remove(path) => Some(path),
            Job::Append { .. } | Job::Run { .. } => None,
        }
    }
}

/// What finished saves have to say.
#[derive(Debug, PartialEq)]
pub enum Report {
    Failed(String),
    /// For the log.
    Note(String),
}

pub struct Saver {
    jobs: Option<mpsc::Sender<Job>>,
    reports: mpsc::Receiver<Report>,
    worker: Option<JoinHandle<()>>,
}

impl Saver {
    pub fn new() -> Self {
        let (jobs, queue) = mpsc::channel::<Job>();
        let (report, reports) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("rettui-saver".into())
            .spawn(move || {
                while let Ok(first) = queue.recv() {
                    // Everything already waiting, keeping the newest write
                    // or removal per file, and every append and task.
                    let mut batch = vec![first];
                    batch.extend(queue.try_iter());
                    let mut latest: Vec<Job> = Vec::with_capacity(batch.len());
                    for job in batch {
                        // A newer write or removal replaces an older one of
                        // the same file (it goes last, after any appends
                        // queued meanwhile), unless a task waits between
                        // them: tasks look at the files as they are then.
                        if let Some(path) = job.replaceable() {
                            let older = latest.iter().rposition(|queued| queued.replaceable() == Some(path));
                            if let Some(at) = older
                                && !latest[at..].iter().any(|queued| matches!(queued, Job::Run { .. }))
                            {
                                latest.remove(at);
                            }
                        }
                        latest.push(job);
                    }
                    for job in latest {
                        match run(job) {
                            Ok(notes) => notes.into_iter().for_each(|note| {
                                let _ = report.send(Report::Note(note));
                            }),
                            Err(e) => {
                                let _ = report.send(Report::Failed(e));
                            }
                        }
                    }
                }
            })
            .expect("spawning the save thread");
        Self { jobs: Some(jobs), reports, worker: Some(worker) }
    }

    /// Encode and write `path` in the background; `what` names it in errors.
    pub fn write(&self, path: PathBuf, what: impl Into<String>, encode: impl FnOnce() -> Result<Vec<u8>> + Send + 'static) {
        self.send(Job::Write { path, what: what.into(), encode: Box::new(encode) });
    }

    /// Encode and add to the end of `path` in the background (creating it);
    /// `what` names it in errors.
    pub fn append(&self, path: PathBuf, what: impl Into<String>, encode: impl FnOnce() -> Result<Vec<u8>> + Send + 'static) {
        self.send(Job::Append { path, what: what.into(), encode: Box::new(encode) });
    }

    /// Run `task` in the background after what's already waiting; the lines
    /// it returns go to the log. `what` names it in errors.
    pub fn run(&self, what: impl Into<String>, task: impl FnOnce() -> Result<Vec<String>> + Send + 'static) {
        self.send(Job::Run { what: what.into(), task: Box::new(task) });
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

    /// Errors and notes from finished saves, for the log.
    pub fn reports(&self) -> Vec<Report> {
        self.reports.try_iter().collect()
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

fn run(job: Job) -> Result<Vec<String>, String> {
    match job {
        Job::Write { path, what, encode } => encode()
            .and_then(|bytes| crate::config::write_atomic(&path, &bytes))
            .map(|()| Vec::new())
            .map_err(|e| format!("Could not save {what}: {e}")),
        Job::Append { path, what, encode } => encode()
            .and_then(|bytes| {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
                file.write_all(&bytes)?;
                Ok(Vec::new())
            })
            .map_err(|e| format!("Could not save {what}: {e}")),
        Job::Remove(path) => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("Could not delete {}: {e}", path.display())),
            _ => Ok(Vec::new()),
        },
        Job::Run { what, task } => task().map_err(|e| format!("Could not {what}: {e}")),
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
        assert_eq!(saver.reports(), vec![Report::Failed("Could not save b: no".to_string())]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn appends_are_all_written_in_order() {
        let dir = std::env::temp_dir().join(format!("rettui-saver-append-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log.txt");
        let other = dir.join("other.txt");
        let mut saver = Saver::new();
        for n in 0..50 {
            saver.append(log.clone(), "log", move || Ok(format!("{n},").into_bytes()));
            // Writes to another file in between are still coalesced.
            saver.write(other.clone(), "other", move || Ok(n.to_string().into_bytes()));
        }
        // Tasks run after what was queued before them, and report.
        let seen = log.clone();
        saver.run("check", move || Ok(vec![format!("{} bytes", std::fs::metadata(&seen)?.len())]));
        saver.finish();
        let expected: String = (0..50).map(|n| format!("{n},")).collect();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), expected);
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "49");
        assert_eq!(saver.reports(), vec![Report::Note(format!("{} bytes", expected.len()))]);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn tasks_see_the_writes_queued_before_them() {
        let dir = std::env::temp_dir().join(format!("rettui-saver-task-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.txt");
        let mut saver = Saver::new();
        // The rest queue up meanwhile, and are handled as one batch.
        saver.run("wait", || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            Ok(Vec::new())
        });
        saver.write(file.clone(), "f", || Ok(b"first".to_vec()));
        let seen = file.clone();
        saver.run("look", move || Ok(vec![std::fs::read_to_string(&seen)?]));
        saver.write(file.clone(), "f", || Ok(b"second".to_vec()));
        saver.finish();
        assert_eq!(saver.reports(), vec![Report::Note("first".into())]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "second");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
