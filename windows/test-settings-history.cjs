const {readFileSync}=require('node:fs');
const {join}=require('node:path');
const vm=require('node:vm');
const assert=require('node:assert/strict');
const {test}=require('node:test');
const settingsHtml=readFileSync(join(__dirname,'codenotch/ui/settings.html'),'utf8');
const telemetryHtml=readFileSync(join(__dirname,'codenotch/ui/telemetry.html'),'utf8');
const settingsScript=settingsHtml.split('<script>')[1].split('</script>')[0];
const historyScript=settingsScript.slice(settingsScript.indexOf("const TABS ="),settingsScript.indexOf('// END SETTINGS USAGE'));
const telemetryScript=telemetryHtml.split('<script>')[1].split('</script>')[0];
const configured={enabled:true,export_directory:'D:\\Shared\\telemetria',suggested_directory:null,machine:'PC',writer_id:'existing-writer',utc_offset_minutes:-180,history_days:90,project_mappings:[]};
const emptyReport={configured:true,enabled:true,rows:[],totals:{event_count:0},coverage:[],issues:[],recent_events:[]};
const flush=()=>new Promise(resolve=>setImmediate(resolve));
function dom(){
  const nodes={};
  const node=id=>nodes[id]??(nodes[id]={id,value:'',checked:false,hidden:false,open:false,disabled:false,innerHTML:'',textContent:'',children:[],style:{},dataset:{},attributes:{},listeners:{},classList:{toggle(){},add(){}},focus(){this.focused=true;},setAttribute(k,v){this.attributes[k]=v;},addEventListener(k,v){this.listeners[k]=v;},appendChild(n){this.children.push(n);},querySelectorAll(){return [];},remove(){this.removed=true;}});
  node('grain').value='day';
  const document={hidden:false,documentElement:{dataset:{theme:'dark'}},body:node('body'),getElementById:node,querySelectorAll:()=>[],createElement:tag=>node('new-'+tag)};
  return {nodes,node,document};
}
function host(){
  const {nodes,node,document}=dom(),calls=[],updates=[],listeners={},memory={};
  let bridge,frame;
  document.createElement=()=>{
    frame={...node('frame'),contentWindow:{location:{href:''},CodenotchHistory:{mount:async(api,context)=>{bridge=api;updates.push(context);},update:context=>updates.push(context)}}};
    Object.defineProperty(frame,'src',{set(v){this.contentWindow.location.href=v;},get(){return this.contentWindow.location.href;}});
    return frame;
  };
  const window={location:{href:'http://tauri.localhost/settings.html',search:''},__TAURI__:{event:{listen:(name,fn)=>{listeners[name]=fn;}}}};
  const c=vm.createContext({URL,URLSearchParams,Set,window,document,uiLang:'en',ui:(k,v)=>v,strip(){},errText:String,invoke:async(command,args)=>{calls.push({command,args});return true;},localStorage:{getItem:k=>memory[k],setItem:(k,v)=>memory[k]=v}});
  vm.runInContext(historyScript,c);
  return {c,nodes,calls,updates,listeners,memory,get frame(){return frame;},get bridge(){return bridge;}};
}
function panel(api){
  const {nodes,node,document}=dom(),intervals=new Map(),listeners={};let intervalId=0;
  const window={location:{search:'?embedded=1',href:'http://tauri.localhost/telemetry.html?embedded=1'},parent:{location:{href:'http://tauri.localhost/settings.html'}},addEventListener:(name,fn)=>{listeners[name]=fn;},__TAURI__:{core:{invoke:()=>{throw Error('embedded IPC must use host');}}}};
  const c=vm.createContext({URL,URLSearchParams,Intl,window,document,setInterval:fn=>{intervals.set(++intervalId,fn);return intervalId;},clearInterval:id=>intervals.delete(id)});
  vm.runInContext(telemetryScript,c);
  return {c,nodes,intervals,window,listeners,mount:context=>window.CodenotchHistory.mount({invoke:api},context),update:context=>window.CodenotchHistory.update(context)};
}
const context={active:true,language:'pt-BR',theme:'light',provider:null};
test('History is a keyboard-selectable Settings section with a single reused local report page',()=>{
  new vm.Script(settingsScript);new vm.Script(telemetryScript);
  assert.match(settingsHtml,/id="tab-history" role="tab" aria-controls="pane-history"/);
  assert.match(settingsHtml,/id="pane-history" role="tabpanel" aria-labelledby="tab-history"/);
  assert.doesNotMatch(settingsHtml,/btn-telemetry|invoke\('open_telemetry'\)/);
  assert.match(settingsScript,/showTab\(route.section\|\|savedTab\(\)\)/);
  const h=host();h.c.showTab('history',true);
  assert.equal(h.nodes['tab-history'].attributes['aria-selected'],'true');assert.equal(h.nodes['tab-history'].focused,true);assert.equal(h.nodes['pane-history'].hidden,false);
  assert.equal(h.memory['codenotch.settings.pane'],'history');assert.equal(h.c.savedTab(),'history');
  assert.equal(h.frame.src,'http://tauri.localhost/telemetry.html?embedded=1');
});
test('route and event validate provider, reuse frame and propagate active/language/theme',async()=>{
  const h=host();assert.equal(h.c.historyRoute('?section=history&provider=kiro').provider,'kiro');
  assert.equal(h.c.historyRoute('?section=history&provider=evil').provider,null);assert.equal(h.c.historyRoute('?section=evil').section,null);
  h.listeners.settings_navigate({payload:{section:'history',provider:'codex'}});const frame=h.frame;await frame.onload();
  assert.equal(h.updates[0].provider,'codex');assert.equal(h.updates[0].active,true);
  h.c.showTab('general');assert.equal(h.updates.at(-1).active,false);
  h.c.uiLang='pt-BR';h.c.document.documentElement.dataset.theme='light';h.c.navigateSettings({section:'history',provider:'kiro'});
  assert.equal(h.frame,frame);assert.equal(h.updates.at(-1).provider,'kiro');assert.equal(h.updates.at(-1).language,'pt-BR');assert.equal(h.updates.at(-1).theme,'light');
});
test('host whitelist rejects other commands and a navigated or external iframe',async()=>{
  const h=host();h.c.showTab('history');await h.frame.onload();await h.bridge.invoke('get_telemetry_settings');assert.equal(h.calls.length,1);
  await assert.rejects(h.bridge.invoke('quit_app'));assert.equal(h.calls.length,1);
  h.frame.contentWindow.location.href='https://external.example/telemetry.html?embedded=1';await assert.rejects(h.bridge.invoke('get_telemetry_report'));assert.equal(h.calls.length,1);
  await h.frame.onload();assert.equal(h.nodes['history-error'].hidden,false);
  h.nodes['history-retry'].listeners.click();assert.equal(h.nodes['history-error'].hidden,true);assert.equal(h.frame.src,'http://tauri.localhost/telemetry.html?embedded=1');
});
test('embedded first use performs no IPC before mount and no collection/report before Save',async()=>{
  const calls=[],p=panel(async(command)=>{calls.push(command);if(command==='get_telemetry_settings')return {...configured,export_directory:null,enabled:false,suggested_directory:'D:\\Suggested'};throw Error(command);});
  assert.equal(calls.length,0);assert.equal(p.intervals.size,0);await p.mount(context);
  assert.deepEqual(calls,['get_telemetry_settings']);assert.equal(p.nodes.directory.value,'D:\\Suggested');assert.equal(p.nodes.history.hidden,true);assert.equal(p.nodes.setup.open,true);
  assert.equal(p.c.document.documentElement.lang,'pt-BR');assert.equal(p.c.document.documentElement.dataset.theme,'light');
});
test('reentering preserves drafts/filters, refreshes provider and only runs timer while active',async()=>{
  const calls=[],p=panel(async(command,args)=>{calls.push({command,args});return command==='get_telemetry_settings'?configured:emptyReport;});
  await p.mount({...context,provider:'codex'});assert.equal(p.intervals.size,1);assert.equal(calls.at(-1).args.query.provider,'codex');
  p.nodes.directory.value='E:\\Unsaved';p.nodes.project.value='Repo';
  p.update({...context,active:false,provider:'codex'});assert.equal(p.intervals.size,0);const count=calls.length;await p.c.refreshReport();assert.equal(calls.length,count);
  p.update({...context,provider:'kiro'});await flush();assert.equal(p.intervals.size,1);assert.equal(p.nodes.directory.value,'E:\\Unsaved');assert.equal(p.nodes.project.value,'Repo');assert.equal(calls.at(-1).args.query.provider,'kiro');
  assert.equal(calls.filter(c=>c.command==='get_telemetry_settings').length,1);assert.equal(calls.filter(c=>c.command==='set_telemetry_settings').length,0);
  p.listeners.pagehide();assert.equal(p.intervals.size,0);
});
test('embedded panel rejects a different parent origin or path',async()=>{
  for(const href of ['https://external.example/settings.html','http://tauri.localhost/other.html']){
    const p=panel(async()=>{throw Error('must not call');});p.window.parent.location.href=href;await assert.rejects(p.mount(context));assert.equal(p.intervals.size,0);
  }
});
test('provider navigation during the first pending report refreshes the new filter',async()=>{
  const queries=[];let resolveFirst;
  const p=panel(async(command,args)=>{
    if(command==='get_telemetry_settings')return configured;
    queries.push(args.query.provider);
    if(queries.length===1)return new Promise(resolve=>{resolveFirst=resolve;});
    return emptyReport;
  });
  const mounting=p.mount({...context,provider:'codex'});await flush();assert.deepEqual(queries,['codex']);
  p.update({...context,provider:'kiro'});resolveFirst(emptyReport);await mounting;
  assert.deepEqual(queries,['codex','kiro']);assert.equal(p.nodes.provider.value,'kiro');
});
