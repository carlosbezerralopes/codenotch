//! Bounded official quota observations. Queries read memory only; discovery is a background tick.
use crate::usage::{canonical_window_kind, quota_key, CreditScope, LimitWindow, QuotaReadingTransport, QuotaResetBasis, UsageSnapshot};
use crate::usage_insights::{BudgetUnit, PeriodInsights, ProviderInsights};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub(crate) struct QuotaHistoryRequest { pub window_index: usize, pub series_key: String, pub from_ms: u64, pub to_ms: u64 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuotaHistoryState { Complete, Warming, Incomplete, Truncated, Conflict, Unavailable }
#[derive(Debug, Clone)]
pub(crate) struct QuotaRateSample { pub series_key: String, pub at_ms: u64, pub value: f64, pub allowance: f64, pub unit: BudgetUnit }
#[derive(Debug, Clone)]
pub(crate) struct QuotaHistoryResult {
    pub state: QuotaHistoryState, pub samples: Vec<QuotaRateSample>, pub complete_horizon: bool,
    pub segment_started_at_ms: Option<u64>, pub revised: bool,
}

pub(crate) fn series_key_for(provider: &str, window: &LimitWindow, period: &PeriodInsights) -> Option<String> {
    let account = window.quota_account_key.as_deref()?;
    let bucket = window.quota_bucket_key.as_deref()?;
    if !opaque_key(account) || !opaque_key(bucket) || window.count.is_some() || window.derived
        || !matches!(window.reset_basis, Some(QuotaResetBasis::Absolute | QuotaResetBasis::OfficialCalendarMonth))
        || !matches!((provider, window.reading_transport),
            ("claude", Some(QuotaReadingTransport::ClaudeUsageHttp)) |
            ("codex", Some(QuotaReadingTransport::CodexHttp)) |
            ("kiro", Some(QuotaReadingTransport::KiroUsageApi))) { return None; }
    let start = period.start_ms?;
    let duration = period.duration_seconds?;
    if duration == 0 || duration > 366 * 86400 || start.checked_add(duration.checked_mul(1000)?)? != period.reset_ms
        || window.resets_at != Some(period.reset_ms) || window.used < 0.0 || window.used > 1.0 || !window.used.is_finite() { return None; }
    let (_, allowance, unit) = official_value(provider, window)?;
    Some(quota_key(&("quota-series-v1", provider, account, bucket, canonical_window_kind(provider, &window.id),
        period.start_ms, period.reset_ms, period.duration_seconds, period.basis, unit, allowance)))
}

fn opaque_key(key: &str) -> bool { key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) }
fn official_value(provider: &str, w: &LimitWindow) -> Option<(f64, f64, BudgetUnit)> {
    if provider == "kiro" {
        let c = w.credits.as_ref()?;
        if w.credit_scope != Some(CreditScope::Plan) || c.allowance <= 0.0 || c.consumed < 0.0
            || !c.allowance.is_finite() || !c.consumed.is_finite() { return None; }
        Some((c.consumed, c.allowance, BudgetUnit::Credits))
    } else if matches!(provider, "claude" | "codex") { Some((w.used * 100.0, 100.0, BudgetUnit::Percent)) }
    else { None }
}

const PER_SERIES: usize = 32768;
const GLOBAL: usize = 131072;
const SERIES: usize = 64;
const TICK_ENTRIES: usize = 512;
const TICK_FILES: usize = 32;
const TICK_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
struct Series {
    points: BTreeMap<u64, QuotaRateSample>, conflicts: BTreeMap<u64, ()>, reset_ms: u64,
    live_from: Option<u64>, truncated_through: Option<u64>,
}
struct Directory { depth: usize, entries: std::fs::ReadDir }
struct Discovery {
    queue: VecDeque<(PathBuf, usize)>, directory: Option<Directory>, requests: Vec<QuotaHistoryRequest>, failed: bool,
    stamps: Vec<(PathBuf, Option<std::time::SystemTime>)>, verifying: bool,
    generation: u64,
}
#[derive(Default)]
struct Changes {
    owned: BTreeMap<PathBuf, ()>, directories: BTreeMap<PathBuf, bool>, pending_owned: BTreeMap<PathBuf, ()>,
    generation: u64, overflow: bool,
}
impl Changes {
    fn event(&mut self, root: &Path, event: notify::Result<notify::Event>) {
        let Ok(event) = event else { self.overflow = true; return; };
        if event.need_rescan() { self.overflow = true; return; }
        if matches!(event.kind, notify::EventKind::Access(_)) { return; }
        if event.paths.is_empty() { self.overflow = true; return; }
        for path in event.paths {
            let Ok(relative) = path.strip_prefix(root) else { self.overflow = true; continue; };
            let parts: Vec<_> = relative.components().collect();
            if parts.first().is_none_or(|p| p.as_os_str() != "machines") { continue; }
            if parts.len() > 3 && parts[3].as_os_str() != "quota-readings-v2" { continue; }
            if path.file_name().and_then(|s| s.to_str()).is_some_and(|s| s.starts_with('.') && s.ends_with(".tmp")) { continue; }
            if self.owned.contains_key(&path) {
                if self.pending_owned.len() >= TICK_ENTRIES { self.overflow = true; }
                else { self.pending_owned.insert(path, ()); }
            } else if !(self.directories.contains_key(&path) && matches!(event.kind, notify::EventKind::Modify(notify::event::ModifyKind::Any | notify::event::ModifyKind::Metadata(_)))
                || self.directories.get(&path) == Some(&true) && matches!(event.kind,notify::EventKind::Create(_))) {
                self.generation = self.generation.saturating_add(1);
            }
        }
    }
}

