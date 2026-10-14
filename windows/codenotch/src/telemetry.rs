//! Local, numeric telemetry. Agent sources are read-only; durable batches precede cursors.
mod collector;
mod parsers;
mod quota_journal;
pub(crate) mod quota_index;
pub(crate) mod store;

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::{Path, PathBuf}};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: Option<u64>, pub cached_input: Option<u64>, pub cache_creation: Option<u64>,
    pub output: Option<u64>, pub reasoning_output: Option<u64>, pub total: Option<u64>,
}
impl Tokens {
    pub(crate) fn merge_max(&mut self, other: &Self) {
        fn field(a: &mut Option<u64>, b: Option<u64>) { if let Some(b) = b { *a = Some(a.unwrap_or(0).max(b)); } }
        field(&mut self.input, other.input); field(&mut self.cached_input, other.cached_input);
        field(&mut self.cache_creation, other.cache_creation); field(&mut self.output, other.output);
        field(&mut self.reasoning_output, other.reasoning_output); field(&mut self.total, other.total);
    }
    fn add(&mut self, other: &Self) {
        fn field(a: &mut Option<u64>, b: Option<u64>) { if let Some(b) = b { *a = Some(a.unwrap_or(0).saturating_add(b)); } }
        field(&mut self.input, other.input); field(&mut self.cached_input, other.cached_input);
        field(&mut self.cache_creation, other.cache_creation); field(&mut self.output, other.output);
        field(&mut self.reasoning_output, other.reasoning_output); field(&mut self.total, other.total);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance { pub source_key: String, pub source_path: String, pub offset: u64 }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub schema_version: u32, pub event_id: String, pub provider: String, pub source: String,
    pub occurred_at: String, pub observed_at: String, pub machine: String, pub writer_id: String,
    pub account: Option<String>, pub session_id: String, pub request_or_execution_id: Option<String>,
    pub folder: Option<String>, pub folders: Vec<String>, pub project: Option<String>,
    pub project_source: Option<String>, pub model: Option<String>, pub requested_model: Option<String>,
    pub status: Option<String>, pub tokens: Option<Tokens>, pub credits: Option<f64>,
    pub quality: String, pub provenance: Provenance,
    #[serde(default)] pub observed_machines: Vec<String>,
    #[serde(default)] pub attribution_source: Option<String>,
    #[serde(default)] pub gaps: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMapping { pub prefix: String, pub project: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetrySettings {
    pub enabled: bool, pub export_directory: Option<String>, pub suggested_directory: Option<String>,
    pub machine: String, pub writer_id: String, pub utc_offset_minutes: i32,
    pub history_days: u32, pub project_mappings: Vec<ProjectMapping>,
}
impl Default for TelemetrySettings {
    fn default() -> Self {
        let machine = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "machine".into());
        Self { enabled: true, export_directory: None, suggested_directory: suggested_directory(),
            machine: safe_label(&machine), writer_id: String::new(), utc_offset_minutes: -180,
            history_days: 90, project_mappings: vec![] }
    }
}
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct TelemetryQuery {
    pub from_date: Option<String>, pub to_date: Option<String>, pub grain: String,
    pub provider: Option<String>, pub project: Option<String>, pub folder: Option<String>, pub machine: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct Totals { pub tokens: Option<Tokens>, pub credits: Option<f64>, pub event_count: u64 }
impl Totals {
    fn add(&mut self, event: &Event) {
        self.event_count += 1;
        if let Some(tokens) = &event.tokens { self.tokens.get_or_insert_with(Tokens::default).add(tokens); }
        if let Some(credits) = event.credits { self.credits = Some(self.credits.unwrap_or(0.0) + credits); }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct ReportRow {
    pub period: String, pub provider: String, pub project: Option<String>, pub folder: Option<String>,
    pub machine: String, pub tokens: Option<Tokens>, pub credits: Option<f64>, pub event_count: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coverage { pub provider: String, pub source: String, pub status: String, pub message: String }
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryReport {
    pub configured: bool, pub enabled: bool, pub export_directory: Option<String>, pub last_scan_at: Option<String>,
    pub coverage: Vec<Coverage>, pub totals: Totals, pub rows: Vec<ReportRow>, pub recent_events: Vec<Event>,
    pub issues: Vec<String>, pub truncated: bool,
}

pub struct TelemetryService {
    settings: TelemetrySettings, config_path: PathBuf, cache_path: PathBuf,
    dataset: store::Dataset, coverage: Vec<Coverage>, issues: Vec<String>, last_scan_at: Option<String>,
    scan_next: usize, invalid_config: bool,
    quota_index: quota_index::QuotaIndex,
}
impl TelemetryService {
    /// Reads only this app's settings. No discovery, dataset writes or agent reads before selection.
    pub fn new() -> Result<Self, String> {
        let base = dirs::config_dir().ok_or("Application config directory unavailable")?.join("codenotch");
        Self::at(base.join("telemetry.json"), base.join("telemetry-cache.sqlite"))
    }
    fn at(config_path: PathBuf, cache_path: PathBuf) -> Result<Self, String> {
        let mut initial_issues=vec![];let mut invalid_config=false;
        let mut settings: TelemetrySettings = if config_path.exists() {
            match std::fs::read(&config_path).ok().and_then(|bytes|serde_json::from_slice(&bytes).ok()){
                Some(settings)=>settings,None=>{invalid_config=true;initial_issues.push("Telemetry settings unavailable or invalid; original file is preserved until Save".into());TelemetrySettings::default()}
            }
        } else { TelemetrySettings::default() };
        settings.suggested_directory = suggested_directory();
        if settings.writer_id.is_empty() { settings.writer_id = unique_id(); }
        let quota_index = quota_index::QuotaIndex::new(cache_path.with_file_name("quota-rate-cache.sqlite"));
        let mut service=Self { settings, config_path, cache_path, dataset: store::Dataset::default(), coverage: vec![], issues: initial_issues, last_scan_at: None, scan_next: 0,invalid_config, quota_index };
        service.quota_index.set_root(if service.settings.enabled { service.settings.export_directory.as_deref() } else { None });
        if let Some(path)=service.settings.export_directory.as_ref(){
            match store::load(Path::new(path),&service.cache_path,&mut service.issues){Ok(dataset)=>service.dataset=dataset,Err(error)=>service.issues.push(error)}
        }
        Ok(service)
    }
    pub fn settings(&self) -> TelemetrySettings { self.settings.clone() }
    pub fn configure(&mut self, mut settings: TelemetrySettings) -> Result<TelemetrySettings, String> {
        settings.export_directory = settings.export_directory.as_ref().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        if !(-1439..=1439).contains(&settings.utc_offset_minutes) { return Err("UTC offset must be between -1439 and 1439 minutes".into()); }
        if !(1..=3650).contains(&settings.history_days) { return Err("History must be between 1 and 3650 days".into()); }
        if settings.machine.trim().is_empty() { return Err("Machine name is required".into()); }
        settings.machine = safe_label(settings.machine.trim());
        settings.writer_id = self.settings.writer_id.clone();
        settings.suggested_directory = suggested_directory();
        for mapping in &settings.project_mappings {
            if !parsers::absolute_folder(&mapping.prefix) || mapping.project.trim().is_empty() { return Err("Project mappings require an absolute folder and project name".into()); }
        }
        let mut dataset = store::Dataset::default();
        let mut issues = vec![];
        if let Some(dir) = &settings.export_directory {
            let path = Path::new(dir);
            if !path.is_absolute() { return Err("Telemetry destination must be an absolute folder".into()); }
            store::initialize(path)?;
            dataset = store::load(path, &self.cache_path, &mut issues)?;
        }
        // Save and reload before changing the confirmed in-memory state.
        if self.invalid_config && self.config_path.exists(){
            let preserved=std::fs::read(&self.config_path).map_err(|e|format!("Cannot preserve invalid telemetry settings: {e}"))?;
            let archive=self.config_path.with_file_name(format!("telemetry-invalid-{}.json",unique_id()));store::atomic_file(&archive,&preserved,false)?;
        }
        store::atomic_file(&self.config_path, &serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?, true)?;
        let persisted: TelemetrySettings = serde_json::from_slice(&std::fs::read(&self.config_path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        self.settings = persisted; self.dataset = dataset; self.issues = issues; self.coverage.clear(); self.last_scan_at = None; self.scan_next = 0;self.invalid_config=false;
        self.quota_index.set_root(if self.settings.enabled { self.settings.export_directory.as_deref() } else { None });
        Ok(self.settings())
    }
    pub fn scan(&mut self) -> Result<(), String> {
        if !self.settings.enabled || self.settings.export_directory.is_none() { return Ok(()); }
        let dir = PathBuf::from(self.settings.export_directory.as_ref().unwrap());
        // A failed quota commit remains visible even while stale sources are skipped.
        // Only a successful quota retry (or explicit configure) clears that provider's error.
        self.issues.retain(|issue| issue.starts_with("Quota journal ("));
        self.dataset = match store::load(&dir, &self.cache_path, &mut self.issues) {Ok(dataset)=>dataset,Err(error)=>{self.issues.push(error.clone());return Err(error);}};
        self.coverage = match collector::scan(&self.settings, &dir, &mut self.dataset, &mut self.scan_next, &mut self.issues){Ok(coverage)=>coverage,Err(error)=>{self.issues.push(error.clone());return Err(error);}};
        self.last_scan_at = Some(Utc::now().to_rfc3339());
        Ok(())
    }
    /// Official quota snapshots are a separate journal; they never add usage events.
    pub fn record_quota_reading(
        &mut self,
        provider: &str,
        snapshot: &crate::usage::UsageSnapshot,
        insights: &crate::usage_insights::ProviderInsights,
    ) -> Result<(), String> {
        self.ingest_quota_snapshot(provider, snapshot, insights);
        let prefix = format!("Quota journal ({provider}): ");
        self.issues.retain(|issue| !issue.starts_with(&prefix));
        if let Some(path) = quota_journal::owned_v2_path(&self.settings, provider, snapshot) { self.quota_index.note_owned_write(path); }
        let result = quota_journal::record(&self.settings, provider, snapshot, insights);
        if let Err(error) = &result { self.issues.push(format!("{prefix}{error}")); }
        result
    }
    pub(crate) fn ingest_quota_snapshot(&mut self, provider: &str, snapshot: &crate::usage::UsageSnapshot,
        base: &crate::usage_insights::ProviderInsights) {
        self.quota_index.ingest(provider, snapshot, base);
    }
    pub(crate) fn quota_rate_history(&self, requests: &[quota_index::QuotaHistoryRequest])
        -> std::collections::HashMap<usize, quota_index::QuotaHistoryResult> {
        self.quota_index.history(requests)
    }
    pub(crate) fn take_quota_index_refresh(&mut self) -> Option<quota_index::QuotaIndex> {
        if !self.settings.enabled { return None; }
        self.quota_index.take_refresh()
    }
    pub(crate) fn finish_quota_index_refresh(&mut self, completed: quota_index::QuotaIndex, issues: Vec<String>) {
        if self.quota_index.finish_refresh(completed) { self.issues.extend(issues); }
    }
    pub fn report(&self, query: TelemetryQuery) -> TelemetryReport {
        let mut report = TelemetryReport { configured: self.settings.export_directory.is_some(), enabled: self.settings.enabled,
            export_directory: self.settings.export_directory.clone(), last_scan_at: self.last_scan_at.clone(), coverage: self.coverage.clone(),
            totals: Totals::default(), rows: vec![], recent_events: vec![], issues: self.issues.clone(), truncated: false };
        if !report.configured { return report; }
        let persisted_gaps:u64=self.dataset.checkpoints.values().map(|rows|rows.iter().map(|c|c.malformed_lines+c.incomplete_records).max().unwrap_or(0)).sum();
        if persisted_gaps>0{report.issues.push(format!("Dataset has {persisted_gaps} persisted malformed/incomplete source records"));}
        if self.last_scan_at.is_none(){report.issues.push("Local source collection has not yet run; displayed records come from the selected dataset".into());}
        let offset = FixedOffset::east_opt(self.settings.utc_offset_minutes * 60).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
        let parse_date = |v: &Option<String>| -> Result<Option<NaiveDate>, String> { v.as_ref().filter(|x| !x.is_empty()).map(|x| NaiveDate::parse_from_str(x, "%Y-%m-%d").map_err(|_| "Dates must use YYYY-MM-DD".to_string())).transpose() };
        let (mut from, to) = match (parse_date(&query.from_date), parse_date(&query.to_date)) {
            (Ok(f), Ok(t)) if !matches!((f,t), (Some(f),Some(t)) if f>t) => (f,t),
            _ => { report.issues.push("Invalid date interval".into()); return report; }
        };
        if from.is_none() && to.is_none(){from=Some((Utc::now().with_timezone(&offset).date_naive())-Duration::days(self.settings.history_days.saturating_sub(1) as i64));}
        let grain = if query.grain.is_empty() { "day" } else { &query.grain };
        if !["event","hour","day","week","month"].contains(&grain) { report.issues.push("Unknown report grain".into()); return report; }
        let mut groups: BTreeMap<(String,String,Option<String>,Option<String>,String), Totals> = BTreeMap::new();
        let mut events: Vec<&Event> = self.dataset.events.values().collect();
        events.sort_by(|a,b| b.occurred_at.cmp(&a.occurred_at).then_with(|| a.event_id.cmp(&b.event_id)));
        let mut has_partial=false;
        for original in events {
            let mut derived=original.clone();
            if let Some(folder)=&derived.folder {let (project,source)=parsers::project(folder,&self.settings);
                if project.is_some() || !original.project_source.as_deref().is_some_and(|s|s.starts_with("git:")){derived.project=project;derived.project_source=source;}
            }else{derived.project=None;derived.project_source=None;}
            let event=&derived;
            if query.provider.as_ref().is_some_and(|v| v != &event.provider) || query.machine.as_ref().is_some_and(|v| v != &event.machine && !event.observed_machines.contains(v))
                || query.project.as_ref().is_some_and(|v| v != event.project.as_deref().unwrap_or(""))
                || query.folder.as_ref().is_some_and(|v| v != event.folder.as_deref().unwrap_or("")) { continue; }
            let Ok(timestamp) = DateTime::parse_from_rfc3339(&event.occurred_at) else { report.issues.push("An event has an invalid source timestamp".into()); continue; };
            let local = timestamp.with_timezone(&offset); let date = local.date_naive();
            if from.is_some_and(|f| date<f) || to.is_some_and(|t| date>t) { continue; }
            report.totals.add(event);
            has_partial|=event.quality!="reported" || !event.gaps.is_empty() || event.project.is_none() || event.folder.is_none();
            if report.recent_events.len()<200 { report.recent_events.push(event.clone()); } else { report.truncated=true; }
            let period = match grain {
                "event" => format!("{}#{}", local.to_rfc3339(), event.event_id),
                "hour" => local.format("%Y-%m-%dT%H:00%:z").to_string(),
                "week" => (date - Duration::days(date.weekday().num_days_from_monday() as i64)).to_string(),
                "month" => local.format("%Y-%m").to_string(), _ => date.to_string(),
            };
            groups.entry((period,event.provider.clone(),event.project.clone(),event.folder.clone(),event.machine.clone())).or_default().add(event);
        }
        let mut rows: Vec<_> = groups.into_iter().map(|((period,provider,project,folder,machine),t)| ReportRow { period,provider,project,folder,machine,tokens:t.tokens,credits:t.credits,event_count:t.event_count }).collect();
        rows.sort_by(|a,b| b.period.cmp(&a.period).then_with(|| a.provider.cmp(&b.provider)).then_with(|| a.project.cmp(&b.project)).then_with(|| a.folder.cmp(&b.folder)).then_with(|| a.machine.cmp(&b.machine)));
        if rows.len()>200 { rows.truncate(200); report.truncated=true; }
        report.rows=rows;
        if has_partial { report.issues.push("Some selected records have incomplete counts, source gaps or unattributed project/folder metadata".into()); }
        report
    }
}

pub(crate) fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
pub(crate) fn unique_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    hash(format!("{}:{}:{}", Utc::now().timestamp_nanos_opt().unwrap_or(0), std::process::id(), COUNTER.fetch_add(1,std::sync::atomic::Ordering::Relaxed)).as_bytes())
}
pub(crate) fn safe_label(raw: &str) -> String {
    let value: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c=='-' || c=='_' {c} else {'_'}).take(80).collect();
    if value.is_empty() {"machine".into()} else {value}
}
fn suggested_directory() -> Option<String> {
    let append=|value:PathBuf|Some(value.join("Projetos").join("Codenotch").join("telemetria").to_string_lossy().into_owned());
    if let Some(value)=std::env::var_os("OneDriveCommercial").filter(|v|!v.is_empty()){return append(PathBuf::from(value));}
    let mut registry_business:Option<PathBuf>=None;
    let mut registry_personal:Option<PathBuf>=None;
    // Business installations may expose only their native OneDrive account registry entry.
    #[cfg(windows)]
    {
        let candidates=onedrive_account_folders();
        registry_business=candidates.iter().find(|(personal,_)|!*personal).map(|(_,value)|PathBuf::from(value));
        registry_personal=candidates.iter().find(|(personal,_)|*personal).map(|(_,value)|PathBuf::from(value));
    }
    if let Some(value)=registry_business{return append(value);}
    for name in ["OneDriveConsumer","OneDrive"]{if let Some(value)=std::env::var_os(name).filter(|v|!v.is_empty()){return append(PathBuf::from(value));}}
    if let Some(value)=registry_personal{return append(value);}
    None
}
fn decode_registry_folder(words:&[u16])->Option<String>{
    let end=words.iter().position(|w|*w==0).unwrap_or(words.len());String::from_utf16(&words[..end]).ok().filter(|s|!s.is_empty() && parsers::absolute_folder(s))
}
#[cfg(windows)]
fn onedrive_account_folders()->Vec<(bool,String)>{
    // Native UTF-16 reads avoid shell code pages and inspect only the non-secret UserFolder.
    #[link(name="advapi32")]extern "system"{
        fn RegOpenKeyExW(key:isize,subkey:*const u16,options:u32,access:u32,result:*mut isize)->i32;
        fn RegEnumKeyExW(key:isize,index:u32,name:*mut u16,name_len:*mut u32,reserved:*mut u32,class:*mut u16,class_len:*mut u32,last_write:*mut std::ffi::c_void)->i32;
        fn RegGetValueW(key:isize,subkey:*const u16,value:*const u16,flags:u32,value_type:*mut u32,data:*mut std::ffi::c_void,size:*mut u32)->i32;
        fn RegCloseKey(key:isize)->i32;
    }
    let path:Vec<u16>=r"Software\Microsoft\OneDrive\Accounts".encode_utf16().chain(Some(0)).collect();let name:Vec<u16>="UserFolder".encode_utf16().chain(Some(0)).collect();let mut key=0isize;
    if unsafe{RegOpenKeyExW(-2147483647isize,path.as_ptr(),0,0x20019,&mut key)}!=0{return vec![];}
    let mut candidates=vec![];
    for index in 0..1024u32{
        let mut account=[0u16;256];let mut length=account.len() as u32;let status=unsafe{RegEnumKeyExW(key,index,account.as_mut_ptr(),&mut length,std::ptr::null_mut(),std::ptr::null_mut(),std::ptr::null_mut(),std::ptr::null_mut())};
        if status==259{break;}if status!=0{continue;}let Ok(label)=String::from_utf16(&account[..length as usize])else{continue;};if !label.starts_with("Business") && label!="Personal"{continue;}
        let subkey:Vec<u16>=label.encode_utf16().chain(Some(0)).collect();let mut size=0u32;
        if unsafe{RegGetValueW(key,subkey.as_ptr(),name.as_ptr(),2,std::ptr::null_mut(),std::ptr::null_mut(),&mut size)}!=0 || size==0 || size>65536 || size%2!=0{continue;}
        let mut words=vec![0u16;(size/2) as usize];if unsafe{RegGetValueW(key,subkey.as_ptr(),name.as_ptr(),2,std::ptr::null_mut(),words.as_mut_ptr().cast(),&mut size)}!=0{continue;}
        if let Some(folder)=decode_registry_folder(&words[..(size/2) as usize]){candidates.push((label=="Personal",folder));}
    }
    unsafe{RegCloseKey(key)};candidates.sort();candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(id:&str,time:&str,machine:&str,provider:&str)->Event {
        Event { schema_version:1,event_id:id.into(),provider:provider.into(),source:"fixture".into(),occurred_at:time.into(),observed_at:time.into(),machine:machine.into(),writer_id:"writer".into(),account:None,session_id:"session".into(),request_or_execution_id:Some(id.into()),folder:None,folders:vec![],project:None,project_source:None,model:None,requested_model:None,status:None,tokens:if provider=="codex"{Some(Tokens{input:Some(10),cached_input:Some(4),output:Some(2),total:Some(12),..Default::default()})}else{None},credits:if provider=="kiro"{Some(1.5)}else{None},quality:"reported".into(),provenance:Provenance{source_key:"fixture".into(),source_path:"fixture".into(),offset:0},observed_machines:vec![machine.into()],attribution_source:None,gaps:vec![] }
    }
    #[test] fn setup_is_required_and_report_units_dates_are_independent() {
        let base=std::env::temp_dir().join("codenotch").join(format!("telemetry-report-{}",unique_id()));
        let mut service=TelemetryService::at(base.join("config.json"),base.join("cache.sqlite")).unwrap();
        service.scan().unwrap();assert!(!base.exists());assert!(!service.report(TelemetryQuery::default()).configured);
        let settings=TelemetrySettings{export_directory:Some(base.join("dataset").to_string_lossy().into()),..service.settings()};
        service.configure(settings).unwrap();
        for event in [event("a","2026-10-12T02:30:00Z","machineA","codex"),event("b","2026-10-12T03:30:00Z","machineB","kiro")] {service.dataset.events.insert(event.event_id.clone(),event);}
        let query=TelemetryQuery{from_date:Some("2026-10-11".into()),to_date:Some("2026-10-11".into()),grain:"hour".into(),..Default::default()};
        let report=service.report(query);assert_eq!(report.totals.event_count,1);assert_eq!(report.rows[0].period,"2026-10-11T23:00-03:00");assert_eq!(report.totals.tokens.unwrap().total,Some(12));assert!(report.totals.credits.is_none());
        let all=service.report(TelemetryQuery{grain:"week".into(),..Default::default()});assert_eq!(all.rows[0].period,"2026-10-12");assert_eq!(all.rows[1].period,"2026-10-05");assert_eq!(all.totals.credits,Some(1.5));assert_eq!(all.totals.tokens.unwrap().total,Some(12));
        let filtered=service.report(TelemetryQuery{machine:Some("machineB".into()),grain:"month".into(),project:Some(String::new()),..Default::default()});assert_eq!(filtered.totals.event_count,1);assert_eq!(filtered.rows[0].period,"2026-10");assert!(filtered.totals.tokens.is_none());
        let _=std::fs::remove_dir_all(base);
    }
    pub(super)fn sample_event()->Event{event("shared","2026-10-10T12:00:00Z","machineA","codex")}
    #[test]fn mapping_changes_derive_history_and_never_rewrite_batch(){
        let base=std::env::temp_dir().join("codenotch").join(format!("telemetry-mapping-{}",unique_id()));let data=base.join("dataset");let folder=base.join("workspace").to_string_lossy().into_owned();let mut service=TelemetryService::at(base.join("config.json"),base.join("cache.sqlite")).unwrap();service.configure(TelemetrySettings{export_directory:Some(data.to_string_lossy().into()),project_mappings:vec![ProjectMapping{prefix:folder.clone(),project:"A".into()}],..service.settings()}).unwrap();
        let mut e=event("mapped",&Utc::now().to_rfc3339(),"machineA","codex");e.folder=Some(folder.clone());e.folders=vec![folder.clone()];e.project=Some("A".into());e.project_source=Some(format!("mapping:{folder}"));let batch=store::Batch{schema_version:1,writer_id:"writer".into(),machine:"machineA".into(),created_at:Utc::now().to_rfc3339(),events:vec![e],checkpoints:vec![]};let path=store::publish(&data,&batch).unwrap();let original=std::fs::read(&path).unwrap();
        service.configure(TelemetrySettings{project_mappings:vec![ProjectMapping{prefix:folder,project:"B".into()}],..service.settings()}).unwrap();let report=service.report(TelemetryQuery{project:Some("B".into()),..Default::default()});assert_eq!(report.totals.event_count,1);assert_eq!(report.rows[0].project.as_deref(),Some("B"));assert_eq!(service.dataset.events["mapped"].project.as_deref(),Some("A"));
        service.configure(TelemetrySettings{project_mappings:vec![],..service.settings()}).unwrap();assert!(service.report(Default::default()).rows[0].project.is_none());assert_eq!(std::fs::read(&path).unwrap(),original);let _=std::fs::remove_dir_all(base);
    }
    #[test]fn partial_selected_record_beyond_details_limit_is_still_reported(){
        let base=std::env::temp_dir().join("codenotch").join(format!("telemetry-coverage-{}",unique_id()));let mut service=TelemetryService::at(base.join("config.json"),base.join("cache.sqlite")).unwrap();let folder=base.join("workspace").to_string_lossy().into_owned();service.settings.export_directory=Some(base.to_string_lossy().into());service.settings.project_mappings=vec![ProjectMapping{prefix:folder.clone(),project:"Known".into()}];
        for index in 0..201{let time=(Utc::now()-Duration::minutes(index)).to_rfc3339();let mut e=event(&format!("e{index}"),&time,"machineA","codex");e.folder=Some(folder.clone());if index==200{e.quality="partial".into();e.gaps.push("fixture_gap".into());}service.dataset.events.insert(e.event_id.clone(),e);}
        let report=service.report(Default::default());assert_eq!(report.recent_events.len(),200);assert!(report.recent_events.iter().all(|e|e.quality=="reported"));assert!(report.issues.iter().any(|i|i.contains("Some selected records")));
        let mut cp=collector::Checkpoint::fixture();cp.incomplete_records=7;service.dataset.checkpoints.insert(cp.source_key.clone(),vec![cp]);assert!(service.report(Default::default()).issues.iter().any(|i|i.contains("7 persisted")));
    }
    #[test]fn invalid_own_config_survives_until_explicit_save(){
        let base=std::env::temp_dir().join("codenotch").join(format!("telemetry-config-{}",unique_id()));std::fs::create_dir_all(&base).unwrap();let cfg=base.join("config.json");std::fs::write(&cfg,b"{invalid-original").unwrap();let mut service=TelemetryService::at(cfg.clone(),base.join("cache.sqlite")).unwrap();assert!(service.settings.export_directory.is_none());assert!(!service.issues.is_empty());assert_eq!(std::fs::read(&cfg).unwrap(),b"{invalid-original");
        service.configure(service.settings()).unwrap();let archives:Vec<_>=std::fs::read_dir(&base).unwrap().filter_map(Result::ok).filter(|e|e.file_name().to_string_lossy().starts_with("telemetry-invalid-")).collect();assert_eq!(archives.len(),1);assert_eq!(std::fs::read(archives[0].path()).unwrap(),b"{invalid-original");let _=std::fs::remove_dir_all(base);
    }
    #[test]fn registry_folder_utf16_preserves_portuguese_names(){let raw=r"C:\Users\André\OneDrive - COMIT";let words:Vec<u16>=raw.encode_utf16().chain(Some(0)).collect();let parsed=decode_registry_folder(&words).unwrap();let json=serde_json::to_string(&parsed).unwrap();assert_eq!(serde_json::from_str::<String>(&json).unwrap(),raw);assert!(decode_registry_folder(&[0xd800,0]).is_none());}
}
