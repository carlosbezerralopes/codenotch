const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const assert=require('node:assert/strict');
const {test}=require('node:test');
const notch=fs.readFileSync(path.join(__dirname,'codenotch/ui/notch.html'),'utf8');
const settings=fs.readFileSync(path.join(__dirname,'codenotch/ui/settings.html'),'utf8');
const pace=notch.split('// BEGIN TESTABLE WORK PACE')[1].split('// END TESTABLE WORK PACE')[0];
const form=settings.split('// BEGIN TESTABLE CALENDAR FORM')[1].split('// END TESTABLE CALENDAR FORM')[0];
const DAY=86400000;
const utc=value=>Date.parse(value);
const defaultCalendar=()=>({country:'BR',state:'CE',city:'Fortaleza',utc_offset_minutes:-180,overrides:[],covered_years:[2025,2026,2027],
  holidays:[{date:'2026-10-12',name:'Nossa Senhora Aparecida'},{date:'2027-01-01',name:'Confraternização Universal'}],
  source_coverage:{national:true,state:true,municipal:true}});
function context(lang='pt-BR'){
  const c=vm.createContext({uiLang:lang,Intl,ui:()=>({locale:lang==='pt-BR'?'pt-BR':'en-US'}),
    esc:value=>String(value).replace(/[&<>"']/g,ch=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[ch]))});
  vm.runInContext(pace,c);
  return c;
}
function sample(start,now,used=0.25){
  return {window:{id:'primary',label:'Vendor label',used,duration_seconds:604800,resets_at:start+7*DAY},snapshot:{status:'ok',fetched_at:now},now};
}
function compute(c,fixture,calendar=defaultCalendar()){
  return c.computePace(fixture.window,fixture.snapshot,fixture.now,calendar);
}
function fn(source,name){
  const start=source.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert.ok(start>=0,name);
  return source.slice(start,source.indexOf('\n}',start)+2);
}
function formContext(){
  const c=vm.createContext({calendarSettings:defaultCalendar(),calendarDraft:{...defaultCalendar()},calendarBusy:false,calendarOperation:'',
    calendarVersion:0,calendarDirty:false,calendarReady:true,calendarMessage:'',renderCalendar:()=>{},errText:String});
  vm.runInContext(form+['cancelCalendarEdits','detectCalendarSettings','saveCalendarSettings'].map(name=>fn(settings,name)).join('\n'),c);
  return c;
}

test('The weekend and Monday holiday advance calendar time 40% while business time stays at zero',()=>{
  const c=context(),start=utc('2026-10-10T03:00:00Z'),now=start+2.8*DAY;
  const p=compute(c,sample(start,now));
  assert.equal(p.elapsed,0.4);
  assert.equal(p.business,0);
  assert.equal(p.total_days,4);
  assert.equal(p.gap_pp,25);
  assert.match(c.workPaceHtml(p),/0 \/ 4 dias/);
  assert.match(c.workPaceHtml(p),/25 pp acima/);
});

test('Friday partial work and Tuesday reopening use actual seconds rather than arbitrary weekend weights',()=>{
  const c=context(),start=utc('2026-10-09T21:00:00Z');
  const sunday=compute(c,sample(start,utc('2026-10-11T15:00:00Z')));
  const tuesday=compute(c,sample(start,utc('2026-10-13T03:00:00Z')));
  assert.equal(sunday.elapsed_days,0.25);
  assert.equal(tuesday.elapsed_days,0.25);
  assert.ok(tuesday.elapsed>sunday.elapsed);
  assert.equal(tuesday.business,0.0625);
  const followingMorning=compute(c,sample(start,utc('2026-10-13T09:00:00Z')));
  assert.equal(followingMorning.elapsed_days,0.5);
});

test('Partial first/last days and a partial current day contribute only their overlapping seconds',()=>{
  const c=context(),start=utc('2026-10-13T15:00:00Z'),now=utc('2026-10-14T09:00:00Z');
  const p=compute(c,sample(start,now));
  assert.equal(p.total_days,5);
  assert.equal(p.elapsed_days,0.75);
  assert.equal(p.business,0.15);
});

test('A cycle across years excludes New Year using the configured offset, independent of the host timezone',()=>{
  const c=context(),start=utc('2026-12-30T03:00:00Z'),now=utc('2027-01-04T03:00:00Z');
  const p=compute(c,sample(start,now));
  assert.equal(p.total_days,4);
  assert.equal(p.elapsed_days,2);
  assert.equal(p.business,0.5);
  const shifted={...defaultCalendar(),utc_offset_minutes:0};
  const boundary=utc('2027-01-03T23:00:00Z');
  const other=compute(c,sample(start,boundary),shifted);
  assert.notEqual(other.elapsed_days,compute(c,sample(start,boundary)).elapsed_days,'partial local days follow the selected offset');
});

test('No business-time denominator and unavailable calendars are unknown, never fabricated zero percentages',()=>{
  const c=context(),start=utc('2026-10-10T03:00:00Z'),fixture=sample(start,start+DAY);
  const allOff=defaultCalendar();
  allOff.overrides=['13','14','15','16'].map(day=>({date:'2026-10-'+day,name:'Closed',nonworking:true}));
  const p=compute(c,fixture,allOff);
  assert.equal(p.business,null);
  assert.equal(p.reason,'noWorkdays');
  assert.match(c.workPaceHtml(p),/Não há tempo útil/);
  assert.equal((c.workPaceHtml(p).match(/role="progressbar"/g)||[]).length,1,'only known elapsed time has a track');
  assert.equal(compute(c,fixture,null).business,null);
  assert.equal(compute(c,fixture,{...defaultCalendar(),covered_years:[2025]}).reason,'outsideCoverage');
});

test('Only fresh, live, provider-reported six-to-eight-day intervals get pace bars regardless of their IDs',()=>{
  const c=context(),start=utc('2026-10-10T03:00:00Z'),fixture=sample(start,start+DAY,0);
  assert.equal(compute(c,fixture).gap_pp,0,'known zero usage remains zero');
  for(const patch of [{duration_seconds:null},{duration_seconds:undefined},{duration_seconds:300*60},{duration_seconds:30*86400},
    {used:null},{used:NaN},{resets_at:null},{resets_at:fixture.now},{resets_at:fixture.now-1}]){
    assert.equal(compute(c,{...fixture,window:{...fixture.window,id:'secondary',label:'Weekly',...patch}}),null);
  }
  for(const status of ['stale','backoff','error']) assert.equal(compute(c,{...fixture,snapshot:{...fixture.snapshot,status}}),null);
  assert.equal(compute(c,{...fixture,snapshot:{status:'ok',fetched_at:fixture.now-900001}}),null);
  assert.equal(compute(c,{...fixture,snapshot:{status:'ok',fetched_at:0}}),null);
  assert.equal(compute(c,{...fixture,now:start-1}),null);
  assert.ok(compute(c,{...fixture,window:{...fixture.window,id:'arbitrary',label:'Monthly'}}),'duration, not a name, identifies the week');
});

test('Custom weekday holiday overrides work, weekends stay extra, and partial local coverage is disclosed',()=>{
  const c=context(),start=utc('2026-10-10T03:00:00Z'),now=utc('2026-10-13T03:00:00Z');
  const calendar=defaultCalendar();
  calendar.overrides=[{date:'2026-10-12',name:'We work',nonworking:false},{date:'2026-10-11',name:'Sunday override',nonworking:false}];
  calendar.source_coverage={national:true,state:false,municipal:false};
  const p=compute(c,sample(start,now),calendar);
  assert.equal(p.elapsed_days,1);
  assert.equal(p.total_days,5);
  assert.equal(p.partial,true);
  assert.match(c.workPaceHtml(p),/calendário parcial|Feriados locais não confirmados/);
  assert.match(c.workPaceHtml(p),/consumo em fins de semana/);
  const en=context('en');
  assert.match(en.workPaceHtml(p),/Business days · partial calendar/);
});

test('Location/date input validation rejects bad codes, missing offsets, nonexistent dates and controls',()=>{
  const c=formContext(),draft=c.calendarInputs(defaultCalendar());
  assert.equal(c.validCalendarDraft(draft),true);
  for(const patch of [{country:'Brazil'},{state:'XX'},{city:''},{city:'Fortaleza\nSecret'},
    {utc_offset_minutes:NaN},{utc_offset_minutes:1000},{overrides:[{date:'2026-02-30',name:'Closed',nonworking:true}]}]){
    assert.equal(c.validCalendarDraft({...draft,...patch}),false);
  }
  assert.throws(()=>c.suggestedCalendar(draft,{country:'BR',state:'CE',city:'Fortaleza',utc_offset_minutes:null}));
});

test('IP detection is explicit, suggests only, and cancellation ignores an outstanding response',async()=>{
  const c=formContext(),calls=[];
  let complete;
  c.invoke=(command,args)=>{calls.push([command,args]);return new Promise(resolve=>{complete=resolve;});};
  assert.equal(calls.length,0,'initializing the form performs no IP lookup');
  const request=c.detectCalendarSettings();
  assert.equal(calls[0][0],'detect_calendar_location');
  c.cancelCalendarEdits();
  complete({country:'US',state:'CA',city:'San Francisco',utc_offset_minutes:-420});
  await request;
  assert.equal(c.calendarDraft.city,'Fortaleza');
  assert.equal(c.calendarBusy,false);
  c.invoke=async command=>{calls.push([command]);return {country:'US',state:'CA',city:'San Francisco',utc_offset_minutes:-420};};
  await c.detectCalendarSettings();
  assert.equal(c.calendarDraft.city,'San Francisco');
  assert.equal(c.calendarSettings.city,'Fortaleza','detection never applies or saves');
  assert.ok(calls.every(([command])=>command==='detect_calendar_location'));
});

test('IP and save failures preserve the draft, while applying a calendar requires confirmed save',async()=>{
  const c=formContext();
  c.calendarDraft.city='Sobral';
  c.invoke=async()=>{throw new Error('Service unavailable');};
  await c.detectCalendarSettings();
  assert.equal(c.calendarDraft.city,'Sobral');
  await c.saveCalendarSettings();
  assert.equal(c.calendarSettings.city,'Fortaleza');
  assert.equal(c.calendarDraft.city,'Sobral');
  assert.equal(c.calendarBusy,false);
  let complete;
  c.invoke=(command,args)=>new Promise(resolve=>{complete=()=>resolve({...args.settings,holidays:[],covered_years:[2026],source_coverage:{municipal:false}});});
  const save=c.saveCalendarSettings();
  c.cancelCalendarEdits();
  assert.equal(c.calendarOperation,'save','an already dispatched save cannot be mistaken for cancelled edits');
  complete();await save;
  assert.equal(c.calendarSettings.city,'Sobral');
  assert.equal(c.calendarOperation,'');
});

test('The actual Codex weekly card renders the three tracks and keeps other/stale cards unchanged',()=>{
  const c=context(),now=Date.now();
  let p={id:'codex',base:'codex',name:'Codex',snap:{status:'ok',fetched_at:now,windows:[
    {id:'primary',label:'Primary',used:0.25,duration_seconds:604800,resets_at:now+5*DAY},
  ]}};
  const card={innerHTML:'',querySelector:()=>null};
  Object.assign(c,{document:{getElementById:()=>card},providers:()=>[p],hoverId:'codex',glyphHtml:()=>'',
    ui:()=>({locale:'pt-BR',title:name=>name,andMore:count=>String(count)}),
    staleOf:()=>false,textCopy:value=>value,resetCopy:()=>'',tone:()=> '#fff',usedCopy:()=> '25% used',creditAmountsHtml:()=>'',
    offersClaudeSignIn:()=>false,activity:[],stateSnap:{agg:'running',sessions:[{state:'running',title:'SecretProject/SecretPrompt'}]},
    STATE_DOT:{},TRACK:'#666',INK:'#fff',WATCH:'#aa0',SESSION_ROWS:5,moreRow:()=>'',placeCard:()=>{},
    calendarFixture:{...defaultCalendar(),covered_years:[new Date(now).getUTCFullYear()-1,new Date(now).getUTCFullYear(),new Date(now).getUTCFullYear()+1]}});
  const privacy=notch.split('// BEGIN TESTABLE ACTIVITY PRIVACY')[1].split('// END TESTABLE ACTIVITY PRIVACY')[0];
  vm.runInContext(privacy+'\n'+fn(notch,'renderCard')+'\nworkCalendar=calendarFixture;',c);
  c.renderCard();
  assert.match(card.innerHTML,/Cota oficial/);
  assert.match(card.innerHTML,/Tempo corrido · derivado/);
  assert.match(card.innerHTML,/Dias úteis · derivado/);
  assert.equal((card.innerHTML.match(/class="(?:w-track|pace-track)"/g)||[]).length,3);
  assert.match(card.innerHTML,/width:25%/);
  assert.doesNotMatch(card.innerHTML,/SecretProject|SecretPrompt/);
  for(const base of ['claude','kiro','cursor']){
    p={...p,id:base,base};c.renderCard();
    assert.equal((card.innerHTML.match(/class="(?:w-track|pace-track)"/g)||[]).length,1);
    assert.doesNotMatch(card.innerHTML,/Tempo corrido · derivado|Dias úteis · derivado/);
    assert.doesNotMatch(card.innerHTML,/SecretProject|SecretPrompt/);
  }
  p={...p,id:'codex',base:'codex',snap:{...p.snap,status:'stale'}};c.renderCard();
  assert.equal((card.innerHTML.match(/class="(?:w-track|pace-track)"/g)||[]).length,1);
});
