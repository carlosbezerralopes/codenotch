//! Kiro CLI /usage is authoritative; GetUsageLimits only enriches its credit ceilings.
//! Borrowed SQLite authentication is read-only and never refreshed by Codenotch.
use crate::usage::{quota_key, CreditAmounts, CreditScope, LimitWindow, QuotaReadingTransport, QuotaResetBasis, UsageSnapshot};
use crate::AppState;
use chrono::{Datelike, Local, NaiveDate, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const ARGS: [&str; 3] = ["chat", "--no-interactive", "/usage"];
const OUTPUT_LIMIT: usize = 64 * 1024;
const POLL_ACTIVE_MS: u64 = 30_000;
const POLL_IDLE_MS: u64 = 30 * 60_000;
const ACTIVITY_RECENT_MS: u64 = 120_000;
const MAX_FAILURES: u8 = 3;
static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() { REFRESH.store(true, Ordering::Relaxed); }
fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
fn store_path() -> PathBuf { crate::config::config_path().with_file_name("kiro-usage.json") }
fn poll_path() -> PathBuf { crate::config::config_path().with_file_name("kiro-poll.json") }
fn absent() -> UsageSnapshot { UsageSnapshot { status: "absent".into(), ..Default::default() } }

fn runnable(path: &Path) -> bool {
    path.is_absolute() && path.is_file()
        && path.extension().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("exe"))
}

/// Explicit invalid overrides stop discovery; relative PATH entries and shell shims are refused.
fn locate(override_path: Option<&str>, candidates: &[PathBuf], path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    if let Some(raw) = override_path.filter(|s| !s.trim().is_empty()) {
        let p = PathBuf::from(raw.trim());
        return runnable(&p).then_some(p);
    }
    candidates.iter().cloned().chain(path.into_iter().flat_map(std::env::split_paths)
        .filter(|p| p.is_absolute()).map(|p| p.join("kiro-cli.exe")))
        .find(|p| runnable(p))
}

pub fn find_cli() -> Option<PathBuf> {
    // This adapter deliberately does not claim an IDE or WSL session.
    #[cfg(not(windows))]
    { None }
    #[cfg(windows)]
    {
        let mut candidates = Vec::new();
        // cli.kiro.dev/install.ps1 installs the MSI in Program Files/Kiro-Cli.
        if let Some(p) = std::env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(p).join("Kiro-Cli/kiro-cli.exe"));
        }
        if let Some(h) = dirs::home_dir() {
            candidates.push(h.join(".local/bin/kiro-cli.exe"));
            candidates.push(h.join(".kiro/bin/kiro-cli.exe"));
        }
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(p).join("kiro-cli/bin/kiro-cli.exe"));
        }
        locate(std::env::var("KIRO_CLI_PATH").ok().as_deref(), &candidates, std::env::var_os("PATH").as_deref())
    }
}

pub fn load_persisted() -> UsageSnapshot {
    if find_cli().is_none() { return absent(); }
    std::fs::read_to_string(store_path()).ok()
        .and_then(|s| serde_json::from_str::<UsageSnapshot>(&s).ok())
        .map(|mut s| { if !s.windows.is_empty() { s.status = "stale".into(); } s })
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum ReadError { NeedsAuth, TimedOut, BadOutput, Process, RateLimited }

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct PollState {
    version: u8,
    consecutive_failures: u8,
    standby: bool,
    last_failure: Option<ReadError>,
    // Internal change detector only; credentials themselves never enter app state.
    auth_fingerprint: Option<String>,
    #[serde(default)] post_probe_observation_pending: bool,
    #[serde(skip)] storage_blocked: bool,
    last_attempt_ms: u64,
}
impl Default for PollState {
    fn default() -> Self { Self { version: 1, consecutive_failures: 0, standby: false,
        last_failure: None, auth_fingerprint: None, post_probe_observation_pending: false,
        storage_blocked: false, last_attempt_ms: 0 } }
}

impl PollState {
    fn observe(&mut self, auth: &AuthObservation, manual: bool) {
        let mut changed = false;
        if let Some(fingerprint) = &auth.fingerprint {
            let different = self.auth_fingerprint.as_ref() != Some(fingerprint);
            changed = !self.post_probe_observation_pending && self.auth_fingerprint.is_some() && different;
            // An unchanged token after a failed probe does not prove its late refresh is over.
            if !self.post_probe_observation_pending || different || self.consecutive_failures == 0 {
                self.post_probe_observation_pending = false;
            }
            self.auth_fingerprint = Some(fingerprint.clone());
        }
        if manual || (changed && !self.storage_blocked) {
            self.consecutive_failures = 0; self.standby = false;
            self.last_failure = None; self.last_attempt_ms = 0;
        }
    }
    fn due(&self, now: u64, active: bool) -> bool {
        !self.standby && (self.last_attempt_ms == 0 || now < self.last_attempt_ms
            || now.saturating_sub(self.last_attempt_ms) >= if active || self.consecutive_failures > 0 { POLL_ACTIVE_MS } else { POLL_IDLE_MS })
    }
    fn block_expired(&mut self, auth: &AuthObservation, manual: bool, now: u64) -> bool {
        if auth.expired(now) && !manual {
            self.standby = true; self.last_failure = Some(ReadError::NeedsAuth); true
        } else { false }
    }
    fn record(&mut self, now: u64, error: Option<ReadError>) {
        self.last_attempt_ms = now;
        self.last_failure = error;
        self.consecutive_failures = if error.is_some() { self.consecutive_failures.saturating_add(1).min(MAX_FAILURES) } else { 0 };
        self.standby = self.consecutive_failures >= MAX_FAILURES;
    }
    fn reserve_probe(&mut self, path: &Path, now: u64) -> std::io::Result<()> {
        // A crash or failed final write consumes this attempt after restart.
        let mut reserved = self.clone();
        reserved.record(now, Some(ReadError::Process));
        reserved.post_probe_observation_pending = true;
        if let Err(error) = save_poll_state(path, &reserved) {
            self.standby = true; self.storage_blocked = true; return Err(error);
        }
        self.post_probe_observation_pending = true;
        self.storage_blocked = false;
        Ok(())
    }
}

#[derive(Default)]
struct AuthObservation { fingerprint: Option<String>, expires_at_ms: Option<u64> }
impl AuthObservation {
    fn expired(&self, now: u64) -> bool { self.expires_at_ms.is_some_and(|expiry| expiry <= now) }
}

fn first_use_state(path: &Path) -> bool {
    if !matches!(std::fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound) { return false; }
    for parent in path.ancestors().skip(1) {
        match std::fs::symlink_metadata(parent) {
            Ok(metadata) => return metadata.is_dir(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return false,
        }
    }
    false
}

fn load_poll_state(path: &Path) -> PollState {
    use std::io::Read;
    let blocked = || PollState { standby: true, storage_blocked: true, ..Default::default() };
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if first_use_state(path) { PollState::default() } else { blocked() };
        }
        Err(_) => return blocked(),
    };
    let mut raw = Vec::new();
    if file.take((OUTPUT_LIMIT + 1) as u64).read_to_end(&mut raw).is_err() || raw.len() > OUTPUT_LIMIT { return blocked(); }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) else { return blocked(); };
    if !["version", "consecutive_failures", "standby", "last_failure", "auth_fingerprint", "last_attempt_ms"]
        .iter().all(|key| value.get(key).is_some()) { return blocked(); }
    let Ok(mut state) = serde_json::from_value::<PollState>(value) else { return blocked(); };
    if state.consecutive_failures > MAX_FAILURES { return blocked(); }
    state.standby |= state.consecutive_failures >= MAX_FAILURES;
    if state.version != 1 { return blocked(); }
    state.last_attempt_ms = 0;
    state
}

fn save_poll_state(path: &Path, state: &PollState) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let bytes = serde_json::to_vec(state).map_err(std::io::Error::other)?;
    let temporary = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(&bytes)?; file.sync_all()?; drop(file);
    std::fs::rename(temporary, path)?;
    std::fs::OpenOptions::new().write(true).open(path)?.sync_all()?;
    #[cfg(unix)] std::fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
    Ok(())
}

fn auth_fault(text: &str) -> bool {
    let lower = strip_ansi(text).to_ascii_lowercase();
    ["not logged in", "login required", "failed to initialize auth portal", "kiro-cli login", "oauth error"]
        .iter().any(|s| lower.contains(s))
}

fn process_output(exit: u32, stdout: &str, stderr: &str) -> Result<String, ReadError> {
    // Exit status alone cannot distinguish expired auth from transport or startup failure.
    if auth_fault(stdout) || auth_fault(stderr) { return Err(ReadError::NeedsAuth); }
    if [stdout, stderr].iter().any(|s| {
        let lower = s.to_ascii_lowercase();
        ["too many requests", "rate limit exceeded", "rate limit reached", "rate limited", "http 429", "http429"]
            .iter().any(|needle| lower.contains(needle))
    }) { return Err(ReadError::RateLimited); }
    if exit != 0 { return Err(ReadError::Process); }
    usage_text(stdout, stderr).ok_or(ReadError::BadOutput)
}

fn usage_text(stdout: &str, stderr: &str) -> Option<String> {
    let text = if stdout.contains("Estimated Usage") { stdout }
        else if stderr.contains("Estimated Usage") { stderr }
        else if !stdout.trim().is_empty() { stdout } else { stderr };
    (!text.trim().is_empty()).then(|| text.to_string())
}

