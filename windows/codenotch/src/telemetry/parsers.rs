use super::{Event, Tokens, TelemetrySettings, hash};
use chrono::{DateTime,Utc};
use serde::{Deserialize,Serialize};
use serde_json::Value;
use std::path::{Path,PathBuf};

#[derive(Debug,Clone,Default,PartialEq,Serialize,Deserialize)]
pub(super) struct Context {
    pub session_id: Option<String>, pub folder: Option<String>, pub folders: Vec<String>,
    pub model: Option<String>, pub requested_model: Option<String>, pub turn_id: Option<String>,
    pub counters: Option<Tokens>, pub reset_epoch: u64,
    #[serde(default)]pub imported: bool,
}
fn text(value:&Value,key:&str)->Option<String>{value.get(key).and_then(Value::as_str).filter(|s|!s.is_empty()).map(String::from)}
fn count(value:&Value,key:&str)->Option<u64>{value.get(key).and_then(Value::as_u64)}
fn timestamp(value:&Value)->Option<String>{text(value,"timestamp").and_then(|s|DateTime::parse_from_rfc3339(&s).ok()).map(|d|d.with_timezone(&Utc).to_rfc3339())}
pub(super)fn imported(value:&Value)->bool{["/_meta/sincronizarConversas","/metadata/sincronizarConversas","/payload/_meta/sincronizarConversas","/payload/metadata/sincronizarConversas","/message/_meta/sincronizarConversas"].iter().any(|path|value.pointer(path).is_some())}

