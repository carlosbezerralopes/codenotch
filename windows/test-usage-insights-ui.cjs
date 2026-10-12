const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const assert=require('node:assert/strict');
const {test}=require('node:test');
const read=name=>fs.readFileSync(path.join(__dirname,'codenotch/ui',name),'utf8');
const formatter=read('usage-insights.js'),notch=read('notch.html'),panel=read('usage-summary.html'),settings=read('settings.html');
function context(extra={}){const c=vm.createContext({Intl,...extra});vm.runInContext(formatter,c);return c;}
function fixture(patch={}){return {window_index:0,window_id:'weekly_all',label:'Weekly',group:null,fresh:true,official_used_percent:25,official_remaining_percent:75,credits:null,quota_role:'general',period_kind:'weekly',
  period:{reset_ms:1792411200000,elapsed_percent:40},work:{cycle_total_days:4,cycle_elapsed_days:0,remaining_days:4,elapsed_percent:0,used_minus_work_pp:25,calendar_partial:false},
  budget:{unit:'percent',remaining:75,per_remaining_workday:18.75,reference_per_cycle_workday:25,week_remaining_workdays:2,week_remaining_budget:37.5,week_reference_budget:50},display_budget:{mode:'per_workday',unit:'percent',amount:18.75,workdays_remaining:4,horizon_end_ms:1792411200000,is_current:true},projection:{state:'before_reset',basis:'observed_recent',unit:'percent',rate_per_workday:25,remaining_workdays:4,exhaustion_workdays:3,exhaustion_at_ms:1792324800000,early_workdays:1,reasons:[]},reasons:[],...patch};}
function fn(source,name){const start=source.indexOf('function '+name+'(');assert.ok(start>=0);return source.slice(start,source.indexOf('\n}',start)+2);}
const section=(source,name)=>source.split('// BEGIN '+name)[1].split('// END '+name)[0];

