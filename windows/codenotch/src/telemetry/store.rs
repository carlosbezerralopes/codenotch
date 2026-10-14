use super::{collector::Checkpoint, Event, hash, unique_id};
use chrono::Utc;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::{self, OpenOptions}, io::Write, path::{Path,PathBuf}};

#[derive(Default)]
pub(super) struct Dataset {
    pub events: BTreeMap<String,Event>,
    pub checkpoints: BTreeMap<String,Vec<Checkpoint>>,
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub(super) struct Batch {
    pub schema_version: u32, pub writer_id: String, pub machine: String, pub created_at: String,
    pub events: Vec<Event>, pub checkpoints: Vec<Checkpoint>,
}

/// The cache contains normalized data only and can be deleted without losing a cursor or event.
pub(super) fn load(root: &Path, cache: &Path, issues: &mut Vec<String>) -> Result<Dataset,String> {
    verify_manifest(root)?;
    let paths=batch_paths(root)?;
    let mut attempt_issues=vec![];
    match load_cached(root,cache,&paths,&mut attempt_issues) {
        Ok(dataset)=>{issues.extend(attempt_issues);Ok(dataset)},
        Err(LoadFailure::Durable(error))=>{issues.extend(attempt_issues);Err(error)},
        Err(LoadFailure::Cache(_error))=>{
            issues.push("Local telemetry cache unavailable; durable batches are replayed in memory; cache file was preserved".into());
            // This path has no SQLite operations, so even a late transaction/commit failure
            // cannot make the disposable cache a prerequisite for recovering the journal.
            let mut dataset=Dataset::default();
            for path in &paths {
                match read_batch(path,None,issues) {
                    Ok(Some((batch,_,_)))=>absorb(&mut dataset,&batch),Ok(None)=>{},
                    Err(LoadFailure::Durable(error))=>return Err(error),
                    Err(LoadFailure::Cache(_))=>unreachable!("No cache is consulted during durable replay"),
                }
            }
            sort_checkpoints(&mut dataset);Ok(dataset)
        }
    }
}
#[derive(Debug)]enum LoadFailure { Cache(String), Durable(String) }
fn cache_operation<T>(result:rusqlite::Result<T>)->Result<T,LoadFailure>{result.map_err(|error|LoadFailure::Cache(error.to_string()))}
fn load_cached(root:&Path,cache:&Path,paths:&[PathBuf],issues:&mut Vec<String>)->Result<Dataset,LoadFailure>{
    if let Some(parent)=cache.parent(){fs::create_dir_all(parent).map_err(|error|LoadFailure::Cache(error.to_string()))?;}
    let mut connection=cache_operation(Connection::open(cache))?;
    cache_operation(connection.busy_timeout(std::time::Duration::from_secs(3)))?;
    // Schema, scope and row updates roll back together when any subsequent cache operation fails.
    let transaction=cache_operation(connection.transaction())?;
    schema(&transaction).map_err(LoadFailure::Cache)?;
    let scope=root.to_string_lossy().to_string();
    let old_scope=match transaction.query_row("SELECT value FROM metadata WHERE key='scope'",[],|r|r.get::<_,String>(0)){
        Ok(value)=>Some(value),Err(rusqlite::Error::QueryReturnedNoRows)=>None,Err(error)=>return Err(LoadFailure::Cache(error.to_string())),
    };
    if old_scope.as_deref()!=Some(&scope) {
        cache_operation(transaction.execute("DELETE FROM batches",[]))?;
        cache_operation(transaction.execute("INSERT OR REPLACE INTO metadata VALUES ('scope',?1)",[&scope]))?;
    }
    let cached: BTreeMap<String,(String,String,String)> = {
        let mut stmt=cache_operation(transaction.prepare("SELECT path,signature,hash,json FROM batches"))?;
        let rows=cache_operation(stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,(r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))))?;
        cache_operation(rows.collect::<Result<_,_>>())?
    };
    let mut dataset=Dataset::default();
    let mut seen=std::collections::BTreeSet::new();
    for path in paths {
        let key=path.to_string_lossy().to_string(); seen.insert(key.clone());
        let Some((batch,_bytes,signature))=read_batch(path,cached.get(&key),issues)? else{
            cache_operation(transaction.execute("DELETE FROM batches WHERE path=?1",[&key]))?;continue;
        };
        let normalized=serde_json::to_string(&batch).map_err(|error|LoadFailure::Durable(error.to_string()))?;
        // The checksum covers exactly the normalized JSON stored in this row, not the
        // source file's optional whitespace or older schema's missing optional fields.
        cache_operation(transaction.execute("INSERT OR REPLACE INTO batches(path,signature,hash,json) VALUES (?1,?2,?3,?4)",params![key,signature,hash(normalized.as_bytes()),normalized]))?;
        absorb(&mut dataset,&batch);
    }
    for key in cached.keys().filter(|key|!seen.contains(*key)) { cache_operation(transaction.execute("DELETE FROM batches WHERE path=?1",[key]))?; }
    cache_operation(transaction.commit())?;
    sort_checkpoints(&mut dataset);
    Ok(dataset)
}
fn read_batch(path:&Path,cached:Option<&(String,String,String)>,issues:&mut Vec<String>)->Result<Option<(Batch,Vec<u8>,String)>,LoadFailure>{
    let meta=fs::metadata(path).map_err(|error|LoadFailure::Durable(format!("Telemetry batch metadata {}: {error}",path.display())))?;
    if meta.len()>64*1024*1024{issues.push(format!("Oversized telemetry batch skipped: {}",path.display()));return Ok(None);}
    let signature=format!("{}:{:?}",meta.len(),meta.modified().ok());
    let mut matched=cached.filter(|(old,_,_)|old==&signature);
    if let Some((_,digest,json))=matched{if hash(json.as_bytes())!=*digest{
        issues.push(format!("Local telemetry cache unavailable for batch {}; checksum mismatch; durable source is reloaded",path.display()));matched=None;
    }}
    let bytes=if let Some((_,_,json))=matched{json.as_bytes().to_vec()}else{match fs::read(path){Ok(bytes)=>bytes,Err(error)=>{issues.push(format!("Unreadable telemetry batch {}: {error}",path.display()));return Ok(None);}}};
    let batch:Batch=match serde_json::from_slice(&bytes){Ok(batch)=>batch,Err(_)=>{
        if matched.is_some(){return Err(LoadFailure::Cache("Invalid cached batch JSON".into()));}
        issues.push(format!("Invalid telemetry batch skipped: {}",path.display()));return Ok(None);
    }};
    if batch.schema_version!=1 || batch.events.iter().any(|event|!valid_event(event)) || batch.checkpoints.iter().any(|checkpoint|!checkpoint.valid_shape()){
        if matched.is_some(){return Err(LoadFailure::Cache("Invalid cached batch schema".into()));}
        issues.push(format!("Unsupported or invalid telemetry batch skipped: {}",path.display()));return Ok(None);
    }
    Ok(Some((batch,bytes,signature)))
}
fn sort_checkpoints(dataset:&mut Dataset){for checkpoints in dataset.checkpoints.values_mut(){checkpoints.sort_by(|a,b|b.offset.cmp(&a.offset).then_with(||b.observed_at.cmp(&a.observed_at)));checkpoints.dedup_by(|a,b|a.offset==b.offset && a.anchor==b.anchor && a.state==b.state);}}
fn valid_event(event:&Event)->bool {
    event.schema_version==1 && !event.event_id.is_empty() && ["claude","codex","kiro"].contains(&event.provider.as_str())
        && chrono::DateTime::parse_from_rfc3339(&event.occurred_at).is_ok()
        && event.credits.is_none_or(|v|v.is_finite() && v>=0.0)
}
pub(super) fn absorb(dataset:&mut Dataset,batch:&Batch) {
    for event in &batch.events {
        match dataset.events.get_mut(&event.event_id) {
            Some(existing)=>merge(existing,event),None=>{let mut event=event.clone();if !event.observed_machines.contains(&event.machine){event.observed_machines.push(event.machine.clone());}dataset.events.insert(event.event_id.clone(),event);}
        }
    }
    for checkpoint in &batch.checkpoints { dataset.checkpoints.entry(checkpoint.source_key.clone()).or_default().push(checkpoint.clone()); }
}
fn merge(existing:&mut Event,new:&Event) {
    // A streamed Claude snapshot updates a request, never adds another request's spend.
    if let Some(tokens)=&new.tokens { existing.tokens.get_or_insert_with(Default::default).merge_max(tokens); }
    if let Some(tokens)=existing.tokens.as_mut(){if existing.provider=="claude"{tokens.total=tokens.input.zip(tokens.output).map(|(i,o)|i.saturating_add(o));}super::parsers::validate_tokens(tokens,&mut existing.gaps);}
    if let Some(credits)=new.credits { existing.credits=Some(existing.credits.unwrap_or(0.0).max(credits)); }
    if existing.folder.is_none() { existing.folder=new.folder.clone();existing.folders=new.folders.clone(); }
    if existing.project.is_none() { existing.project=new.project.clone();existing.project_source=new.project_source.clone(); }
    if existing.model.is_none() {existing.model=new.model.clone();}
    if new.quality=="reported" {existing.quality=new.quality.clone();}
    if existing.provider=="claude" && new.occurred_at>existing.occurred_at {existing.occurred_at=new.occurred_at.clone();}
    existing.observed_machines.push(existing.machine.clone());existing.observed_machines.push(new.machine.clone());existing.observed_machines.extend(new.observed_machines.iter().cloned());existing.observed_machines.sort();existing.observed_machines.dedup();
    existing.gaps.extend(new.gaps.iter().cloned());existing.gaps.sort();existing.gaps.dedup();
    if (&new.observed_at,&new.machine,&new.writer_id)<(&existing.observed_at,&existing.machine,&existing.writer_id){existing.machine=new.machine.clone();existing.writer_id=new.writer_id.clone();existing.observed_at=new.observed_at.clone();existing.provenance=new.provenance.clone();}
    if existing.attribution_source.is_none(){existing.attribution_source=new.attribution_source.clone();}
}
fn schema(connection:&Connection)->Result<(),String>{
    connection.execute("CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY,value TEXT NOT NULL)",[]).map_err(|error|error.to_string())?;
    connection.execute("CREATE TABLE IF NOT EXISTS batches (path TEXT PRIMARY KEY,signature TEXT NOT NULL,hash TEXT NOT NULL,json TEXT NOT NULL)",[]).map_err(|error|error.to_string())?;Ok(())
}