fn strip_ansi(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c != '\x1b' { out.push(c); continue; }
        match chars.next() {
            Some('[') => { for x in chars.by_ref() { if ('@'..='~').contains(&x) { break; } } }
            Some(']') => {
                while let Some(x) = chars.next() {
                    if x == '\x07' { break; }
                    if x == '\x1b' && chars.peek() == Some(&'\\') { chars.next(); break; }
                }
            }
            Some('(' | ')') => { chars.next(); }
            _ => {}
        }
    }
    out
}

fn number_prefix(s: &str) -> Option<f64> {
    let raw: String = s.trim_start().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    raw.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0)
}
fn pair(s: &str, separator: &str) -> Option<(f64, f64)> {
    let (a, b) = s.split_once(separator)?;
    let valid = |raw: &str| raw.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0);
    Some((valid(a.trim())?, valid(b.split_whitespace().next()?)?))
}
fn display_plan(raw: &str) -> String {
    let s = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if !s.to_ascii_lowercase().contains("kiro") { return s; }
    s.split_whitespace().map(|w| {
        let mut c = w.chars();
        c.next().map(|h| format!("{}{}", h.to_uppercase(), c.as_str().to_ascii_lowercase())).unwrap_or_default()
    }).collect::<Vec<_>>().join(" ")
}

fn reset_stamp(raw: &str, now: chrono::DateTime<Local>) -> Option<u64> {
    let stamp = raw.split_whitespace().next()?;
    // An explicit source offset is authoritative. Date-only monthly resets use UTC,
    // as documented at https://kiro.dev/docs/billing/related-questions/.
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(stamp) {
        return u64::try_from(date.timestamp_millis()).ok();
    }
    let now = now.with_timezone(&Utc);
    let date = if stamp.contains('-') {
        NaiveDate::parse_from_str(stamp, "%Y-%m-%d").ok()?
    } else {
        let (m, d) = stamp.split_once('/')?;
        let make = |year| NaiveDate::from_ymd_opt(year, m.parse().ok()?, d.parse().ok()?);
        let date = make(now.year())?;
        if date >= now.date_naive() { date } else { make(now.year() + 1)? }
    };
    Utc.from_local_datetime(&date.and_hms_opt(0, 0, 0)?).single()
        .map(|d| d.timestamp_millis().max(0) as u64)
}

/// A calendar month, not thirty days. Inconsistent reported reset dates stay unknown.
pub(crate) fn monthly_duration_seconds(reset_ms: u64) -> Option<u64> {
    let reset = chrono::DateTime::<Utc>::from_timestamp_millis(i64::try_from(reset_ms).ok()?)?;
    if reset.day() != 1 || reset.hour() != 0 || reset.minute() != 0
        || reset.second() != 0 || reset.timestamp_subsec_millis() != 0 { return None; }
    let start = reset.checked_sub_months(chrono::Months::new(1))?;
    u64::try_from((reset - start).num_seconds()).ok()
}

#[derive(Debug)]
struct Reading { plan: Option<String>, windows: Vec<LimitWindow> }

fn parse_cli(text: &str, now: chrono::DateTime<Local>) -> Result<Reading, ReadError> {
    let clean = strip_ansi(text);
    if auth_fault(&clean) { return Err(ReadError::NeedsAuth); }
    let plan = clean.lines().find_map(|line| {
        if let Some((_, tail)) = line.split_once("Plan:") {
            return tail.split('|').next().map(str::trim).filter(|s| !s.is_empty()).map(display_plan);
        }
        let parts: Vec<_> = line.split('|').map(str::trim).collect();
        if line.contains("Estimated Usage") && parts.len() >= 3 && !parts[2].is_empty() {
            return Some(display_plan(parts[2]));
        }
        parts.iter().find(|p| p.starts_with("KIRO ")).map(|p| display_plan(p))
    });
    let percent = clean.lines().find_map(|line| {
        let start = line.find('█')?;
        let tail = line[start..].trim_start_matches('█').trim_start();
        let (n, _) = tail.split_once('%')?;
        n.trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0).map(|n| n / 100.0)
    });
    let credits = clean.split('(').skip(1).find_map(|s| {
        if !s.split(')').next().unwrap_or("").contains("covered") { return None; }
        pair(s, " of ")
    });
    let resets_at = clean.split_once("resets on ").and_then(|(_, s)| reset_stamp(s, now));
    let mut windows = Vec::new();
    if let Some(used) = percent.or_else(|| credits.filter(|(_, total)| *total > 0.0).map(|(used, total)| used / total)) {
        let amounts = credits.filter(|(_, total)| *total > 0.0)
            .map(|(consumed, allowance)| CreditAmounts { consumed, allowance });
        // Current CLI displays can combine monthly plan and add-on credits. Only the
        // enrichment's unambiguous plan pair can authorize a monthly credit budget.
        windows.push(LimitWindow { id: "credits".into(), label: "Credits".into(), used, resets_at,
            reading_transport: Some(QuotaReadingTransport::KiroCli), reset_basis: Some(QuotaResetBasis::Unknown),
            credits: amounts, credit_scope: Some(CreditScope::Unknown), ..Default::default() });
    }
    let bonus = clean.split_once("Bonus credits:").and_then(|(_, s)| pair(s, "/"));
    if let Some((used, total)) = bonus.filter(|(_, total)| *total > 0.0) {
        let expiry = clean.split_once("expires in ").and_then(|(_, s)| number_prefix(s))
            .filter(|n| *n <= 36500.0)
            .and_then(|n| now.checked_add_days(chrono::Days::new(n as u64)))
            .map(|d| d.timestamp_millis().max(0) as u64);
        windows.push(LimitWindow { id: "bonus".into(), label: "Credits".into(), group: Some("Bonus".into()), used: used / total, resets_at: expiry,
            reading_transport: Some(QuotaReadingTransport::KiroCli), reset_basis: Some(QuotaResetBasis::Relative),
            credits: Some(CreditAmounts { consumed: used, allowance: total }), ..Default::default() });
    }
    if plan.is_none() && windows.is_empty() { return Err(ReadError::BadOutput); }
    Ok(Reading { plan, windows })
}

/// Quote for CreateProcessW; no shell ever interprets these arguments.
#[cfg(windows)]
fn quote_arg(arg: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        if c == '\\' { slashes += 1; continue; }
        out.extend(std::iter::repeat('\\').take(if c == '"' { slashes * 2 + 1 } else { slashes }));
        slashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat('\\').take(slashes * 2));
    out.push('"');
    out
}

#[cfg(windows)]
fn run_process(program: &Path, args: &[&str], cwd: &Path, timeout: Duration) -> Result<String, ReadError> {
    use std::io::Read;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, DuplicateHandle, HANDLE, DUPLICATE_SAME_ACCESS, WAIT_OBJECT_0};
    use windows::Win32::System::JobObjects::*;
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::*;
    struct Handle(HANDLE);
    impl Drop for Handle { fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } } }
    let fail = |_| ReadError::Process;
    let job = Handle(unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(fail)?);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe { SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, &limits as *const _ as _, std::mem::size_of_val(&limits) as u32) }.map_err(fail)?;
    let inherit = |h: HANDLE| -> Result<Handle, ReadError> {
        let mut dup = HANDLE::default();
        unsafe { DuplicateHandle(GetCurrentProcess(), h, GetCurrentProcess(), &mut dup, 0, true, DUPLICATE_SAME_ACCESS) }.map_err(fail)?;
        Ok(Handle(dup))
    };
    let pipe = || -> Result<(std::fs::File, Handle), ReadError> {
        let mut r = HANDLE::default(); let mut w = HANDLE::default();
        unsafe { CreatePipe(&mut r, &mut w, None, 0) }.map_err(fail)?;
        let reader = unsafe { std::fs::File::from_raw_handle(r.0 as _) };
        let writer = Handle(w);
        Ok((reader, inherit(writer.0)?))
    };
    let (stdout, out_write) = pipe()?;
    let (stderr, err_write) = pipe()?;
    let null = std::fs::File::open("NUL").map_err(|_| ReadError::Process)?;
    let input = inherit(HANDLE(null.as_raw_handle() as _))?;
    let inherited = [input.0, out_write.0, err_write.0];
    let mut attr_size = 0;
    unsafe { let _ = InitializeProcThreadAttributeList(LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()), 1, 0, &mut attr_size); }
    let mut storage = vec![0usize; attr_size.div_ceil(std::mem::size_of::<usize>())];
    let attr = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr() as _);
    unsafe { InitializeProcThreadAttributeList(attr, 1, 0, &mut attr_size) }.map_err(fail)?;
    struct Attr(LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for Attr { fn drop(&mut self) { unsafe { DeleteProcThreadAttributeList(self.0); } } }
    let _attr_guard = Attr(attr);
    unsafe { UpdateProcThreadAttribute(attr, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, Some(inherited.as_ptr() as _), std::mem::size_of_val(&inherited), None, None) }.map_err(fail)?;
    let mut si = STARTUPINFOEXW::default();
    si.StartupInfo.cb = std::mem::size_of_val(&si) as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = input.0;
    si.StartupInfo.hStdOutput = out_write.0;
    si.StartupInfo.hStdError = err_write.0;
    si.lpAttributeList = attr;
    let program_w: Vec<u16> = program.as_os_str().encode_wide().chain(Some(0)).collect();
    let cwd_w: Vec<u16> = cwd.as_os_str().encode_wide().chain(Some(0)).collect();
    let command = std::iter::once(quote_arg(&program.to_string_lossy())).chain(args.iter().map(|s| quote_arg(s))).collect::<Vec<_>>().join(" ");
    let mut command_w: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
    let mut environment: Vec<(std::ffi::OsString, std::ffi::OsString)> = std::env::vars_os()
        .filter(|(k, _)| !k.to_string_lossy().eq_ignore_ascii_case("TERM") && !k.to_string_lossy().eq_ignore_ascii_case("KIRO_CHAT_UI")).collect();
    environment.extend([("TERM".into(), "dumb".into()), ("KIRO_CHAT_UI".into(), "classic".into())]);
    environment.sort_by_key(|(k, _)| k.to_string_lossy().to_ascii_lowercase());
    let mut env_w = Vec::new();
    for (key, value) in environment {
        env_w.extend(key.encode_wide()); env_w.push('=' as u16); env_w.extend(value.encode_wide()); env_w.push(0);
    }
    env_w.push(0);
    let mut pi = PROCESS_INFORMATION::default();
    unsafe { CreateProcessW(PCWSTR(program_w.as_ptr()), PWSTR(command_w.as_mut_ptr()), None, None, true,
        EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
        Some(env_w.as_ptr() as _), PCWSTR(cwd_w.as_ptr()), &si.StartupInfo, &mut pi) }.map_err(fail)?;
    let process = Handle(pi.hProcess); let thread = Handle(pi.hThread);
    if unsafe { AssignProcessToJobObject(job.0, process.0) }.is_err() {
        unsafe { let _ = TerminateProcess(process.0, 1); }
        return Err(ReadError::Process);
    }
    // Suspended until inside the job: no child can escape during startup.
    if unsafe { ResumeThread(thread.0) } == u32::MAX { return Err(ReadError::Process); }
    drop(out_write); drop(err_write); drop(input);
    let drain = |mut file: std::fs::File| std::thread::spawn(move || {
        let mut stored = Vec::new(); let mut chunk = [0u8; 4096]; let mut overflow = false;
        loop {
            let n = match file.read(&mut chunk) { Ok(0) | Err(_) => break, Ok(n) => n };
            let keep = n.min(OUTPUT_LIMIT.saturating_sub(stored.len()));
            overflow |= keep != n;
            stored.extend_from_slice(&chunk[..keep]);
        }
        (stored, overflow)
    });
    let out = drain(stdout); let err = drain(stderr);
    let done = unsafe { WaitForSingleObject(process.0, timeout.as_millis().min(u32::MAX as u128) as u32) } == WAIT_OBJECT_0;
    let mut exit = 1;
    if done { unsafe { let _ = GetExitCodeProcess(process.0, &mut exit); } }
    // Also kill children after normal exit so they cannot hold pipe writers open.
    unsafe { let _ = TerminateJobObject(job.0, 1); }
    drop(job);
    let (out, out_big) = out.join().map_err(|_| ReadError::Process)?;
    let (err, err_big) = err.join().map_err(|_| ReadError::Process)?;
    if !done { return Err(ReadError::TimedOut); }
    if out_big || err_big { return Err(ReadError::BadOutput); }
    process_output(exit, &String::from_utf8_lossy(&out), &String::from_utf8_lossy(&err))
}

