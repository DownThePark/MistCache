use log::{debug, info, warn};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const GB: u64 = 1024 * 1024 * 1024;

// One cached file: its size and when it was last used
struct Entry {
    size: u64,
    tick: u64,
}

// In-memory record of what's on disk, ordered by last use
#[derive(Default)]
struct Inner {
    entries: HashMap<PathBuf, Entry>,
    // Maps tick -> path. The lowest tick is the least recently used file.
    order: BTreeMap<u64, PathBuf>,
    total_bytes: u64,
    next_tick: u64,
}

impl Inner {
    // Adds a file (or replaces it) as the most recently used
    fn insert(&mut self, path: PathBuf, size: u64) {
        self.next_tick += 1;
        let tick = self.next_tick;
        // If the path was already tracked, remove the old size first
        if let Some(old) = self.entries.insert(path.clone(), Entry { size, tick }) {
            self.order.remove(&old.tick);
            self.total_bytes -= old.size;
        }
        self.order.insert(tick, path);
        self.total_bytes += size;
    }

    // Moves a file to the most recently used position
    fn touch(&mut self, path: &Path) {
        let Some(entry) = self.entries.get_mut(path) else { return };
        if let Some(p) = self.order.remove(&entry.tick) {
            self.next_tick += 1;
            entry.tick = self.next_tick;
            self.order.insert(entry.tick, p);
        }
    }

    // Drops the least recently used files from the record until the total
    // fits under the limit. Returns them so the caller can delete the files.
    fn take_victims(&mut self, limit: u64) -> Vec<(PathBuf, u64)> {
        let mut victims = Vec::new();
        while self.total_bytes > limit {
            let Some((_, path)) = self.order.pop_first() else { break };
            if let Some(entry) = self.entries.remove(&path) {
                self.total_bytes -= entry.size;
                victims.push((path, entry.size));
            }
        }
        victims
    }
}

// Keeps track of the cache's total size and deletes the least recently used
// files once the size limit is exceeded. A limit of 0 turns this all off.
pub struct CacheIndex {
    max_bytes: u64,
    inner: Mutex<Inner>,
}

impl CacheIndex {
    pub fn load(cache_dir: &Path, max_gb: u64) -> Self {
        Self::load_bytes(cache_dir, max_gb.saturating_mul(GB))
    }

    // Scans the cache folder at startup and trims it if it's over the limit
    fn load_bytes(cache_dir: &Path, max_bytes: u64) -> Self {
        let index = Self { max_bytes, inner: Mutex::new(Inner::default()) };
        // No limit means nothing to track, so skip the scan
        if max_bytes == 0 {
            info!("Cache index: disabled (no size limit set), skipping scan");
            return index;
        }

        // Logged before scanning since large caches can take a while
        info!("Loading cache entries from {} ...", cache_dir.display());
        let started = Instant::now();
        let mut files = Vec::new();
        scan_dir(cache_dir, &mut files);
        // Oldest first, so usage order from before the restart is kept
        files.sort_by_key(|f| f.2);
        {
            let mut inner = index.lock();
            for (path, size, _) in files {
                inner.insert(path, size);
            }
            info!(
                "Cache index loaded: {} entries, {:.2} GB used ({:.1}s)",
                inner.entries.len(),
                inner.total_bytes as f64 / GB as f64,
                started.elapsed().as_secs_f64()
            );
        }
        index.purge();
        index
    }

    fn enabled(&self) -> bool {
        self.max_bytes > 0
    }

    // Locks the index (still usable even if another thread panicked)
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // Called when a new file finishes downloading. Adds it, then deletes
    // old files if the cache is now too big.
    pub fn record(&self, path: PathBuf, size: u64) {
        if !self.enabled() {
            return;
        }
        self.lock().insert(path, size);
        self.purge();
    }