test('Saldo e prazo uses backend capacity, rate and timeline coordinates in both locales',()=>{
  const I=context().CodenotchInsights,w=fixture();
  const pt=I.details(w,'pt-BR'),en=I.details(w,'en');
  assert.match(pt,/width:75%/);assert.match(pt,/left:75%/);
  assert.match(pt,/18,75%/);assert.match(pt,/Ritmo observado/);assert.match(pt,/25% \/ dias úteis/);
  assert.match(en,/18.75%/);assert.match(en,/Observed rate/);
  assert.match(en,/At this rate, runs out/);assert.match(pt,/1 dias úteis antes/);
  assert.doesNotMatch(pt,/Tempo corrido|Referência do ciclo|Disponível nesta semana|pp acima/);
  assert.doesNotMatch(formatter,/86400000|duration_seconds|\.used\s*\*/,'no calendar or provider arithmetic in renderer');
});
test('Unknown, stale, partial and credit scope retain explicit qualifications without fabricated tracks',()=>{
  const I=context().CodenotchInsights;
  const unknown=I.details(fixture({period:null,work:null,budget:null,display_budget:null,projection:null,reasons:['unknown_start','calendar_missing']}),'pt-BR');
  assert.match(unknown,/Início do ciclo desconhecido/);assert.match(unknown,/Calendário indisponível/);assert.doesNotMatch(unknown,/progressbar/);
  assert.match(I.details(fixture({fresh:false,reasons:['missing_window_freshness']}),'pt-BR'),/A atualização desta conta não foi confirmada/);
  assert.match(I.details(fixture({fresh:false,reasons:['missing_window_freshness']}),'en'),/freshness of this account reading is unknown/);
  const stale=I.details(fixture({fresh:false,reasons:['stale']}),'en');
  assert.match(stale,/Outdated|outdated/);assert.doesNotMatch(stale,/progressbar|18.75/);
  const partial=I.details(fixture({work:{...fixture().work,calendar_partial:true},reasons:['calendar_partial']}),'pt-BR');
  assert.match(partial,/Feriados locais não confirmados/);
  const combined=I.details(fixture({credits:{consumed:82.79,allowance:5000,remaining:4917.21,scope:'combined'},period:null,work:null,budget:null,display_budget:null,projection:null,reasons:['unknown_credit_scope']}),'pt-BR');
  assert.match(combined,/créditos adicionais/);assert.doesNotMatch(combined,/Pode usar/);
  const credit=I.details(fixture({credits:{remaining:4917.21},display_budget:{...fixture().display_budget,unit:'credits',amount:245.8605}}),'en');
  assert.match(credit,/245.86 credits/);assert.doesNotMatch(credit,/245.86%/);
});
test('Malformed values and reason strings cannot inject markup or Infinity into formatted UI',()=>{
  const I=context().CodenotchInsights;
  const html=I.details(fixture({official_remaining_percent:NaN,work:null,period:{elapsed_percent:Infinity},budget:null,reasons:['<img src=x onerror=alert(1)>']}));
  assert.doesNotMatch(html,/NaN|Infinity|<img|progressbar/);
  assert.equal(I.esc('<img "x">'), '&lt;img &quot;x&quot;&gt;');
});
test('Provider request order, stale completions and mismatched responses cannot overwrite current values',async()=>{
  const I=context().CodenotchInsights,pending=[];
  const loader=I.loader(provider=>new Promise(resolve=>pending.push({provider,resolve})),()=>{});
  const old=loader.refresh('claude'),codex=loader.refresh('codex'),current=loader.refresh('claude');
  pending[2].resolve({provider:'claude',windows:[fixture({window_id:'new'})]});await current;
  pending[0].resolve({provider:'claude',windows:[fixture({window_id:'old'})]});await old;
  pending[1].resolve({provider:'claude',windows:[fixture()]});await codex;
  assert.equal(loader.get('claude').windows[0].window_id,'new');assert.equal(loader.get('codex'),null);
  const hidden=loader.refresh('kiro');loader.invalidate();pending[3].resolve({provider:'kiro',windows:[fixture()]});await hidden;
  assert.equal(loader.get('kiro'),null);
});
test('Claude account matches original window index, identity and group after filtering',()=>{
  const I=context().CodenotchInsights,dto={windows:[fixture({window_index:0,window_id:'weekly',group:'A'}),fixture({window_index:3,window_id:'weekly@second',group:'B'})]};
  assert.equal(I.match(dto,{id:'weekly@second',group:'B'},3).window_index,3);
  for(const [id,group,index] of [['weekly','B',3],['weekly@second','A',3],['weekly@second','B',0]])assert.equal(I.match(dto,{id,group},index),null);
  const c=context({usage:{windows:[{id:'session',group:'A'},{id:'weekly',group:'A'},{id:'session@second',group:'B'},{id:'weekly@second',group:'B'}]}});
  vm.runInContext(fn(notch,'claudeCells'),c);
  const second=c.claudeCells()[1].snap.windows[1];
  assert.equal(second.id,'weekly');assert.equal(second.insight_window_id,'weekly@second');assert.equal(second.insight_window_index,3);assert.equal(second.group,'B');
});
test('Actual notch card expands Claude, Codex and Kiro DTOs while preserving official bar and hidden activity',async()=>{
  const card={innerHTML:'',querySelector:()=>null,classList:{contains:()=>false}};
  let p={id:'codex',base:'codex',name:'Codex',snap:{status:'ok',windows:[{id:'weekly_all',label:'Weekly',used:0.25,resets_at:fixture().period.reset_ms}]}};
  const c=context({document:{getElementById:()=>card},card,uiLang:'pt-BR',providers:()=>[p],hoverId:'codex',glyphHtml:()=>'',
    staleOf:()=>false,ui:()=>({locale:'pt-BR',title:x=>x}),esc:x=>x,textCopy:x=>x,resetCopy:()=>'',tone:()=> '#fff',usedCopy:()=> '25% used',creditAmountsHtml:()=>'',offersClaudeSignIn:()=>false,placeCard:()=>{},
    selectActivityProvider:()=>{},activityPrivacyHtml:()=>'',invoke:async(_,{provider})=>({provider,windows:[fixture()]})});
  vm.runInContext(section(notch,'TESTABLE WORK PACE')+'\n'+fn(notch,'renderCard'),c);
  for(const provider of ['claude','codex','kiro']){
    p={...p,id:provider,base:provider};c.hoverId=provider;
    await c.refreshInsights(provider);c.renderCard();
    assert.match(card.innerHTML,/Cota oficial|Official quota/);assert.match(card.innerHTML,/width:25%/);
    assert.match(card.innerHTML,/Pode usar/);assert.match(card.innerHTML,/18,75%/);
  }
  p={...p,id:'codex',base:'codex',snap:{...p.snap,windows:[{...p.snap.windows[0],used:0.1}]}};c.hoverId='codex';
  c.invoke=async(_,{provider})=>({provider,windows:[fixture({official_used_percent:30,official_remaining_percent:70})]});
  await c.refreshInsights('codex');c.renderCard();
  assert.match(card.innerHTML,/width:10%/);assert.doesNotMatch(card.innerHTML,/Tempo corrido|70%|18,75%/);
  p={...p,snap:{...p.snap,windows:[{...p.snap.windows[0],used:0.25,resets_at:fixture().period.reset_ms+1}]}};
  c.invoke=async(_,{provider})=>({provider,windows:[fixture()]});await c.refreshInsights('codex');
  assert.equal(c.insightFor(p,p.snap.windows[0],0),null,'new reset cannot reuse the old budget');
  p={...p,snap:{...p.snap,windows:[{...p.snap.windows[0],resets_at:fixture().period.reset_ms,credits:{consumed:1,allowance:100},credit_scope:'plan'}]}};
  c.invoke=async(_,{provider})=>({provider,windows:[fixture({credits:{consumed:2,allowance:100,remaining:98,scope:'plan'}})]});await c.refreshInsights('codex');
  assert.equal(c.insightFor(p,p.snap.windows[0],0),null,'changed explicit credit pair cannot reuse the old budget');
  p.snap.windows[0].credits.consumed=2;
  await c.refreshInsights('codex');
  assert.ok(c.insightFor(p,p.snap.windows[0],0),'exact reading accepts the DTO');
  p.snap.windows[0].reading_status='stale';
  assert.equal(c.insightFor(p,p.snap.windows[0],0),null,'stale source account cannot display fresh derived references');
  p.snap.windows[0].reading_status='ok';p.snap.windows[0].reading_fetched_at=Date.now()+3600000;
  assert.equal(c.insightFor(p,p.snap.windows[0],0),null,'future source account freshness cannot authorize derived references');
  delete p.snap.windows[0].reading_fetched_at;
  p.snap.windows[0].credits.allowance=200;
  assert.equal(c.insightFor(p,p.snap.windows[0],0),null,'allowance change invalidates the budget');
  p={...p,id:'cursor',base:'cursor'};c.hoverId='cursor';c.renderCard();assert.doesNotMatch(card.innerHTML,/Tempo corrido/);
});
test('Initial provider reads cannot overwrite newer event readings when bootstrap promises complete late',async()=>{
  const pending=[],c=context({invoke:command=>new Promise(resolve=>pending.push({command,resolve})),renderRing(){}});
  vm.runInContext(section(notch,'TESTABLE WORK PACE'),c);
  for(const [provider,command] of [['claude','get_usage'],['codex','get_codex'],['kiro','get_kiro']]){
    let current=null;
    const initial=c.loadInitialUsage(provider,command,value=>{current=value;});
    c.acceptUsageEvent(provider,value=>{current=value;},{reading:'new'});
    pending.find(p=>p.command===command).resolve({reading:'old'});
    assert.equal(await initial,false);assert.equal(current.reading,'new');
  }
  assert.equal(pending.length,3,'rejected bootstrap does not dispatch obsolete insight requests');
});
test('Embedded panel has no direct Tauri access and focuses exact index without account-filter offsets',async()=>{
  const report={innerHTML:'',addEventListener:()=>{}},focused=[];
  const c=context({window:{},document:{documentElement:{dataset:{}},getElementById:id=>id==='report'?report:report.innerHTML.includes('id="'+id+'"')?{focus:()=>focused.push(id),scrollIntoView:()=>{}}:null},setInterval:()=>1,clearInterval:()=>{}});
  // Formatter attaches to window when running embedded.
  c.CodenotchInsights=c.window.CodenotchInsights;
  vm.runInContext([...panel.matchAll(/<script(?![^>]*src=)[^>]*>([\s\S]*?)<\/script>/g)][0][1],c);
  await c.window.CodenotchUsage.mount({get_provider_insights:async provider=>({provider,windows:[fixture({window_index:7,label:'<img src=x>',group:'Account B'})]}),open_telemetry:async()=>{}},{active:true,provider:'claude',window_index:7,language:'pt-BR'});
  assert.deepEqual(focused,['usage-claude-7']);assert.match(report.innerHTML,/&lt;img src=x&gt;/);assert.doesNotMatch(report.innerHTML,/<img src=x>/);
  await c.window.CodenotchUsage.update({active:false});await c.window.CodenotchUsage.update({active:true,provider:'claude',window_index:7});
  assert.deepEqual(focused,['usage-claude-7','usage-claude-7'],'reentry focuses the same indicator after its DOM was rebuilt');
  assert.doesNotMatch(panel,/__TAURI__|postMessage/);
  assert.match(settings,/location.href===expected.href/);assert.match(settings,/get_provider_insights:provider/);
});
test('Dates accept real ECMAScript boundaries and return unknown before Intl receives an invalid date',()=>{
  const I=context().CodenotchInsights;
  for(const value of [8800000000000000,8640000000000001,Number.MAX_SAFE_INTEGER,NaN,Infinity,null,-1])assert.equal(I.date(value,'pt-BR'),'—');
  assert.notEqual(I.date(8640000000000000,'en'),'—');
  assert.match(I.date(Date.parse('2026-10-12T12:00:00Z'),'pt-BR'),/12\/10\/2026/);
});
test('Settings route accepts bounded integer index only',()=>{
  const c=context();vm.runInContext(fn(settings,'validUsageIndex'),c);
  for(const value of [0,4095,'7'])assert.equal(c.validUsageIndex(value),Number(value));
  for(const value of [null,undefined,'',-1,4096,1.5,'1e3','<img>','00000',true])assert.equal(c.validUsageIndex(value),null);
});
test('Usage host rejects provider and iframe navigation outside its exact same-origin page',async()=>{
  const nodes={},calls=[],routes=[];let frame,bridge;
  const node=id=>nodes[id]??(nodes[id]={style:{},hidden:true,appendChild(){},addEventListener(){}});
  const c=vm.createContext({URL,Promise,Number,window:{location:{href:'http://tauri.localhost/settings.html'},__TAURI__:{event:{listen(){}}}},
    document:{documentElement:{dataset:{theme:'dark'}},getElementById:node,createElement(){
      frame={style:{},setAttribute(){},contentWindow:{location:{href:''},CodenotchUsage:{mount:async(api)=>{bridge=api;},update(){},refresh(){}}}};
      Object.defineProperty(frame,'src',{get(){return frame.contentWindow.location.href;},set(value){frame.contentWindow.location.href=value;}});return frame;
    }},curTab:'usage',uiLang:'en',historyProvider:'claude',ui:(_key,value)=>value,
    invoke:async(command,args)=>{calls.push({command,args});},navigateSettings:route=>routes.push(route)});
  vm.runInContext(section(settings,'SETTINGS USAGE'),c);
  c.ensureUsageFrame();await frame.onload();
  await bridge.get_provider_insights('codex');assert.equal(calls[0].command,'get_provider_insights');assert.equal(calls[0].args.provider,'codex');
  await assert.rejects(bridge.get_provider_insights('evil'));assert.equal(calls.length,1);
  await bridge.open_telemetry('kiro');assert.equal(routes[0].section,'history');assert.equal(routes[0].provider,'kiro');
  frame.contentWindow.location.href='http://tauri.localhost/other.html';
  await assert.rejects(bridge.get_provider_insights('codex'));await assert.rejects(bridge.open_telemetry('claude'));
  await frame.onload();assert.equal(nodes['usage-error'].hidden,false);
});
test('Panel failures become readable and inactive lifecycle ignores pending responses and stops IPC',async()=>{
  const report={innerHTML:'',addEventListener(){}},timers=new Set();let count=0;
  const c=context({window:{},document:{documentElement:{dataset:{}},getElementById:id=>id==='report'?report:null},setInterval:()=>{timers.add(1);return 1;},clearInterval:id=>timers.delete(id)});
  c.CodenotchInsights=c.window.CodenotchInsights;
  vm.runInContext([...panel.matchAll(/<script(?![^>]*src=)[^>]*>([\s\S]*?)<\/script>/g)][0][1],c);
  await c.window.CodenotchUsage.mount({get_provider_insights:async()=>{count++;throw Error('failure');},open_telemetry:async()=>{}},{active:true,language:'pt-BR'});
  assert.match(report.innerHTML,/Referências da cota indisponíveis/);assert.doesNotMatch(report.innerHTML,/failure/);assert.equal(timers.size,1);
  await c.window.CodenotchUsage.update({active:false});await c.window.CodenotchUsage.refresh();
  assert.equal(count,3);assert.equal(timers.size,0);
});

