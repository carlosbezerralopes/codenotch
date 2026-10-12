/* Formatting only: quota, calendar and budget arithmetic belongs to the Rust DTO. */
(function(root){
  'use strict';
  const supported=provider=>['claude','codex','kiro'].includes(provider);
  const finite=value=>typeof value==='number'&&Number.isFinite(value);
  const esc=value=>String(value??'').replace(/[&<>"']/g,ch=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[ch]));
  const TEXT={
    en:{official:'Official quota',elapsed:'Elapsed time · derived',business:'Business days · derived',partial:'Business days · partial calendar',remaining:'Remaining',until:'until reset',days:'business days',daily:'Available per remaining business day',week:'Available this week until reset',baseline:'Cycle reference per business day',weekBaseline:'Cycle reference for this week',above:'pp above business time in this cycle',below:'pp below business time in this cycle',equal:'Usage matches business time in this cycle',reference:'Derived reference; the provider defines the official quota.',stale:'Reading is outdated. Derived references are unavailable.',missing:'Derived reference unavailable.',calendar_missing:'Calendar unavailable. Check the location in Settings.',calendar_outside_coverage:'Calendar does not cover this interval.',calendar_partial:'Local holidays are not confirmed.',unknown_start:'Cycle start is unknown; cycle comparison is unavailable.',unknown_credit_scope:'The balance may include add-on credits; monthly references are unavailable.',nonrenewing_wallet:'Bonus and overage are separate from the monthly quota.',no_remaining_workdays:'No business time remains before reset.',no_workdays:'No business time in this cycle.',missing_reset:'Reset date unavailable.',reset_expired:'Waiting for a new reset date.',invalid_period:'Cycle interval unavailable.',invalid_reading:'Quota reading unavailable.',missing_reading:'Quota reading unavailable.',missing_window_freshness:'The freshness of this account reading is unknown. Derived references are unavailable.',credits:'credits',history:'History',used:'used',loading:'Loading quota references…',error:'Quota references are unavailable.'},
    'pt-BR':{official:'Cota oficial',elapsed:'Tempo corrido · derivado',business:'Dias úteis · derivado',partial:'Dias úteis · calendário parcial',remaining:'Saldo',until:'até a renovação',days:'dias úteis',daily:'Disponível por dia útil restante',week:'Disponível nesta semana até a renovação',baseline:'Referência do ciclo por dia útil',weekBaseline:'Referência do ciclo nesta semana',above:'pp acima do tempo útil neste ciclo',below:'pp abaixo do tempo útil neste ciclo',equal:'Consumo igual ao tempo útil neste ciclo',reference:'Referência derivada; o provedor define a cota oficial.',stale:'Leitura desatualizada. Referências derivadas indisponíveis.',missing:'Referência derivada indisponível.',calendar_missing:'Calendário indisponível. Confira a localização nas Configurações.',calendar_outside_coverage:'O calendário não cobre este intervalo.',calendar_partial:'Feriados locais não confirmados.',unknown_start:'Início do ciclo desconhecido; comparação do ciclo indisponível.',unknown_credit_scope:'O saldo pode incluir créditos adicionais; referências mensais indisponíveis.',nonrenewing_wallet:'Bônus e uso excedente são separados da cota mensal.',no_remaining_workdays:'Não há tempo útil restante antes da renovação.',no_workdays:'Não há tempo útil neste ciclo.',missing_reset:'Data de renovação indisponível.',reset_expired:'Aguardando nova data de renovação.',invalid_period:'Intervalo do ciclo indisponível.',invalid_reading:'Leitura da cota indisponível.',missing_reading:'Leitura da cota indisponível.',missing_window_freshness:'A atualização desta conta não foi confirmada. Referências derivadas indisponíveis.',credits:'créditos',history:'Histórico',used:'usado',loading:'Carregando referências da cota…',error:'Referências da cota indisponíveis.'}
  };
  const words=language=>TEXT[language]||TEXT.en;
  Object.assign(TEXT.en,{available:'Available / business day',untilAvailable:'Available until reset',extrasAvailable:'Balance for extra days until reset',observed:'Observed rate',mean:'Cycle average',rateMissing:'Rate unavailable',runsOut:'At this rate, runs out',before:'business days before reset',covers:'At this rate, lasts until reset',exhausted:'Official limit reached',due:'At this rate, the balance would already be exhausted; wait for an update.',renew:'Resets',out:'Runs out',today:'Reading',less:'less than 0.1',last:'Last reading',previous:'Previous cycle',minutes:'minutes old',quotaDetails:'Quota details',generalMissing:'General weekly quota not reported',insufficient_work_time:'Not yet one full business day to estimate a rate.',insufficient_history:'Not enough history to estimate a rate.',identity_unknown:'Recent rate unavailable for this source.',no_observed_increase:'No reported increase in the interval.',no_remaining_work_time:'No business time remains before reset.',history_incomplete:'History is incomplete; rate unavailable.',history_warming:'History is loading; rate unavailable.',history_truncated:'History limit reached; rate unavailable.',history_unavailable:'History unavailable.',history_conflict:'Conflicting readings; rate unavailable.',history_revision:'Revised reading; waiting for more history.',insufficient_coverage:'Insufficient history coverage.',no_recent_rate_measured:'No recent rate measured.',cycle_mean_includes_extras:'Cycle average includes extra days.',mixed_or_extra_consumption:'Consumption in extra days is not projected.',relative_reset:'Recent rate needs a confirmed cycle.',missing_origin:'Reading time unavailable.',unsupported_window:'Projection unavailable for this limit.',unsupported_credit_scope:'Monthly projection unavailable for these credits.',sampling:'Samples',coverage:'coverage'});
  Object.assign(TEXT['pt-BR'],{available:'Pode usar / dia útil',untilAvailable:'Disponível até renovar',extrasAvailable:'Saldo para dias extras até renovar',observed:'Ritmo observado',mean:'Média do ciclo',rateMissing:'Ritmo indisponível',runsOut:'Neste ritmo, esgota',before:'dias úteis antes da renovação',covers:'Neste ritmo, cobre até a renovação',exhausted:'Limite oficial atingido',due:'Pelo ritmo, o saldo já teria esgotado; aguarde atualização.',renew:'Renova',out:'Esgota',today:'Leitura',less:'menos de 0,1',last:'Última leitura',previous:'Ciclo anterior',minutes:'minutos de idade',quotaDetails:'Detalhes da cota',generalMissing:'Cota semanal geral não informada',insufficient_work_time:'Ainda não há um dia útil completo para medir ritmo.',insufficient_history:'Histórico insuficiente para medir ritmo.',identity_unknown:'Ritmo recente indisponível para esta fonte.',no_observed_increase:'Sem aumento reportado no intervalo.',no_remaining_work_time:'Não há tempo útil restante antes da renovação.',history_incomplete:'Histórico incompleto; ritmo indisponível.',history_warming:'Histórico carregando; ritmo indisponível.',history_truncated:'Limite do histórico atingido; ritmo indisponível.',history_unavailable:'Histórico indisponível.',history_conflict:'Leituras divergentes; ritmo indisponível.',history_revision:'Leitura revisada; aguardando mais histórico.',insufficient_coverage:'Cobertura de histórico insuficiente.',no_recent_rate_measured:'Sem velocidade recente medida.',cycle_mean_includes_extras:'Média do ciclo inclui dias extras.',mixed_or_extra_consumption:'Consumo em dias extras não projetado.',relative_reset:'Ritmo recente exige ciclo confirmado.',missing_origin:'Horário da leitura indisponível.',unsupported_window:'Projeção indisponível para este limite.',unsupported_credit_scope:'Projeção mensal indisponível para estes créditos.',sampling:'Amostras',coverage:'cobertura'});
  const locale=language=>language==='pt-BR'?'pt-BR':'en-US';
  function number(value,language,max=2){return finite(value)?new Intl.NumberFormat(locale(language),{maximumFractionDigits:max}).format(value):'—';}
  function date(value,language){
    if(!Number.isSafeInteger(value)||value<=0)return '—';
    const instant=new Date(value);
    return Number.isFinite(instant.getTime())?new Intl.DateTimeFormat(locale(language),{dateStyle:'short',timeStyle:'short'}).format(instant):'—';
  }
  function amount(value,unit,language){return number(value,language)+(unit==='credits'?' '+words(language).credits:'%');}
  function track(label,percent,copy){
    if(!finite(percent)||percent<0||percent>100)return '';
    return `<div class="pace-label"><span>${esc(label)}</span><span>${esc(copy)}</span></div><div class="pace-track" role="progressbar" aria-label="${esc(label)}" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${percent.toFixed(2)}"><div class="pace-fill" style="width:${percent.toFixed(2)}%"></div></div>`;
  }
  function row(label,value){return `<div class="pace-label"><span>${esc(label)}</span><span>${esc(value)}</span></div>`;}
  function role(w){return ['session','general','scoped','other'].includes(w?.quota_role)?w.quota_role:'other';}
  function key(w){return JSON.stringify([w.window_index,w.window_id,w.group||null]);}
  function accounts(windows){
    const groups=new Map();
    for(const w of windows){const id=String(w.window_id||''),at=id.indexOf('@'),account=JSON.stringify([at<0?'':id.slice(at+1),w.group||null]);if(!groups.has(account))groups.set(account,[]);groups.get(account).push(w);}
    return [...groups].map(([id,windows])=>({id,windows}));
  }
  function general(windows){
    const priority=['seven_day','weekly_all','weekly'];
    return windows.filter(w=>role(w)==='general'&&w.period_kind==='weekly'&&priority.includes(String(w.window_id).split('@')[0])).sort((a,b)=>priority.indexOf(String(a.window_id).split('@')[0])-priority.indexOf(String(b.window_id).split('@')[0])||a.window_index-b.window_index)[0]||null;
  }
  function qualify(w,language){
    const t=words(language),p=w.projection;
    const reasons=new Set([...(w.reasons||[]),...(p?.reasons||[])]);
    if(w.work?.calendar_partial)reasons.add('calendar_partial');
    if(!w.fresh)reasons.add('stale');
    if(p?.extra_or_mixed_consumption)reasons.add('mixed_or_extra_consumption');
    if(p?.basis==='cycle_mean'){reasons.add('cycle_mean_includes_extras');reasons.add('no_recent_rate_measured');}
    return [...reasons].filter(r=>t[r]).map(r=>`<div class="pace-note">${esc(t[r])}</div>`).join('');
  }
  function compact(w,language='en'){
    const t=words(language);
    return row(w.label,number(w.official_used_percent,language,1)+'% '+t.used)+(w.period?row(t.renew,date(w.period.reset_ms,language)):'')+qualify(w,language);
  }
  function pairedAmount(value,unit,percent,language){return amount(value,unit,language)+(unit==='credits'&&finite(percent)?' ('+amount(percent,'percent',language)+')':'');}
  function displayAmount(b,language){return pairedAmount(b.amount,b.unit,b.amount_percent,language);}
  function timeline(p,b,language){
    const t=words(language),remaining=p.remaining_workdays;
    if(!finite(remaining)||remaining<=0||!['before_reset','covers_reset','projected_due'].includes(p.state))return '';
    const out=['before_reset','projected_due'].includes(p.state)&&finite(p.exhaustion_workdays);
    // Only geometry: both useful-time coordinates are computed by Rust.
    const position=out?Math.max(0,Math.min(100,p.exhaustion_workdays/remaining*100)):100;
    const outCopy=t.out+' · '+date(p.exhaustion_at_ms,language),resetCopy=t.renew+' · '+date(b.horizon_end_ms,language);
    return `<div class="pace-timeline" role="img" aria-label="${esc((out?outCopy+'; ':'')+resetCopy)}"><div class="pace-line"><div class="pace-line-fill" style="width:${position}%"></div>${out?`<span class="pace-stem" style="left:${position}%"></span>`:''}<span class="pace-stem pace-stem-end"></span></div><div class="pace-axis"><span>${esc(t.today)}</span><span>${esc(number(remaining,language)+' '+t.days)}</span></div><div class="pace-timeline-labels">${out?`<span>${esc(outCopy)}</span>`:''}<span>${esc(resetCopy)}</span></div></div>`;
  }
  function details(w,language='en'){
    if(!w)return '';
    const t=words(language),b=w.display_budget,p=w.projection,last=w.last_reading_reference;
    if(b?.mode==='session_quota_reset'||role(w)==='session')return qualify(w,language);
    let html='';
    if(w.fresh&&b?.is_current&&finite(b.amount)&&['per_workday','until_reset','extra_days_until_reset'].includes(b.mode)){
      const label=b.mode==='per_workday'?t.available:b.mode==='until_reset'?t.untilAvailable:t.extrasAvailable;
      html+=`<div class="pace-body"><div class="pace-note">${esc(label)}</div><div class="pace-value">${esc(displayAmount(b,language))}</div></div>`;
      if(b.mode==='per_workday'){
        const rate=p&&finite(p.rate_per_workday)&&['observed_recent','cycle_mean'].includes(p.basis)?pairedAmount(p.rate_per_workday,p.unit,p.rate_percent,language)+' / '+t.days:null;
        html+=row(p?.basis==='cycle_mean'?t.mean:p?.basis==='observed_recent'?t.observed:t.rateMissing,rate||'—');
      }
      if(p)html+=timeline(p,b,language);
    }
    if(w.fresh&&p){
      let forecast='';
      if(p.state==='exhausted')forecast=t.exhausted;
      else if(p.state==='projected_due')forecast=t.due;
      else if(p.state==='before_reset')forecast=t.runsOut+' '+date(p.exhaustion_at_ms,language)+(finite(p.early_workdays)&&p.early_workdays>0?' · '+(p.early_workdays<.1?t.less:number(p.early_workdays,language,1))+' '+t.before:'');
      else if(p.state==='covers_reset')forecast=t.covers;
      else forecast=t[p.state]||t.rateMissing;
      html+=`<div class="pace-forecast" data-warning="${['before_reset','projected_due','exhausted'].includes(p.state)}">${esc(forecast)}</div>`;
    }
    if(!w.fresh&&last){
      html+=row(t.last,date(last.as_of_ms,language)+' · '+number(last.age_ms/60000,language,0)+' '+t.minutes);
      if(last.previous_cycle)html+=`<div class="pace-note">${esc(t.previous)}</div>`;
      else if(last.display_budget&&finite(last.display_budget.amount))html+=row(t.last+' · '+(last.display_budget.mode==='per_workday'?t.available:t.untilAvailable),displayAmount(last.display_budget,language));
      else if(finite(last.official_remaining))html+=row(t.last+' · '+t.remaining,amount(last.official_remaining,last.unit,language));
    }
    html+=qualify(w,language);
    if(w.fresh&&p){
      let evidence=row(t.sampling,number(p.sample_count,language,0)+(finite(p.coverage_fraction)?' · '+number(p.coverage_fraction*100,language,0)+'% '+t.coverage:''));
      if(p.sampled_from_ms&&p.sampled_to_ms)evidence+=row(t.sampling,date(p.sampled_from_ms,language)+' – '+date(p.sampled_to_ms,language));
      if(b?.mode!=='per_workday'&&finite(p.rate_per_workday)&&['observed_recent','cycle_mean'].includes(p.basis))evidence+=row(p.basis==='cycle_mean'?t.mean:t.observed,pairedAmount(p.rate_per_workday,p.unit,p.rate_percent,language)+' / '+t.days);
      html+=`<details class="quota-evidence" data-evidence-key="${esc(key(w))}"><summary>${esc(t.quotaDetails)}</summary>${evidence}</details>`;
    }
    return html||`<div class="pace-note">${esc(t.missing)}</div>`;
  }
  function extended(w,language='en'){
    return details(w,language);
  }
  function match(dto,window,index){
    if(!dto||!Array.isArray(dto.windows))return null;
    return dto.windows.find(w=>w.window_index===index&&w.window_id===String(window.id)&&((w.group||null)===(window.group||null)))||null;
  }
  function loader(fetch,onChange){
    const versions=new Map(),values=new Map(),errors=new Set();
    return {get:provider=>values.get(provider)||null,failed:provider=>errors.has(provider),async refresh(provider){
      if(!supported(provider))return;
      const version=(versions.get(provider)||0)+1;versions.set(provider,version);values.delete(provider);errors.delete(provider);onChange(provider);
      try{const dto=await fetch(provider);if(versions.get(provider)!==version)return;if(dto?.provider!==provider||!Array.isArray(dto.windows))throw new Error('Invalid quota response');values.set(provider,dto);onChange(provider);}catch(_){if(versions.get(provider)===version){errors.add(provider);onChange(provider);}}
    },invalidate(){for(const provider of versions.keys())versions.set(provider,versions.get(provider)+1);values.clear();errors.clear();}};
  }
  root.CodenotchInsights={supported,esc,words,number,date,amount,track,details,extended,compact,qualify,role,key,accounts,general,match,loader};
})(typeof window==='object'?window:globalThis);