pub(super) fn initialize(root:&Path)->Result<(),String> {
    fs::create_dir_all(root).map_err(|e|format!("Telemetry destination: {e}"))?;
    let manifest=root.join("codenotch-telemetry-v1.json");
    if !manifest.exists() { if let Err(error)=atomic_file(&manifest,br#"{"schema_version":1,"format":"codenotch-numeric-telemetry"}"#,false) {if verify_manifest(root).is_err(){return Err(error);}} }
    verify_manifest(root)
}
fn verify_manifest(root:&Path)->Result<(),String> {
    let bytes=fs::read(root.join("codenotch-telemetry-v1.json")).map_err(|e|format!("Telemetry dataset unavailable: {e}"))?;
    let manifest:serde_json::Value=serde_json::from_slice(&bytes).map_err(|_|"Invalid telemetry dataset manifest".to_string())?;
    if manifest["schema_version"]!=1 || manifest["format"]!="codenotch-numeric-telemetry" {return Err("Unsupported telemetry dataset".into());} Ok(())
}
pub(super) fn publish(root:&Path,batch:&Batch)->Result<PathBuf,String> {
    let path=root.join("machines").join(super::safe_label(&batch.machine)).join(super::safe_label(&batch.writer_id)).join("batches").join(Utc::now().format("%Y-%m").to_string()).join(format!("{}.json",unique_id()));
    atomic_file(&path,&serde_json::to_vec(batch).map_err(|e|e.to_string())?,false)?;
    Ok(path)
}
pub(crate) fn atomic_file(path:&Path,bytes:&[u8],replace:bool)->Result<(),String> {
    let parent=path.parent().ok_or("Destination has no parent")?;
    fs::create_dir_all(parent).map_err(|e|e.to_string())?;
    let temp=parent.join(format!(".{}.tmp",unique_id()));
    let result=(||{
        let mut file=OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e|e.to_string())?;
        file.write_all(bytes).and_then(|_|file.sync_all()).map_err(|e|e.to_string())?;drop(file);
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            #[link(name="kernel32")] extern "system" {fn MoveFileExW(from:*const u16,to:*const u16,flags:u32)->i32;}
            // Rust file operations accept long paths, but this raw Win32 call needs the
            // extended-length syntax. The new destination does not exist yet, so resolve
            // its existing parent, then append the unchanged final file name.
            let from_path=fs::canonicalize(&temp).map_err(|error|error.to_string())?;
            let to_path=fs::canonicalize(parent).map_err(|error|error.to_string())?.join(path.file_name().ok_or("Destination has no file name")?);
            let from:Vec<u16>=from_path.as_os_str().encode_wide().chain(Some(0)).collect();let to:Vec<u16>=to_path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe{MoveFileExW(from.as_ptr(),to.as_ptr(),0x8|if replace{0x1}else{0})}==0 {return Err(std::io::Error::last_os_error().to_string());}
            return Ok(());
        }
        #[cfg(not(windows))]{if !replace && path.exists() {return Err("Immutable telemetry file already exists".into());}fs::rename(&temp,path).map_err(|e|e.to_string())?;#[cfg(unix)]{std::fs::File::open(parent).and_then(|dir|dir.sync_all()).map_err(|e|e.to_string())?;}Ok(())}
    })();
    if result.is_err() {let _=fs::remove_file(&temp);}result
}
fn directories(root:&Path)->Result<Vec<PathBuf>,String> {
    let entries=match fs::read_dir(root){Ok(entries)=>entries,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(vec![]),Err(error)=>return Err(format!("Telemetry dataset directory {}: {error}",root.display()))};
    let mut paths=vec![];
    for entry in entries {let entry=entry.map_err(|e|e.to_string())?;if entry.file_type().map_err(|e|e.to_string())?.is_dir(){paths.push(entry.path());}}
    paths.sort();Ok(paths)
}
fn batch_paths(root:&Path)->Result<Vec<PathBuf>,String> {
    let mut paths=vec![];
    for machine in directories(&root.join("machines"))? {for writer in directories(&machine)? {for month in directories(&writer.join("batches"))? {
        for entry in fs::read_dir(month).map_err(|e|e.to_string())? {let entry=entry.map_err(|e|e.to_string())?;if entry.file_type().map_err(|e|e.to_string())?.is_file() && entry.path().extension().is_some_and(|v|v=="json"){paths.push(entry.path());}}
    }}}
    paths.sort();Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]fn immutable_batch_restores_checkpoint_with_no_cache(){
        let root=std::env::temp_dir().join("codenotch").join(format!("telemetry-test-{}",unique_id()));
        initialize(&root).unwrap();let checkpoint=Checkpoint::fixture();
        let batch=Batch{schema_version:1,writer_id:"writer".into(),machine:"machine".into(),created_at:Utc::now().to_rfc3339(),events:vec![],checkpoints:vec![checkpoint.clone()]};
        publish(&root,&batch).unwrap();let mut issues=vec![];let restored=load(&root,&root.join("cache.sqlite"),&mut issues).unwrap();
        assert_eq!(restored.checkpoints[&checkpoint.source_key][0].offset,checkpoint.offset);assert!(issues.is_empty());
        let _=fs::remove_dir_all(root);
    }
    #[test]fn machines_share_canonical_requests_and_stream_updates_take_max(){
        let root=std::env::temp_dir().join("codenotch").join(format!("telemetry-machines-{}",unique_id()));initialize(&root).unwrap();
        let event=super::super::tests::sample_event();
        for (machine,writer,output) in [("machineA","writerA",2),("machineB","writerB",6)]{
            let mut event=event.clone();event.machine=machine.into();event.tokens.as_mut().unwrap().output=Some(output);event.tokens.as_mut().unwrap().total=Some(10+output);
            let batch=Batch{schema_version:1,writer_id:writer.into(),machine:machine.into(),created_at:Utc::now().to_rfc3339(),events:vec![event],checkpoints:vec![Checkpoint::fixture()]};publish(&root,&batch).unwrap();
        }
        let mut issues=vec![];let restored=load(&root,&root.join("cache.sqlite"),&mut issues).unwrap();assert_eq!(restored.events.len(),1);assert_eq!(restored.events["shared"].tokens.as_ref().unwrap().total,Some(16));assert_eq!(restored.checkpoints["fixture"].len(),1);
        let junk=root.join("machines/machineC/writerC/batches/2026-10/bad.json");fs::create_dir_all(junk.parent().unwrap()).unwrap();fs::write(junk,"{truncated").unwrap();let restored=load(&root,&root.join("cache.sqlite"),&mut issues).unwrap();assert_eq!(restored.events.len(),1);assert!(!issues.is_empty());
        let _=fs::remove_dir_all(root);
    }
    #[test]fn corrupt_cache_does_not_block_durable_history_and_is_preserved(){
        let root=std::env::temp_dir().join("codenotch").join(format!("telemetry-cache-{}",unique_id()));initialize(&root).unwrap();let event=super::super::tests::sample_event();let batch=Batch{schema_version:1,writer_id:"writer".into(),machine:"machine".into(),created_at:Utc::now().to_rfc3339(),events:vec![event],checkpoints:vec![Checkpoint::fixture()]};publish(&root,&batch).unwrap();
        let cache=root.join("cache.sqlite");fs::write(&cache,b"CORRUPT_CACHE_SENTINEL").unwrap();let mut issues=vec![];let restored=load(&root,&cache,&mut issues).unwrap();assert_eq!(restored.events.len(),1);assert!(issues.iter().any(|i|i.contains("cache unavailable")));assert_eq!(fs::read(&cache).unwrap(),b"CORRUPT_CACHE_SENTINEL");let _=fs::remove_dir_all(root);
    }
    #[test]fn restored_writer_identifier_cannot_escape_selected_dataset(){
        let root=std::env::temp_dir().join("codenotch").join(format!("telemetry-path-{}",unique_id()));initialize(&root).unwrap();
        for writer in ["../../outside",r"C:\outside\writer","/absolute/writer",r"..\..\outside"]{let batch=Batch{schema_version:1,writer_id:writer.into(),machine:"../../machine".into(),created_at:Utc::now().to_rfc3339(),events:vec![],checkpoints:vec![]};let path=publish(&root,&batch).unwrap();assert!(path.starts_with(&root));assert!(path.strip_prefix(&root).unwrap().components().all(|c|matches!(c,std::path::Component::Normal(_))));}
        let target=root.join("immutable.json");atomic_file(&target,b"original",false).unwrap();assert!(atomic_file(&target,b"replacement",false).is_err());assert_eq!(fs::read(target).unwrap(),b"original");let _=fs::remove_dir_all(root);
    }
    fn journal_fixture(root:&Path)->PathBuf{
        initialize(root).unwrap();let batch=Batch{schema_version:1,writer_id:"writer".into(),machine:"machine".into(),created_at:Utc::now().to_rfc3339(),events:vec![super::super::tests::sample_event()],checkpoints:vec![Checkpoint::fixture()]};publish(root,&batch).unwrap()
    }
    fn assert_journal_recovered(dataset:&Dataset,issues:&[String]){
        assert_eq!(dataset.events.len(),1);assert_eq!(dataset.events["shared"].tokens.as_ref().unwrap().total,Some(12));assert_eq!(dataset.checkpoints["fixture"][0].offset,20);assert!(issues.iter().any(|issue|issue.contains("cache unavailable")));
    }
    #[test]fn partial_cache_schema_recovers_both_events_and_checkpoints(){
        let root=std::env::temp_dir().join("codenotch").join(format!("telemetry-partial-cache-{}",unique_id()));let path=journal_fixture(&root);let original=fs::read(&path).unwrap();
        for (name,sql) in [("metadata","CREATE TABLE metadata (key TEXT PRIMARY KEY);CREATE TABLE batches(path TEXT PRIMARY KEY,signature TEXT NOT NULL,hash TEXT NOT NULL,json TEXT NOT NULL);"),("batches","CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE batches(path TEXT PRIMARY KEY);")] {
            let cache=root.join(format!("cache-{name}.sqlite"));let connection=Connection::open(&cache).unwrap();connection.execute_batch(sql).unwrap();
            if name=="batches"{connection.execute("INSERT INTO metadata VALUES ('scope',?1)",[root.to_string_lossy().as_ref()]).unwrap();}drop(connection);
            let before=fs::read(&cache).unwrap();let mut issues=vec![];let dataset=load(&root,&cache,&mut issues).unwrap();assert_journal_recovered(&dataset,&issues);assert_eq!(fs::read(&cache).unwrap(),before);assert_eq!(fs::read(&path).unwrap(),original);
        }
        let _=fs::remove_dir_all(root);
    }
    #[test]fn late_cache_insert_failure_rolls_back_and_replays_durable_journal(){
        let root=std::env::temp_dir().join("codenotch").join(format!("telemetry-cache-trigger-{}",unique_id()));let path=journal_fixture(&root);let original=fs::read(&path).unwrap();let cache=root.join("cache.sqlite");
        let connection=Connection::open(&cache).unwrap();schema(&connection).unwrap();connection.execute("INSERT INTO metadata VALUES ('scope',?1)",[root.to_string_lossy().as_ref()]).unwrap();connection.execute_batch("CREATE TRIGGER reject_cache_insert BEFORE INSERT ON batches BEGIN SELECT RAISE(ABORT,'fixture late insert failure');END;").unwrap();drop(connection);
        let before=fs::read(&cache).unwrap();let mut issues=vec![];let dataset=load(&root,&cache,&mut issues).unwrap();assert_journal_recovered(&dataset,&issues);assert_eq!(fs::read(&cache).unwrap(),before);assert_eq!(fs::read(&path).unwrap(),original);
        let connection=Connection::open(&cache).unwrap();assert_eq!(connection.query_row("SELECT COUNT(*) FROM batches",[],|row|row.get::<_,i64>(0)).unwrap(),0);assert_eq!(connection.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name='reject_cache_insert'",[],|row|row.get::<_,i64>(0)).unwrap(),1);drop(connection);let _=fs::remove_dir_all(root);
    }
    #[test]fn corrupt_cached_payload_replays_source_and_durable_errors_remain_visible(){
        let root=std::env::temp_dir().join("codenotch").join(format!("telemetry-cached-json-{}",unique_id()));let path=journal_fixture(&root);let original=fs::read(&path).unwrap();let cache=root.join("cache.sqlite");let mut issues=vec![];load(&root,&cache,&mut issues).unwrap();
        let connection=Connection::open(&cache).unwrap();connection.execute("UPDATE batches SET json='{}'",[]).unwrap();drop(connection);issues.clear();let dataset=load(&root,&cache,&mut issues).unwrap();assert_journal_recovered(&dataset,&issues);assert_eq!(fs::read(&path).unwrap(),original);
        let bad=root.join("machines/broken/writer/batches/2026-10/bad.json");fs::create_dir_all(bad.parent().unwrap()).unwrap();fs::write(&bad,b"{broken durable batch").unwrap();issues.clear();let dataset=load(&root,&cache,&mut issues).unwrap();assert_eq!(dataset.events.len(),1);assert!(issues.iter().any(|issue|issue.contains("Invalid telemetry batch skipped")));
        fs::write(root.join("codenotch-telemetry-v1.json"),b"{invalid manifest").unwrap();assert!(load(&root,&cache,&mut vec![]).is_err());let _=fs::remove_dir_all(root);
    }
    #[test]fn valid_but_altered_cached_numbers_are_repaired_from_durable_source(){
        let root=std::env::temp_dir().join("codenotch").join(format!("telemetry-cache-checksum-{}",unique_id()));let path=journal_fixture(&root);let original=fs::read(&path).unwrap();let cache=root.join("cache.sqlite");let mut issues=vec![];load(&root,&cache,&mut issues).unwrap();
        let connection=Connection::open(&cache).unwrap();let (json,digest)=connection.query_row("SELECT json,hash FROM batches",[],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).unwrap();assert_eq!(hash(json.as_bytes()),digest);
        let mut payload:serde_json::Value=serde_json::from_str(&json).unwrap();payload["events"][0]["tokens"]["input"]=serde_json::json!(999);payload["events"][0]["tokens"]["output"]=serde_json::json!(99);payload["events"][0]["tokens"]["total"]=serde_json::json!(1098);connection.execute("UPDATE batches SET json=?1",[serde_json::to_string(&payload).unwrap()]).unwrap();drop(connection);
        issues.clear();let dataset=load(&root,&cache,&mut issues).unwrap();assert_journal_recovered(&dataset,&issues);assert_eq!(dataset.events["shared"].tokens.as_ref().unwrap().input,Some(10));assert_eq!(fs::read(&path).unwrap(),original);
        let connection=Connection::open(&cache).unwrap();let (json,digest)=connection.query_row("SELECT json,hash FROM batches",[],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).unwrap();assert_eq!(hash(json.as_bytes()),digest);drop(connection);issues.clear();load(&root,&cache,&mut issues).unwrap();assert!(issues.is_empty());let _=fs::remove_dir_all(root);
    }
    #[test]fn older_original_file_digest_is_rebuilt_for_normalized_cache_json(){
        let root=std::env::temp_dir().join("codenotch").join(format!("telemetry-old-digest-{}",unique_id()));let path=journal_fixture(&root);let parsed:Batch=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();let pretty=serde_json::to_vec_pretty(&parsed).unwrap();fs::write(&path,&pretty).unwrap();let cache=root.join("cache.sqlite");let mut issues=vec![];load(&root,&cache,&mut issues).unwrap();
        let connection=Connection::open(&cache).unwrap();connection.execute("UPDATE batches SET hash=?1",[hash(&pretty)]).unwrap();drop(connection);issues.clear();let dataset=load(&root,&cache,&mut issues).unwrap();assert_journal_recovered(&dataset,&issues);assert_eq!(fs::read(&path).unwrap(),pretty);issues.clear();load(&root,&cache,&mut issues).unwrap();assert!(issues.is_empty());let _=fs::remove_dir_all(root);
    }
    #[cfg(windows)]
    #[test]fn long_unicode_windows_paths_keep_atomic_create_replace_and_immutability(){
        use std::os::windows::ffi::OsStrExt;
        let root=std::env::temp_dir().join("codenotch").join(format!("telemetry-long-path-{}",unique_id()));
        let parent=root.join(format!("André-{}","a".repeat(120))).join(format!("telemetria-{}","b".repeat(120)));let target=parent.join("batch-á.json");
        assert!(target.as_os_str().encode_wide().count()>260);
        atomic_file(&target,b"original",false).unwrap();assert_eq!(fs::read(&target).unwrap(),b"original");
        assert!(atomic_file(&target,b"immutable replacement",false).is_err());assert_eq!(fs::read(&target).unwrap(),b"original");
        atomic_file(&target,b"updated",true).unwrap();assert_eq!(fs::read(&target).unwrap(),b"updated");
        let canonical=fs::canonicalize(&target).unwrap();assert!(canonical.to_string_lossy().starts_with(r"\\?\"));
        let temporary_count=fs::read_dir(&parent).unwrap().filter_map(Result::ok).filter(|entry|entry.file_name().to_string_lossy().ends_with(".tmp")).count();assert_eq!(temporary_count,0);
        let _=fs::remove_dir_all(root);
    }
}