#[cfg(not(windows))]
fn run_process(_program: &Path, _args: &[&str], _cwd: &Path, _timeout: Duration) -> Result<String, ReadError> {
    Err(ReadError::Process)
}

fn database_paths() -> Vec<PathBuf> {
    if let Some(p) = std::env::var_os("KIRO_DATA_DIR").filter(|s| !s.is_empty()) {
        let p = PathBuf::from(p);
        return if p.is_absolute() { vec![p.join("data.sqlite3")] } else { Vec::new() };
    }
    ["LOCALAPPDATA", "APPDATA"].iter().filter_map(|k| std::env::var_os(k))
        .map(|p| PathBuf::from(p).join("kiro-cli/data.sqlite3")).filter(|p| p.is_absolute()).collect()
}

fn readonly_database(path: &Path) -> Option<rusqlite::Connection> {
    use rusqlite::OpenFlags;
    if !path.is_absolute() || !path.is_file() { return None; }
    let conn = rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    conn.busy_timeout(Duration::from_millis(50)).ok()?;
    conn.execute_batch("PRAGMA query_only=ON;").ok()?;
    Some(conn)
}

fn database_json(conn: &rusqlite::Connection, sql: &str, key: &str) -> Option<serde_json::Value> {
    let raw: String = conn.query_row(sql, [key], |r| r.get(0)).ok()?;
    if raw.len() > OUTPUT_LIMIT { return None; }
    serde_json::from_str(&raw).ok()
}

fn auth_from_value(value: &serde_json::Value) -> AuthObservation {
    let Some(token) = ["access_token", "accessToken"].iter().find_map(|key|
        value.get(key).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty())) else { return AuthObservation::default(); };
    let expiry = ["expires_at", "expiresAt", "expires_at_ms", "expiresAtMs", "expiration"]
        .iter().find_map(|key| value.get(key).and_then(|v| {
            if let Some(raw) = v.as_str() {
                if let Ok(date) = chrono::DateTime::parse_from_rfc3339(raw) { return u64::try_from(date.timestamp_millis()).ok(); }
            }
            let n = v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))?;
            if key.ends_with("ms") || key.ends_with("Ms") || n >= 100_000_000_000 { Some(n) } else { n.checked_mul(1000) }
        }));
    let mut hash = Sha256::new(); hash.update(token.as_bytes()); hash.update([0]);
    if let Some(expiry) = expiry { hash.update(expiry.to_le_bytes()); }
    AuthObservation { fingerprint: Some(format!("{:x}", hash.finalize())), expires_at_ms: expiry }
}

fn observe_auth() -> AuthObservation {
    database_paths().iter().find_map(|path| {
        let conn = readonly_database(path)?;
        let value = database_json(&conn, "SELECT value FROM auth_kv WHERE key = ?", "kirocli:odic:token")?;
        let auth = auth_from_value(&value);
        auth.fingerprint.is_some().then_some(auth)
    }).unwrap_or_default()
}

fn credentials(path: &Path) -> Option<(String, String)> {
    // A filesystem path, not a URI: ?/# cannot change SQLite's opening mode.
    let conn = readonly_database(path)?;
    let token = database_json(&conn, "SELECT value FROM auth_kv WHERE key = ?", "kirocli:odic:token")?;
    let token = ["access_token", "accessToken"].iter().find_map(|k| token.get(k).and_then(|s| s.as_str()).map(str::trim).filter(|s| !s.is_empty()))?.to_string();
    let profile = database_json(&conn, "SELECT value FROM state WHERE key = ?", "api.codewhisperer.profile")?;
    let arn = profile.get("arn")?.as_str()?.trim().to_string();
    endpoint(&arn)?;
    Some((token, arn))
}

fn plain_metadata(path: &Path) -> Option<std::fs::Metadata> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() { return None; }
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 { return None; }
    }
    Some(metadata)
}

#[derive(Default, Deserialize)]
struct ActivityMetadata {
    #[serde(rename = "rootPaths", default)] roots: Vec<String>,
    #[serde(rename = "workspacePaths", default)] workspaces: Vec<String>,
    #[serde(rename = "_meta", default)] meta: ActivityImport,
}
#[derive(Default, Deserialize)]
struct ActivityImport {
    #[serde(rename = "sincronizarConversas", default, deserialize_with = "import_marker")]
    imported: bool,
}
fn import_marker<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?; Ok(true)
}

fn poller_folder(raw: &str, poll_directory: &Path) -> bool {
    let normalized = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_ascii_lowercase();
    let raw = normalized(raw); let poll = normalized(&poll_directory.to_string_lossy());
    raw == poll || raw.strip_prefix(&poll).is_some_and(|tail| tail.starts_with('/'))
}

fn recent_activity_at(root: &Path, poll_directory: &Path, now: u64) -> bool {
    use std::io::Read;
    if !plain_metadata(root).is_some_and(|m| m.is_dir()) { return false; }
    let Ok(buckets) = std::fs::read_dir(root) else { return false; };
    let mut entries_left = 4096usize;
    let mut bytes_left = 256 * 1024usize;
    for bucket in buckets.take(128).flatten() {
        if bucket.file_name() == "cli" || !plain_metadata(&bucket.path()).is_some_and(|m| m.is_dir()) { continue; }
        let Ok(sessions) = std::fs::read_dir(bucket.path()) else { continue; };
        for session in sessions.flatten() {
            if entries_left == 0 { return false; } entries_left -= 1;
            if !session.file_name().to_string_lossy().starts_with("sess_")
                || !plain_metadata(&session.path()).is_some_and(|m| m.is_dir()) { continue; }
            let Some(messages) = plain_metadata(&session.path().join("messages.jsonl")).filter(|m| m.is_file() && m.len() > 0) else { continue; };
            let Some(modified) = messages.modified().ok().and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64) else { continue; };
            if !recent_activity(modified, now) { continue; }
            let metadata_path = session.path().join("session.json");
            let Some(metadata) = plain_metadata(&metadata_path).filter(|m| m.is_file() && m.len() <= OUTPUT_LIMIT as u64) else { continue; };
            let length = metadata.len() as usize;
            if length > bytes_left { continue; } bytes_left -= length;
            let Ok(file) = std::fs::File::open(metadata_path) else { continue; };
            let mut raw = Vec::new();
            if file.take((length + 1) as u64).read_to_end(&mut raw).is_err() || raw.len() > length { continue; }
            let Ok(metadata) = serde_json::from_slice::<ActivityMetadata>(&raw) else { continue; };
            let roots = if metadata.roots.is_empty() { &metadata.workspaces } else { &metadata.roots };
            if !metadata.meta.imported && roots.iter().any(|root| Path::new(root).is_absolute())
                && !roots.iter().any(|root| poller_folder(root, poll_directory)) { return true; }
        }
    }
    false
}
fn recent_activity(modified: u64, now: u64) -> bool {
    modified <= now && now - modified <= ACTIVITY_RECENT_MS
}
fn kiro_active(poll_directory: &Path, now: u64) -> bool {
    let root = std::env::var_os("KIRO_HOME").map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|p| p.join(".kiro")));
    root.filter(|p| p.is_absolute()).is_some_and(|p| recent_activity_at(&p.join("sessions"), poll_directory, now))
}