pub(super) fn parse(provider:&str,value:&Value,state:&mut Context,mut event:Event,settings:&TelemetrySettings)->Result<Option<Event>,String>{
    if imported(value){state.imported=true;return Ok(None);}if state.imported{return Ok(None);}
    let parsed=match provider {
        "claude"=>claude(value,state,&mut event),"codex"=>codex(value,state,&mut event),"kiro"=>kiro(value,state,&mut event),_=>Ok(false)
    }?;
    if !parsed{return Ok(None);}
    event.occurred_at=timestamp(value).ok_or("Usage record has no valid source timestamp")?;
    event.folder=state.folder.clone();event.folders=state.folders.clone();
    event.session_id=state.session_id.clone().unwrap_or_default();
    event.model=state.model.clone();event.requested_model=state.requested_model.clone();
    event.attribution_source=Some(if provider=="kiro"{"native-session-metadata-current"}else{"native-record-or-session-context"}.into());
    if let Some(folder)=&event.folder {let (project,source)=project(folder,settings);event.project=project;event.project_source=source;}
    Ok(Some(event))
}
fn claude(value:&Value,state:&mut Context,event:&mut Event)->Result<bool,String>{
    if let Some(folder)=text(value,"cwd"){set_folder(state,folder);}
    if let Some(id)=text(value,"sessionId"){state.session_id=Some(id);}
    let Some(message)=value.get("message") else{return Ok(false);};
    let Some(usage)=message.get("usage").filter(|v|v.is_object()) else{return Ok(false);};
    if value.get("type").and_then(Value::as_str)!=Some("assistant"){return Ok(false);}
    let id=text(message,"id").ok_or("Claude usage has no message ID")?;
    let request=text(value,"requestId");
    event.event_id=format!("claude:{}",hash(format!("{}:{id}",request.as_deref().unwrap_or("")).as_bytes()));
    event.request_or_execution_id=request.or(Some(id));
    state.model=text(message,"model");
    let base=count(usage,"input_tokens");let cached=count(usage,"cache_read_input_tokens");let creation=count(usage,"cache_creation_input_tokens");let output=count(usage,"output_tokens");
    // Claude reports uncached input; normalize it to inclusive input so cache is a subset.
    let input=base.zip(cached).zip(creation).map(|((n,c),w)|n.saturating_add(c).saturating_add(w));
    let total=input.zip(output).map(|(i,o)|i.saturating_add(o));
    if base.is_none() && output.is_none(){return Err("Claude usage has no numeric token counts".into());}
    event.quality=if base.is_some() && output.is_some() && cached.is_some() && creation.is_some(){"reported"}else{"partial"}.into();
    event.tokens=Some(Tokens{input,cached_input:cached,cache_creation:creation,output,reasoning_output:None,total});
    Ok(true)
}
fn codex_tokens(value:&Value)->Tokens{
    let input=count(value,"input_tokens");let output=count(value,"output_tokens");
    Tokens{input,cached_input:count(value,"cached_input_tokens"),cache_creation:None,output,
        reasoning_output:count(value,"reasoning_output_tokens"),total:count(value,"total_tokens")}
}
fn delta(new:&Tokens,old:&Tokens)->Option<Tokens>{
    fn field(new:Option<u64>,old:Option<u64>)->Option<Option<u64>>{match(new,old){(Some(n),Some(o)) if n<o=>None,(Some(n),Some(o))=>Some(Some(n-o)),_=>Some(None)}}
    Some(Tokens{input:field(new.input,old.input)?,cached_input:field(new.cached_input,old.cached_input)?,cache_creation:field(new.cache_creation,old.cache_creation)?,output:field(new.output,old.output)?,reasoning_output:field(new.reasoning_output,old.reasoning_output)?,total:field(new.total,old.total)?})
}
fn codex(value:&Value,state:&mut Context,event:&mut Event)->Result<bool,String>{
    let Some(payload)=value.get("payload") else{return Ok(false);};
    match value.get("type").and_then(Value::as_str){
        Some("session_meta")=>{state.session_id=text(payload,"id").or_else(||text(payload,"session_id"));if let Some(folder)=text(payload,"cwd"){set_folder(state,folder);}return Ok(false);}
        Some("turn_context")=>{state.turn_id=text(payload,"turn_id");if let Some(model)=text(payload,"model"){state.model=Some(model);}if let Some(folder)=text(payload,"cwd"){set_folder(state,folder);}return Ok(false);}
        Some("event_msg") if payload.get("type").and_then(Value::as_str)==Some("token_count")=>{},_=>return Ok(false)
    }
    let info=payload.get("info").filter(|v|v.is_object()).ok_or("Codex token_count has no info")?;
    let current=info.get("total_token_usage").filter(|v|v.is_object()).map(codex_tokens);
    let last=info.get("last_token_usage").filter(|v|v.is_object()).map(codex_tokens);
    let request=text(payload,"request_id").or_else(||text(info,"request_id")).or_else(||text(value,"request_id"));
    let mut tokens=if let Some(current)=&current{
        if state.counters.as_ref().is_some_and(|old|old==current || old.total.zip(current.total).is_some_and(|(a,b)|a==b)){
            state.counters=Some(current.clone());return Ok(false);
        }
        let previous=state.counters.replace(current.clone());
        match previous{
            None=>{let Some(last)=last.clone()else{return Err("Codex initial sample established baseline; request usage unavailable".into());};
                if current.total.zip(last.total).is_none_or(|(c,l)|c>l){event.gaps.push("initial_cumulative_baseline_unavailable".into());}last},
            Some(old)=>match delta(current,&old){Some(delta)=>{
                if current.cached_input.is_some() && old.cached_input.is_none(){event.gaps.push("cached_input_delta_baseline_unavailable".into());}
                if current.reasoning_output.is_some() && old.reasoning_output.is_none(){event.gaps.push("reasoning_delta_baseline_unavailable".into());}
                if current.total.is_some() && old.total.is_none(){event.gaps.push("total_delta_baseline_unavailable".into());}
                delta},None=>{state.reset_epoch+=1;event.gaps.push("cumulative_counters_reset".into());last.clone().ok_or("Codex counters reset without last request usage")?}},
        }
    }else if request.is_some(){last.clone().ok_or("Codex has no numeric token usage")?}
    else{return Err("Codex last-only usage has no request identity".into());};
    let occurred=timestamp(value).ok_or("Codex token_count has no timestamp")?;
    let identity=if let Some(id)=&request{format!("request:{id}")}else{
        serde_json::to_string(&(occurred,&state.turn_id,&current,&last,&state.folder,&state.model)).map_err(|e|e.to_string())?
    };
    event.event_id=format!("codex:{}",hash(identity.as_bytes()));event.request_or_execution_id=request.or_else(||state.turn_id.clone());
    validate_tokens(&mut tokens,&mut event.gaps);
    event.quality=if tokens.input.is_some() && tokens.output.is_some() && tokens.total.is_some() && event.gaps.is_empty(){"reported"}else{"partial"}.into();
    event.tokens=Some(tokens);Ok(true)
}
fn kiro(value:&Value,state:&mut Context,event:&mut Event)->Result<bool,String>{
    let Some(payload)=value.get("payload") else{return Ok(false);};
    if payload.get("type").and_then(Value::as_str)!=Some("usage_summary"){return Ok(false);}
    let execution=text(payload,"executionId").or_else(||text(value,"id")).ok_or("Kiro completion has no event identity")?;
    let session=state.session_id.as_ref().ok_or("Kiro completion has no native session ID")?;
    event.event_id=format!("kiro:{}",hash(format!("{session}:{execution}").as_bytes()));event.request_or_execution_id=Some(execution);
    let summaries=payload.get("promptTurnSummaries").and_then(Value::as_array).ok_or("Kiro completion has no usage summaries")?;
    let mut credits=0.0;
    for summary in summaries {
        if summary.get("unit").and_then(Value::as_str)!=Some("credit"){return Err("Kiro usage unit is not credit".into());}
        let number=summary.get("usage").and_then(Value::as_f64).filter(|n|n.is_finite() && *n>=0.0).ok_or("Kiro usage amount is invalid")?;
        credits+=number;if !credits.is_finite(){return Err("Kiro credit sum overflow".into());}
    }
    event.credits=if summaries.is_empty(){None}else{Some(credits)};event.status=text(payload,"status");
    event.quality=if event.credits.is_some(){"reported"}else{"partial"}.into();state.model=None;Ok(true)
}
pub(super) fn project(folder:&str,settings:&TelemetrySettings)->(Option<String>,Option<String>){
    let normalized=|s:&str|{let v=s.replace('\\',"/").trim_end_matches('/').to_string();if cfg!(windows) || s.as_bytes().get(1)==Some(&b':') || s.starts_with("\\\\"){v.to_lowercase()}else{v}};
    let candidate=normalized(folder);
    if let Some(mapping)=settings.project_mappings.iter().filter(|m|{let prefix=normalized(&m.prefix);candidate==prefix || candidate.starts_with(&(prefix+"/"))}).max_by_key(|m|m.prefix.len()){
        return(Some(mapping.project.clone()),Some(format!("mapping:{}",mapping.prefix)));
    }
    let mut path=PathBuf::from(folder);
    while path.is_absolute(){
        let git=path.join(".git");
        if git.is_dir(){return(project_name(&path),Some(format!("git:{}",path.display())));}
        if git.is_file(){
            if let Ok(text)=std::fs::read_to_string(&git){if let Some(dir)=text.trim().strip_prefix("gitdir:"){
                let dir=PathBuf::from(dir.trim());let dir=if dir.is_absolute(){dir}else{path.join(dir)};
                if let Ok(common)=std::fs::read_to_string(dir.join("commondir")){let common=PathBuf::from(common.trim());let common=if common.is_absolute(){common}else{dir.join(common)};
                    if let Ok(common)=common.canonicalize(){let root=common.parent().unwrap_or(&path);return(project_name(root),Some(format!("git:{}",root.display())));}
                }
                if dir.exists(){return(project_name(&path),Some(format!("git:{}",path.display())));}
            }}
        }
        if !path.pop(){break;}
    }
    (None,None)
}
fn project_name(path:&Path)->Option<String>{path.file_name().map(|n|n.to_string_lossy().into_owned())}
pub(super)fn absolute_folder(folder:&str)->bool{let bytes=folder.as_bytes();Path::new(folder).is_absolute() || folder.starts_with('/') || (bytes.len()>2 && bytes[0].is_ascii_alphabetic() && bytes[1]==b':' && (bytes[2]==b'\\' || bytes[2]==b'/')) || folder.starts_with("\\\\")}
fn set_folder(state:&mut Context,folder:String){state.folder=if absolute_folder(&folder){Some(folder.clone())}else{None};state.folders=vec![folder];}
pub(super) fn validate_tokens(tokens:&mut Tokens,gaps:&mut Vec<String>){
    if tokens.cached_input.zip(tokens.input).is_some_and(|(c,i)|c>i){tokens.cached_input=None;gaps.push("cached_input_exceeds_input".into());}
    if tokens.reasoning_output.zip(tokens.output).is_some_and(|(r,o)|r>o){tokens.reasoning_output=None;gaps.push("reasoning_output_exceeds_output".into());}
    if let Some(((input,output),total))=tokens.input.zip(tokens.output).zip(tokens.total){if input.checked_add(output)!=Some(total){tokens.total=None;gaps.push("reported_total_inconsistent".into());}}
}