test('Observed, mean, unavailable, zero and very small early forecasts keep distinct meanings',()=>{
  const I=context().CodenotchInsights,p=fixture().projection;
  const mean=I.details(fixture({projection:{...p,basis:'cycle_mean',state:'covers_reset'}}),'pt-BR');
  assert.match(mean,/Média do ciclo/);assert.match(mean,/Sem velocidade recente medida/);assert.match(mean,/inclui dias extras/);assert.match(mean,/Neste ritmo, cobre/);assert.doesNotMatch(mean,/Ritmo observado/);
  const unknown=I.details(fixture({projection:{...p,state:'insufficient_work_time',basis:null,rate_per_workday:null,exhaustion_at_ms:null,early_workdays:null}}),'pt-BR');
  assert.match(unknown,/Ritmo indisponível/);assert.match(unknown,/um dia útil completo/);assert.doesNotMatch(unknown,/Neste ritmo|Esgota|0% \/ dias/);
  const zero=I.details(fixture({projection:{...p,state:'no_observed_increase',rate_per_workday:null}}),'en');
  assert.match(zero,/No reported increase/);assert.doesNotMatch(zero,/lasts until reset/);
  const early=I.details(fixture({projection:{...p,early_workdays:.0001}}),'pt-BR');
  assert.match(early,/menos de 0,1 dias úteis antes/);assert.doesNotMatch(early,/· 0 dias úteis antes/);
  assert.match(I.details(fixture({projection:{...p,state:'exhausted',rate_per_workday:null}}),'pt-BR'),/Limite oficial atingido/);
  assert.match(I.details(fixture({projection:{...p,state:'projected_due'}}),'pt-BR'),/aguarde atualização/);
});