pub(crate) struct QuotaIndex {
    series: HashMap<String, Series>, root: Option<PathBuf>, cache_path: PathBuf,
    pending: Mutex<Vec<QuotaHistoryRequest>>, discovery: Option<Discovery>, proved: Vec<QuotaHistoryRequest>,
    incomplete: bool, evicted: BTreeMap<String, ()>,
    known_files: BTreeMap<PathBuf, (u64, Option<std::time::SystemTime>)>,
    changes: Arc<Mutex<Changes>>, watcher: Option<notify::RecommendedWatcher>, proved_generation: u64,
}
impl QuotaIndex {
    pub(super) fn new(cache_path: PathBuf) -> Self {
        Self { series: HashMap::new(), root: None, cache_path, pending: Mutex::new(vec![]), discovery: None,
            proved: vec![], incomplete: false, evicted: BTreeMap::new(), known_files: BTreeMap::new(),
            changes: Arc::new(Mutex::new(Changes::default())), watcher: None, proved_generation: 0 }
    }
    pub(super) fn set_root(&mut self, root: Option<&str>) {
        let root = root.map(PathBuf::from);
        if root != self.root {
            self.root = root; self.series.clear(); self.proved.clear(); self.discovery = None;
            self.pending.lock().unwrap().clear(); self.evicted.clear(); self.incomplete = false;
            self.known_files.clear();
            self.watcher = None; self.changes = Arc::new(Mutex::new(Changes::default())); self.proved_generation = 0;
        }
    }
    pub(super) fn take_refresh(&mut self) -> Option<Self> {
        self.root.as_ref()?;
        let mut placeholder = Self::new(self.cache_path.clone()); placeholder.root = self.root.clone();
        placeholder.changes = self.changes.clone();
        Some(std::mem::replace(self, placeholder))
    }
    pub(super) fn note_owned_write(&mut self, path: PathBuf) {
        if !self.root.as_ref().is_some_and(|root| path.starts_with(root)) { return; }
        let mut changes = self.changes.lock().unwrap();
        if changes.owned.len() >= GLOBAL && !changes.owned.contains_key(&path) { changes.owned.pop_first(); }
        changes.owned.insert(path.clone(), ());
        // Known own ancestors are allowed, but every foreign final record still invalidates coverage.
        let mut parent = path.parent();
        while let Some(dir) = parent {
            if Some(dir) == self.root.as_deref() { break; }
            if changes.directories.len() >= TICK_ENTRIES && !changes.directories.contains_key(dir) { changes.directories.pop_first(); }
            changes.directories.insert(dir.into(), true); parent = dir.parent();
        }
    }
    pub(super) fn finish_refresh(&mut self, mut completed: Self) -> bool {
        // Configure may select another root while IO runs. Never restore its old namespace.
        if self.root != completed.root { return false; }
        let pending = self.pending.lock().unwrap().clone();
        for (key, mut current) in std::mem::take(&mut self.series) {
            for point in std::mem::take(&mut current.points).into_values() { completed.insert(point, current.reset_ms, true); }
            if let Some(series) = completed.series.get_mut(&key) {
                series.conflicts.extend(current.conflicts);
                series.live_from = match (series.live_from, current.live_from) { (Some(a),Some(b)) => Some(a.min(b)), (a,b) => a.or(b) };
                series.truncated_through = match (series.truncated_through,current.truncated_through) {
                    (Some(a),Some(b)) => Some(a.max(b)), (a,b) => a.or(b)
                };
            }
        }
        { let mut requests = completed.pending.lock().unwrap();
          for r in pending {
            if !requests.iter().any(|p| p.series_key == r.series_key && p.from_ms <= r.from_ms && p.to_ms >= r.to_ms) {
                requests.retain(|p| p.series_key != r.series_key); requests.push(r);
            }
          }
          requests.sort_by(|a,b| b.to_ms.cmp(&a.to_ms).then_with(|| a.series_key.cmp(&b.series_key))); requests.truncate(SERIES);
        }
        *self = completed;
        true
    }
    pub(super) fn ingest(&mut self, provider: &str, snapshot: &UsageSnapshot, base: &ProviderInsights) {
        if snapshot.status != "ok" || base.provider != provider { return; }
        for i in &base.windows {
            let Some(w) = snapshot.windows.get(i.window_index) else { continue; };
            let Some(period) = &i.period else { continue; };
            let Some(key) = series_key_for(provider, w, period) else { continue; };
            let Some((at, status)) = crate::usage_insights::window_origin(provider, snapshot, w) else { continue; };
            if status != "ok" || at == 0 || at > base.generated_at || at < period.start_ms.unwrap_or(u64::MAX) || at >= period.reset_ms { continue; }
            let Some((value, allowance, unit)) = official_value(provider, w) else { continue; };
            self.insert(QuotaRateSample { series_key: key, at_ms: at, value, allowance, unit }, period.reset_ms, true);
        }
    }
    fn insert(&mut self, sample: QuotaRateSample, reset_ms: u64, live: bool) {
        let key = sample.series_key.clone();
        let s = self.series.entry(key.clone()).or_default();
        s.reset_ms = reset_ms;
        if live { s.live_from = Some(s.live_from.unwrap_or(sample.at_ms).min(sample.at_ms)); }
        if let Some(old) = s.points.get(&sample.at_ms) {
            if old.value != sample.value || old.allowance != sample.allowance || old.unit != sample.unit {
                s.conflicts.insert(sample.at_ms, ());
            }
        } else { s.points.insert(sample.at_ms, sample); }
        while s.points.len() > PER_SERIES {
            if let Some((at, _)) = s.points.pop_first() {
                s.truncated_through = Some(s.truncated_through.unwrap_or(0).max(at)); s.conflicts.remove(&at);
            }
        }
        self.enforce_bounds();
    }
    fn enforce_bounds(&mut self) {
        while self.series.len() > SERIES || self.series.values().map(|s| s.points.len()).sum::<usize>() > GLOBAL {
            let victim = self.series.iter().min_by_key(|(k, s)| (s.reset_ms, s.points.last_key_value().map(|(t, _)| *t).unwrap_or(0), (*k).clone()))
                .map(|(k, _)| k.clone()).unwrap();
            self.series.remove(&victim); self.evicted.insert(victim, ());
            self.known_files.clear(); self.proved.clear();
            while self.evicted.len() > SERIES { self.evicted.pop_first(); }
            self.incomplete = true;
        }
    }
    pub(super) fn history(&self, requests: &[QuotaHistoryRequest]) -> HashMap<usize, QuotaHistoryResult> {
        let changes = self.changes.lock().unwrap();
        let changed = changes.overflow || changes.generation != self.proved_generation || !changes.pending_owned.is_empty();
        let mut pending = self.pending.lock().unwrap();
        for r in requests.iter().take(SERIES) {
            if !pending.iter().any(|p| p.series_key == r.series_key && p.from_ms <= r.from_ms && p.to_ms >= r.to_ms) {
                pending.retain(|p| p.series_key != r.series_key); pending.push(r.clone());
            }
        }
        pending.truncate(SERIES);
        requests.iter().take(SERIES).map(|r| {
            let s = self.series.get(&r.series_key);
            let proved = !self.incomplete && !changed && (self.proved.iter().any(|p| p.series_key == r.series_key && p.from_ms <= r.from_ms
                    && (p.to_ms >= r.to_ms || s.and_then(|s| s.live_from).is_some_and(|from| from <= p.to_ms)))
                || (self.root.is_none() && s.and_then(|s| s.live_from).is_some_and(|from| from <= r.from_ms)));
            let truncated = self.evicted.contains_key(&r.series_key) || s.is_some_and(|s| s.truncated_through.is_some_and(|at| r.from_ms <= at));
            let conflict = s.is_some_and(|s| s.conflicts.range(..=r.to_ms).next_back().is_some());
            let mut samples: Vec<_> = s.map(|s| s.points.range(r.from_ms..=r.to_ms).map(|(_, p)| p.clone()).collect()).unwrap_or_default();
            let mut segment = None; let mut revised = false;
            if let Some(s) = s {
                let mut previous: Option<&QuotaRateSample> = None;
                for p in s.points.range(..=r.to_ms).map(|(_, p)| p) {
                    if previous.is_some_and(|old| p.value < old.value) { segment = Some(p.at_ms); revised = true; }
                    previous = Some(p);
                }
            }
            if let Some(start) = segment { samples.retain(|s| s.at_ms >= start); }
            let state = if truncated { QuotaHistoryState::Truncated } else if conflict { QuotaHistoryState::Conflict }
                else if proved { QuotaHistoryState::Complete } else if self.incomplete { QuotaHistoryState::Incomplete }
                else { QuotaHistoryState::Warming };
            (r.window_index, QuotaHistoryResult { state, samples, complete_horizon: proved && !truncated && !conflict,
                segment_started_at_ms: segment, revised })
        }).collect()
    }
    /// All filesystem work is confined to this background method, never history().
    pub(super) fn refresh(&mut self, issues: &mut Vec<String>) {
        let Some(root) = self.root.clone() else { return; };
        if self.changes.lock().unwrap().overflow {
            self.watcher = None; self.proved.clear(); self.incomplete = true;
            let mut changes = self.changes.lock().unwrap(); changes.overflow = false;
            changes.pending_owned.clear(); changes.generation = changes.generation.saturating_add(1);
        }
        if self.watcher.is_none() {
            use notify::Watcher;
            let changes = self.changes.clone(); let watched = root.clone(); let native = native_watch_path(&root);
            let event_base = native.clone();
            if safe_path(&root, &root) {
                self.watcher = notify::recommended_watcher(move |mut event: notify::Result<notify::Event>| {
                    if let Ok(event) = &mut event { for path in &mut event.paths {
                        if let Ok(relative) = path.strip_prefix(&event_base) { *path = watched.join(relative); }
                    }}
                    changes.lock().unwrap().event(&watched, event)
                }).ok().and_then(|mut watcher| watcher.watch(&native, notify::RecursiveMode::Recursive).ok().map(|_| watcher));
            }
        }
        let mut entries = 0; let mut files = 0; let mut bytes = 0;
        // Own records are registered before atomic publication. Validate notifications,
        // including overwrites, before extending a proven horizon with live RAM endpoints.
        while files < TICK_FILES && bytes + 2 * 1024 * 1024 <= TICK_BYTES {
            let path = self.changes.lock().unwrap().pending_owned.pop_first().map(|(path, _)| path);
            let Some(path) = path else { break; };
            files += 1; entries += 1;
            match self.index_file(&root, &path) {
                Ok(size) => bytes += size,
                Err(_) => { self.incomplete = true; self.proved.clear(); self.known_files.remove(&path);
                    let mut changes = self.changes.lock().unwrap(); changes.generation = changes.generation.saturating_add(1); }
            }
        }
        if self.discovery.is_none() {
            let requests = self.pending.lock().unwrap().clone();
            if requests.is_empty() { return; }
            if self.incomplete { self.proved.clear(); }
            self.incomplete = false;
            let mut queue = VecDeque::new(); queue.push_back((root.join("machines"), 0));
            let generation = self.changes.lock().unwrap().generation;
            self.discovery = Some(Discovery { queue, directory: None, requests, failed: self.watcher.is_none(), stamps: vec![], verifying: false, generation });
        }
        let mut d = self.discovery.take().unwrap();
        while entries < TICK_ENTRIES && files < TICK_FILES && bytes + 2 * 1024 * 1024 <= TICK_BYTES {
            if d.verifying {
                let Some((path, original)) = d.stamps.pop() else { break; };
                entries += 1;
                if self.watcher.is_none() && (original.is_none() || std::fs::metadata(path).ok().and_then(|m| m.modified().ok()) != original) { d.failed = true; }
                continue;
            }
            if d.directory.is_none() {
                let Some((path, depth)) = d.queue.pop_front() else { d.verifying = true; continue; };
                if !safe_path(&root, &path) { d.failed = true; continue; }
                { let mut changes=self.changes.lock().unwrap();
                  if changes.directories.len() < TICK_ENTRIES || changes.directories.contains_key(&path) {
                    changes.directories.entry(path.clone()).or_insert(false);
                  } else { d.failed = true; }
                }
                entries += 1;
                if d.stamps.len() >= TICK_ENTRIES { d.failed = true; break; }
                d.stamps.push((path.clone(), std::fs::metadata(&path).ok().and_then(|m| m.modified().ok())));
                match std::fs::read_dir(&path) {
                    Ok(rd) => d.directory = Some(Directory { depth, entries: rd }),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => { d.failed = true; continue; }
                }
            }
            let dir = d.directory.as_mut().unwrap();
            let Some(entry) = dir.entries.next() else { d.directory = None; continue; };
            entries += 1;
            let Ok(entry) = entry else { d.failed = true; continue; };
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { d.failed = true; continue; };
            if !safe_path(&root, &path) { d.failed = true; continue; }
            if meta.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let include = match dir.depth {
                    0 | 1 => true,
                    2 => name == "quota-readings-v2",
                    3 => day_relevant(&name, &d.requests),
                    _ => false,
                };
                if include {
                    if d.queue.len() >= TICK_ENTRIES { d.failed = true; }
                    else { d.queue.push_back((path, dir.depth + 1)); }
                }
            } else if dir.depth == 4 && path.extension().is_some_and(|s| s == "json") {
                if meta.len() > 2 * 1024 * 1024 { d.failed = true; continue; }
                let signature = (meta.len(), meta.modified().ok());
                if self.known_files.get(&path) == Some(&signature) { continue; }
                files += 1;
                bytes += meta.len() as usize;
                if self.index_file(&root, &path).is_err() { d.failed = true; }
            }
        }
        if d.verifying && d.stamps.is_empty() {
            let changes = self.changes.lock().unwrap();
            let retry_for_change = !d.failed && !changes.overflow && changes.generation != d.generation;
            d.failed |= changes.overflow || changes.generation != d.generation;
            drop(changes);
            self.incomplete |= d.failed;
            if !d.failed { self.proved = d.requests; self.proved_generation = d.generation; }
            else { self.proved.clear(); }
            if d.failed { issues.push(if retry_for_change { "Quota history: dataset changed during discovery; continuing bounded scan" }
                else { "Quota history: incomplete journal discovery; records preserved" }.into()); }
            if let Err(_) = self.persist_cache() { self.incomplete = true; issues.push("Quota history: local cache unavailable; journal preserved".into()); }
        } else if d.failed && d.stamps.len() >= TICK_ENTRIES {
            self.incomplete = true; issues.push("Quota history: directory discovery exceeds bounded capacity".into());
        } else { self.discovery = Some(d); }
    }
    fn index_file(&mut self, root: &Path, path: &Path) -> Result<usize, ()> {
        if !safe_path(root, path) { return Err(()); }
        let metadata = std::fs::metadata(path).map_err(|_| ())?;
        if metadata.len() > 2 * 1024 * 1024 { return Err(()); }
        let record = super::quota_journal::read_official(path).map_err(|_| ())?;
        if self.known_files.len() >= GLOBAL && !self.known_files.contains_key(path) { return Err(()); }
        self.known_files.insert(path.into(), (metadata.len(), metadata.modified().ok()));
        if let Some((provider, snapshot)) = record {
            for w in &snapshot.windows {
                let Some((at, status)) = crate::usage_insights::window_origin(&provider, &snapshot, w) else { continue; };
                let one = UsageSnapshot { status: status.into(), fetched_at: at, windows: vec![w.clone()], ..Default::default() };
                let historical = crate::usage_insights::compute_provider_insights(&provider, &one, None, at);
                let Some(period) = &historical.windows[0].period else { continue; };
                let Some(key) = series_key_for(&provider, w, period) else { continue; };
                if status != "ok" || at == 0 || at > snapshot.fetched_at || at < period.start_ms.unwrap_or(u64::MAX) || at >= period.reset_ms { continue; }
                if let Some((value, allowance, unit)) = official_value(&provider, w) {
                    self.insert(QuotaRateSample { series_key: key, at_ms: at, value, allowance, unit }, period.reset_ms, false);
                }
            }
        }
        Ok(metadata.len() as usize)
    }
    fn persist_cache(&self) -> Result<(), rusqlite::Error> {
        match self.persist_cache_once() {
            Err(rusqlite::Error::SqliteFailure(error, _)) if matches!(error.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
                // Only this separate disposable local cache is rebuilt; durable journals are untouched.
                if std::fs::remove_file(&self.cache_path).is_ok() { self.persist_cache_once() }
                else { Err(rusqlite::Error::SqliteFailure(error, None)) }
            }
            result => result,
        }
    }
    fn persist_cache_once(&self) -> Result<(), rusqlite::Error> {
        if let Some(parent) = self.cache_path.parent() { let _ = std::fs::create_dir_all(parent); }
        let mut db = rusqlite::Connection::open(&self.cache_path)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS quota_samples(series TEXT NOT NULL, at_ms INTEGER NOT NULL, value REAL NOT NULL, allowance REAL NOT NULL, unit TEXT NOT NULL, PRIMARY KEY(series,at_ms)); CREATE TABLE IF NOT EXISTS quota_scope(root TEXT NOT NULL);")?;
        let tx = db.transaction()?;
        tx.execute("DELETE FROM quota_samples", [])?; tx.execute("DELETE FROM quota_scope", [])?;
        tx.execute("INSERT INTO quota_scope(root) VALUES(?1)", [self.root.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()])?;
        { let mut insert = tx.prepare("INSERT INTO quota_samples VALUES(?1,?2,?3,?4,?5)")?;
          for s in self.series.values() { for p in s.points.values() {
            insert.execute(rusqlite::params![p.series_key, p.at_ms, p.value, p.allowance, if p.unit == BudgetUnit::Credits { "credits" } else { "percent" }])?;
          }} }
        tx.commit()
    }
}

