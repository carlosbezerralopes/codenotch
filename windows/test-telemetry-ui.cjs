const {readFileSync}=require('node:fs');
const {join}=require('node:path');
const vm=require('node:vm');
const assert=require('node:assert/strict');
const {test}=require('node:test');
const html=readFileSync(join(__dirname,'codenotch/ui/telemetry.html'),'utf8');
const script=html.split('<script>')[1].split('</script>')[0];
const pure=script.split('// BEGIN TESTABLE TELEMETRY')[1].split('// END TESTABLE TELEMETRY')[0];
function pureContext(){const c=vm.createContext({Intl,language:'pt-BR',tr:s=>s});vm.runInContext(pure,c);return c;}
const plain=v=>JSON.parse(JSON.stringify(v));
const settings={enabled:false,export_directory:null,suggested_directory:'D:\\Business\\Projetos\\Codenotch\\telemetria',machine:'DESKTOP',writer_id:'writer',utc_offset_minutes:-180,history_days:90,project_mappings:[]};
const values={directory:settings.suggested_directory,enabled:true,history_days:'90',utc_offset_minutes:'-180',project_mappings:[{prefix:' C:\\repo ',project:' Repo '}]};
test('formatting preserves unknowns, known zero and separate units without summing subsets',()=>{
  const c=pureContext();
  for(const v of [undefined,null,NaN,Infinity,-1,'50'])assert.equal(c.amount(v),'—');
  assert.equal(c.amount(0),'0');assert.equal(c.amount(12000),'12.000');assert.equal(c.amount(0.171234,true),'0,171234');
  const row=c.rowHtml({period:'2026-10-10',provider:'codex',machine:'PC',tokens:{input:100,cached_input:90,output:10,reasoning_output:5,total:110},credits:null,event_count:1});
  assert.match(row,/>110</);assert.match(row,/>90</);assert.doesNotMatch(row,/>205</);
  assert.match(row,/>—</);
});
test('all report strings and source provenance are escaped; dates honor configured offset',()=>{
  const c=pureContext(), hostile='<img src=x onerror="oops">';
  const e={occurred_at:'2026-10-10T01:00:00Z',provider:hostile,machine:hostile,project:hostile,folder:hostile,source:hostile,quality:hostile,provenance:{file:hostile},folders:[hostile],credits:0.17,tokens:null};
  const rendered=c.eventHtml(e,-180);
  assert.doesNotMatch(rendered,/<img/);assert.match(rendered,/&lt;img/);assert.match(rendered,/09\/10\/2026/);assert.match(rendered,/22:00:00/);
  assert.doesNotMatch(c.rowHtml({period:hostile,provider:hostile,folder:hostile}),/<img/);
});
test('query DTO distinguishes all from unknown and preserves every filter and grain',()=>{
  const c=pureContext();
  for(const grain of ['event','hour','day','week','month']){
    const q=c.queryFrom({from_date:'2026-10-01',to_date:'2026-10-10',grain,provider:'kiro',machine:'PC',project:'Repo',folder:'C:\\repo'});
    assert.deepEqual(plain(q),{from_date:'2026-10-01',to_date:'2026-10-10',grain,provider:'kiro',machine:'PC',project:'Repo',folder:'C:\\repo'});
  }
  const all=c.queryFrom({grain:'day'});assert.equal(all.machine,null);assert.equal(all.project,null);assert.equal(all.folder,null);
  const unknown=c.queryFrom({grain:'day',unknown_project:true,unknown_folder:true});assert.equal(unknown.project,'');assert.equal(unknown.folder,'');
  for(const args of [{from_date:'2026-10-11',to_date:'2026-10-01',grain:'day'},{from_date:'2026-02-31',grain:'day'},{grain:'pace'}])assert.throws(()=>c.queryFrom(args));
});
test('numeric sorting orders numbers rather than formatted strings and keeps unknown last',()=>{
  const c=pureContext(),rows=[{credits:null},{credits:12},{credits:2}];
  assert.deepEqual(Array.from(c.sortedRows(rows,'credits',1),r=>r.credits),[2,12,null]);
  assert.deepEqual(Array.from(c.sortedRows(rows,'credits',-1),r=>r.credits),[12,2,null]);
  assert.deepEqual(Array.from(c.sortedRows([{tokens:{input:20}},{tokens:null},{tokens:{input:3}}],'input',1),r=>r.tokens?.input??null),[3,20,null]);
});
test('configuration requires chosen directory, validates mappings, preserves writer and machine',async()=>{
  const c=pureContext();let calls=[];
  const api=async(command,args)=>{calls.push({command,args});return args.settings;};
  await assert.rejects(c.persistSettings(api,settings,{...values,directory:' '}));assert.equal(calls.length,0);
  await assert.rejects(c.persistSettings(api,settings,{...values,project_mappings:[{prefix:'',project:'Repo'}]}));
  for(const change of [{history_days:0},{history_days:2.5},{utc_offset_minutes:999}])assert.throws(()=>c.settingsFrom(settings,{...values,...change}));
  const saved=await c.persistSettings(api,settings,values);
  assert.equal(calls[0].command,'set_telemetry_settings');assert.equal(saved.export_directory,values.directory);assert.equal(saved.enabled,true);assert.equal(saved.writer_id,'writer');assert.equal(saved.machine,'DESKTOP');
  assert.deepEqual(plain(saved.project_mappings),[{prefix:'C:\\repo',project:'Repo'}]);
  assert.equal(c.settingsFrom(settings,{...values,enabled:false}).enabled,false);
  const renamed=c.settingsFrom(settings,{...values,machine:' Dell '});
  assert.equal(renamed.machine,'Dell');assert.equal(renamed.writer_id,'writer');
  for(const machine of [' ', 'a'.repeat(81), 'bad\nname'])assert.throws(()=>c.settingsFrom(settings,{...values,machine}));
});