test('Less than one remaining workday and extra days display the backend balance without a daily budget',()=>{
  const I=context().CodenotchInsights,w=fixture();
  for(const [mode,label] of [['until_reset','Disponível até renovar'],['extra_days_until_reset','Saldo para dias extras']]){
    const html=I.details(fixture({display_budget:{...w.display_budget,mode,amount:75},projection:null}),'pt-BR');
    assert.match(html,new RegExp(label));assert.match(html,/75%/);assert.doesNotMatch(html,/18,75|Pode usar|Ritmo observado|pace-line/);
  }
  const short=I.details(fixture({quota_role:'session',display_budget:{mode:'session_quota_reset'}}),'pt-BR');
  assert.doesNotMatch(short,/Pode usar|Ritmo|pace-line|pace-forecast/);
  const old=I.details(fixture({fresh:false,last_reading_reference:{as_of_ms:1791900000000,age_ms:1200000,unit:'percent',previous_cycle:false,display_budget:{...w.display_budget,amount:19,is_current:false}}}),'pt-BR');
  assert.match(old,/Última leitura/);assert.match(old,/20 minutos de idade/);assert.match(old,/19%/);assert.doesNotMatch(old,/Neste ritmo|18,75%/);
  const previous=I.details(fixture({fresh:false,last_reading_reference:{as_of_ms:1791900000000,age_ms:1200000,unit:'percent',previous_cycle:true,display_budget:{...w.display_budget,amount:19}}}),'pt-BR');
  assert.match(previous,/Ciclo anterior/);assert.doesNotMatch(previous,/19%/);
});