    // Called on a cache hit to mark the file as recently used
    pub fn touch(&self, path: &Path) {
        if !self.enabled() {
            return;
        }
        self.lock().touch(path);
        // Also update the file's modified time, so the order survives a restart
        let _ = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(SystemTime::now()));
    }

    // Deletes the least recently used files until the cache fits the limit
    fn purge(&self) {
        // Pick files to remove, then release the lock before touching the disk
        let (victims, total) = {
            let mut inner = self.lock();
            let victims = inner.take_victims(self.max_bytes);
            (victims, inner.total_bytes)
        };
        if victims.is_empty() {
            return;
        }

        // Delete the files; a file that's already gone isn't an error
        let mut freed = 0u64;
        for (path, size) in &victims {
            match fs::remove_file(path) {
                Ok(()) => {
                    freed += size;
                    debug!("Evicted {} ({} KB)", path.display(), size / 1024);
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => warn!("Could not evict {}: {}", path.display(), e),
            }
        }
        info!(
            "Cache over limit: purged {} entries ({:.1} MB), now {:.2} / {:.2} GB",
            victims.len(),
            freed as f64 / (1024.0 * 1024.0),
            total as f64 / GB as f64,
            self.max_bytes as f64 / GB as f64
        );
    }
}

// Walks the cache folder and collects (path, size, modified time) for each
// file. Leftover .temp files from interrupted downloads are deleted.
fn scan_dir(dir: &Path, out: &mut Vec<(PathBuf, u64, SystemTime)>) {
    let Ok(read_dir) = fs::read_dir(dir) else { return };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            scan_dir(&path, out);
        } else if file_type.is_file() {
            // Unfinished download from a previous run
            if path.extension().is_some_and(|e| e == "temp") {
                let _ = fs::remove_file(&path);
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                out.push((path, meta.len(), meta.modified().unwrap_or(UNIX_EPOCH)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    // Makes a fresh, empty folder for each test
    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lancache-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // Creates a file of the given size inside the folder
    fn write(dir: &Path, name: &str, size: usize) -> PathBuf {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![0u8; size]).unwrap();
        path
    }

    // Going over the limit deletes the oldest file first
    #[test]
    fn evicts_oldest_first() {
        let dir = temp_dir();
        let index = CacheIndex::load_bytes(&dir, 3000);
        let a = write(&dir, "a", 1000);
        let b = write(&dir, "b", 1000);
        let c = write(&dir, "c", 1000);
        index.record(a.clone(), 1000);
        index.record(b.clone(), 1000);
        index.record(c.clone(), 1000);
        assert!(a.exists() && b.exists() && c.exists());

        let d = write(&dir, "d", 1000);
        index.record(d.clone(), 1000);
        assert!(!a.exists(), "oldest entry should be purged");
        assert!(b.exists() && c.exists() && d.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // A file that was just used survives, and the next oldest is deleted
    #[test]
    fn hit_protects_entry_from_eviction() {
        let dir = temp_dir();
        let index = CacheIndex::load_bytes(&dir, 2000);
        let a = write(&dir, "a", 1000);
        let b = write(&dir, "b", 1000);
        index.record(a.clone(), 1000);
        index.record(b.clone(), 1000);
        index.touch(&a);

        let c = write(&dir, "c", 1000);
        index.record(c.clone(), 1000);
        assert!(a.exists(), "recently used entry should survive");
        assert!(!b.exists(), "least recently used entry should be purged");
        assert!(c.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // Startup removes old files over the limit and deletes leftover .temp files
    #[test]
    fn startup_scan_purges_and_cleans_temp_files() {
        let dir = temp_dir();
        write(&dir, "depot/1/old", 1000);
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&dir, "depot/1/new", 1000);
        let stale = write(&dir, "depot/1/partial.temp", 500);

        let _index = CacheIndex::load_bytes(&dir, 1500);
        assert!(!dir.join("depot/1/old").exists());
        assert!(dir.join("depot/1/new").exists());
        assert!(!stale.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // Recording the same file twice counts its size only once
    #[test]
    fn rerecording_a_path_does_not_double_count() {
        let dir = temp_dir();
        let index = CacheIndex::load_bytes(&dir, 1500);
        let a = write(&dir, "a", 1000);
        index.record(a.clone(), 1000);
        index.record(a.clone(), 1000);
        assert!(a.exists());
        assert_eq!(index.lock().total_bytes, 1000);
        let _ = fs::remove_dir_all(&dir);
    }

    // With a limit of 0, nothing is tracked or deleted
    #[test]
    fn zero_limit_disables_tracking() {
        let dir = temp_dir();
        let index = CacheIndex::load_bytes(&dir, 0);
        let a = write(&dir, "a", 1000);
        index.record(a.clone(), 1000);
        assert!(a.exists());
        assert_eq!(index.lock().total_bytes, 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