fn endpoint(arn: &str) -> Option<&'static str> {
    let p: Vec<_> = arn.splitn(6, ':').collect();
    if p.len() != 6 || p[0] != "arn" || p[1] != "aws" || p[2] != "codewhisperer"
        || p[4].is_empty() || !p[5].starts_with("profile/") || p[5].len() <= 8
        || arn.chars().any(|c| c.is_whitespace() || c.is_control()) { return None; }
    match p[3] {
        "us-east-1" => Some("https://codewhisperer.us-east-1.amazonaws.com/"),
        "eu-central-1" => Some("https://q.eu-central-1.amazonaws.com/"),
        _ => None,
    }
}

#[derive(Debug)]
struct CreditLimits { plan_used: f64, plan_limit: f64, overage_used: f64, overage_cap: Option<f64>, reset: Option<u64>, bonuses: bool }
fn parse_limits(v: &serde_json::Value) -> Option<CreditLimits> {
    let rows: Vec<_> = v.get("usageBreakdownList")?.as_array()?.iter().filter(|r| r.get("resourceType").and_then(|s| s.as_str()) == Some("CREDIT")).collect();
    if rows.len() != 1 { return None; }
    let c = rows[0];
    let number = |precise: &str, legacy: &str| c.get(precise).and_then(|n| n.as_f64())
        .or_else(|| c.get(legacy).and_then(|n| n.as_f64()));
    let valid = |n: f64| n.is_finite() && n >= 0.0;
    let plan_limit = number("usageLimitWithPrecision", "usageLimit")?;
    let total = number("currentUsageWithPrecision", "currentUsage")?;
    let overage_used = number("currentOveragesWithPrecision", "currentOverages").unwrap_or(0.0);
    if !valid(plan_limit) || !valid(total) || !valid(overage_used) || total < overage_used { return None; }
    let bonuses = c.get("bonuses").and_then(|b| b.as_array()).is_some_and(|a| !a.is_empty());
    let plan_used = total - overage_used;
    if !bonuses && plan_used > plan_limit { return None; }
    let enabled = v.pointer("/overageConfiguration/overageStatus").and_then(|s| s.as_str()).is_some_and(|s| s.eq_ignore_ascii_case("ENABLED"));
    let overage_cap = if enabled { number("overageCapWithPrecision", "overageCap") } else { None };
    if overage_cap.is_some_and(|cap| !valid(cap)) { return None; }
    let reset = c.get("nextDateReset").and_then(|n| n.as_f64()).or_else(|| v.get("nextDateReset").and_then(|n| n.as_f64()))
        .filter(|n| n.is_finite() && (1_000_000_000.0..=4_102_444_800.0).contains(n)).map(|s| (s * 1000.0) as u64);
    Some(CreditLimits { plan_used, plan_limit, overage_used, overage_cap, reset, bonuses })
}

fn apply_limits(reading: &mut Reading, limits: CreditLimits) {
    if limits.plan_limit > 0.0 && !limits.bonuses {
        let old = reading.windows.iter().position(|w| w.id == "credits");
        let reset = limits.reset.or_else(|| old.and_then(|i| reading.windows[i].resets_at));
        let w = LimitWindow { id: "credits".into(), label: "Credits".into(), used: limits.plan_used / limits.plan_limit, resets_at: reset,
            reading_transport: Some(QuotaReadingTransport::KiroUsageApi),
            reset_basis: Some(if limits.reset.and_then(monthly_duration_seconds).is_some() { QuotaResetBasis::OfficialCalendarMonth }
                else { QuotaResetBasis::Unknown }),
            credits: Some(CreditAmounts { consumed: limits.plan_used, allowance: limits.plan_limit }),
            credit_scope: Some(CreditScope::Plan),
            duration_seconds: reset.and_then(monthly_duration_seconds), ..Default::default() };
        if let Some(i) = old { reading.windows[i] = w; } else { reading.windows.insert(0, w); }
    } else if limits.bonuses {
        if let Some(w) = reading.windows.iter_mut().find(|w| w.id == "credits") {
            w.credit_scope = Some(CreditScope::Combined);
        }
    }
    if let Some(cap) = limits.overage_cap.filter(|c| *c > 0.0) {
        reading.windows.push(LimitWindow { id: "overage".into(), label: "Overage".into(), used: limits.overage_used / cap, resets_at: limits.reset,
            reading_transport: Some(QuotaReadingTransport::KiroUsageApi), reset_basis: Some(QuotaResetBasis::Unknown),
            credits: Some(CreditAmounts { consumed: limits.overage_used, allowance: cap }), ..Default::default() });
    }
}

#[derive(Default)]
struct Enrichment { until: u64, attempts: u32 }
fn attach_api_identity(reading: &mut Reading, arn: &str) {
    for w in &mut reading.windows {
        if w.reading_transport != Some(QuotaReadingTransport::KiroUsageApi) { continue; }
        w.quota_account_key = Some(quota_key(&("quota-account-v1", "kiro", arn)));
        w.quota_bucket_key = Some(quota_key(&("quota-bucket-v1", "kiro", &w.id)));
    }
}
fn backoff(attempt: u32, retry_after: u64) -> u64 { (60 * (1u64 << attempt.min(4))).max(retry_after).min(900) }
impl Enrichment {
    fn enrich(&mut self, reading: &mut Reading) {
        if self.until > now_ms() { return; }
        let Some((token, arn)) = database_paths().iter().find_map(|p| credentials(p)) else { return; };
        let Some(url) = endpoint(&arn) else { return; };
        let response = ureq::post(url).timeout(Duration::from_secs(10))
            .set("Content-Type", "application/x-amz-json-1.0")
            .set("X-Amz-Target", "AmazonCodeWhispererService.GetUsageLimits")
            .set("Authorization", &format!("Bearer {token}"))
            .send_json(serde_json::json!({"profileArn":arn}));
        match response {
            Ok(r) => {
                self.until = 0; self.attempts = 0;
                use std::io::Read;
                let mut body = String::new();
                if r.into_reader().take((OUTPUT_LIMIT + 1) as u64).read_to_string(&mut body).is_ok() && body.len() <= OUTPUT_LIMIT {
                    if let Some(limits) = serde_json::from_str(&body).ok().and_then(|v| parse_limits(&v)) {
                        apply_limits(reading, limits);
                        attach_api_identity(reading, &arn);
                    }
                }
            }
            Err(ureq::Error::Status(429, r)) => {
                let retry = r.header("Retry-After").and_then(|s| s.trim().parse::<u64>().ok().or_else(||
                    chrono::DateTime::parse_from_rfc2822(s).ok().map(|d| d.timestamp_millis().saturating_sub(now_ms() as i64).max(0) as u64 / 1000))).unwrap_or(0);
                self.until = now_ms().saturating_add(backoff(self.attempts, retry) * 1000);
                self.attempts = self.attempts.saturating_add(1);
            }
            _ => {} // CLI windows survive authentication, transport and schema failures.
        }
    }
}

fn from_result(prev: &UsageSnapshot, result: Result<Reading, ReadError>, enrichment: &mut Enrichment) -> UsageSnapshot {
    match result {
        Ok(mut reading) => {
            enrichment.enrich(&mut reading);
            if reading.windows.is_empty() { return from_result(prev, Err(ReadError::BadOutput), enrichment); }
            UsageSnapshot { status: "ok".into(), windows: reading.windows, fetched_at: now_ms(),
                note: reading.plan.map(|p| format!("{p} · via Kiro CLI")).unwrap_or_else(|| "via Kiro CLI".into()),
                backoff_until: enrichment.until }
        }
        Err(ReadError::NeedsAuth) => {
            let mut snap = prev.clone();
            snap.status = if snap.windows.is_empty() { "needsAuth" } else { "stale" }.into();
            snap.note = "Kiro sign-in required. Last reading retained.".into();
            snap
        }
        Err(error) => {
            let mut snap = prev.clone();
            snap.status = if snap.windows.is_empty() { "error" } else { "stale" }.into();
            snap.note = match error { ReadError::TimedOut => "Kiro CLI /usage timed out", ReadError::BadOutput => "Kiro CLI reported no usable usage", ReadError::RateLimited => "Kiro CLI /usage rate limited", _ => "Cannot run Kiro CLI /usage" }.into();
            snap.backoff_until = enrichment.until;
            snap
        }
    }
}

fn resolved_error(error: Option<ReadError>, snapshot: &UsageSnapshot) -> Option<ReadError> {
    error.or_else(|| (snapshot.status != "ok" || snapshot.windows.is_empty()).then_some(ReadError::BadOutput))
}