test('Claude classification uses DTO role and exact weekly identity; scoped never replaces general',()=>{
  const I=context().CodenotchInsights;
  const general=fixture({window_index:4,window_id:'weekly_all@two'}),scoped=fixture({window_index:1,window_id:'weekly_scoped@two',quota_role:'scoped',label:'Weekly (model-scoped)',official_used_percent:100});
  assert.equal(I.general([scoped,general]),general);
  assert.equal(I.general([scoped]),null);
  assert.equal(I.general([fixture({quota_role:undefined,label:'Weekly all models'})]),null);
  assert.equal(I.general([fixture({window_id:'offer',quota_role:'general'})]),null);
  assert.equal(I.accounts([general,scoped,fixture({window_id:'weekly_all@other'}),fixture({window_id:'weekly_all@two',group:'conflicting group'})]).length,3);
  const c=context({usage:{windows:[{id:'weekly_all@a',group:'Same'},{id:'weekly_all@b',group:'Same'}]}});vm.runInContext(fn(notch,'claudeCells'),c);
  assert.equal(c.claudeCells().length,2);assert.equal(c.claudeCells()[1].snap.windows[0].insight_window_index,1);
});

test('Notch rejects a forecast from a different raw account even when quota and reset are identical',async()=>{
  const raw={id:'weekly_all',group:'A',used:.25,resets_at:fixture().period.reset_ms,quota_account_key:'account-A',quota_bucket_key:'bucket-A',reading_fetched_at:Date.now(),reading_status:'ok'};
  const p={id:'claude',base:'claude',snap:{status:'ok',windows:[raw]}},c=context({providers:()=>[p],invoke:async()=>({provider:'claude',windows:[fixture({group:'A'})]}),staleOf:()=>false});
  vm.runInContext(section(notch,'TESTABLE WORK PACE'),c);await c.refreshInsights('claude');assert.ok(c.insightFor(p,raw,0));
  raw.quota_account_key='account-B';assert.equal(c.insightFor(p,raw,0),null);
  raw.quota_account_key='account-A';raw.quota_bucket_key='bucket-B';assert.equal(c.insightFor(p,raw,0),null);
  raw.quota_bucket_key='bucket-A';raw.reading_fetched_at++;assert.equal(c.insightFor(p,raw,0),null);
});

