use super::{Coverage, Event, Provenance, TelemetrySettings, hash, parsers::{self,Context}, store::{self,Dataset,Batch}};
use chrono::Utc;
use serde::{Deserialize,Serialize};
use std::{collections::BTreeMap, fs::{self,File}, io::{BufRead,BufReader,Read,Seek,SeekFrom}, path::{Path,PathBuf}};

const TICK_BYTES:usize=32*1024*1024;
const MAX_LINE:usize=8*1024*1024;
const MAX_SOURCES:usize=16;
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
pub(super) struct Checkpoint {
    pub source_key:String, pub source_path:String, pub provider:String, pub head:String, pub anchor:String, pub offset:u64,
    pub observed_at:String, pub state:Context, pub malformed_lines:u64, pub incomplete_records:u64,
}
impl Checkpoint {
    pub(super) fn valid_shape(&self)->bool{!self.source_key.is_empty() && self.head.len()==64 && self.anchor.len()==64 && ["claude","codex","kiro"].contains(&self.provider.as_str())}
    #[cfg(test)]pub(super)fn fixture()->Self{Self{source_key:"fixture".into(),source_path:"fixture".into(),provider:"kiro".into(),head:hash(b"head"),anchor:hash(b"anchor"),offset:20,observed_at:Utc::now().to_rfc3339(),state:Context::default(),malformed_lines:0,incomplete_records:0}}
}
#[derive(Clone)]struct Source{provider:&'static str,path:PathBuf,metadata:Option<PathBuf>}
#[derive(Default,Deserialize)]struct KiroSession{
    id:Option<String>,#[serde(rename="rootPaths",default)]roots:Vec<String>,#[serde(rename="workspacePaths",default)]workspaces:Vec<String>,
    #[serde(rename="modelId")]model:Option<String>,#[serde(rename="_meta",default)]meta:KiroImport,
}
#[derive(Default,Deserialize)]struct KiroImport{#[serde(rename="sincronizarConversas",default,deserialize_with="present_marker")]import:bool}
fn present_marker<'de,D:serde::Deserializer<'de>>(deserializer:D)->Result<bool,D::Error>{let _=serde::de::IgnoredAny::deserialize(deserializer)?;Ok(true)}