fn storage_pause(previous: &UsageSnapshot) -> UsageSnapshot {
    let mut retained = previous.clone();
    retained.status = if retained.windows.is_empty() { "error" } else { "stale" }.into();
    retained.note = "Kiro polling paused: local retry state is unavailable. Repair local state and use Refresh to retry.".into();
    retained
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let mut enrichment = Enrichment { until: app.state::<AppState>().kiro.lock().unwrap().backoff_until, ..Default::default() };
        let dir = std::env::temp_dir().join("codenotch/kiro-usage");
        let mut gate = load_poll_state(&poll_path());
        let mut last_check = 0u64;
        loop {
            let manual = REFRESH.swap(false, Ordering::Relaxed);
            let now = now_ms();
            if manual || last_check == 0 || now < last_check || now.saturating_sub(last_check) >= 5000 {
                last_check = now;
                let old_gate = gate.clone();
                let auth = observe_auth();
                gate.observe(&auth, manual);
                let previous = app.state::<AppState>().kiro.lock().unwrap().clone();
                let mut snap = if let Some(cli) = find_cli() {
                    if gate.storage_blocked && !manual {
                        let retained = storage_pause(&previous);
                        (retained.note != previous.note || retained.status != previous.status).then_some(retained)
                    } else if gate.block_expired(&auth, manual, now) {
                        let mut snap = from_result(&previous, Err(ReadError::NeedsAuth), &mut enrichment);
                        snap.note = "Kiro credential expired; polling paused. Sign in externally or use Refresh. Last reading retained.".into();
                        (previous.note != snap.note || previous.status != snap.status).then_some(snap)
                    } else if gate.due(now, kiro_active(&dir, now)) {
                        if gate.reserve_probe(&poll_path(), now).is_err() { Some(storage_pause(&previous)) }
                        else {
                            let result = std::fs::create_dir_all(&dir).map_err(|_| ReadError::Process)
                                .and_then(|_| run_process(&cli, &ARGS, &dir, Duration::from_secs(20)))
                                .and_then(|text| parse_cli(&text, Local::now()));
                            let error = result.as_ref().err().copied();
                            let mut snap = from_result(&previous, result, &mut enrichment);
                            gate.record(now_ms(), resolved_error(error, &snap));
                            // A busy auth database defers this observation, including across restarts.
                            gate.observe(&observe_auth(), false);
                            if gate.standby { snap.note.push_str(" Polling paused after 3 failures; sign in externally or use Refresh."); }
                            else if gate.consecutive_failures > 0 { snap.note.push_str(&format!(" Attempt {}/3; retry in 30s.", gate.consecutive_failures)); }
                            Some(snap)
                        }
                    } else { None }
                } else {
                    enrichment = Enrichment::default();
                    (previous.status != "absent").then(absent)
                };
                if !gate.storage_blocked && old_gate != gate && save_poll_state(&poll_path(), &gate).is_err() {
                    gate.standby = true; gate.storage_blocked = true;
                    snap = Some(storage_pause(snap.as_ref().unwrap_or(&previous)));
                }
                if let Some(snap) = snap {
                    *app.state::<AppState>().kiro.lock().unwrap() = snap.clone();
                    let _ = crate::agy_cli::save_persisted_to(&store_path(), &snap);
                    let _ = app.emit("kiro", &snap);
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

pub fn probe() -> String {
    match find_cli() {
        Some(path) => format!("Kiro: native CLI at {} (usage checked when polled; SQLite enrichment {})", path.display(), if database_paths().iter().any(|p| credentials(p).is_some()) { "available" } else { "unavailable" }),
        None => "Kiro: native CLI not found; install it or set an absolute KIRO_CLI_PATH (.exe)".into(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn api_identity_is_attached_only_to_api_windows_not_preserved_cli_bonus() {
        let mut reading=fixture("Plan: Q Developer Pro\nCredits (12.50 of 50 covered in plan)\n████ 25%\nBonus credits: 10/100 credits used, expires in 19 days");
        let cli_bonus=reading.windows.iter().find(|w| w.id=="bonus").unwrap().clone();
        apply_limits(&mut reading,parse_limits(&limits()).unwrap());
        attach_api_identity(&mut reading,"arn:aws:codewhisperer:us-east-1:123:profile/synthetic");
        let plan=reading.windows.iter().find(|w| w.id=="credits").unwrap();
        assert!(plan.quota_account_key.is_some());
        assert_eq!(plan.reading_transport,Some(QuotaReadingTransport::KiroUsageApi));
        let bonus=reading.windows.iter().find(|w| w.id=="bonus").unwrap();
        assert!(bonus.quota_account_key.is_none()); assert_eq!(bonus.used,cli_bonus.used);
        assert_eq!(bonus.reading_transport,Some(QuotaReadingTransport::KiroCli));
    }
    use super::*;
    use serde_json::json;

    fn clock() -> chrono::DateTime<Local> { Local.with_ymd_and_hms(2026, 1, 10, 12, 0, 0).unwrap() }
    fn fixture(text: &str) -> Reading { parse_cli(text, clock()).unwrap() }
    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join("codenotch").join(format!("kiro-tests-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap(); p
    }
    fn limits() -> serde_json::Value {
        json!({"usageBreakdownList":[{"resourceType":"CREDIT", "currentUsageWithPrecision":3000.25,
            "usageLimitWithPrecision":3000.0,"currentOveragesWithPrecision":0.25,"overageCapWithPrecision":1000.0,
            "nextDateReset":1780272000.0}],"overageConfiguration":{"overageStatus":"ENABLED"}})
    }

    #[test]
    fn free_tier_and_decimal_ratio_are_used_not_remaining() {
        let free = fixture("| KIRO FREE |\n█████ 25%\n(12.50 of 50 covered in plan), resets on 01/15");
        assert_eq!(free.plan.as_deref(), Some("Kiro Free"));
        assert_eq!(free.windows[0].used, 0.25);
        let ratio = fixture("| KIRO PRO |\n(0.17 of 50 covered in plan)");
        assert!((ratio.windows[0].used - 0.0034).abs() < 0.000001);
        assert!(fixture("Plan: KIRO FREE\n(0.00 of 0 covered in plan)").windows.is_empty());
    }

    #[test]
    fn two_x_ansi_card_and_bonus_wallet_preserve_cli_percent() {
        let text = "\x1b[1mEstimated Usage\x1b[0m | resets on 2026-06-01 | \x1b[mKIRO FREE\x1b[0m\n🎁 Bonus credits: 45.53/2000 credits used, expires in 19 days\nCredits (0.17 of 50 covered in plan)\n███████ 0%\nOverages: Disabled";
        let r = fixture(text);
        assert_eq!(r.plan.as_deref(), Some("Kiro Free"));
        assert_eq!(r.windows.len(), 2);
        assert_eq!(r.windows[0].id, "credits");
        assert_eq!(r.windows[0].used, 0.0);
        assert_eq!(r.windows[0].credits.as_ref().unwrap().consumed, 0.17);
        assert_eq!(r.windows[0].credits.as_ref().unwrap().allowance, 50.0);
        assert_eq!(r.windows[1].group.as_deref(), Some("Bonus"));
        assert!((r.windows[1].used - 45.53 / 2000.0).abs() < 0.000001);
        assert_eq!(r.windows[1].credits.as_ref().unwrap().consumed, 45.53);
        assert_eq!(r.windows[1].credits.as_ref().unwrap().allowance, 2000.0);
        assert!(r.windows[1].resets_at.is_some());
        assert!(!r.windows.iter().any(|w| w.id == "weekly"));
        assert_eq!(strip_ansi("\x1b]8;;https://kiro.dev\x07hello\x1b]8;;\x1b\\\x1b(Bliteral"), "helloliteral");
    }

    #[test]
    fn plan_only_and_managed_accounts_never_claim_zero_usage() {
        assert!(fixture("\x1b[32mPlan: KIRO PRO MAX | 1 usage breakdowns\x1b[0m").windows.is_empty());
        let managed = fixture("Plan: Q Developer Pro\nYour plan is managed by admin");
        assert_eq!(managed.plan.as_deref(), Some("Q Developer Pro"));
        assert!(managed.windows.is_empty());
        assert!(matches!(parse_cli("random startup warning", clock()), Err(ReadError::BadOutput)));
        for text in ["not logged in", "Login required", "Failed to initialize auth portal", "kiro-cli login --use-device-flow", "OAuth error"] {
            assert!(matches!(parse_cli(text, clock()), Err(ReadError::NeedsAuth)));
        }
    }

    #[test]
    fn reset_dates_use_utc_and_preserve_explicit_source_offsets() {
        let today = Local.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        let reset = reset_stamp("01/15", today).unwrap();
        assert_eq!(Utc.timestamp_millis_opt(reset as i64).unwrap(), Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap());
        let next = reset_stamp("01/14", today).unwrap();
        assert_eq!(Utc.timestamp_millis_opt(next as i64).unwrap(), Utc.with_ymd_and_hms(2027, 1, 14, 0, 0, 0).unwrap());
        assert_eq!(reset_stamp("2026-11-01T00:00:00-03:00", today),
            Some(Utc.with_ymd_and_hms(2026, 11, 1, 3, 0, 0).unwrap().timestamp_millis() as u64));
        assert!(reset_stamp("02/31", today).is_none());
        assert!(reset_stamp("2026-13-01", today).is_none());
    }

    #[test]
    fn monthly_periods_use_real_calendar_lengths_only_at_utc_month_boundaries() {
        let epoch = |year, month| Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).unwrap().timestamp_millis() as u64;
        assert_eq!(monthly_duration_seconds(epoch(2026, 11)), Some(31 * 86400));
        assert_eq!(monthly_duration_seconds(epoch(2027, 3)), Some(28 * 86400));
        assert_eq!(monthly_duration_seconds(epoch(2028, 3)), Some(29 * 86400));
        assert_eq!(monthly_duration_seconds(epoch(2026, 5)), Some(30 * 86400));
        assert_eq!(monthly_duration_seconds(epoch(2026, 11) + 1), None);
        assert_eq!(monthly_duration_seconds(u64::MAX), None);
    }

    #[test]
    fn streams_choose_usage_card_over_stray_output() {
        assert_eq!(usage_text("warning", "Estimated Usage | KIRO FREE").as_deref(), Some("Estimated Usage | KIRO FREE"));
        assert_eq!(usage_text("Estimated Usage", "warning").as_deref(), Some("Estimated Usage"));
        assert_eq!(usage_text("", "card").as_deref(), Some("card"));
        assert!(usage_text(" \n", "\t").is_none());
    }

    #[test]
    fn native_discovery_rejects_overrides_directories_and_shims() {
        let dir = scratch("discovery");
        let exe = dir.join("kiro-cli.exe"); std::fs::write(&exe, "fixture").unwrap();
        let shim = dir.join("kiro-cli.cmd"); std::fs::write(&shim, "fixture").unwrap();
        assert_eq!(locate(Some(exe.to_str().unwrap()), &[], None), Some(exe.clone()));
        assert!(locate(Some("relative/kiro-cli.exe"), &[exe.clone()], None).is_none());
        assert!(locate(Some(shim.to_str().unwrap()), &[exe.clone()], None).is_none());
        assert!(locate(Some(dir.to_str().unwrap()), &[exe.clone()], None).is_none());
        assert!(locate(Some(dir.join("missing.exe").to_str().unwrap()), &[exe.clone()], None).is_none());
        let path = std::env::join_paths([PathBuf::from("."), dir.clone()]).unwrap();
        assert_eq!(locate(None, &[], Some(&path)), Some(exe.clone()));
        let relative = std::env::join_paths([PathBuf::from(".")]).unwrap();
        assert!(locate(None, &[], Some(&relative)).is_none());
        std::fs::remove_file(exe).unwrap(); std::fs::remove_file(shim).unwrap(); std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn usage_limits_split_overage_and_keep_bonus_wallet() {
        let parsed = parse_limits(&limits()).unwrap();
        assert_eq!(parsed.plan_used, 3000.0);
        let mut r = fixture("Plan: KIRO POWER\n████ 80%\nBonus credits: 5.00/10 credits used, expires in 7 days");
        apply_limits(&mut r, parsed);
        assert_eq!(r.windows.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["credits", "bonus", "overage"]);
        assert_eq!(r.windows[0].used, 1.0);
        assert_eq!(r.windows[1].used, 0.5);
        assert_eq!(r.windows[2].used, 0.00025);
        assert_eq!(r.windows[0].credits.as_ref().unwrap().consumed, 3000.0);
        assert_eq!(r.windows[0].credits.as_ref().unwrap().allowance, 3000.0);
        assert_eq!(r.windows[0].credit_scope, Some(CreditScope::Plan));
        assert_eq!(r.windows[0].duration_seconds, Some(31 * 86400));
        assert_eq!(r.windows[1].credits.as_ref().unwrap().consumed, 5.0);
        assert_eq!(r.windows[2].credits.as_ref().unwrap().consumed, 0.25);
        assert_eq!(r.windows[2].credits.as_ref().unwrap().allowance, 1000.0);
        let mut v = limits(); v["usageBreakdownList"][0]["bonuses"] = json!([{}]);
        let mut r = fixture("Plan: KIRO POWER\n████ 80%\n(0.17 of 50 covered in plan)");
        apply_limits(&mut r, parse_limits(&v).unwrap());
        assert_eq!(r.windows[0].used, 0.8, "unseparated API bonus cannot replace CLI credits");
        assert_eq!(r.windows[0].credits.as_ref().unwrap().consumed, 0.17);
        assert_eq!(r.windows[0].credits.as_ref().unwrap().allowance, 50.0);
        assert_eq!(r.windows[0].credit_scope, Some(CreditScope::Combined));
    }

    #[test]
    fn credit_amounts_require_an_explicit_valid_pair_and_survive_cache_roundtrip() {
        for text in ["Plan: KIRO FREE\n████ 0%", "Plan: KIRO FREE\n████ 25%\n(no data covered in plan)",
            "Plan: KIRO FREE\n████ 25%\n(0.17junk of 50 covered in plan)",
            "Plan: KIRO FREE\n████ 25%\n(-1 of 50 covered in plan)",
            "Plan: KIRO FREE\n████ 25%\n(0.17 of 0 covered in plan)"] {
            assert!(fixture(text).windows[0].credits.is_none());
        }
        let old = json!({"id":"credits","label":"Credits","used":0.25,"resets_at":null});
        let window: LimitWindow = serde_json::from_value(old).unwrap();
        assert!(window.credits.is_none());
        assert!(serde_json::to_value(window).unwrap().get("credits").is_none());
        let reading = fixture("Plan: KIRO FREE\n████ 0%\n(0.17 of 50 covered in plan)");
        let restored: LimitWindow = serde_json::from_value(serde_json::to_value(&reading.windows[0]).unwrap()).unwrap();
        assert_eq!(restored.used, 0.0);
        assert_eq!(restored.credits.unwrap().consumed, 0.17);
        let zero = fixture("Plan: KIRO FREE\n(0.00 of 50 covered in plan)");
        assert_eq!(zero.windows[0].credits.as_ref().unwrap().consumed, 0.0);
    }

    #[test]
    fn malformed_or_ambiguous_limits_do_not_replace_cli() {
        let mut v = limits(); v["usageBreakdownList"][0]["currentOveragesWithPrecision"] = json!(4000);
        assert!(parse_limits(&v).is_none());
        let mut v = limits(); v["usageBreakdownList"][0]["currentUsageWithPrecision"] = json!(4000);
        assert!(parse_limits(&v).is_none());
        let mut v = limits(); let duplicate = v["usageBreakdownList"][0].clone(); v["usageBreakdownList"].as_array_mut().unwrap().push(duplicate);
        assert!(parse_limits(&v).is_none());
        let mut v = limits(); v["usageBreakdownList"][0]["nextDateReset"] = json!(1780272000000u64);
        assert!(parse_limits(&v).unwrap().reset.is_none());
        let mut v = limits(); v["usageBreakdownList"][0]["currentUsageWithPrecision"] = serde_json::Value::Null;
        v["usageBreakdownList"][0]["currentUsage"] = json!(2000);
        assert_eq!(parse_limits(&v).unwrap().plan_used, 1999.75);
        let mut v = limits(); v["overageConfiguration"]["overageStatus"] = json!("DISABLED");
        assert!(parse_limits(&v).unwrap().overage_cap.is_none());
    }

    #[test]
    fn regional_arn_is_a_host_allowlist() {
        assert_eq!(endpoint("arn:aws:codewhisperer:us-east-1:123:profile/test"), Some("https://codewhisperer.us-east-1.amazonaws.com/"));
        assert_eq!(endpoint("arn:aws:codewhisperer:eu-central-1:123:profile/test"), Some("https://q.eu-central-1.amazonaws.com/"));
        for arn in ["arn:aws:codewhisperer:us-west-2:123:profile/test", "arn:aws:evil:us-east-1:123:profile/test", "arn:aws:codewhisperer:us-east-1:123:profile/", "arn:aws:codewhisperer:us-east-1:123:profile/test\n"] { assert!(endpoint(arn).is_none()); }
    }

    #[test]
    fn sqlite_borrowing_does_not_modify_owner_credentials() {
        let dir = scratch("sqlite"); let path = dir.join("data.sqlite3");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE auth_kv(key TEXT, value TEXT); CREATE TABLE state(key TEXT, value TEXT);").unwrap();
        conn.execute("INSERT INTO auth_kv VALUES (?1, ?2)", ["kirocli:odic:token", "{\"accessToken\":\"fixture-access-only\",\"refresh_token\":\"fixture\"}"]).unwrap();
        conn.execute("INSERT INTO state VALUES (?1, ?2)", ["api.codewhisperer.profile", "{\"arn\":\"arn:aws:codewhisperer:us-east-1:123:profile/test\"}"]).unwrap();
        drop(conn);
        let before = std::fs::read(&path).unwrap();
        let (token, _) = credentials(&path).unwrap(); assert_eq!(token, "fixture-access-only");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn errors_keep_history_and_enrichment_backoff_does_not_block_cli() {
        let prev = UsageSnapshot { status: "ok".into(), windows: fixture("Plan: KIRO FREE\n████ 25%").windows, fetched_at: 1, ..Default::default() };
        let mut e = Enrichment { until: u64::MAX, attempts: 3 };
        let stale = from_result(&prev, Err(ReadError::TimedOut), &mut e);
        assert_eq!(stale.status, "stale"); assert_eq!(stale.fetched_at, 1);
        let signed_out = from_result(&prev, Err(ReadError::NeedsAuth), &mut e);
        assert_eq!(signed_out.status, "stale"); assert_eq!(signed_out.windows[0].used, 0.25);
        assert_eq!(signed_out.fetched_at, 1);
        let empty = from_result(&UsageSnapshot::default(), Err(ReadError::NeedsAuth), &mut e);
        assert_eq!(empty.status, "needsAuth"); assert!(empty.windows.is_empty());
        let current = from_result(&prev, Ok(fixture("Plan: KIRO FREE\n████ 10%")), &mut e);
        assert_eq!(current.status, "ok"); assert_eq!(current.windows[0].used, 0.1);
        let plan_only = from_result(&prev, Ok(fixture("Plan: Q Developer Pro")), &mut e);
        assert_eq!(plan_only.status, "stale"); assert_eq!(plan_only.windows[0].used, 0.25);
        assert_eq!(plan_only.fetched_at, 1);
        assert!(absent().windows.is_empty());
        assert_eq!(backoff(0, 0), 60); assert_eq!(backoff(3, 0), 480); assert_eq!(backoff(9, 3600), 900);
    }

    #[test]
    fn process_failures_are_not_automatically_auth_failures() {
        assert!(matches!(process_output(1, "", "network connection failed"), Err(ReadError::Process)));
        assert!(matches!(process_output(1, "", "Login required"), Err(ReadError::NeedsAuth)));
        assert!(matches!(process_output(0, "", "not logged in"), Err(ReadError::NeedsAuth)));
        assert!(matches!(process_output(0, "", ""), Err(ReadError::BadOutput)));
        assert_eq!(process_output(0, "Estimated Usage", "warning").unwrap(), "Estimated Usage");
        assert!(matches!(process_output(1, "", "HTTP 429: too many requests"), Err(ReadError::RateLimited)));
        assert_eq!(process_output(0, "Estimated Usage", "Rate limits are shown in this card").unwrap(), "Estimated Usage");
        let card = "Estimated Usage\nRate limits: monthly credits";
        assert_eq!(process_output(0, card, "").unwrap(), card);
        for failure in ["Max rate limit exceeded", "Rate limit reached", "Rate limited", "HTTP429", "Too many requests"] {
            assert!(matches!(process_output(0, "", failure), Err(ReadError::RateLimited)));
        }
    }

    #[test]
    fn polling_caps_all_failure_causes_and_success_reopens_budget() {
        for cause in [ReadError::NeedsAuth, ReadError::TimedOut, ReadError::BadOutput, ReadError::Process, ReadError::RateLimited] {
            let mut gate = PollState::default();
            for attempt in 1..=3 {
                assert!(gate.due(attempt * POLL_ACTIVE_MS, false));
                gate.record(attempt * POLL_ACTIVE_MS, Some(cause));
                assert_eq!(gate.consecutive_failures, attempt as u8);
                assert_eq!(gate.last_failure, Some(cause));
            }
            for now in [POLL_IDLE_MS, u64::MAX] {
                assert!(!gate.due(now, false)); assert!(!gate.due(now, true), "activity cannot leave standby");
            }
            gate.observe(&AuthObservation::default(), true);
            assert!(gate.due(100_000, false));
            gate.record(100_000, None);
            assert_eq!(gate.consecutive_failures, 0); assert!(!gate.standby); assert!(gate.last_failure.is_none());
        }
        let mut mixed = PollState::default();
        for cause in [ReadError::TimedOut, ReadError::Process, ReadError::NeedsAuth] { mixed.record(1, Some(cause)); }
        assert!(mixed.standby, "changing error type cannot bypass the three-failure cap");
    }

    #[test]
    fn standby_survives_restart_without_storing_credentials() {
        let dir = scratch("poll-state"); let path = dir.join("kiro-poll.json");
        let auth = auth_from_value(&json!({"accessToken":"fixture-sensitive-token", "expiresAt":2_000_000_000}));
        let mut gate = PollState::default(); gate.observe(&auth, false);
        for _ in 0..3 { gate.record(123, Some(ReadError::TimedOut)); }
        save_poll_state(&path, &gate).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("fixture-sensitive-token"));
        let mut restored = load_poll_state(&path);
        assert!(restored.standby); assert_eq!(restored.consecutive_failures, 3);
        restored.observe(&auth, false);
        assert!(!restored.due(u64::MAX, true));
        restored.observe(&auth, true);
        assert!(restored.due(1000, false));
        save_poll_state(&path, &restored).unwrap();
        assert!(!load_poll_state(&path).standby);
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn invalid_or_unreadable_local_state_fails_closed_until_explicit_repair() {
        let dir = scratch("invalid-state"); let path = dir.join("kiro-poll.json");
        assert!(!load_poll_state(&path).storage_blocked, "missing file is first use");
        for raw in ["broken json", "{}", "[]", "{\"version\":1}"] {
            std::fs::write(&path, raw).unwrap();
            let mut gate = load_poll_state(&path);
            assert!(gate.storage_blocked); assert!(gate.standby); assert!(!gate.due(u64::MAX, true));
            gate.observe(&auth_from_value(&json!({"accessToken":"fixture-new-login"})), false);
            assert!(gate.standby, "new auth cannot repair invalid retry state");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
            gate.observe(&AuthObservation::default(), true);
            gate.reserve_probe(&path, 1).unwrap();
            assert!(!gate.storage_blocked); assert!(!load_poll_state(&path).storage_blocked);
        }
        let complete = serde_json::to_value(PollState::default()).unwrap();
        for key in ["version", "consecutive_failures", "standby", "last_failure", "auth_fingerprint", "last_attempt_ms"] {
            let mut missing = complete.clone(); missing.as_object_mut().unwrap().remove(key);
            std::fs::write(&path, missing.to_string()).unwrap(); assert!(load_poll_state(&path).storage_blocked);
        }
        let mut legacy = complete.clone(); legacy.as_object_mut().unwrap().remove("post_probe_observation_pending");
        std::fs::write(&path, legacy.to_string()).unwrap(); assert!(!load_poll_state(&path).storage_blocked);
        let mut cap = complete.clone(); cap["consecutive_failures"] = json!(3); cap["standby"] = json!(false);
        std::fs::write(&path, cap.to_string()).unwrap(); assert!(load_poll_state(&path).standby);
        cap["consecutive_failures"] = json!(4);
        std::fs::write(&path, cap.to_string()).unwrap(); assert!(load_poll_state(&path).storage_blocked);
        std::fs::remove_file(&path).unwrap(); std::fs::create_dir(&path).unwrap();
        assert!(load_poll_state(&path).storage_blocked, "a directory is an unreadable state file");
        std::fs::remove_dir(path).unwrap(); std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn blocked_state_writes_prevent_probes_even_after_refresh_and_restart() {
        let dir = scratch("write-blocked"); let parent = dir.join("parent-is-file");
        std::fs::write(&parent, "fixture").unwrap(); let path = parent.join("kiro-poll.json");
        let mut gate = PollState::default();
        assert!(gate.reserve_probe(&path, 1).is_err());
        assert!(gate.storage_blocked); assert!(!gate.due(100_000, true));
        gate.observe(&AuthObservation::default(), true);
        assert!(gate.reserve_probe(&path, 2).is_err(), "manual retry still requires durable state");
        assert!(!gate.due(100_000, true));
        let mut restarted = load_poll_state(&path);
        assert!(restarted.storage_blocked); assert!(!restarted.due(u64::MAX, false));
        restarted.observe(&AuthObservation::default(), true);
        assert!(restarted.reserve_probe(&path, 3).is_err());
        std::fs::remove_file(parent).unwrap(); std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn durable_reservation_caps_incomplete_probes_across_restart() {
        let dir = scratch("reservation"); let path = dir.join("kiro-poll.json");
        for attempt in 1..=3 {
            let mut gate = load_poll_state(&path);
            assert!(gate.due(attempt, false));
            gate.reserve_probe(&path, attempt).unwrap();
            let restarted = load_poll_state(&path);
            assert_eq!(restarted.consecutive_failures, attempt as u8);
            assert!(restarted.post_probe_observation_pending);
        }
        assert!(!load_poll_state(&path).due(u64::MAX, true));
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn deferred_own_refresh_observation_never_reopens_failure_budget() {
        let dir = scratch("pending-auth"); let path = dir.join("kiro-poll.json");
        let auth = |id: u64| auth_from_value(&json!({"accessToken":format!("fixture-{id}")}));
        let mut gate = PollState::default(); gate.observe(&auth(1), false);
        for attempt in 1..=3 {
            gate.reserve_probe(&path, attempt).unwrap();
            gate.record(attempt, Some(ReadError::NeedsAuth));
            gate.observe(&AuthObservation::default(), false); // Auth database busy after own CLI refresh.
            save_poll_state(&path, &gate).unwrap();
            gate = load_poll_state(&path);
            gate.observe(&auth(attempt + 1), false);
            assert_eq!(gate.consecutive_failures, attempt as u8);
            assert!(!gate.post_probe_observation_pending);
        }
        assert!(gate.standby); assert!(!gate.due(u64::MAX, true));
        gate.observe(&auth(99), false);
        assert!(!gate.standby, "subsequent observable external login reopens the gate");
        gate.observe(&auth(99), true); assert!(gate.due(1, false));
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn unchanged_post_probe_token_defers_late_own_refresh_without_resetting_cap() {
        let dir = scratch("late-auth"); let path = dir.join("kiro-poll.json");
        let auth = |id: u64| auth_from_value(&json!({"accessToken":format!("fixture-{id}")}));
        let mut gate = PollState::default(); gate.observe(&auth(1), false);
        for attempt in 1..=3 {
            gate.reserve_probe(&path, attempt).unwrap();
            gate.record(attempt, Some(ReadError::NeedsAuth));
            gate.observe(&auth(attempt), false); // Post-probe read succeeds, but still sees pre-probe token.
            assert!(gate.post_probe_observation_pending);
            save_poll_state(&path, &gate).unwrap(); gate = load_poll_state(&path);
            gate.observe(&auth(attempt + 1), false); // Late token change belongs to an ambiguous failed probe.
            assert_eq!(gate.consecutive_failures, attempt as u8);
        }
        assert!(gate.standby); assert!(!gate.due(u64::MAX, true));
        gate.observe(&auth(4), true); assert!(gate.due(1, false));
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn only_observable_credential_change_or_manual_refresh_reopens_standby() {
        let original = auth_from_value(&json!({"access_token":"fixture-one", "expires_at":"2026-11-01T00:00:00Z"}));
        let unchanged = auth_from_value(&json!({"access_token":"fixture-one", "expires_at":"2026-11-01T00:00:00Z", "unrelated":"changed"}));
        let changed = auth_from_value(&json!({"access_token":"fixture-two", "expires_at":"2026-11-01T00:00:00Z"}));
        let mut gate = PollState::default(); gate.observe(&original, false);
        for _ in 0..3 { gate.record(1, Some(ReadError::NeedsAuth)); }
        gate.observe(&unchanged, false); assert!(gate.standby);
        gate.observe(&AuthObservation::default(), false); assert!(gate.standby, "unreadable store is not new credentials");
        gate.observe(&changed, false); assert!(!gate.standby); assert!(gate.due(2, false));
        let mut unknown = PollState::default();
        for _ in 0..3 { unknown.record(1, Some(ReadError::NeedsAuth)); }
        unknown.observe(&original, false); assert!(unknown.standby, "first fingerprint is only a baseline");
        unknown.observe(&changed, false); assert!(!unknown.standby);
        // A refresh performed by our own CLI is remembered without resetting failures.
        gate.record(3, Some(ReadError::NeedsAuth)); gate.auth_fingerprint = original.fingerprint.clone();
        gate.observe(&original, false); assert_eq!(gate.consecutive_failures, 1);
    }

    #[test]
    fn known_expiry_blocks_background_but_unknown_native_auth_remains_bounded() {
        let now = 1_800_000_000_000u64;
        for value in [json!({"accessToken":"fixture", "expiresAt":1_800_000_000}),
            json!({"accessToken":"fixture", "expires_at_ms":now}),
            json!({"accessToken":"fixture", "expires_at":"2026-01-01T00:00:00Z"})] {
            let auth = auth_from_value(&value); let mut gate = PollState::default(); gate.observe(&auth, false);
            assert!(gate.block_expired(&auth, false, now)); assert!(!gate.due(now, true));
            assert_eq!(gate.consecutive_failures, 0, "preflight does not launch a failed probe");
            gate.observe(&auth, true); assert!(!gate.block_expired(&auth, true, now)); assert!(gate.due(now, false));
        }
        for auth in [AuthObservation::default(), auth_from_value(&json!({"accessToken":"fixture"})),
            auth_from_value(&json!({"accessToken":"fixture", "expiresAt":"unrecognized"})),
            auth_from_value(&json!({"accessToken":"fixture", "expiresAt":1_900_000_000}))] {
            let mut gate = PollState::default(); gate.observe(&auth, false);
            assert!(!gate.block_expired(&auth, false, now)); assert!(gate.due(now, false));
        }
    }

    #[test]
    fn cadence_tracks_recent_messages_without_confusing_activity_with_login() {
        let mut gate = PollState::default(); gate.record(1000, None);
        assert!(!gate.due(1000 + POLL_ACTIVE_MS - 1, true));
        assert!(gate.due(1000 + POLL_ACTIVE_MS, true));
        assert!(!gate.due(1000 + POLL_ACTIVE_MS, false));
        assert!(gate.due(1000 + POLL_IDLE_MS, false));
        assert!(recent_activity(1000, 1000 + ACTIVITY_RECENT_MS));
        assert!(!recent_activity(1000, 1000 + ACTIVITY_RECENT_MS + 1));
        assert!(!recent_activity(1001, 1000));
        for _ in 0..3 { gate.record(1000, Some(ReadError::Process)); }
        assert!(!gate.due(1000 + POLL_IDLE_MS, true));
    }

    #[test]
    fn sign_in_failure_preserves_last_official_two_percent_and_credit_pair() {
        let prev = UsageSnapshot { status: "ok".into(), windows: fixture("Plan: KIRO POWER\n████ 2%\n(82.79 of 5000 covered in plan)").windows,
            fetched_at: 1_800_000_000_000, ..Default::default() };
        let mut enrichment = Enrichment { until: u64::MAX, ..Default::default() };
        let stale = from_result(&prev, Err(ReadError::NeedsAuth), &mut enrichment);
        assert_eq!(stale.status, "stale"); assert_eq!(stale.fetched_at, prev.fetched_at);
        assert_eq!(stale.windows[0].used, 0.02);
        assert_eq!(stale.windows[0].credits.as_ref().unwrap().consumed, 82.79);
        assert_eq!(stale.windows[0].credits.as_ref().unwrap().allowance, 5000.0);
        let saved = serde_json::to_vec(&stale).unwrap();
        let restored: UsageSnapshot = serde_json::from_slice(&saved).unwrap();
        assert_eq!(restored.status, "stale"); assert_eq!(restored.windows[0].used, 0.02);
    }

    #[test]
    fn plan_only_without_usable_enrichment_preserves_two_percent_and_consumes_budget() {
        let prev = UsageSnapshot { status: "ok".into(), windows: fixture("Plan: KIRO POWER\n████ 2%\n(82.79 of 5000 covered in plan)").windows,
            fetched_at: 123, ..Default::default() };
        let mut enrichment = Enrichment { until: u64::MAX, ..Default::default() }; // No network, credentials or live enrichment.
        let mut gate = PollState::default();
        for attempt in 1..=3 {
            let snapshot = from_result(&prev, Ok(fixture("Plan: Q Developer Pro")), &mut enrichment);
            let error = resolved_error(None, &snapshot);
            assert_eq!(error, Some(ReadError::BadOutput));
            gate.record(attempt, error);
            assert_eq!(snapshot.status, "stale"); assert_eq!(snapshot.fetched_at, 123);
            assert_eq!(snapshot.windows[0].used, 0.02);
            assert_eq!(snapshot.windows[0].credits.as_ref().unwrap().consumed, 82.79);
        }
        assert!(gate.standby); assert!(!gate.due(u64::MAX, true));
        gate.observe(&AuthObservation::default(), true);
        let mut enriched = fixture("Plan: Q Developer Pro");
        apply_limits(&mut enriched, parse_limits(&limits()).unwrap());
        let snapshot = from_result(&prev, Ok(enriched), &mut enrichment);
        assert_eq!(snapshot.status, "ok"); assert!(!snapshot.windows.is_empty());
        gate.record(4, resolved_error(None, &snapshot));
        assert_eq!(gate.consecutive_failures, 0); assert!(!gate.standby);
    }

    #[test]
    fn activity_uses_bounded_native_metadata_and_excludes_imports_and_poller() {
        let dir = scratch("activity"); let root = dir.join("sessions");
        let session = root.join("native/sess_fixture"); std::fs::create_dir_all(&session).unwrap();
        let messages = session.join("messages.jsonl"); std::fs::write(&messages, "fixture\n").unwrap();
        let metadata = session.join("session.json");
        let now = std::fs::metadata(&messages).unwrap().modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let poll = dir.join("kiro-usage");
        std::fs::write(&metadata, json!({"rootPaths":[dir.join("project").to_string_lossy()]}).to_string()).unwrap();
        assert!(recent_activity_at(&root, &poll, now));
        assert!(!recent_activity_at(&root, &poll, now + ACTIVITY_RECENT_MS + 1));
        std::fs::write(&metadata, json!({"rootPaths":[dir.join("project").to_string_lossy()],"_meta":{"sincronizarConversas":{}}}).to_string()).unwrap();
        assert!(!recent_activity_at(&root, &poll, now));
        std::fs::write(&metadata, json!({"rootPaths":[poll.to_string_lossy()]}).to_string()).unwrap();
        assert!(!recent_activity_at(&root, &poll, now));
        std::fs::write(&metadata, json!({"workspacePaths":[poll.join("child").to_string_lossy()]}).to_string()).unwrap();
        assert!(!recent_activity_at(&root, &poll, now));
        std::fs::write(&metadata, json!({"workspacePaths":[dir.join("project").to_string_lossy()]}).to_string()).unwrap();
        assert!(recent_activity_at(&root, &poll, now));
        std::fs::remove_file(messages).unwrap(); std::fs::remove_file(metadata).unwrap();
        std::fs::remove_dir(session).unwrap(); std::fs::remove_dir(root.join("native")).unwrap();
        std::fs::remove_dir(root).unwrap(); std::fs::remove_dir(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn native_runner_drains_both_streams_and_owns_child_timeout() {
        let dir = scratch("runner");
        let cmd = PathBuf::from(std::env::var_os("ComSpec").unwrap());
        let script = dir.join("fixture.cmd");
        std::fs::write(&script, "@echo off\r\necho warning\r\necho Estimated Usage 1>&2\r\necho %TERM% %KIRO_CHAT_UI% 1>&2\r\n").unwrap();
        let text = run_process(&cmd, &["/D", "/C", script.to_str().unwrap()], &dir, Duration::from_secs(3)).unwrap();
        assert!(text.contains("Estimated Usage")); assert!(text.contains("dumb classic"));
        // Child would write later if it escaped the JobObject.
        let child = dir.join("child.cmd");
        let marker = dir.join("escaped.txt");
        std::fs::write(&child, "@echo off\r\nping -n 3 127.0.0.1 >nul\r\necho escaped >escaped.txt\r\n").unwrap();
        std::fs::write(&script, "@echo off\r\nstart \"\" /b cmd /d /c child.cmd\r\nping -n 20 127.0.0.1 >nul\r\n").unwrap();
        let started = std::time::Instant::now();
        assert!(matches!(run_process(&cmd, &["/D", "/C", script.to_str().unwrap()], &dir, Duration::from_millis(300)), Err(ReadError::TimedOut)));
        assert!(started.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_secs(3)); assert!(!marker.exists());
        std::fs::write(&script, "@echo off\r\nfor /l %%i in (1,1,10000) do echo Estimated Usage output overflow\r\n").unwrap();
        assert!(matches!(run_process(&cmd, &["/D", "/C", script.to_str().unwrap()], &dir, Duration::from_secs(5)), Err(ReadError::BadOutput)));
        std::fs::remove_file(script).unwrap(); std::fs::remove_file(child).unwrap(); std::fs::remove_dir(dir).unwrap();
    }
}