#[cfg(test)]
mod tests{
    use super::*;
    fn event(provider:&str)->Event{Event{schema_version:1,event_id:String::new(),provider:provider.into(),source:"fixture".into(),occurred_at:String::new(),observed_at:Utc::now().to_rfc3339(),machine:"a".into(),writer_id:"w".into(),account:None,session_id:String::new(),request_or_execution_id:None,folder:None,folders:vec![],project:None,project_source:None,model:None,requested_model:None,status:None,tokens:None,credits:None,quality:"partial".into(),provenance:super::super::Provenance{source_key:"s".into(),source_path:"fixture".into(),offset:0},observed_machines:vec!["a".into()],attribution_source:None,gaps:vec![]}}
    #[test]fn kiro_abort_empty_units_and_imports(){
        let mut state=Context{session_id:Some("native".into()),folders:vec!["/one".into(),"/two".into()],..Default::default()};
        let value=serde_json::json!({"id":"e","timestamp":"2026-10-10T02:00:00Z","payload":{"type":"usage_summary","executionId":"x","status":"aborted","promptTurnSummaries":[{"unit":"credit","usage":1.25}]}});
        let e=parse("kiro",&value,&mut state,event("kiro"),&TelemetrySettings::default()).unwrap().unwrap();assert_eq!(e.credits,Some(1.25));assert_eq!(e.status.as_deref(),Some("aborted"));assert!(e.folder.is_none());assert_eq!(e.folders.len(),2);assert!(e.tokens.is_none());
        let mut empty=value.clone();empty["payload"]["promptTurnSummaries"]=serde_json::json!([]);assert!(parse("kiro",&empty,&mut state,event("kiro"),&TelemetrySettings::default()).unwrap().unwrap().credits.is_none());
        empty["_meta"]=serde_json::json!({"sincronizarConversas":{}});assert!(parse("kiro",&empty,&mut state,event("kiro"),&TelemetrySettings::default()).unwrap().is_none());
    }
    #[test]fn claude_stream_ids_cache_subset_and_no_content_export(){
        let mut state=Context::default();let value=serde_json::json!({"type":"assistant","sessionId":"s","requestId":"r","timestamp":"2026-10-10T02:00:00Z","message":{"id":"m","model":"model","content":"PRIVATE_SENTINEL","usage":{"input_tokens":10,"output_tokens":3,"cache_read_input_tokens":5,"cache_creation_input_tokens":2}}});
        let first=parse("claude",&value,&mut state,event("claude"),&TelemetrySettings::default()).unwrap().unwrap();let mut update=value.clone();update["message"]["usage"]["output_tokens"]=serde_json::json!(8);
        let second=parse("claude",&update,&mut state,event("claude"),&TelemetrySettings::default()).unwrap().unwrap();assert_eq!(first.event_id,second.event_id);assert_eq!(first.tokens.unwrap().input,Some(17));assert!(!serde_json::to_string(&second).unwrap().contains("PRIVATE_SENTINEL"));
    }
    #[test]fn codex_repeat_reset_and_subsets(){
        let mut state=Context::default();let value=serde_json::json!({"type":"event_msg","timestamp":"2026-10-10T02:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"reasoning_output_tokens":3,"total_tokens":110},"last_token_usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110}}}});
        let first=parse("codex",&value,&mut state,event("codex"),&TelemetrySettings::default()).unwrap().unwrap();assert_eq!(first.tokens.unwrap().total,Some(110));assert!(parse("codex",&value,&mut state,event("codex"),&TelemetrySettings::default()).unwrap().is_none());
        let mut reset=value.clone();reset["timestamp"]=serde_json::json!("2026-10-10T03:00:00Z");reset["payload"]["info"]["total_token_usage"]=serde_json::json!({"input_tokens":2,"output_tokens":1,"total_tokens":3});reset["payload"]["info"]["last_token_usage"]=serde_json::json!({"input_tokens":2,"output_tokens":1,"total_tokens":3});
        assert_eq!(parse("codex",&reset,&mut state,event("codex"),&TelemetrySettings::default()).unwrap().unwrap().tokens.unwrap().total,Some(3));
    }
    fn token_record(time:&str,input:u64,output:u64,last_input:u64,last_output:u64)->Value{
        serde_json::json!({"type":"event_msg","timestamp":time,"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":input,"output_tokens":output,"total_tokens":input+output},"last_token_usage":{"input_tokens":last_input,"output_tokens":last_output,"total_tokens":last_input+last_output}}}})
    }
    #[test]fn truncated_copy_cannot_promote_cumulative_to_request_spend(){
        let settings=TelemetrySettings::default();let mut full=Context::default();let first=token_record("2026-10-10T01:00:00Z",100,10,100,10);let second=token_record("2026-10-10T02:00:00Z",200,20,100,10);
        let a=parse("codex",&first,&mut full,event("codex"),&settings).unwrap().unwrap();let b=parse("codex",&second,&mut full,event("codex"),&settings).unwrap().unwrap();
        let copied=parse("codex",&second,&mut Context::default(),event("codex"),&settings).unwrap().unwrap();assert_eq!(copied.event_id,b.event_id);assert_eq!(copied.tokens.as_ref().unwrap().total,Some(110));assert!(copied.gaps.contains(&"initial_cumulative_baseline_unavailable".into()));
        let mut dataset=super::super::store::Dataset::default();let batch=super::super::store::Batch{schema_version:1,writer_id:"writer".into(),machine:"a".into(),created_at:Utc::now().to_rfc3339(),events:vec![a,b,copied],checkpoints:vec![]};super::super::store::absorb(&mut dataset,&batch);
        assert_eq!(dataset.events.len(),2);assert_eq!(dataset.events.values().map(|e|e.tokens.as_ref().unwrap().total.unwrap()).sum::<u64>(),220);
        let mut no_last=first.clone();no_last["payload"]["info"].as_object_mut().unwrap().remove("last_token_usage");let mut state=Context::default();assert!(parse("codex",&no_last,&mut state,event("codex"),&settings).is_err());assert_eq!(state.counters.unwrap().total,Some(110));
    }
    #[test]fn newly_exposed_fields_have_no_invented_zero_baseline(){
        let settings=TelemetrySettings::default();let mut state=Context::default();let first=token_record("2026-10-10T01:00:00Z",100,10,100,10);parse("codex",&first,&mut state,event("codex"),&settings).unwrap();
        let mut second=token_record("2026-10-10T02:00:00Z",150,15,50,5);second["payload"]["info"]["total_token_usage"]["cached_input_tokens"]=serde_json::json!(100);
        let e=parse("codex",&second,&mut state,event("codex"),&settings).unwrap().unwrap();let counts=e.tokens.unwrap();assert_eq!(counts.input,Some(50));assert_eq!(counts.cached_input,None);assert_eq!(counts.total,Some(55));assert_eq!(e.quality,"partial");
        let mut third=token_record("2026-10-10T03:00:00Z",200,20,50,5);third["payload"]["info"]["total_token_usage"]["cached_input_tokens"]=serde_json::json!(120);
        assert_eq!(parse("codex",&third,&mut state,event("codex"),&settings).unwrap().unwrap().tokens.unwrap().cached_input,Some(20));
        state.counters.as_mut().unwrap().total=None;let fourth=token_record("2026-10-10T04:00:00Z",250,25,50,5);let e=parse("codex",&fourth,&mut state,event("codex"),&settings).unwrap().unwrap();assert!(e.tokens.unwrap().total.is_none());
        let mut impossible=Tokens{input:Some(10),cached_input:Some(99),output:Some(3),reasoning_output:Some(88),total:Some(999),..Default::default()};let mut gaps=vec![];validate_tokens(&mut impossible,&mut gaps);assert_eq!(impossible.cached_input,None);assert_eq!(impossible.reasoning_output,None);assert_eq!(impossible.total,None);assert_eq!(gaps.len(),3);
    }
    #[test]fn imported_header_guard_survives_subsequent_records(){
        for provider in ["claude","codex"]{let mut state=Context::default();let header=serde_json::json!({"type":"session_meta","payload":{"_meta":{"sincronizarConversas":{}}}});assert!(parse(provider,&header,&mut state,event(provider),&TelemetrySettings::default()).unwrap().is_none());
            let restored:Context=serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();state=restored;assert!(state.imported);
            let usage=if provider=="codex"{token_record("2026-10-10T02:00:00Z",100,10,100,10)}else{serde_json::json!({"type":"assistant","timestamp":"2026-10-10T02:00:00Z","message":{"id":"m","usage":{"input_tokens":10,"output_tokens":2}}})};assert!(parse(provider,&usage,&mut state,event(provider),&TelemetrySettings::default()).unwrap().is_none());}
    }
}