test('Claude panel keeps each account independent, opens exact scoped target and retains official blocking limits',async()=>{
  const report={innerHTML:'',addEventListener(){}},focused=[];
  const data=[fixture({window_index:0,group:'A'}),fixture({window_index:1,group:'A',window_id:'session',quota_role:'session',display_budget:{mode:'session_quota_reset'},projection:null}),fixture({window_index:7,group:'B',window_id:'weekly_scoped@B',quota_role:'scoped',label:'Weekly (model-scoped)',official_used_percent:100}),fixture({window_index:8,group:'B',window_id:'weekly_all@B'})];
  const c=context({window:{},document:{documentElement:{dataset:{}},getElementById:id=>id==='report'?report:report.innerHTML.includes('id="'+id+'"')?{focus:()=>focused.push(id),scrollIntoView(){}}:null},setInterval:()=>1,clearInterval(){}});c.CodenotchInsights=c.window.CodenotchInsights;
  vm.runInContext([...panel.matchAll(/<script(?![^>]*src=)[^>]*>([\s\S]*?)<\/script>/g)][0][1],c);
  await c.window.CodenotchUsage.mount({get_provider_insights:async provider=>({provider,windows:provider==='claude'?data:[]}),open_telemetry:async()=>{}},{active:true,provider:'claude',window_index:7,language:'pt-BR'});
  assert.deepEqual(focused,['usage-claude-7']);assert.equal((report.innerHTML.match(/class="summary-account"/g)||[]).length,2);
  assert.match(report.innerHTML,/<summary>Weekly \(model-scoped\) · 100% usado · Limite oficial atingido/);
  assert.match(report.innerHTML,/<details class="quota-detail"[^>]+ open>/);assert.doesNotMatch(report.innerHTML,/Opus|Fable|activity-details|quota consolidada/);
  assert.equal((report.innerHTML.match(/aria-label="Cota oficial"/g)||[]).length,2,'only general quota has a full official bar per account');
  assert.equal((report.innerHTML.match(/id="usage-claude-7"/g)||[]).length,1);
});

test('Native disclosure and timeline footer labels preserve keyboard semantics and avoid marker text collisions',()=>{
  assert.match(notch,/e\.target\.closest\('details,summary'\)/);assert.match(notch,/resetQuotaDetails\(\);card\.innerHTML/);
  assert.match(panel,/expanded\.clear\(\);accountSignatures\.clear/);
  assert.match(formatter,/<details class="quota-evidence"/);assert.match(formatter,/<summary>/);
  assert.match(notch,/pace-timeline-labels[^\n]*flex-wrap:wrap/);assert.match(panel,/pace-timeline-labels[^\n]*flex-wrap:wrap/);
  const I=context().CodenotchInsights;
  for(const exhaustion of [.001,2,3.999]){const html=I.details(fixture({projection:{...fixture().projection,exhaustion_workdays:exhaustion}}));assert.match(html,/pace-stem/);assert.match(html,/pace-timeline-labels/);assert.doesNotMatch(html,/pace-marker/);}
});

