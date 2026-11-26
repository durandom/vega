//! Share paced Windows Git discovery between registry entries and sidebar labels.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime},
};
use vega_herdr::{project, Failure};

type Resolution = Result<Option<project::Project>, Failure>;

#[derive(Clone)]
pub struct Projects {
    entries: Arc<Mutex<HashMap<PathBuf, Entry>>>,
    requests: mpsc::Sender<PathBuf>,
}

struct Entry {
    result: Option<Cached>,
    used_at: Instant,
    queued: bool,
}

struct Cached {
    result: Arc<Resolution>,
    at: Instant,
    config_mtime: Option<SystemTime>,
}

impl Projects {
    pub fn start(host: String) -> Self {
        let entries = Arc::new(Mutex::new(HashMap::<PathBuf, Entry>::new()));
        let (requests, pending) = mpsc::channel::<PathBuf>();
        let cache = Arc::clone(&entries);
        thread::spawn(move || {
            let mut next_git = Instant::now();
            for path in pending {
                let result = project::resolve_optional_paced(&path, &host, || {
                    thread::sleep(next_git.saturating_duration_since(Instant::now()));
                    next_git = Instant::now() + Duration::from_millis(600);
                });
                let config_mtime = config_mtime(&result);
                let mut entries = cache.lock().expect("project cache");
                let entry = entries.get_mut(&path).expect("queued project");
                entry.result = Some(Cached {
                    result: Arc::new(result),
                    config_mtime,
                    at: Instant::now(),
                });
                entry.queued = false;
            }
        });
        Self { entries, requests }
    }

    /// Keep a previous result during refresh. With no result yet, the sidebar
    /// leaves the project label alone and the relay defers that registry entry.
    pub fn lookup(&self, path: &Path) -> Option<Arc<Resolution>> {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("project cache");
        let entry = entries.entry(path.to_path_buf()).or_insert(Entry {
            result: None,
            used_at: now,
            queued: false,
        });
        entry.used_at = now;
        if !entry.queued && entry.result.as_ref().is_none_or(|cached| !cached.fresh(now)) {
            entry.queued = self.requests.send(path.to_path_buf()).is_ok();
        }
        entry.result.as_ref().map(|cached| Arc::clone(&cached.result))
    }

    pub fn prune(&self) {
        self.entries.lock().expect("project cache").retain(|_, entry| {
            entry.queued || entry.used_at.elapsed() < Duration::from_secs(600)
        });
    }
}

impl Cached {
    fn fresh(&self, now: Instant) -> bool {
        let age = now.duration_since(self.at);
        match self.result.as_ref() {
            Ok(Some(_)) => {
                age < Duration::from_secs(600) && config_mtime(&self.result) == self.config_mtime
            }
            _ => age < Duration::from_secs(60),
        }
    }
}

fn config_mtime(result: &Resolution) -> Option<SystemTime> {
    let project = result.as_ref().ok()?.as_ref()?;
    std::fs::metadata(project.location.common_dir.join("config"))
        .and_then(|m| m.modified())
        .ok()
}