test('copied usage exposes observed machines and attribution gaps without leaking markup',()=>{
  const c=pureContext();
  const e={occurred_at:'2026-10-10T01:00:00Z',provider:'kiro',machine:'PC1',observed_machines:['PC1','<PC2>'],attribution_source:'native-session-metadata-current',gaps:['missing <baseline>'],credits:0.17};
  const rendered=c.eventHtml(e,-180);
  assert.match(rendered,/PC1, &lt;PC2&gt;/);assert.match(rendered,/native-session-metadata-current/);assert.match(rendered,/missing &lt;baseline&gt;/);
  assert.doesNotMatch(rendered,/<PC2>|<baseline>/);
});
test('Browse cancellation preserves suggestion and picker errors can be retried',async()=>{
  const c=pureContext();let calls=0;
  assert.equal(await c.browseDirectory(async command=>{assert.equal(command,'choose_telemetry_directory');return null;},values.directory),values.directory);
  await assert.rejects(c.browseDirectory(async()=>{calls++;throw Error('picker unavailable');},values.directory));
  assert.equal(await c.browseDirectory(async()=>{calls++;return 'E:\\history';},values.directory),'E:\\history');assert.equal(calls,2);
});
function controller(api){
  const nodes={};const node=id=>nodes[id]??(nodes[id]={value:'',checked:false,disabled:false,hidden:false,open:false,innerHTML:'',textContent:'',className:'',children:[],querySelectorAll:()=>[],appendChild:n=>nodes[id].children.push(n)});
  for(const id of ['directory','machine','enabled','history-days','utc-offset','mappings','first-use','setup','history','save','browse','retry','refresh','from-date','to-date','grain','provider','filter-machine','project','folder','unknown-project','unknown-folder','columns','rows','totals','scan','coverage','issues','truncated','events','status','add-mapping','filters'])node(id);
  node('grain').value='day';
  const document={getElementById:node,querySelectorAll:()=>[],documentElement:{dataset:{}},hidden:false};
  const c=vm.createContext({Intl,document,window:{__TAURI__:{core:{invoke:api},event:{listen:()=>Promise.resolve()}}},setInterval:()=>{},console});
  vm.runInContext(script.replace('initialize();',''),c);
  return {c,nodes,set:(key,value)=>{c.injected=value;vm.runInContext(`${key}=injected`,c);}};
}
test('first-use controller does not report or collect before Save; setup failures leave retry available',async()=>{
  const calls=[];let fail=false;
  const {c,nodes}=controller(async(command,args)=>{calls.push(command);if(command==='get_telemetry_settings'){if(fail)throw Error('read failed');return settings;}throw Error(command);});
  await c.loadSettings();assert.deepEqual(calls,['get_telemetry_settings']);assert.equal(nodes.directory.value,settings.suggested_directory);assert.equal(nodes.history.hidden,true);assert.equal(nodes.setup.open,true);assert.equal(nodes.retry.disabled,false);
  fail=true;await c.loadSettings();assert.equal(nodes.retry.disabled,false);assert.match(nodes.status.textContent,/read failed/);
});
test('Save failure retains entered directory and does not query history; retry succeeds and loads report',async()=>{
  let fail=true;const calls=[];
  const {c,nodes,set}=controller(async(command,args)=>{calls.push(command);if(command==='set_telemetry_settings'){if(fail)throw Error('write failed');return args.settings;}if(command==='get_telemetry_report')return {configured:true,enabled:true,rows:[],totals:{tokens:null,credits:null,event_count:0},coverage:[],recent_events:[],issues:[],truncated:false};throw Error(command);});
  set('settings',settings);c.renderSettings();nodes['history-days'].value='90';nodes['utc-offset'].value='-180';nodes.directory.value='E:\\history';
  await c.saveSettings();assert.deepEqual(calls,['set_telemetry_settings']);assert.equal(nodes.directory.value,'E:\\history');assert.match(nodes.status.textContent,/write failed/);
  fail=false;await c.saveSettings();assert.deepEqual(calls,['set_telemetry_settings','set_telemetry_settings','get_telemetry_report']);assert.equal(nodes.history.hidden,false);assert.equal(nodes.setup.open,false);
  assert.match(nodes.totals.innerHTML,/—/);assert.equal(nodes.refresh.disabled,false);
});
test('report rendering exposes gaps, truncation and source quality without executing content',()=>{
  const {c,nodes,set}=controller(async()=>{});set('settings',{...settings,export_directory:'E:\\history'});
  set('report',{configured:true,enabled:false,rows:[],totals:{tokens:null,credits:null,event_count:0},coverage:[{provider:'kiro',source:'<script>x</script>',status:'partial',message:'missing tokens'}],recent_events:[],issues:['failed file'],truncated:true});
  c.renderReport();assert.equal(nodes.truncated.hidden,false);assert.equal(nodes.issues.textContent,'failed file');assert.match(nodes.coverage.innerHTML,/&lt;script/);assert.doesNotMatch(nodes.coverage.innerHTML,/<script>/);assert.match(nodes.scan.innerHTML,/Collection paused/);
});