test('Quota disclosure toggle survives refresh and closes on account change or inactive lifecycle',async()=>{
  const decode=s=>s.replace(/&quot;/g,'"').replace(/&amp;/g,'&');
  const report={innerHTML:'',addEventListener(){},querySelectorAll(selector){
    if(selector!=='[data-quota-key]')return [];
    const nodes=[...this.innerHTML.matchAll(/<details class="quota-detail" data-quota-key="([^"]*)"([^>]*)>/g)].map(m=>({dataset:{quotaKey:decode(m[1])},open:/\bopen\b/.test(m[2])}));
    this.details=nodes;return nodes;
  }};
  let group='A';
  const c=context({window:{},document:{documentElement:{dataset:{}},getElementById:id=>id==='report'?report:null},setInterval:()=>1,clearInterval(){}});c.CodenotchInsights=c.window.CodenotchInsights;
  vm.runInContext([...panel.matchAll(/<script(?![^>]*src=)[^>]*>([\s\S]*?)<\/script>/g)][0][1],c);
  await c.window.CodenotchUsage.mount({get_provider_insights:async provider=>({provider,windows:provider==='claude'?[fixture({group}),fixture({group,window_index:1,window_id:'weekly_scoped',quota_role:'scoped'})]:[]}),open_telemetry:async()=>{}},{active:true});
  assert.equal(report.details[0].open,false);
  report.details[0].open=true;report.details[0].ontoggle();
  await c.window.CodenotchUsage.refresh();assert.equal(report.details[0].open,true);
  group='B';await c.window.CodenotchUsage.refresh();assert.equal(report.details[0].open,false);
  report.details[0].open=true;report.details[0].ontoggle();
  await c.window.CodenotchUsage.update({active:false});await c.window.CodenotchUsage.update({active:true});assert.equal(report.details[0].open,false);
});

test('Quota click stops propagation and never invokes activity revelation',async()=>{
  const card={innerHTML:'',querySelector:()=>null,classList:{contains:()=>false}},p={id:'claude',base:'claude',name:'Claude',snap:{status:'ok',windows:[{id:'weekly_all',label:'Weekly',used:.25,resets_at:fixture().period.reset_ms}]}},calls=[];
  const c=context({document:{getElementById:()=>card},card,uiLang:'en',providers:()=>[p],hoverId:'claude',glyphHtml:()=>'',staleOf:()=>false,ui:()=>({title:x=>x}),esc:x=>x,textCopy:x=>x,resetCopy:()=>'',tone:()=> '#fff',usedCopy:()=> '25% used',creditAmountsHtml:()=>'',offersClaudeSignIn:()=>false,placeCard(){},selectActivityProvider(){},activityPrivacyHtml:()=>'',activityPrivacyClick:()=>calls.push('activity'),invoke:async()=>({provider:'claude',windows:[fixture()]})});
  vm.runInContext(section(notch,'TESTABLE WORK PACE')+'\n'+fn(notch,'renderCard'),c);await c.refreshInsights('claude');c.renderCard();
  let stopped=false;card.onclick({target:{closest:()=>({tagName:'SUMMARY'})},stopPropagation(){stopped=true;}});
  assert.equal(stopped,true);assert.deepEqual(calls,[]);
  card.onclick({target:{closest:()=>null}});assert.deepEqual(calls,['activity']);
});