pub(crate) fn refresh_quota_index(mut job: QuotaIndex) -> (QuotaIndex, Vec<String>) {
    let mut issues = vec![]; job.refresh(&mut issues); (job, issues)
}

fn day_relevant(name: &str, requests: &[QuotaHistoryRequest]) -> bool {
    use chrono::TimeZone;
    let Ok(day) = chrono::NaiveDate::parse_from_str(name, "%Y-%m-%d") else { return false; };
    let start = chrono::Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).unwrap()).timestamp_millis();
    if start < 0 { return false; }
    requests.iter().any(|r| r.from_ms <= start as u64 + 86_399_999 && r.to_ms >= start as u64)
}
fn safe_path(root: &Path, path: &Path) -> bool {
    if !path.starts_with(root) || !supported_path_length(path) { return false; }
    let Ok(relative) = path.strip_prefix(root) else { return false; };
    let mut current = root.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(c) = component {
            if !matches!(c, std::path::Component::Normal(_)) { return false; }
            current.push(c);
        }
        let Ok(metadata) = std::fs::symlink_metadata(&current) else { return false; };
        #[cfg(not(windows))] if metadata.file_type().is_symlink() { return false; }
        #[cfg(windows)] {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 && !reparse_is_safe(reparse_tag(&current)) { return false; }
        }
    }
    true
}
fn supported_path_length(path: &Path) -> bool {
    #[cfg(windows)] { use std::os::windows::ffi::OsStrExt; path.as_os_str().encode_wide().count() < 32760 }
    #[cfg(not(windows))] { path.to_string_lossy().len() < 4096 }
}
fn native_watch_path(path: &Path) -> PathBuf {
    #[cfg(windows)] {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.len() < 248 || wide.starts_with(&[92,92,63,92]) { return path.into(); }
        let mut extended = if wide.starts_with(&[92,92]) { vec![92,92,63,92,85,78,67,92] } else { vec![92,92,63,92] };
        extended.extend_from_slice(if wide.starts_with(&[92,92]) { &wide[2..] } else { &wide });
        PathBuf::from(std::ffi::OsString::from_wide(&extended))
    }
    #[cfg(not(windows))] { path.into() }
}
fn reparse_is_safe(tag: Option<u32>) -> bool {
    // Windows SDK winnt.h IsReparseTagNameSurrogate: CLOUD/CLOUD_1..F are
    // placeholders, while SYMLINK/MOUNT_POINT redirect paths outside the root.
    tag.is_some_and(|tag| tag & 0x2000_0000 == 0)
}
#[cfg(windows)]
fn reparse_tag(path: &Path) -> Option<u32> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    #[repr(C)] struct AttributeTagInfo { attributes: u32, tag: u32 }
    #[link(name="kernel32")] extern "system" {
        fn GetFileInformationByHandleEx(handle: *mut std::ffi::c_void, class: i32,
            data: *mut std::ffi::c_void, size: u32) -> i32;
    }
    let file = std::fs::OpenOptions::new().read(true).custom_flags(0x02000000 | 0x00200000).open(path).ok()?;
    let mut info = AttributeTagInfo { attributes: 0, tag: 0 };
    let result = unsafe { GetFileInformationByHandleEx(file.as_raw_handle(), 9,
        (&mut info as *mut AttributeTagInfo).cast(), std::mem::size_of::<AttributeTagInfo>() as u32) };
    (result != 0).then_some(info.tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{QuotaReadingTransport, QuotaResetBasis};
    fn fixture(at: u64, value: f64) -> (UsageSnapshot, ProviderInsights) {
        let reset = 1_791_806_400_000;
        let window = LimitWindow { id:"secondary".into(), used:value / 100.0, resets_at:Some(reset), duration_seconds:Some(604800),
            quota_account_key:Some("a".repeat(64)), quota_bucket_key:Some("b".repeat(64)),
            reading_transport:Some(QuotaReadingTransport::CodexHttp), reset_basis:Some(QuotaResetBasis::Absolute), ..Default::default() };
        let snapshot = UsageSnapshot { status:"ok".into(), fetched_at:at, windows:vec![window], ..Default::default() };
        let base = crate::usage_insights::compute_provider_insights("codex", &snapshot, None, at);
        (snapshot, base)
    }
    fn at(delta: u64) -> u64 { 1_791_806_400_000 - 604800000 + delta }
    fn request(s: &UsageSnapshot, b: &ProviderInsights, from: u64) -> QuotaHistoryRequest {
        QuotaHistoryRequest { window_index:0, series_key:series_key_for("codex", &s.windows[0], b.windows[0].period.as_ref().unwrap()).unwrap(),
            from_ms:from, to_ms:s.fetched_at }
    }
    #[test]
    fn cloud_reparse_tags_are_not_path_surrogates_and_unknown_tags_fail_closed() {
        assert!(reparse_is_safe(Some(0x9000001A)));
        for lane in 0..=15 { assert!(reparse_is_safe(Some(0x9000001A | (lane << 12)))); }
        assert!(!reparse_is_safe(Some(0xA0000003))); assert!(!reparse_is_safe(Some(0xA000000C)));
        assert!(!reparse_is_safe(None));
    }
    #[cfg(windows)]
    #[test]
    fn utf16_path_limits_preserve_supported_long_paths() {
        assert!(supported_path_length(Path::new(&"x".repeat(400))));
        assert!(supported_path_length(Path::new(&"😀".repeat(16000))));
        assert!(!supported_path_length(Path::new(&"😀".repeat(16380))));
        let long=PathBuf::from(format!("C:\\{}", "x".repeat(300)));
        use std::os::windows::ffi::OsStrExt;
        assert_eq!(native_watch_path(&long).as_os_str().encode_wide().take(4).collect::<Vec<_>>(),vec![92,92,63,92]);
    }
    fn change(path: PathBuf) -> notify::Result<notify::Event> {
        Ok(notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File)).add_path(path))
    }
    #[test]
    fn change_overflow_and_foreign_imports_invalidate_proof_without_io_in_queries() {
        let root=PathBuf::from("synthetic-root"); let mut changes=Changes::default();
        let path=root.join("machines/m/w/quota-readings-v2/2026-10-12/new.json");
        changes.event(&root,change(path.clone())); assert_eq!(changes.generation,1);
        changes.owned.insert(path.clone(),()); changes.event(&root,change(path));
        assert_eq!(changes.generation,1); assert_eq!(changes.pending_owned.len(),1);
        for n in 0..TICK_ENTRIES { let path=root.join(format!("machines/m/w/quota-readings-v2/2026-10-12/{n}.json"));
            changes.owned.insert(path.clone(),()); changes.event(&root,change(path)); }
        assert!(changes.overflow); assert_eq!(changes.pending_owned.len(),TICK_ENTRIES);
        let mut rescan=Changes::default();
        rescan.event(&root,Ok(notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan)));
        assert!(rescan.overflow);
        let mut pathless=Changes::default(); pathless.event(&root,Ok(notify::Event::new(notify::EventKind::Other)));
        assert!(pathless.overflow);
        let directory=root.join("machines/m/w/quota-readings-v2/2026-10-12");
        let mut moved=Changes::default(); moved.directories.insert(directory.clone(),true);
        moved.event(&root,Ok(notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any)).add_path(directory.clone())));
        assert_eq!(moved.generation,0);
        moved.event(&root,Ok(notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Name(notify::event::RenameMode::Any))).add_path(directory)));
        assert_eq!(moved.generation,1);
    }
    #[test]
    fn continuous_owned_publication_finishes_bounded_scan_and_foreign_conflicts_still_block() {
        let context=std::env::temp_dir().join("codenotch/usage-insights");
        let scratch=context.join(format!("quota-active-tests-{}",super::super::unique_id())); let root=scratch.join("dataset");
        let mut settings=super::super::TelemetrySettings { export_directory:Some(root.to_string_lossy().into()),
            machine:"fixture-machine".into(),writer_id:"active".into(),..Default::default() };
        let first_at=at(1); let mut last_at=first_at;
        for n in 0..65 {
            last_at=at(if n < 32 { 1+n*30000 } else {86400001+(n-32)*30000});
            let (s,b)=fixture(last_at,10.0+n as f64*0.1);
            super::super::quota_journal::record(&settings,"codex",&s,&b).unwrap();
        }
        let mut index=QuotaIndex::new(scratch.join("quota-rate-cache.sqlite")); index.set_root(settings.export_directory.as_deref());
        let (s,b)=fixture(last_at,16.4); index.ingest("codex",&s,&b);
        let r=request(&s,&b,first_at); assert!(!index.history(&[r])[&0].complete_horizon);
        let mut issues=vec![]; let mut usable=false; let mut endpoints=0;
        for tick in 0..32 {
            last_at+=30000; let (s,b)=fixture(last_at,16.5+tick as f64*0.1);
            let path=super::super::quota_journal::owned_v2_path(&settings,"codex",&s).unwrap();
            index.note_owned_write(path.clone());
            super::super::quota_journal::record(&settings,"codex",&s,&b).unwrap();
            index.ingest("codex",&s,&b);
            // Deterministic event injection checks classification; native delivery may coalesce events.
            index.changes.lock().unwrap().event(&root,change(path));
            let r=request(&s,&b,first_at); index.history(&[r.clone()]);
            std::thread::sleep(std::time::Duration::from_millis(10));
            let job=index.take_refresh().unwrap(); let (job,found)=refresh_quota_index(job);
            assert!(index.finish_refresh(job)); issues.extend(found);
            let result=index.history(&[r]).remove(&0).unwrap();
            if result.complete_horizon { usable=true; endpoints=result.samples.len(); break; }
        }
        assert!(usable,"active publication never completed: {issues:?}"); assert!(endpoints>=66);
        assert!(issues.is_empty(),"{issues:?}");
        settings.writer_id="foreign-import".into(); let (foreign,b)=fixture(first_at,99.0);
        let path=super::super::quota_journal::owned_v2_path(&settings,"codex",&foreign).unwrap();
        super::super::quota_journal::record(&settings,"codex",&foreign,&b).unwrap();
        index.changes.lock().unwrap().event(&root,change(path));
        let (s,b)=fixture(last_at,16.5+(endpoints-66) as f64*0.1); let r=request(&s,&b,first_at);
        assert!(!index.history(&[r.clone()])[&0].complete_horizon);
        for _ in 0..10 { index.refresh(&mut issues); }
        assert_eq!(index.history(&[r])[&0].state,QuotaHistoryState::Conflict);
        drop(index); assert_eq!(scratch.parent(),Some(context.as_path())); let _=std::fs::remove_dir_all(scratch);
    }
    #[test]
    fn duplicate_origins_conflicts_revisions_and_queries_are_memory_only() {
        let mut index = QuotaIndex::new(PathBuf::from("not-created/quota-rate-cache.sqlite"));
        let (first, base) = fixture(at(1),10.0);
        index.ingest("codex", &first, &base); index.ingest("codex", &first, &base);
        let (second, base) = fixture(at(86400001),30.0); index.ingest("codex", &second, &base);
        let r = request(&second,&base,first.fetched_at);
        for _ in 0..20 {
            let result = index.history(&[r.clone()]).remove(&0).unwrap();
            assert_eq!(result.state, QuotaHistoryState::Complete); assert_eq!(result.samples.len(),2);
        }
        let (revision, base) = fixture(at(86400002),5.0); index.ingest("codex", &revision, &base);
        let r = request(&revision,&base,first.fetched_at);
        let result = index.history(&[r.clone()]).remove(&0).unwrap();
        assert!(result.revised); assert_eq!(result.segment_started_at_ms,Some(revision.fetched_at)); assert_eq!(result.samples.len(),1);
        let (conflict, base) = fixture(revision.fetched_at,6.0); index.ingest("codex", &conflict,&base);
        assert_eq!(index.history(&[r]).remove(&0).unwrap().state,QuotaHistoryState::Conflict);
        assert!(!Path::new("not-created").exists());
    }
    #[test]
    fn unknown_relative_account_bucket_scope_or_allowance_do_not_join_a_series() {
        let (s,b)=fixture(at(1),10.0); let period=b.windows[0].period.as_ref().unwrap();
        let key=series_key_for("codex",&s.windows[0],period).unwrap();
        let mut w=s.windows[0].clone(); w.quota_account_key=None; assert!(series_key_for("codex",&w,period).is_none());
        w=s.windows[0].clone(); w.reset_basis=Some(QuotaResetBasis::Relative); assert!(series_key_for("codex",&w,period).is_none());
        w=s.windows[0].clone(); w.quota_bucket_key=Some("c".repeat(64)); assert_ne!(series_key_for("codex",&w,period).unwrap(),key);
        w=s.windows[0].clone(); w.count=Some(3); assert!(series_key_for("codex",&w,period).is_none());
        w=s.windows[0].clone(); w.reading_transport=Some(QuotaReadingTransport::CodexRollout);
        assert!(series_key_for("codex",&w,period).is_none());
    }
    #[test]
    fn refresh_job_releases_shared_state_merges_new_readings_and_cancels_root_changes() {
        let mut index=QuotaIndex::new(PathBuf::new()); index.set_root(Some("synthetic-root-a"));
        let (first,b)=fixture(at(1),10.0); index.ingest("codex",&first,&b);
        let job=index.take_refresh().unwrap();
        let (last,b)=fixture(at(86400001),30.0); index.ingest("codex",&last,&b);
        let r=request(&last,&b,first.fetched_at);
        assert_eq!(index.history(&[r.clone()])[&0].state,QuotaHistoryState::Warming);
        assert!(index.finish_refresh(job)); assert_eq!(index.history(&[r.clone()])[&0].samples.len(),2);
        let job=index.take_refresh().unwrap(); index.set_root(Some("synthetic-root-b"));
        assert!(!index.finish_refresh(job)); assert!(index.history(&[r])[&0].samples.is_empty());
    }
    #[test]
    fn bounds_are_deterministic_and_do_not_truncate_a_later_horizon_silently() {
        let mut index=QuotaIndex::new(PathBuf::new());
        for n in 0..=PER_SERIES {
            index.insert(QuotaRateSample { series_key:"test".into(),at_ms:n as u64,value:n as f64,allowance:100.0,unit:BudgetUnit::Percent },999999,true);
        }
        assert_eq!(index.series["test"].points.len(),PER_SERIES);
        let r=QuotaHistoryRequest {window_index:0,series_key:"test".into(),from_ms:0,to_ms:PER_SERIES as u64};
        assert_eq!(index.history(&[r.clone()])[&0].state,QuotaHistoryState::Truncated);
        let later=QuotaHistoryRequest {from_ms:1,..r};
        assert_eq!(index.history(&[later])[&0].state,QuotaHistoryState::Complete);
        for n in 0..SERIES { index.insert(QuotaRateSample{series_key:format!("s{n:02}"),at_ms:1,value:1.0,allowance:100.0,unit:BudgetUnit::Percent},1,true); }
        assert_eq!(index.series.len(),SERIES); assert!(!index.series.contains_key("s00"));
    }
    #[test]
    fn background_enumerates_multiple_utc_days_writers_and_preserves_corrupt_records() {
        let context=std::env::temp_dir().join("codenotch/usage-insights");
        let scratch=context.join(format!("quota-index-tests-{}",super::super::unique_id()));
        let root=scratch.join("dataset");
        let mut settings=super::super::TelemetrySettings {export_directory:Some(root.to_string_lossy().into()),machine:"fixture-machine".into(),writer_id:"first".into(),..Default::default()};
        let (first,b)=fixture(at(1),10.0); super::super::quota_journal::record(&settings,"codex",&first,&b).unwrap();
        settings.writer_id="second".into(); let (last,b)=fixture(at(86400001),30.0);
        super::super::quota_journal::record(&settings,"codex",&last,&b).unwrap();
        let cache=scratch.join("quota-rate-cache.sqlite"); std::fs::write(&cache,b"synthetic corrupt disposable cache").unwrap();
        let mut index=QuotaIndex::new(cache); index.set_root(settings.export_directory.as_deref());
        index.ingest("codex",&last,&b); let r=request(&last,&b,first.fetched_at);
        assert_eq!(index.history(&[r.clone()])[&0].state,QuotaHistoryState::Warming);
        let mut issues=vec![]; for _ in 0..10 { index.refresh(&mut issues); if index.history(&[r.clone()])[&0].complete_horizon {break;} }
        assert!(issues.iter().all(|i|i=="Quota history: dataset changed during discovery; continuing bounded scan"),"{issues:?}");
        let result=index.history(&[r.clone()]).remove(&0).unwrap();
        assert!(result.complete_horizon); assert_eq!(result.samples.len(),2);
        let date=chrono::DateTime::<chrono::Utc>::from_timestamp_millis(last.fetched_at as i64).unwrap().format("%Y-%m-%d").to_string();
        let corrupt=root.join("machines/fixture-machine/second/quota-readings-v2").join(date).join("corrupt.json");
        std::fs::write(&corrupt,b"{broken").unwrap();
        for _ in 0..10 { index.refresh(&mut issues); }
        assert_eq!(index.history(&[r.clone()])[&0].state,QuotaHistoryState::Incomplete);
        assert_eq!(std::fs::read(&corrupt).unwrap(),b"{broken");
        index.set_root(None); assert!(index.history(&[r])[&0].samples.is_empty());
        assert_eq!(scratch.parent(),Some(context.as_path())); let _=std::fs::remove_dir_all(scratch);
    }
}