pub(super)fn scan(settings:&TelemetrySettings,root:&Path,dataset:&mut Dataset,next:&mut usize,issues:&mut Vec<String>)->Result<Vec<Coverage>,String>{
    let sources=discover(issues)?;let mut available=BTreeMap::<&str,usize>::new();for source in &sources{*available.entry(source.provider).or_default()+=1;}
    let mut budget=TICK_BYTES;let mut processed=BTreeMap::<&str,usize>::new();let mut gaps=BTreeMap::<&str,u64>::new();let mut ignored=BTreeMap::<&str,usize>::new();
    let mut read_count=0;
    if !sources.is_empty(){*next%=sources.len();}
    while read_count<MAX_SOURCES.min(sources.len()) && budget>0{
        let source=&sources[*next];*next=(*next+1)%sources.len();read_count+=1;
        match read_source(source,settings,root,dataset,&mut budget){
            Ok((_gap,imported))=>{*processed.entry(source.provider).or_default()+=1;if imported{*ignored.entry(source.provider).or_default()+=1;}},
            Err(error)=>{issues.push(format!("{}: {}: {error}",source.provider,source.path.display()));*gaps.entry(source.provider).or_default()+=1;}
        }
    }
    let mut coverage=vec![];
    for provider in ["claude","codex","kiro"]{
        let count=available.get(provider).copied().unwrap_or(0);let checked=processed.get(provider).copied().unwrap_or(0);let imported=ignored.get(provider).copied().unwrap_or(0);
        let persisted:u64=dataset.checkpoints.values().map(|rows|rows.iter().filter(|c|c.provider==provider).map(|c|c.malformed_lines+c.incomplete_records).max().unwrap_or(0)).sum();
        let discovery_errors=issues.iter().filter(|issue|issue.starts_with(&format!("{provider}: discovery"))).count() as u64;
        let missing=persisted+gaps.get(provider).copied().unwrap_or(0)+discovery_errors;
        let backlog=sources.iter().filter(|s|s.provider==provider).filter(|s|{
            let path=s.path.to_string_lossy();let length=fs::metadata(&s.path).ok().map(|m|m.len());
            !dataset.checkpoints.values().flatten().any(|c| c.source_path==path.as_ref() && Some(c.offset)==length)
        }).count().saturating_sub(imported);
        let status=if missing>0{"partial"}else if count==0{"absent"}else if backlog>0{"indexing"}else{"ok"};
        let message=format!("{count} local sources; {checked} checked this pass; {backlog} queued/incomplete sources; {missing} malformed/incomplete records; {imported} imported sessions ignored. {}",
            if provider=="kiro"{"Native credits only; tokens and historical effective model unavailable."}else{"Cache/reasoning columns are subsets of inclusive input/output; missing values remain unknown."});
        coverage.push(Coverage{provider:provider.into(),source:"local-read-only".into(),status:status.into(),message});
    }
    Ok(coverage)
}
fn read_source(source:&Source,settings:&TelemetrySettings,root:&Path,dataset:&mut Dataset,budget:&mut usize)->Result<(u64,bool),String>{
    let mut initial=Context::default();
    if let Some(path)=&source.metadata{
        let metadata=fs::metadata(path).map_err(|e|e.to_string())?;
        if metadata.len()>MAX_LINE as u64{return Err("Session metadata exceeds the size limit".into());}
        if metadata.len()>*budget as u64{return Ok((0,false));}
        *budget=budget.saturating_sub(metadata.len() as usize);
        let metadata:KiroSession=serde_json::from_slice(&fs::read(path).map_err(|e|e.to_string())?).map_err(|_|"Invalid Kiro metadata".to_string())?;
        if metadata.meta.import{return Ok((0,true));}
        initial.session_id=metadata.id;initial.requested_model=metadata.model;
        initial.folders=if !metadata.roots.is_empty(){metadata.roots}else{metadata.workspaces};
        initial.folders.sort();initial.folders.dedup();
        if initial.folders.len()==1 && parsers::absolute_folder(&initial.folders[0]){initial.folder=Some(initial.folders[0].clone());}
    }
    let mut file=File::open(&source.path).map_err(|e|e.to_string())?;let length=file.metadata().map_err(|e|e.to_string())?.len();
    let mut head=vec![];
    {let mut reader=BufReader::new(&mut file);reader.by_ref().take((MAX_LINE.min(*budget)+1) as u64).read_until(b'\n',&mut head).map_err(|e|e.to_string())?;}
    *budget=budget.saturating_sub(head.len());
    if head.is_empty(){return Ok((0,false));}
    if head.last()!=Some(&b'\n'){return Err("First source line is incomplete or exceeds this pass's byte limit".into());}
    if contains(&head,b"sincronizarConversas"){
        if let Ok(value)=serde_json::from_slice::<serde_json::Value>(&head){initial.imported=parsers::imported(&value);}
    }
    let head_hash=hash(&head);let source_key=format!("{}:{head_hash}",source.provider);
    let saved=dataset.checkpoints.get(&source_key).and_then(|candidates|candidates.iter().filter(|c|c.offset<=length && c.head==head_hash).filter(|c|anchor(&mut file,c.offset).is_ok_and(|v|v==c.anchor)).max_by_key(|c|c.offset)).cloned();
    let mut checkpoint=saved.unwrap_or(Checkpoint{source_key:source_key.clone(),source_path:source.path.to_string_lossy().into(),provider:source.provider.into(),head:head_hash,anchor:hash(b""),offset:0,observed_at:String::new(),state:initial.clone(),malformed_lines:0,incomplete_records:0});
    checkpoint.source_path=source.path.to_string_lossy().into();
    checkpoint.state.imported|=initial.imported;
    if source.provider=="kiro"{checkpoint.state.session_id=initial.session_id;checkpoint.state.folder=initial.folder;checkpoint.state.folders=initial.folders;checkpoint.state.requested_model=initial.requested_model;}
    if checkpoint.offset==length{return Ok((checkpoint.malformed_lines+checkpoint.incomplete_records,false));}
    file.seek(SeekFrom::Start(checkpoint.offset)).map_err(|e|e.to_string())?;
    let mut reader=BufReader::new(file);let before=checkpoint.offset;let mut events=vec![];
    while *budget>0{
        let line_start=checkpoint.offset;let limit=MAX_LINE.min(*budget);let mut line=vec![];
        let read=reader.by_ref().take((limit+1) as u64).read_until(b'\n',&mut line).map_err(|e|e.to_string())?;
        if read==0{break;}*budget=budget.saturating_sub(read);
        if line.last()!=Some(&b'\n'){
            if read>MAX_LINE{return Err("Source line exceeds 8 MiB; cursor was not advanced".into());}
            // The append may finish next tick. Persist only the complete preceding lines.
            break;
        }
        checkpoint.offset+=read as u64;
        let relevant=contains(&line,b"sincronizarConversas") || match source.provider{"kiro"=>contains(&line,b"usage_summary"),"codex"=>[b"session_meta".as_slice(),b"turn_context",b"token_count"].iter().any(|marker|contains(&line,marker)),_=>contains(&line,b"\"usage\"") || contains(&line,b"\"cwd\"")};
        if !relevant{continue;}
        let value:serde_json::Value=match serde_json::from_slice(&line){Ok(v)=>v,Err(_)=>{checkpoint.malformed_lines+=1;continue;}};
        let event=Event{schema_version:1,event_id:String::new(),provider:source.provider.into(),source:if source.provider=="kiro"{"native-v3"}else{"native-jsonl"}.into(),occurred_at:String::new(),observed_at:Utc::now().to_rfc3339(),machine:settings.machine.clone(),writer_id:settings.writer_id.clone(),account:None,session_id:String::new(),request_or_execution_id:None,folder:None,folders:vec![],project:None,project_source:None,model:None,requested_model:None,status:None,tokens:None,credits:None,quality:"partial".into(),provenance:Provenance{source_key:source_key.clone(),source_path:source.path.to_string_lossy().into(),offset:line_start},observed_machines:vec![settings.machine.clone()],attribution_source:None,gaps:vec![]};
        match parsers::parse(source.provider,&value,&mut checkpoint.state,event,settings){
            Ok(Some(event))=>{
                if event.quality!="reported"{checkpoint.incomplete_records+=1;}
                events.push(event);
            },Ok(None)=>{},Err(_)=>checkpoint.incomplete_records+=1,
        }
    }
    if checkpoint.offset==before{return Ok((checkpoint.malformed_lines+checkpoint.incomplete_records,false));}
    checkpoint.anchor=anchor(reader.get_mut(),checkpoint.offset)?;checkpoint.observed_at=Utc::now().to_rfc3339();
    let gaps=checkpoint.malformed_lines+checkpoint.incomplete_records;
    let batch=Batch{schema_version:1,writer_id:settings.writer_id.clone(),machine:settings.machine.clone(),created_at:Utc::now().to_rfc3339(),events,checkpoints:vec![checkpoint]};
    // A failed publish leaves both the event cache and resumable cursor untouched.
    store::publish(root,&batch)?;store::absorb(dataset,&batch);Ok((gaps,false))
}
fn contains(bytes:&[u8],marker:&[u8])->bool{bytes.windows(marker.len()).any(|window|window==marker)}
fn anchor(file:&mut File,offset:u64)->Result<String,String>{
    let start=offset.saturating_sub(256);let length=offset-start;file.seek(SeekFrom::Start(start)).map_err(|e|e.to_string())?;
    let mut bytes=vec![0;length as usize];file.read_exact(&mut bytes).map_err(|e|e.to_string())?;Ok(hash(&bytes))
}
fn dirs(path:&Path,provider:&str,issues:&mut Vec<String>)->Vec<PathBuf>{
    let entries=match fs::read_dir(path){Ok(entries)=>entries,Err(error)=>{if error.kind()!=std::io::ErrorKind::NotFound{issues.push(format!("{provider}: discovery {}: {error}",path.display()));}return vec![];}};
    let mut result=vec![];for entry in entries{match entry{Ok(entry)=>match entry.file_type(){Ok(kind) if kind.is_dir()=>result.push(entry.path()),Ok(_)=>{},Err(error)=>issues.push(format!("{provider}: discovery {}: {error}",entry.path().display()))},Err(error)=>issues.push(format!("{provider}: discovery {}: {error}",path.display()))}}
    result.sort();result
}
fn jsonl(path:&Path,depth:u8,provider:&'static str,output:&mut Vec<Source>,issues:&mut Vec<String>){
    if depth==0{return;}
    let entries=match fs::read_dir(path){Ok(entries)=>entries,Err(error)=>{if error.kind()!=std::io::ErrorKind::NotFound{issues.push(format!("{provider}: discovery {}: {error}",path.display()));}return;}};
    for entry in entries{let entry=match entry{Ok(entry)=>entry,Err(error)=>{issues.push(format!("{provider}: discovery {}: {error}",path.display()));continue;}};let kind=match entry.file_type(){Ok(kind)=>kind,Err(error)=>{issues.push(format!("{provider}: discovery {}: {error}",entry.path().display()));continue;}};let path=entry.path();
        if kind.is_dir(){jsonl(&path,depth-1,provider,output,issues);}else if kind.is_file() && path.extension().is_some_and(|s|s=="jsonl"){output.push(Source{provider,path,metadata:None});}}
}
fn discover(issues:&mut Vec<String>)->Result<Vec<Source>,String>{
    let home=dirs::home_dir().ok_or("User home unavailable")?;let mut sources=vec![];
    let mut claude=vec![home.join(".claude")];claude.extend(dirs(&home,"claude",issues).into_iter().filter(|p|p.file_name().is_some_and(|n|n.to_string_lossy().starts_with(".claude-"))));
    if let Some(local)=std::env::var_os("LOCALAPPDATA"){
        for package in dirs(&PathBuf::from(local).join("Packages"),"claude",issues).into_iter().filter(|p|p.file_name().is_some_and(|n|{let n=n.to_string_lossy();n.starts_with("AnthropicPBC.Claude_") || n.starts_with("Claude_")})){
            for child in ["LocalCache/Roaming/Claude","LocalCache/Local/Claude","LocalCache/Roaming/.claude","LocalCache/Local/.claude"]{let root=package.join(child);claude.push(root.clone());claude.push(root.join("claude-code"));}
        }
    }
    if let Some(roaming)=std::env::var_os("APPDATA"){claude.push(PathBuf::from(roaming).join("Claude"));}
    for root in claude{jsonl(&root.join("projects"),5,"claude",&mut sources,issues);}
    let codex=std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(||home.join(".codex"));
    for child in ["sessions","archived_sessions"]{jsonl(&codex.join(child),6,"codex",&mut sources,issues);}
    let kiro=std::env::var_os("KIRO_HOME").map(PathBuf::from).unwrap_or_else(||home.join(".kiro"));
    for bucket in dirs(&kiro.join("sessions"),"kiro",issues){if bucket.file_name().is_some_and(|n|n=="cli"){continue;}for session in dirs(&bucket,"kiro",issues){if session.file_name().is_some_and(|n|n.to_string_lossy().starts_with("sess_")){
        let path=session.join("messages.jsonl");let metadata=session.join("session.json");if existing_file(&path,"kiro",issues) && existing_file(&metadata,"kiro",issues){sources.push(Source{provider:"kiro",path,metadata:Some(metadata)});}
    }}}
    sources.sort_by(|a,b|a.path.cmp(&b.path));sources.dedup_by(|a,b|a.path==b.path);Ok(sources)
}
fn existing_file(path:&Path,provider:&str,issues:&mut Vec<String>)->bool{match fs::metadata(path){Ok(meta)=>meta.is_file(),Err(error)=>{if error.kind()!=std::io::ErrorKind::NotFound{issues.push(format!("{provider}: discovery {}: {error}",path.display()));}false}}}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn complete_lines_only_and_recovery_after_source_copy(){
        let base=std::env::temp_dir().join("codenotch").join(format!("telemetry-source-{}",super::super::unique_id()));fs::create_dir_all(&base).unwrap();let dataset_path=base.join("data");store::initialize(&dataset_path).unwrap();
        let path=base.join("messages.jsonl");let metadata=base.join("session.json");fs::write(&metadata,br#"{"id":"native","rootPaths":["/workspace"],"workspacePaths":["/workspace"]}"#).unwrap();
        let timestamp=Utc::now().to_rfc3339();let record=serde_json::json!({"id":"event","timestamp":timestamp,"payload":{"type":"usage_summary","executionId":"execution","status":"success","promptTurnSummaries":[{"unit":"credit","usage":2.5}]}});
        let complete=format!("{}\n",record);fs::write(&path,format!("{complete}{{\"payload\":\"usage_summary\"\n{{\"unfinished\":" )).unwrap();
        let source=Source{provider:"kiro",path:path.clone(),metadata:Some(metadata)};let mut dataset=Dataset::default();let settings=TelemetrySettings{writer_id:"writer1".into(),..Default::default()};let mut budget=TICK_BYTES;
        read_source(&source,&settings,&dataset_path,&mut dataset,&mut budget).unwrap();assert_eq!(dataset.events.len(),1);let key=dataset.checkpoints.keys().next().unwrap().clone();let saved=dataset.checkpoints[&key].last().unwrap();assert_eq!(saved.malformed_lines,1);assert!(saved.offset<fs::metadata(&path).unwrap().len());
        let mut issues=vec![];let mut restored=store::load(&dataset_path,&base.join("empty-cache.sqlite"),&mut issues).unwrap();let before=restored.checkpoints[&key][0].offset;
        read_source(&source,&settings,&dataset_path,&mut restored,&mut TICK_BYTES.clone()).unwrap();assert_eq!(restored.events.len(),1);assert_eq!(restored.checkpoints[&key][0].offset,before);
        // Another machine receives both the source prefix and the durable batch. It resumes
        // after the already accounted request, with the native IDs keeping spend unique.
        let copied=base.join("copied.jsonl");let mut second=record.clone();second["id"]=serde_json::json!("event2");second["payload"]["executionId"]=serde_json::json!("execution2");
        fs::write(&copied,format!("{complete}{{\"payload\":\"usage_summary\"\n{second}\n")).unwrap();
        let copied_source=Source{provider:"kiro",path:copied.clone(),metadata:source.metadata.clone()};let other=TelemetrySettings{machine:"machineB".into(),writer_id:"writerB".into(),..settings.clone()};
        read_source(&copied_source,&other,&dataset_path,&mut restored,&mut TICK_BYTES.clone()).unwrap();assert_eq!(restored.events.len(),2);assert!(restored.events.values().any(|e|e.machine=="machineB"));
        let restored_again=store::load(&dataset_path,&base.join("another-empty-cache.sqlite"),&mut issues).unwrap();assert_eq!(restored_again.events.len(),2);
        fs::write(&path,&complete).unwrap();read_source(&source,&settings,&dataset_path,&mut restored,&mut TICK_BYTES.clone()).unwrap();assert_eq!(restored.events.len(),2);
        fs::write(source.metadata.as_ref().unwrap(),br#"{"id":"native","_meta":{"sincronizarConversas":{}}}"#).unwrap();
        let (_,imported)=read_source(&source,&settings,&dataset_path,&mut restored,&mut TICK_BYTES.clone()).unwrap();assert!(imported);assert_eq!(restored.events.len(),2);
        let _=fs::remove_dir_all(base);
    }
    #[test]fn increasing_history_at_eof_restores_old_usage_without_rereading(){
        let base=std::env::temp_dir().join("codenotch").join(format!("telemetry-history-{}",super::super::unique_id()));fs::create_dir_all(&base).unwrap();let data=base.join("dataset");
        let mut service=super::super::TelemetryService::at(base.join("config.json"),base.join("cache.sqlite")).unwrap();let settings=TelemetrySettings{export_directory:Some(data.to_string_lossy().into()),history_days:1,..service.settings()};service.configure(settings).unwrap();
        let path=base.join("messages.jsonl");let metadata=base.join("session.json");fs::write(&metadata,br#"{"id":"old-native"}"#).unwrap();let time=(Utc::now()-chrono::Duration::days(60)).to_rfc3339();let line=serde_json::json!({"id":"old-event","timestamp":time,"payload":{"type":"usage_summary","executionId":"old-execution","promptTurnSummaries":[{"unit":"credit","usage":3.5}]}});fs::write(&path,format!("{line}\n")).unwrap();let source=Source{provider:"kiro",path,metadata:Some(metadata)};
        read_source(&source,&service.settings,&data,&mut service.dataset,&mut TICK_BYTES.clone()).unwrap();assert_eq!(service.dataset.events.len(),1);assert_eq!(service.report(Default::default()).totals.event_count,0);
        let wider=TelemetrySettings{history_days:90,..service.settings()};service.configure(wider).unwrap();assert_eq!(service.report(Default::default()).totals.credits,Some(3.5));
        let offset=service.dataset.checkpoints.values().next().unwrap()[0].offset;read_source(&source,&service.settings,&data,&mut service.dataset,&mut TICK_BYTES.clone()).unwrap();assert_eq!(service.dataset.events.len(),1);assert_eq!(service.dataset.checkpoints.values().next().unwrap()[0].offset,offset);let _=fs::remove_dir_all(base);
    }
    #[test]fn discovery_io_failure_is_reported_instead_of_silent_absence(){
        let base=std::env::temp_dir().join("codenotch").join(format!("telemetry-discovery-{}",super::super::unique_id()));fs::create_dir_all(&base).unwrap();let file=base.join("not-directory");fs::write(&file,b"fixture").unwrap();let mut issues=vec![];let mut sources=vec![];jsonl(&file,2,"claude",&mut sources,&mut issues);assert!(sources.is_empty());assert!(issues.iter().any(|i|i.starts_with("claude: discovery")));let _=fs::remove_dir_all(base);
    }
}
