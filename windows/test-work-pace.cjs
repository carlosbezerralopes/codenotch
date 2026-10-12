const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const assert=require('node:assert/strict');
const {test}=require('node:test');
const notch=fs.readFileSync(path.join(__dirname,'codenotch/ui/notch.html'),'utf8');
const settings=fs.readFileSync(path.join(__dirname,'codenotch/ui/settings.html'),'utf8');
const form=settings.split('// BEGIN TESTABLE CALENDAR FORM')[1].split('// END TESTABLE CALENDAR FORM')[0];
const defaultCalendar=()=>({country:'BR',state:'CE',city:'Fortaleza',utc_offset_minutes:-180,overrides:[],covered_years:[2025,2026,2027],
  holidays:[{date:'2026-10-12',name:'Nossa Senhora Aparecida'},{date:'2027-01-01',name:'Confraternização Universal'}],
  source_coverage:{national:true,state:true,municipal:true}});
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

// Calendar arithmetic fixtures moved to usage_insights.rs (same partial-day,
// holiday, year-boundary, coverage, freshness and unknown-cycle cases).
// This suite retains calendar editing behavior; DTO presentation is covered by
// test-usage-insights-ui.cjs, without recomputing the Rust engine in JavaScript.
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