test('Explicit backend credit percentages accompany capacity and rate without UI conversion',()=>{
  const I=context().CodenotchInsights,w=fixture(),display={...w.display_budget,unit:'credits',amount:200,amount_percent:5},projection={...w.projection,unit:'credits',rate_per_workday:300,rate_percent:7.5};
  const html=I.details(fixture({credits:{scope:'plan',allowance:4000,remaining:800,consumed:3200},display_budget:display,projection}),'pt-BR');
  assert.match(html,/200 créditos \(5%\)/);assert.match(html,/300 créditos \(7,5%\)/);
  const noProof=I.details(fixture({credits:{scope:'combined',allowance:4000,remaining:800,consumed:3200},display_budget:{...display,amount_percent:null},projection:{...projection,rate_percent:null}}),'en');
  assert.match(noProof,/200 credits/);assert.match(noProof,/300 credits/);assert.doesNotMatch(noProof,/credits \(/);
  const malformed=I.details(fixture({display_budget:{...display,amount_percent:NaN},projection:{...projection,rate_percent:Infinity}}),'en');assert.doesNotMatch(malformed,/NaN|Infinity|credits \(/);
  const percent=I.details(fixture({display_budget:{...w.display_budget,amount_percent:18.75},projection:{...w.projection,rate_percent:25}}),'en');assert.doesNotMatch(percent,/18.75% \(|25% \(/);
  const last=I.details(fixture({fresh:false,last_reading_reference:{as_of_ms:1791900000000,age_ms:1200000,unit:'credits',previous_cycle:false,display_budget:{...display,is_current:false}}}),'pt-BR');
  assert.match(last,/Última leitura/);assert.match(last,/200 créditos \(5%\)/);assert.doesNotMatch(last,/300 créditos|Neste ritmo/);
  assert.doesNotMatch(formatter,/allowance\s*[*/]|[*/]\s*[^;]*allowance/,'renderer never derives percentages from allowance');
});

test('Notch RPC carries source proof and rejects a backend account swap before applying identical-value quota',async()=>{
  const fetched=Date.now(),raw={id:'weekly_all@second',group:'Account',used:.25,resets_at:fixture().period.reset_ms,quota_account_key:'A',quota_bucket_key:'bucket',reading_transport:'claude_usage_http',reset_basis:'absolute',reading_fetched_at:fetched,reading_status:'ok',insight_window_id:'weekly_all@second',insight_window_index:7};
  const p={id:'claude@second',base:'claude',snap:{status:'ok',fetched_at:fetched,windows:[raw]}},calls=[];
  let backendAccount='B';
  const c=context({providers:()=>[p],invoke:async(command,args)=>{
    calls.push({command,args});const expected=args.expectedSources[0];
    if(expected.account_key!==backendAccount)throw Error('Usage changed');
    return {provider:'claude',windows:[fixture({window_index:7,window_id:raw.id,group:raw.group})]};
  },staleOf:()=>false});
  vm.runInContext(section(notch,'TESTABLE WORK PACE'),c);await c.refreshInsights('claude');
  assert.equal(c.insightFor(p,raw,0),null);assert.equal(vm.runInContext("insightLoader.failed('claude')",c),true);
  const proof=JSON.parse(JSON.stringify(calls[0].args.expectedSources[0]));
  assert.deepEqual(proof,{window_index:7,window_id:'weekly_all@second',group:'Account',account_key:'A',bucket_key:'bucket',reading_transport:'claude_usage_http',reset_basis:'absolute',fetched_at:fetched,status:'ok'});
  backendAccount='A';await c.refreshInsights('claude');assert.ok(c.insightFor(p,raw,0));
});

test('Ambiguous legacy Claude origin sends null stamps, while single ungrouped origin uses its header',async()=>{
  const calls=[];let p={id:'claude@second',base:'claude',snap:{fetched_at:100,status:'ok',windows:[{id:'weekly_all',insight_window_id:'weekly_all@second',insight_window_index:3,group:'Account'}]}};
  const c=context({providers:()=>[p],invoke:async(_command,args)=>{calls.push(args);return {provider:'claude',windows:[]};}});
  vm.runInContext(section(notch,'TESTABLE WORK PACE'),c);await c.refreshInsights('claude');
  assert.equal(calls[0].expectedSources[0].fetched_at,null);assert.equal(calls[0].expectedSources[0].status,null);
  p={id:'claude',base:'claude',snap:{fetched_at:100,status:'ok',windows:[{id:'weekly_all'}]}};await c.refreshInsights('claude');
  assert.equal(calls[1].expectedSources[0].fetched_at,100);assert.equal(calls[1].expectedSources[0].status,'ok');assert.equal(calls[1].expectedSources[0].account_key,null);
  p.snap.windows[0].reading_status='stale';await c.refreshInsights('claude');assert.equal(calls[2].expectedSources[0].fetched_at,null);assert.equal(calls[2].expectedSources[0].status,null);
});
