//! Immutable official readings, separate from numeric consumption batches/checkpoints.
use super::{hash, safe_label, store, TelemetrySettings};
use crate::usage::{CreditAmounts, CreditScope, LimitWindow, QuotaReadingTransport, QuotaResetBasis, UsageSnapshot};
use crate::usage_insights::ProviderInsights;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::{Path, PathBuf}};

const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;

/// Frozen projection-free wire shape. New UI fields cannot change either journal schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedInsights { provider: String, generated_at: u64, windows: Vec<OwnedWindowInsights> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedWindowInsights {
    window_index: usize, window_id: String, label: String, group: Option<String>, fresh: bool,
    official_used_percent: Option<f64>, official_remaining_percent: Option<f64>,
    credits: Option<OwnedCreditBalance>, period: Option<OwnedPeriodInsights>,
    work: Option<OwnedWorkInsights>, budget: Option<OwnedBudgetInsights>,
    reasons: Vec<crate::usage_insights::InsightReason>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedCreditBalance { consumed: f64, allowance: f64, remaining: f64, scope: CreditScope }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedPeriodInsights {
    start_ms: Option<u64>, reset_ms: u64, duration_seconds: Option<u64>, basis: crate::usage_insights::PeriodBasis,
    elapsed_percent: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedWorkInsights {
    cycle_total_days: Option<f64>, cycle_elapsed_days: Option<f64>, remaining_days: f64,
    elapsed_percent: Option<f64>, used_minus_work_pp: Option<f64>, calendar_partial: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedBudgetInsights {
    unit: crate::usage_insights::BudgetUnit, remaining: f64, per_remaining_workday: f64,
    reference_per_cycle_workday: Option<f64>, week_start_ms: u64, week_end_ms: u64,
    week_remaining_workdays: f64, week_remaining_budget: f64, week_reference_budget: Option<f64>,
}
impl From<&ProviderInsights> for OwnedInsights {
    fn from(i: &ProviderInsights) -> Self {
        Self { provider: i.provider.clone(), generated_at: i.generated_at, windows: i.windows.iter().map(|w| OwnedWindowInsights {
            window_index: w.window_index, window_id: w.window_id.clone(), label: w.label.clone(), group: w.group.clone(),
            fresh: w.fresh, official_used_percent: w.official_used_percent, official_remaining_percent: w.official_remaining_percent,
            credits: w.credits.as_ref().map(|c| OwnedCreditBalance { consumed: c.consumed, allowance: c.allowance, remaining: c.remaining, scope: c.scope }),
            period: w.period.as_ref().map(|p| OwnedPeriodInsights { start_ms: p.start_ms, reset_ms: p.reset_ms, duration_seconds: p.duration_seconds,
                basis: p.basis, elapsed_percent: p.elapsed_percent }),
            work: w.work.as_ref().map(|x| OwnedWorkInsights { cycle_total_days: x.cycle_total_days, cycle_elapsed_days: x.cycle_elapsed_days,
                remaining_days: x.remaining_days, elapsed_percent: x.elapsed_percent, used_minus_work_pp: x.used_minus_work_pp,
                calendar_partial: x.calendar_partial }),
            budget: w.budget.as_ref().map(|x| OwnedBudgetInsights { unit: x.unit, remaining: x.remaining,
                per_remaining_workday: x.per_remaining_workday, reference_per_cycle_workday: x.reference_per_cycle_workday,
                week_start_ms: x.week_start_ms, week_end_ms: x.week_end_ms, week_remaining_workdays: x.week_remaining_workdays,
                week_remaining_budget: x.week_remaining_budget, week_reference_budget: x.week_reference_budget }), reasons: w.reasons.clone(),
        }).collect() }
    }
}

/// An explicit allowlist prevents snapshot notes or future source fields from leaking.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuotaWindow {
    id: String,
    label: String,
    used: f64,
    resets_at: Option<u64>,
    count: Option<i64>,
    derived: bool,
    group: Option<String>,
    credits: Option<CreditAmounts>,
    credit_scope: Option<CreditScope>,
    duration_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reading_fetched_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reading_status: Option<String>,
}

impl From<&LimitWindow> for QuotaWindow {
    fn from(w: &LimitWindow) -> Self {
        Self { id: w.id.clone(), label: w.label.clone(), used: w.used,
            resets_at: w.resets_at, count: w.count, derived: w.derived,
            group: w.group.clone(), credits: w.credits.clone(), credit_scope: w.credit_scope,
            duration_seconds: w.duration_seconds, reading_fetched_at: w.reading_fetched_at,
            reading_status: w.reading_status.clone() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuotaReading {
    schema_version: u32,
    provider: String,
    fetched_at: u64,
    writer_id: String,
    machine: String,
    windows: Vec<QuotaWindow>,
    insights: OwnedInsights,
}

fn reading_key(provider: &str, fetched_at: u64, windows: &[QuotaWindow]) -> Result<String, String> {
    serde_json::to_vec(&(provider, fetched_at, windows)).map(|bytes| hash(&bytes)).map_err(|e| e.to_string())
}

fn valid_windows(windows: &[QuotaWindow]) -> bool {
    !windows.is_empty() && windows.iter().all(|w| w.used.is_finite() && w.used >= 0.0
        && w.count.is_none_or(|n| n >= 0)
        && w.credits.as_ref().is_none_or(|c| c.consumed.is_finite() && c.consumed >= 0.0
            && c.allowance.is_finite() && c.allowance >= 0.0))
}

fn valid_insights(provider: &str, fetched_at: u64, windows: &[QuotaWindow], insights: &OwnedInsights) -> bool {
    // Validate freshness with the same origin policy as the UI/tray engine. An aggregate
    // may legitimately contain an older account whose derived values must remain null.
    let snapshot = UsageSnapshot { status: "ok".into(), fetched_at,
        windows: windows.iter().map(|w| LimitWindow {
            id: w.id.clone(), label: w.label.clone(), used: w.used, resets_at: w.resets_at,
            count: w.count, derived: w.derived, group: w.group.clone(), credits: w.credits.clone(),
            credit_scope: w.credit_scope, duration_seconds: w.duration_seconds,
            reading_fetched_at: w.reading_fetched_at, reading_status: w.reading_status.clone(),
            ..Default::default()
        }).collect(), ..Default::default() };
    let expected = crate::usage_insights::compute_provider_insights(provider, &snapshot, None, fetched_at);
    insights.provider == provider && insights.generated_at == fetched_at
        && insights.windows.len() == windows.len()
        && insights.windows.iter().zip(windows).zip(&expected.windows).enumerate().all(|(index, ((i, w), expected))| {
            i.window_index == index && i.window_id == w.id && i.label == w.label && i.group == w.group
                && i.fresh == expected.fresh
                && (i.fresh || (i.period.is_none() && i.work.is_none() && i.budget.is_none()))
        })
}

fn path_for(root: &Path, reading: &QuotaReading, key: &str) -> Result<PathBuf, String> {
    let date = chrono::DateTime::<Utc>::from_timestamp_millis(i64::try_from(reading.fetched_at)
        .map_err(|_| "Invalid quota reading timestamp")?).ok_or("Invalid quota reading timestamp")?;
    Ok(root.join("machines").join(&reading.machine).join(&reading.writer_id)
        .join("quota-readings").join(date.format("%Y-%m").to_string()).join(format!("{key}.json")))
}

/// Returns false only for an absent destination; any existing invalid file is preserved.
fn existing_valid(path: &Path, expected: &QuotaReading, key: &str) -> Result<bool, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("Cannot read quota journal {}: {error}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take((MAX_RECORD_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_RECORD_BYTES { return Err(format!("Oversized quota journal preserved: {}", path.display())); }
    let reading: QuotaReading = serde_json::from_slice(&bytes)
        .map_err(|_| format!("Invalid quota journal preserved: {}", path.display()))?;
    if reading.schema_version != 1 || reading.provider != expected.provider || reading.fetched_at != expected.fetched_at
        || reading.machine != expected.machine || reading.writer_id != expected.writer_id
        || !valid_windows(&reading.windows)
        || !valid_insights(&reading.provider, reading.fetched_at, &reading.windows, &reading.insights)
        || reading_key(&reading.provider, reading.fetched_at, &reading.windows)? != key {
        return Err(format!("Incompatible quota journal preserved: {}", path.display()));
    }
    Ok(true)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuotaWindowV2 {
    id: String, label: String, used: f64, resets_at: Option<u64>, count: Option<i64>, derived: bool,
    group: Option<String>, credits: Option<CreditAmounts>, credit_scope: Option<CreditScope>, duration_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] reading_fetched_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] reading_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] quota_account_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] quota_bucket_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] reading_transport: Option<QuotaReadingTransport>,
    #[serde(default, skip_serializing_if = "Option::is_none")] reset_basis: Option<QuotaResetBasis>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuotaReadingV2 {
    schema_version: u32, provider: String, fetched_at: u64, writer_id: String, machine: String,
    windows: Vec<QuotaWindowV2>, insights: OwnedInsights,
}
impl From<&LimitWindow> for QuotaWindowV2 {
    fn from(w: &LimitWindow) -> Self {
        Self { id: w.id.clone(), label: w.label.clone(), used: w.used, resets_at: w.resets_at, count: w.count,
            derived: w.derived, group: w.group.clone(), credits: w.credits.clone(), credit_scope: w.credit_scope,
            duration_seconds: w.duration_seconds, reading_fetched_at: w.reading_fetched_at, reading_status: w.reading_status.clone(),
            quota_account_key: w.quota_account_key.clone(), quota_bucket_key: w.quota_bucket_key.clone(),
            reading_transport: w.reading_transport, reset_basis: w.reset_basis }
    }
}
impl QuotaWindowV2 {
    fn base(&self) -> QuotaWindow {
        QuotaWindow { id: self.id.clone(), label: self.label.clone(), used: self.used, resets_at: self.resets_at,
            count: self.count, derived: self.derived, group: self.group.clone(), credits: self.credits.clone(),
            credit_scope: self.credit_scope, duration_seconds: self.duration_seconds,
            reading_fetched_at: self.reading_fetched_at, reading_status: self.reading_status.clone() }
    }
}
fn reading_key_v2(provider: &str, at: u64, windows: &[QuotaWindowV2]) -> Result<String, String> {
    serde_json::to_vec(&(2u32, provider, at, windows)).map(|bytes| hash(&bytes)).map_err(|e| e.to_string())
}
pub(super) fn owned_v2_path(settings: &TelemetrySettings, provider: &str, snapshot: &UsageSnapshot) -> Option<PathBuf> {
    if !settings.enabled || !snapshot.windows.iter().any(|w| w.reading_transport.is_some() || w.reset_basis.is_some()
        || w.quota_account_key.is_some() || w.quota_bucket_key.is_some()) { return None; }
    let root = Path::new(settings.export_directory.as_ref()?);
    let windows: Vec<_> = snapshot.windows.iter().map(QuotaWindowV2::from).collect();
    let key = reading_key_v2(provider, snapshot.fetched_at, &windows).ok()?;
    let date = chrono::DateTime::<Utc>::from_timestamp_millis(i64::try_from(snapshot.fetched_at).ok()?)?;
    Some(root.join("machines").join(safe_label(&settings.machine)).join(safe_label(&settings.writer_id))
        .join("quota-readings-v2").join(date.format("%Y-%m-%d").to_string()).join(format!("{key}.json")))
}
fn valid_v2(r: &QuotaReadingV2) -> bool {
    let base: Vec<_> = r.windows.iter().map(QuotaWindowV2::base).collect();
    let key_valid = |key: &Option<String>| key.as_ref().is_none_or(|s|
        s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    r.schema_version == 2 && matches!(r.provider.as_str(), "claude" | "codex" | "kiro") && r.fetched_at > 0
        && valid_windows(&base) && valid_insights(&r.provider, r.fetched_at, &base, &r.insights)
        && r.windows.iter().all(|w| key_valid(&w.quota_account_key) && key_valid(&w.quota_bucket_key))
}
fn snapshot_v2(r: QuotaReadingV2) -> UsageSnapshot {
    UsageSnapshot { status: "ok".into(), fetched_at: r.fetched_at,
        windows: r.windows.into_iter().map(|w| LimitWindow {
            id: w.id, label: w.label, used: w.used, resets_at: w.resets_at, count: w.count,
            derived: w.derived, group: w.group, credits: w.credits, credit_scope: w.credit_scope,
            duration_seconds: w.duration_seconds, reading_fetched_at: w.reading_fetched_at,
            reading_status: w.reading_status, quota_account_key: w.quota_account_key,
            quota_bucket_key: w.quota_bucket_key, reading_transport: w.reading_transport, reset_basis: w.reset_basis,
        }).collect(), ..Default::default() }
}
/// Dispatch by explicit schema before typed parsing; old records remain identity-unknown.
pub(super) fn read_official(path: &Path) -> Result<Option<(String, UsageSnapshot)>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path).map_err(|_| "Quota journal unavailable")?.take((MAX_RECORD_BYTES + 1) as u64)
        .read_to_end(&mut bytes).map_err(|_| "Quota journal unavailable")?;
    if bytes.len() > MAX_RECORD_BYTES { return Err("Quota journal exceeds size limit".into()); }
    let raw: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "Invalid quota journal")?;
    let key = path.file_stem().and_then(|s| s.to_str()).ok_or("Invalid quota journal filename")?;
    match raw.get("schema_version").and_then(|v| v.as_u64()) {
        Some(1) => {
            let r: QuotaReading = serde_json::from_slice(&bytes).map_err(|_| "Invalid V1 quota journal")?;
            if r.fetched_at == 0 || !matches!(r.provider.as_str(), "claude" | "codex" | "kiro")
                || !valid_windows(&r.windows) || !valid_insights(&r.provider, r.fetched_at, &r.windows, &r.insights)
                || reading_key(&r.provider, r.fetched_at, &r.windows)? != key { return Err("Invalid V1 quota journal".into()); }
            Ok(None)
        }
        Some(2) => {
            let r: QuotaReadingV2 = serde_json::from_slice(&bytes).map_err(|_| "Invalid V2 quota journal")?;
            if !valid_v2(&r) { return Err("Invalid V2 quota journal fields".into()); }
            if reading_key_v2(&r.provider, r.fetched_at, &r.windows)? != key { return Err("Invalid V2 quota journal canonical hash".into()); }
            let date = chrono::DateTime::<Utc>::from_timestamp_millis(i64::try_from(r.fetched_at).map_err(|_| "Invalid V2 timestamp")?)
                .ok_or("Invalid V2 timestamp")?.format("%Y-%m-%d").to_string();
            if safe_label(&r.machine) != r.machine || safe_label(&r.writer_id) != r.writer_id
                || path.parent().and_then(Path::file_name).and_then(|s| s.to_str()) != Some(date.as_str())
                || path.parent().and_then(Path::parent).and_then(Path::file_name).and_then(|s| s.to_str()) != Some("quota-readings-v2")
                || path.parent().and_then(Path::parent).and_then(Path::parent).and_then(Path::file_name).and_then(|s| s.to_str()) != Some(r.writer_id.as_str())
                || path.parent().and_then(Path::parent).and_then(Path::parent).and_then(Path::parent).and_then(Path::file_name).and_then(|s| s.to_str()) != Some(r.machine.as_str()) {
                return Err("Invalid V2 quota journal partition".into());
            }
            let provider = r.provider.clone(); Ok(Some((provider, snapshot_v2(r))))
        }
        _ => Err("Unsupported quota journal schema preserved".into()),
    }
}
fn record_v2(settings: &TelemetrySettings, root: &Path, provider: &str, snapshot: &UsageSnapshot,
    insights: &ProviderInsights) -> Result<(), String> {
    let reading = QuotaReadingV2 { schema_version: 2, provider: provider.into(), fetched_at: snapshot.fetched_at,
        writer_id: safe_label(&settings.writer_id), machine: safe_label(&settings.machine),
        windows: snapshot.windows.iter().map(QuotaWindowV2::from).collect(), insights: OwnedInsights::from(insights) };
    if !valid_v2(&reading) { return Err("Invalid V2 quota reading".into()); }
    let key = reading_key_v2(provider, snapshot.fetched_at, &reading.windows)?;
    let date = chrono::DateTime::<Utc>::from_timestamp_millis(i64::try_from(snapshot.fetched_at)
        .map_err(|_| "Invalid quota reading timestamp")?).ok_or("Invalid quota reading timestamp")?;
    let path = root.join("machines").join(&reading.machine).join(&reading.writer_id)
        .join("quota-readings-v2").join(date.format("%Y-%m-%d").to_string()).join(format!("{key}.json"));
    let expected = serde_json::to_vec(&reading).map_err(|e| e.to_string())?;
    if expected.len() > MAX_RECORD_BYTES { return Err("Quota reading exceeds the journal size limit".into()); }
    let existing = || -> Result<bool, String> {
        match fs::File::open(&path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take((MAX_RECORD_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|_| "V2 quota journal unavailable")?;
                if bytes.len() > MAX_RECORD_BYTES { return Err("Oversized V2 quota journal preserved".into()); }
                let read = read_official(&path)?;
                if read.is_none() || bytes != expected { return Err("Incompatible V2 quota journal preserved".into()); }
                Ok(true)
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err("V2 quota journal unavailable; preserved".into()),
        }
    };
    if existing()? { return Ok(()); }
    match store::atomic_file(&path, &expected, false) {
        Ok(()) => Ok(()), Err(e) => if existing()? { Ok(()) } else { Err(e) },
    }
}

pub(super) fn record(
    settings: &TelemetrySettings, provider: &str, snapshot: &UsageSnapshot, insights: &ProviderInsights,
) -> Result<(), String> {
    // Never turn the suggested destination into a confirmed write destination.
    if !settings.enabled || settings.export_directory.is_none() { return Ok(()); }
    let root = Path::new(settings.export_directory.as_ref().unwrap());
    if !root.is_absolute() { return Err("Quota journal destination must be an absolute folder".into()); }
    if !matches!(provider, "claude" | "codex" | "kiro") || snapshot.status != "ok" || snapshot.fetched_at == 0 {
        return Err("Quota journal requires an official reading with its source timestamp".into());
    }
    if snapshot.windows.iter().any(|w| w.reading_transport.is_some() || w.reset_basis.is_some()
        || w.quota_account_key.is_some() || w.quota_bucket_key.is_some()) {
        return record_v2(settings, root, provider, snapshot, insights);
    }
    let windows: Vec<_> = snapshot.windows.iter().map(QuotaWindow::from).collect();
    let owned = OwnedInsights::from(insights);
    if !valid_windows(&windows) || !valid_insights(provider, snapshot.fetched_at, &windows, &owned) {
        return Err("Quota journal reading and indicators are invalid or do not match".into());
    }
    let reading = QuotaReading { schema_version: 1, provider: provider.into(), fetched_at: snapshot.fetched_at,
        writer_id: safe_label(&settings.writer_id), machine: safe_label(&settings.machine),
        windows, insights: owned };
    let key = reading_key(provider, snapshot.fetched_at, &reading.windows)?;
    let path = path_for(root, &reading, &key)?;
    if existing_valid(&path, &reading, &key)? { return Ok(()); }
    let bytes = serde_json::to_vec(&reading).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_RECORD_BYTES { return Err("Quota reading exceeds the journal size limit".into()); }
    match store::atomic_file(&path, &bytes, false) {
        Ok(()) => Ok(()),
        // Another writer may have committed the same immutable key while we checked it.
        Err(error) => if existing_valid(&path, &reading, &key)? { Ok(()) } else { Err(error) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::{TelemetryQuery, TelemetryService, unique_id};
    use crate::usage_insights::compute_provider_insights;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            Self(std::env::temp_dir().join("codenotch").join("usage-insights")
                .join(format!("quota-journal-tests-{}", unique_id())))
        }
        fn service(&self) -> TelemetryService {
            TelemetryService::at(self.0.join("config.json"), self.0.join("cache.sqlite")).unwrap()
        }
        fn configured(&self) -> TelemetryService {
            let mut service = self.service();
            service.configure(TelemetrySettings { export_directory: Some(self.0.join("dataset").to_string_lossy().into()),
                ..service.settings() }).unwrap();
            service
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let context = std::env::temp_dir().join("codenotch").join("usage-insights");
            assert_eq!(self.0.parent(), Some(context.as_path()));
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> (UsageSnapshot, ProviderInsights) {
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-12T11:38:00Z").unwrap().timestamp_millis() as u64;
        let snapshot = UsageSnapshot { status: "ok".into(), fetched_at: at, note: "RAW_NOTE_SENTINEL_DO_NOT_EXPORT".into(),
            windows: vec![LimitWindow { id: "primary".into(), used: 0.02, resets_at: Some(at + 604800000),
                duration_seconds: Some(604800), ..Default::default() }], ..Default::default() };
        let insights = compute_provider_insights("codex", &snapshot, None, at);
        (snapshot, insights)
    }
    #[test]
    fn v1_wire_key_is_frozen_and_v2_has_distinct_schema_hash_and_utc_partition() {
        let scratch=Scratch::new(); let mut service=scratch.configured();
        let (mut snapshot,base)=fixture();
        service.record_quota_reading("codex",&snapshot,&base).unwrap();
        let v1=records(&service).remove(0); let original=fs::read(&v1).unwrap();
        let windows:Vec<_>=snapshot.windows.iter().map(QuotaWindow::from).collect();
        let old_key=reading_key("codex",snapshot.fetched_at,&windows).unwrap();
        assert_eq!(old_key,"0b543685845f33f541ee5eefd5daf03d920dd23f240702d323f60af17dd79825");
        assert_eq!(v1.file_stem().unwrap().to_str().unwrap(),old_key);
        assert!(read_official(&v1).unwrap().is_none());
        snapshot.windows[0].quota_account_key=Some("a".repeat(64));
        snapshot.windows[0].quota_bucket_key=Some("b".repeat(64));
        snapshot.windows[0].reading_transport=Some(QuotaReadingTransport::CodexHttp);
        snapshot.windows[0].reset_basis=Some(QuotaResetBasis::Absolute);
        let base=compute_provider_insights("codex",&snapshot,None,snapshot.fetched_at);
        service.record_quota_reading("codex",&snapshot,&base).unwrap();
        let settings=service.settings(); let root=PathBuf::from(settings.export_directory.unwrap());
        let folder=root.join("machines").join(safe_label(&settings.machine)).join(safe_label(&settings.writer_id))
            .join("quota-readings-v2/2026-10-12");
        let v2=fs::read_dir(folder).unwrap().next().unwrap().unwrap().path();
        assert_ne!(v2.file_stem().unwrap().to_str().unwrap(),old_key);
        let bytes=fs::read(&v2).unwrap(); let parsed:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["schema_version"],2);
        assert!(parsed["insights"]["windows"][0].get("projection").is_none());
        assert!(parsed["insights"]["windows"][0].get("quota_role").is_none());
        assert_eq!(read_official(&v2).unwrap().unwrap().1.windows[0].quota_account_key,Some("a".repeat(64)));
        assert_eq!(fs::read(&v1).unwrap(),original);
        fs::write(&v2,b"{broken").unwrap();
        assert!(service.record_quota_reading("codex",&snapshot,&base).is_err());
        assert_eq!(fs::read(v2).unwrap(),b"{broken"); assert_eq!(fs::read(v1).unwrap(),original);
    }
    #[test]
    fn immutable_v2_decimal_roundtrip_preserves_float_bits_and_canonical_hash() {
        let scratch=Scratch::new(); let mut service=scratch.configured();
        let (mut snapshot,_)=fixture();
        snapshot.windows[0].reading_transport=Some(QuotaReadingTransport::CodexHttp);
        snapshot.windows[0].reset_basis=Some(QuotaResetBasis::Absolute);
        for used in [0.10099999999999999,0.103,0.10000000000000002,0.07009999999999998] {
            snapshot.fetched_at+=1; snapshot.windows[0].used=used;
            let base=compute_provider_insights("codex",&snapshot,None,snapshot.fetched_at);
            service.record_quota_reading("codex",&snapshot,&base).unwrap();
            let path=owned_v2_path(&service.settings(),"codex",&snapshot).unwrap();
            let bytes=fs::read(&path).unwrap();
            assert_eq!(read_official(&path).unwrap().unwrap().1.windows[0].used.to_bits(),used.to_bits());
            service.record_quota_reading("codex",&snapshot,&base).unwrap(); assert_eq!(fs::read(path).unwrap(),bytes);
        }
    }
    fn records(service: &TelemetryService) -> Vec<PathBuf> {
        let settings = service.settings();
        let dir = PathBuf::from(settings.export_directory.unwrap()).join("machines").join(safe_label(&settings.machine))
            .join(safe_label(&settings.writer_id)).join("quota-readings/2026-10");
        fs::read_dir(dir).map(|entries| entries.map(|e| e.unwrap().path()).collect()).unwrap_or_default()
    }

    #[test]
    fn disabled_or_unconfigured_does_not_create_quota_files_or_use_suggestion() {
        let scratch = Scratch::new();
        let mut service = scratch.service();
        let (snapshot, insights) = fixture();
        service.record_quota_reading("codex", &snapshot, &insights).unwrap();
        assert!(!scratch.0.exists());
        let mut service = scratch.configured();
        service.settings.enabled = false;
        service.record_quota_reading("codex", &snapshot, &insights).unwrap();
        assert!(records(&service).is_empty());
    }

    #[test]
    fn repeated_reading_and_restart_deduplicate_but_new_timestamp_creates_a_record() {
        let scratch = Scratch::new();
        let mut service = scratch.configured();
        let (mut snapshot, mut insights) = fixture();
        service.record_quota_reading("codex", &snapshot, &insights).unwrap();
        let first = records(&service).remove(0);
        let original = fs::read(&first).unwrap();
        snapshot.note = "Different note is not an official-reading key".into();
        service.record_quota_reading("codex", &snapshot, &insights).unwrap();
        assert_eq!(records(&service).len(), 1);
        assert_eq!(fs::read(&first).unwrap(), original);
        drop(service);
        let mut service = scratch.service();
        service.record_quota_reading("codex", &snapshot, &insights).unwrap();
        assert_eq!(records(&service).len(), 1);
        snapshot.fetched_at += 1;
        insights = compute_provider_insights("codex", &snapshot, None, snapshot.fetched_at);
        service.record_quota_reading("codex", &snapshot, &insights).unwrap();
        assert_eq!(records(&service).len(), 2);
        assert_eq!(fs::read(&first).unwrap(), original);
        let text = String::from_utf8(original).unwrap();
        assert!(!text.contains("RAW_NOTE_SENTINEL") && !text.contains("note"));
        assert!(text.contains("\"schema_version\":1"));
    }

    #[test]
    fn journal_errors_are_visible_preserve_existing_records_and_allow_retry() {
        let scratch = Scratch::new();
        let mut service = scratch.configured();
        let (snapshot, insights) = fixture();
        let settings = service.settings();
        let root = PathBuf::from(settings.export_directory.unwrap());
        let parent = root.join("machines").join(safe_label(&settings.machine)).join(safe_label(&settings.writer_id));
        fs::create_dir_all(&parent).unwrap();
        let obstruction = parent.join("quota-readings");
        fs::write(&obstruction, b"Existing file must survive failure").unwrap();
        assert!(service.record_quota_reading("codex", &snapshot, &insights).is_err());
        assert_eq!(fs::read(&obstruction).unwrap(), b"Existing file must survive failure");
        assert!(service.report(TelemetryQuery::default()).issues.iter().any(|i| i.starts_with("Quota journal (codex):")));
        // A real scan failure stops before any agent-source discovery. The observer has
        // no fresh quota to retry; its previous commit error must remain visible.
        let manifest = root.join("codenotch-telemetry-v1.json");
        let original_manifest = fs::read(&manifest).unwrap();
        fs::remove_file(&manifest).unwrap();
        assert!(service.scan().is_err());
        assert!(service.report(TelemetryQuery::default()).issues.iter().any(|i| i.starts_with("Quota journal (codex):")));
        fs::write(&manifest, original_manifest).unwrap();
        fs::remove_file(&obstruction).unwrap();
        service.record_quota_reading("codex", &snapshot, &insights).unwrap();
        assert_eq!(records(&service).len(), 1);
        assert!(!service.report(TelemetryQuery::default()).issues.iter().any(|i| i.starts_with("Quota journal (codex):")));
    }

    #[test]
    fn invalid_existing_key_is_preserved_and_not_accepted_as_deduplication() {
        let scratch = Scratch::new();
        let mut service = scratch.configured();
        let (snapshot, insights) = fixture();
        service.record_quota_reading("codex", &snapshot, &insights).unwrap();
        let path = records(&service).remove(0);
        fs::write(&path, b"{invalid existing record").unwrap();
        assert!(service.record_quota_reading("codex", &snapshot, &insights).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{invalid existing record");
        fs::write(&path, br#"{"schema_version":1}"#).unwrap();
        assert!(service.record_quota_reading("codex", &snapshot, &insights).is_err());
        assert_eq!(fs::read(&path).unwrap(), br#"{"schema_version":1}"#);
    }

    #[test]
    fn quota_journal_never_changes_legacy_batches_checkpoints_or_usage_totals() {
        let scratch = Scratch::new();
        let mut service = scratch.configured();
        let root = PathBuf::from(service.settings().export_directory.unwrap());
        let batch = store::Batch { schema_version: 1, writer_id: "legacy-writer".into(), machine: "legacy-machine".into(),
            created_at: Utc::now().to_rfc3339(), events: vec![crate::telemetry::tests::sample_event()], checkpoints: vec![] };
        let legacy = store::publish(&root, &batch).unwrap();
        let original = fs::read(&legacy).unwrap();
        let (snapshot, insights) = fixture();
        service.record_quota_reading("codex", &snapshot, &insights).unwrap();
        let mut issues = vec![];
        let dataset = store::load(&root, &scratch.0.join("rebuild.sqlite"), &mut issues).unwrap();
        assert_eq!(dataset.events.len(), 1);
        assert!(dataset.checkpoints.is_empty());
        assert_eq!(fs::read(&legacy).unwrap(), original);
        assert_eq!(service.report(TelemetryQuery::default()).totals.event_count, 0);
    }

    #[test]
    fn mixed_accounts_archive_distinct_origins_without_relabeling_old_windows_as_fresh() {
        let scratch = Scratch::new();
        let mut service = scratch.configured();
        let (mut snapshot, _) = fixture();
        let now = snapshot.fetched_at;
        let mut a = snapshot.windows.remove(0);
        a.id = "weekly_all".into(); a.group = Some("A".into());
        a.reading_fetched_at = Some(now); a.reading_status = Some("ok".into());
        let b = LimitWindow { id: "weekly_all@work".into(), group: Some("B".into()),
            reading_fetched_at: Some(now - 86400000), reading_status: Some("ok".into()), ..a.clone() };
        snapshot.windows = vec![a, b];
        let insights = compute_provider_insights("claude", &snapshot, None, now);
        assert!(insights.windows[0].fresh && insights.windows[0].period.is_some());
        assert!(!insights.windows[1].fresh && insights.windows[1].budget.is_none());
        service.record_quota_reading("claude", &snapshot, &insights).unwrap();
        let path = records(&service).remove(0);
        let raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["fetched_at"], now);
        assert_eq!(raw["windows"][0]["reading_fetched_at"], now);
        assert_eq!(raw["windows"][1]["reading_fetched_at"], now - 86400000);
        assert_eq!(raw["windows"][1]["reading_status"], "ok");
        assert_eq!(raw["insights"]["windows"][1]["fresh"], false);
        assert!(raw["insights"]["windows"][1]["work"].is_null());
        assert!(raw["insights"]["windows"][1]["budget"].is_null());
        service.record_quota_reading("claude", &snapshot, &insights).unwrap();
        assert_eq!(records(&service).len(), 1);
        let mut forged = insights;
        forged.windows[1].fresh = true;
        assert!(service.record_quota_reading("claude", &snapshot, &forged).is_err());
        assert_eq!(records(&service).len(), 1);
    }

    #[test]
    fn restored_machine_writer_labels_cannot_escape_selected_dataset() {
        let scratch = Scratch::new();
        let mut service = scratch.configured();
        service.settings.machine = "../../outside".into();
        service.settings.writer_id = r"C:\outside\writer".into();
        let (snapshot, insights) = fixture();
        service.record_quota_reading("codex", &snapshot, &insights).unwrap();
        let path = records(&service).remove(0);
        let root = PathBuf::from(service.settings().export_directory.unwrap());
        assert!(path.starts_with(&root));
        assert!(path.strip_prefix(&root).unwrap().components().all(|c| matches!(c, std::path::Component::Normal(_))));
    }
}
