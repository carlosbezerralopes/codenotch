/* All samples live in this window; the collector owns counter baselines. */
(function () {
  'use strict';
  const RETENTION_MS = 15 * 60 * 1000;
  const finite = value => typeof value === 'number' && Number.isFinite(value);
  function trimHistory(samples, now, retention = RETENTION_MS) {
    return samples.filter(sample => finite(sample.time) && sample.time >= now - retention && sample.time <= now);
  }
  function metricValue(resource) {
    if (!resource || resource.status !== 'ok') return null;
    if (resource.kind === 'network') {
      const rates = [resource.read_bps, resource.write_bps].filter(finite);
      return rates.length ? Math.max(...rates, 0) : null;
    }
    return finite(resource.percent) ? Math.max(0, Math.min(100, resource.percent)) : null;
  }
  function appendSample(samples, time, value, interval, forceGap = false) {
    const previous = samples[samples.length - 1];
    const sample = { time, value: finite(value) ? value : null, gap: forceGap || !!(previous && time - previous.time > 2 * interval) };
    if (previous && time <= previous.time) return trimHistory(samples, Math.max(previous.time, time));
    return trimHistory([...samples, sample], time);
  }
  function chartSegments(samples, start, end, width, height, ceiling = 100) {
    const segments = [];
    let segment = [];
    const flush = () => { if (segment.length) segments.push(segment); segment = []; };
    for (const sample of samples) {
      if (sample.time < start || sample.time > end) continue;
      if (sample.gap || !finite(sample.value)) flush();
      if (!finite(sample.value)) continue;
      segment.push({ x: (sample.time - start) / Math.max(1, end - start) * width, y: height - Math.max(0, Math.min(1, sample.value / Math.max(1, ceiling))) * height });
    }
    flush();
    return segments;
  }
  function formatRate(value) {
    if (!finite(value)) return '—';
    const units = ['B/s', 'KB/s', 'MB/s', 'GB/s'];
    let n = Math.max(0, value), i = 0;
    while (n >= 1000 && i < units.length - 1) { n /= 1000; i++; }
    return `${n.toLocaleString('pt-BR', { maximumFractionDigits: i ? 1 : 0 })} ${units[i]}`;
  }
  function reconcileOrder(order, availableIDs) {
    const available = new Set(availableIDs);
    return [...new Set([...order.filter(id => available.has(id)), ...availableIDs])];
  }
  function moveIndicator(order, id, direction) {
    const next = [...order], index = next.indexOf(id), target = index + direction;
    if (![-1, 1].includes(direction) || index < 0 || target < 0 || target >= next.length) return next;
    [next[index], next[target]] = [next[target], next[index]];
    return next;
  }
  function visibleIndicators(items, order, hiddenIDs) {
    const byID = new Map(items.map(item => [item.id, item]));
    const hidden = hiddenIDs instanceof Set ? hiddenIDs : new Set(hiddenIDs);
    return reconcileOrder(order, [...byID.keys()]).filter(id => !hidden.has(id)).map(id => byID.get(id));
  }
  if (typeof module !== 'undefined' && module.exports) module.exports = { RETENTION_MS, trimHistory, metricValue, appendSample, chartSegments, formatRate, reconcileOrder, moveIndicator, visibleIndicators };
  if (typeof document === 'undefined') return;

  const $ = id => document.getElementById(id);
  const ns = 'http://www.w3.org/2000/svg';
  const histories = new Map();
  let resources = [], selected = null, interval = 3000, windowMs = 300000;
  let paused = false, timer = null, controller = null, epoch = 0, forceGap = false, lastTime = Date.now(), collectionMs = 0, hasSnapshot = false;
  const titles = { cpu: 'CPU', memory: 'RAM', disk: 'Disco', gpu: 'GPU', network: 'Rede' };
  const ai = [ { id: 'claude', name: 'Claude', percent: 32 }, { id: 'codex', name: 'Codex', percent: 57 }, { id: 'kiro', name: 'Kiro', percent: 21 } ];
  let aiOrder = ai.map(item => item.id), hardwareOrder = [], dismissTimer = null, pinned = false, menuTab = 'ai', menuSignature = '';
  const hiddenIDs = new Set(), ringButtons = new Map();
  const paths = {
    cpu: 'M7 7h10v10H7z M10 10h4v4h-4z M8 3v4m4-4v4m4-4v4M8 17v4m4-4v4m4-4v4M3 8h4m-4 4h4m-4 4h4m10-8h4m-4 4h4m-4 4h4',
    memory: 'M3 7h18v10H3z M6 10h3v4H6z M12 10h3v4h-3z M18 10v4 M5 17v3m4-3v3m4-3v3m4-3v3',
    disk: 'M5 3h14v18H5z M8 6h8 M8 9h8 M8 17h2m4 0h2',
    gpu: 'M3 7h17v11H3z M20 10h2m-2 5h2 M7 10h5v5H7z M15 10v5 M6 18v3m4-3v3m4-3v3',
    network: 'M4 8c5-5 11-5 16 0 M7 11c3-3 7-3 10 0 M10 14c1-1 3-1 4 0 M12 18h.01'
  };
  function svgElement(tag, attrs) {
    const element = document.createElementNS(ns, tag);
    Object.entries(attrs || {}).forEach(([key, value]) => element.setAttribute(key, String(value)));
    return element;
  }
  function node(tag, className, text) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    if (text !== undefined) element.textContent = text;
    return element;
  }
  function icon(resource) {
    const vendor = String(resource.vendor || '').toLowerCase();
    if (resource.kind === 'gpu' && (vendor.includes('nvidia') || vendor.includes('intel'))) {
      return node('span', `vendor ${vendor.includes('nvidia') ? 'nvidia' : 'intel'}`, vendor.includes('nvidia') ? 'NVIDIA' : 'intel');
    }
    const svg = svgElement('svg', { viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': 1.4, 'stroke-linecap': 'round', 'stroke-linejoin': 'round', 'aria-hidden': 'true' });
    const wired = resource.kind === 'network' && /ethernet/i.test(`${resource.name} ${resource.subtitle}`);
    svg.append(svgElement('path', { d: wired ? 'M5 4h14v10H5z M8 7v3m4-3v3m4-3v3 M12 14v4 M5 18h14 M5 16v4m14-4v4' : paths[resource.kind] || paths.cpu }));
    return svg;
  }
  function percentLabel(value) { return finite(value) ? `${Math.round(value)}%` : '—'; }
  function currentLabel(resource) {
    return resource.kind === 'network' ? formatRate(metricValue(resource)) : percentLabel(metricValue(resource));
  }
  function resourceTitle(resource) {
    if (resource.kind === 'disk') return resource.name;
    if (resource.kind === 'gpu') return `GPU ${resources.filter(item => item.kind === 'gpu').findIndex(item => item.id === resource.id)} · ${resource.vendor || 'GPU'}`;
    if (resource.kind === 'network') return /ethernet/i.test(`${resource.name} ${resource.subtitle}`) ? 'Ethernet' : 'Wi-Fi';
    return titles[resource.kind] || resource.name;
  }
  function ring(resource, illustrative = false) {
    const value = illustrative ? resource.percent : metricValue(resource);
    let button = ringButtons.get(resource.id);
    if (!button) {
      button = node('button', 'ring-cell'); button.type = 'button'; button.dataset.resource = resource.id;
      button.addEventListener('pointerenter', event => { if (event.pointerType !== 'touch') openDetail(resource.id); });
      button.addEventListener('pointerleave', queueDismiss);
      button.addEventListener('focus', () => openDetail(resource.id));
      button.addEventListener('blur', queueDismiss);
      button.addEventListener('click', () => {
        if (pinned && selected === resource.id) closeDetail();
        else { openDetail(resource.id); pinned = true; }
      });
      ringButtons.set(resource.id, button);
    }
    button.setAttribute('aria-label', `${illustrative ? 'Cota ilustrativa: ' : ''}${resource.name}: ${illustrative ? percentLabel(value) : currentLabel(resource)}`);
    button.setAttribute('aria-expanded', String(selected === resource.id && !$('flyout').hidden));
    button.setAttribute('aria-controls', 'flyout');
    button.classList.toggle('active', selected === resource.id && !$('flyout').hidden);
    const wrapper = node('span', 'ringwrap');
    const svg = svgElement('svg', { viewBox: '0 0 56 56', 'aria-hidden': 'true' });
    svg.append(svgElement('circle', { cx: 28, cy: 28, r: 24, fill: 'none', stroke: 'var(--track)', 'stroke-width': 3 }));
    let ringPercent = value;
    if (!illustrative && resource.kind === 'network') ringPercent = finite(value) ? value / ceilingFor(resource) * 100 : null;
    if (finite(ringPercent)) svg.append(svgElement('circle', { cx: 28, cy: 28, r: 24, fill: 'none', stroke: ringPercent >= 90 ? 'var(--crit)' : ringPercent >= 75 ? 'var(--watch)' : 'var(--ample)', 'stroke-width': 3, 'stroke-linecap': 'round', 'stroke-dasharray': `${150.8 * ringPercent / 100} 150.8`, transform: 'rotate(-90 28 28)' }));
    wrapper.append(svg);
    const glyph = node('span', 'ring-glyph');
    if (illustrative) { const img = node('img'); img.src = `/glyphs/${resource.id}.svg`; img.alt = ''; glyph.append(img); }
    else glyph.append(icon(resource));
    wrapper.append(glyph);
    const reading = node('span', 'cell-reading');
    const label = resource.kind === 'disk' ? resourceTitle(resource) : resource.kind === 'gpu' ? `GPU ${resources.filter(r => r.kind === 'gpu').indexOf(resource)}` : titles[resource.kind];
    reading.append(node('span', 'ring-value', illustrative ? percentLabel(value) : resource.kind === 'network' ? '↕' : percentLabel(value)), node('span', 'cell-label', illustrative ? resource.name : label));
    button.replaceChildren(wrapper, reading);
    return button;
  }
  function cancelDismiss() { clearTimeout(dismissTimer); }
  function queueDismiss() {
    cancelDismiss();
    dismissTimer = setTimeout(() => {
      if (pinned || $('flyout').matches(':hover') || $('flyout').contains(document.activeElement)) return;
      const active = ringButtons.get(selected);
      if (active?.matches(':hover') || active === document.activeElement) return;
      closeDetail();
    }, 180);
  }
  function closeDetail() {
    cancelDismiss(); selected = null; pinned = false; $('flyout').hidden = true;
    for (const button of ringButtons.values()) { button.classList.remove('active'); button.setAttribute('aria-expanded', 'false'); }
  }
  function openDetail(id) {
    if (hiddenIDs.has(id) || $('indicator-menu').open) return;
    cancelDismiss();
    if (selected !== id) pinned = false;
    selected = id; $('flyout').hidden = false;
    for (const [buttonID, button] of ringButtons) { button.classList.toggle('active', buttonID === id); button.setAttribute('aria-expanded', String(buttonID === id)); }
    renderDetail(); positionFlyout();
  }
  function positionFlyout() {
    if ($('flyout').hidden) return;
    const stage = document.querySelector('.desktop').getBoundingClientRect(), anchor = ringButtons.get(selected)?.getBoundingClientRect();
    if (!anchor) return closeDetail();
    const narrow = window.matchMedia('(max-width: 620px)').matches;
    const preferred = narrow ? $('pill').getBoundingClientRect().bottom - stage.top + 12 : anchor.top - stage.top + anchor.height / 2 - $('flyout').offsetHeight / 2;
    const top = Math.max(10, Math.min(preferred, stage.height - $('flyout').offsetHeight - 10));
    $('flyout').style.top = `${top}px`;
  }
  function fitPill() {
    const stage = document.querySelector('.desktop');
    stage.style.height = `${Math.max(1, window.innerHeight - document.querySelector('.page-header').offsetHeight - document.querySelector('.page-footer').offsetHeight)}px`;
    $('pill').style.transform = '';
    if (window.matchMedia('(max-width: 620px)').matches) return;
    const stageHeight = stage.clientHeight, height = $('pill').offsetHeight;
    const scale = Math.min(1, (stageHeight - 54) / Math.max(1, height));
    $('pill').style.transformOrigin = 'right center';
    $('pill').style.transform = `translateY(-50%) scale(${scale})`;
  }
  function ceilingFor(resource) {
    if (resource.kind !== 'network') return 100;
    const values = (histories.get(resource.id) || []).filter(s => s.time >= lastTime - windowMs && finite(s.value)).map(s => s.value);
    const peak = Math.max(1000, ...values);
    return Math.pow(10, Math.ceil(Math.log10(peak)));
  }
  function drawChart(svg, resource, width, height, mini = false) {
    const samples = histories.get(resource.id) || [], ceiling = ceilingFor(resource);
    svg.replaceChildren();
    if (!mini) for (let i = 0; i < 4; i++) svg.append(svgElement('line', { x1: 0, x2: width, y1: i * height / 3, y2: i * height / 3, stroke: 'var(--line)', 'stroke-width': .7, 'stroke-dasharray': '2 3' }));
    for (const segment of chartSegments(samples, lastTime - windowMs, lastTime, width, height, ceiling)) {
      if (segment.length === 1) svg.append(svgElement('circle', { cx: segment[0].x, cy: segment[0].y, r: mini ? 1 : 2, fill: 'currentColor' }));
      else svg.append(svgElement('path', { d: segment.map((point, i) => `${i ? 'L' : 'M'}${point.x.toFixed(2)},${point.y.toFixed(2)}`).join(' '), fill: 'none', stroke: 'currentColor', 'stroke-width': mini ? 1.2 : 1.6, 'stroke-linejoin': 'round' }));
    }
  }
  function addFact(label, value) {
    const div = node('div'); div.append(node('dt', '', label), node('dd', '', value || '—')); $('detail-facts').append(div);
  }
  function renderDetail() {
    if (!selected) return;
    const illustrative = ai.find(item => item.id === selected), resource = resources.find(item => item.id === selected);
    $('detail-facts').replaceChildren();
    if (illustrative) {
      $('detail-source').textContent = 'COTAS ILUSTRATIVAS'; $('live-badge').textContent = 'Ilustrativo';
      $('detail-title').textContent = illustrative.name; $('detail-name').textContent = 'Cotas ilustrativas'; $('detail-value').textContent = percentLabel(illustrative.percent);
      $('chart').replaceChildren(); $('chart').hidden = true; document.querySelector('.chart-axis').hidden = true;
      $('chart-unit').textContent = 'Prévia visual'; $('chart-ceiling').textContent = '';
      $('chart-note').textContent = 'Estes anéis demonstram como as cotas de IA convivem com o monitor. Nenhuma conta, cota, crédito ou token real é consultado neste protótipo.';
      addFact('Fonte', 'Dados ilustrativos'); addFact('Plano da assinatura', 'Ainda não registrado no histórico');
      return;
    }
    $('chart').hidden = false; document.querySelector('.chart-axis').hidden = false;
    $('detail-source').textContent = 'ESTE COMPUTADOR';
    $('live-badge').textContent = paused ? 'Pausado' : resource?.status === 'ok' ? 'Ao vivo' : resource?.status === 'warming' ? 'Aquecendo' : 'Sem leitura';
    $('detail-title').textContent = resource ? resourceTitle(resource) : 'CPU';
    $('detail-name').textContent = resource ? (resource.subtitle || resource.name) : 'Aguardando leitura do Windows';
    $('detail-value').textContent = resource ? currentLabel(resource) : '—';
    $('chart-unit').textContent = resource?.kind === 'network' ? 'Tráfego · maior entre entrada e saída' : 'Utilização';
    $('chart-ceiling').textContent = resource?.kind === 'network' ? formatRate(ceilingFor(resource)) : '100%';
    $('axis-start').textContent = `−${windowMs / 60000} min`;
    $('chart-note').textContent = resource?.kind === 'network' ? 'Escala automática; o anel acompanha a escala do gráfico.' : 'Intervalos sem leitura ficam em branco.';
    if (!resource) { $('chart').replaceChildren(); return; }
    drawChart($('chart'), resource, 300, 108);
    if (resource.status !== 'ok') addFact('Disponibilidade', resource.status === 'warming' ? 'Primeira amostra' : 'Indisponível');
    if (resource.kind === 'disk' || resource.kind === 'network') { addFact(resource.kind === 'disk' ? 'Leitura' : 'Recebido', formatRate(resource.read_bps)); addFact(resource.kind === 'disk' ? 'Gravação' : 'Enviado', formatRate(resource.write_bps)); }
    if (resource.detail && !(['disk', 'network'].includes(resource.kind) && resource.status === 'ok')) addFact(resource.kind === 'memory' ? 'Memória livre / capacidade' : 'Detalhes', resource.detail);
    if (resource.kind === 'gpu') addFact('Identidade do fabricante', resource.vendor || 'Não informada');
    if (finite(resource.temperature_c)) addFact('Temperatura', `${resource.temperature_c} °C`);
  }
  function syncGroup(container, buttons) {
    // Leave attached buttons in place on ordinary samples, preserving pointer and keyboard focus.
    const desired = new Set(buttons);
    for (const child of [...container.children]) if (!desired.has(child)) child.remove();
    buttons.forEach((button, index) => { if (container.children[index] !== button) container.insertBefore(button, container.children[index] || null); });
  }
  function render() {
    aiOrder = reconcileOrder(aiOrder, ai.map(item => item.id));
    hardwareOrder = reconcileOrder(hardwareOrder, resources.map(item => item.id));
    const visibleAI = visibleIndicators(ai, aiOrder, hiddenIDs), visibleHardware = visibleIndicators(resources, hardwareOrder, hiddenIDs);
    syncGroup($('ai-rings'), visibleAI.map(item => ring(item, true)));
    syncGroup($('hardware-rings'), visibleHardware.map(item => ring(item)));
    $('ai-rings').hidden = !visibleAI.length; $('hardware-rings').hidden = !visibleHardware.length;
    $('pill-divider').hidden = !visibleAI.length || !visibleHardware.length;
    $('empty-hint').hidden = !!(visibleAI.length + visibleHardware.length);
    if (selected && ![...visibleAI, ...visibleHardware].some(item => item.id === selected)) closeDetail();
    renderDetail(); fitPill(); positionFlyout();
    const signature = JSON.stringify([...ai, ...resources].map(item => [item.id, item.name]));
    if ($('indicator-menu').open && signature !== menuSignature) renderMenu();
  }
  function chooseTab(tab) {
    menuTab = tab;
    for (const group of ['ai', 'hardware']) {
      $(`tab-${group}`).setAttribute('aria-selected', String(group === tab));
      $(`menu-${group}`).classList.toggle('tab-active', group === tab);
    }
  }
  function renderMenu() {
    menuSignature = JSON.stringify([...ai, ...resources].map(item => [item.id, item.name]));
    const focused = document.activeElement?.dataset.menuItem, focusedAction = document.activeElement?.dataset.action;
    for (const group of ['ai', 'hardware']) {
      const items = group === 'ai' ? ai : resources, order = group === 'ai' ? aiOrder : hardwareOrder;
      const byID = new Map(items.map(item => [item.id, item]));
      const rows = order.map((id, index) => {
        const item = byID.get(id), row = node('div', 'menu-item'), label = node('label');
        const checkbox = node('input'); checkbox.type = 'checkbox'; checkbox.checked = !hiddenIDs.has(id); checkbox.dataset.menuItem = id; checkbox.dataset.action = 'visibility';
        checkbox.addEventListener('change', () => {
          checkbox.checked ? hiddenIDs.delete(id) : hiddenIDs.add(id);
          if (selected === id && !checkbox.checked) closeDetail();
          render();
        });
        const text = node('span', 'menu-item-text');
        const subtitle = group === 'ai' ? 'Cota ilustrativa' : item.kind === 'memory' ? 'Memória física' : item.subtitle || item.name;
        text.append(node('span', 'menu-item-title', group === 'ai' ? item.name : resourceTitle(item)), node('span', 'menu-item-subtitle', subtitle));
        label.append(checkbox, text); row.append(label);
        for (const direction of [-1, 1]) {
          const button = node('button', '', direction === -1 ? '↑' : '↓'); button.type = 'button'; button.disabled = direction === -1 ? index === 0 : index === order.length - 1;
          button.dataset.menuItem = id; button.dataset.action = String(direction);
          button.setAttribute('aria-label', `${direction === -1 ? 'Mover para cima' : 'Mover para baixo'}: ${group === 'ai' ? item.name : resourceTitle(item)}`);
          button.addEventListener('click', () => {
            if (group === 'ai') aiOrder = moveIndicator(aiOrder, id, direction); else hardwareOrder = moveIndicator(hardwareOrder, id, direction);
            render(); renderMenu();
          }); row.append(button);
        }
        return row;
      });
      if (!rows.length) rows.push(node('p', 'menu-note', 'Aguardando recursos do Windows.'));
      $(`${group}-options`).replaceChildren(...rows);
    }
    chooseTab(menuTab);
    if (focused) [...$('indicator-menu').querySelectorAll('[data-menu-item]')].find(el => el.dataset.menuItem === focused && el.dataset.action === focusedAction && !el.disabled)?.focus({ preventScroll: true });
  }
  function openMenu() { closeDetail(); renderMenu(); $('indicator-menu').showModal(); }
  function status(text, live = false) {
    $('status').textContent = text; $('status-dot').classList.toggle('live', live);
    $('page-status').textContent = text;
    $('live-badge').textContent = paused ? 'Pausado' : live ? 'Ao vivo' : 'Sem leitura';
  }
  function validateSnapshot(snapshot) {
    if (snapshot?.schema_version !== 1 || !Array.isArray(snapshot.resources) || !finite(Date.parse(snapshot.sampled_at))) throw new Error('Resposta da coleta local inválida.');
    if (snapshot.resources.some(r => !r || typeof r.id !== 'string' || !Object.hasOwn(titles, r.kind) || typeof r.name !== 'string')) throw new Error('Identidade de recurso inválida.');
    return snapshot;
  }
  async function collect() {
    if (paused || controller) return;
    const started = performance.now(), generation = epoch;
    controller = new AbortController();
    const activeController = controller;
    const timeout = setTimeout(() => activeController.abort(), 20000);
    try {
      const response = await fetch('/api/snapshot', { signal: activeController.signal, cache: 'no-store' });
      if (!response.ok) throw new Error(`Coleta local respondeu HTTP ${response.status}.`);
      const snapshot = validateSnapshot(await response.json());
      if (generation !== epoch || paused) return;
      resources = [...snapshot.resources].sort((a, b) => ['cpu', 'memory', 'disk', 'gpu', 'network'].indexOf(a.kind) - ['cpu', 'memory', 'disk', 'gpu', 'network'].indexOf(b.kind));
      const previousTime = hasSnapshot ? lastTime : null;
      lastTime = Date.parse(snapshot.sampled_at); collectionMs = snapshot.collection_ms; hasSnapshot = true;
      const present = new Set(resources.map(r => r.id));
      for (const [id, samples] of histories) histories.set(id, trimHistory(samples, lastTime));
      for (const resource of resources) histories.set(resource.id, appendSample(histories.get(resource.id) || [], lastTime, metricValue(resource), interval, forceGap));
      for (const [id, samples] of histories) if (!present.has(id)) histories.set(id, appendSample(samples, lastTime, null, interval, forceGap));
      if (lastTime !== previousTime) forceGap = false;
      if (selected && !ai.some(item => item.id === selected) && !present.has(selected)) closeDetail();
      $('error').hidden = true;
      const time = new Date(lastTime).toLocaleTimeString('pt-BR');
      status(`${time} · coleta ${finite(collectionMs) ? Math.round(collectionMs) + ' ms' : '—'}${snapshot.issues?.length ? ' · ' + snapshot.issues.length + ' recurso(s) com aviso' : ''}`, true);
      render();
    } catch (error) {
      if (generation !== epoch || paused) return;
      forceGap = true;
      resources = resources.map(r => ({ ...r, percent: null, read_bps: null, write_bps: null, status: 'unavailable', value: '—', detail: 'A coleta local está indisponível.' }));
      $('error').hidden = false;
      $('error-text').textContent = error.name === 'AbortError' ? 'A coleta excedeu 20 segundos. Tentando reconectar.' : `${error.message} Tentando reconectar.`;
      status(hasSnapshot ? `Coleta indisponível · última leitura ${new Date(lastTime).toLocaleTimeString('pt-BR')}` : 'Coleta indisponível · nenhuma leitura recebida'); render();
    } finally {
      clearTimeout(timeout);
      if (controller === activeController) controller = null;
      if (!paused && generation === epoch) timer = setTimeout(collect, Math.max(0, interval - (performance.now() - started)));
      else if (!paused && !controller) timer = setTimeout(collect, 0);
    }
  }
  function restart() {
    clearTimeout(timer); epoch++; controller?.abort();
    if (!paused && !controller) collect();
  }
  $('interval').addEventListener('change', event => { interval = Number(event.target.value); restart(); });
  $('window').addEventListener('change', event => { windowMs = Number(event.target.value); render(); });
  $('pause').addEventListener('click', () => {
    paused = !paused; $('pause').textContent = paused ? 'Retomar' : 'Pausar';
    forceGap = true; status(paused ? 'Pausado · histórico em memória' : 'Retomando coleta…'); restart();
  });
  $('retry').addEventListener('click', () => { paused = false; $('pause').textContent = 'Pausar'; restart(); });
  $('theme').addEventListener('click', () => {
    const light = document.documentElement.dataset.theme !== 'light';
    document.documentElement.dataset.theme = light ? 'light' : 'dark';
    $('theme').textContent = light ? 'Escuro' : 'Claro'; $('theme').setAttribute('aria-label', `Alternar para tema ${light ? 'escuro' : 'claro'}`);
  });
  for (const id of ['indicators', 'gear', 'empty-menu']) $(id).addEventListener('click', openMenu);
  $('close-menu').addEventListener('click', () => $('indicator-menu').close());
  $('close-detail').addEventListener('click', closeDetail);
  $('tab-ai').addEventListener('click', () => chooseTab('ai'));
  $('tab-hardware').addEventListener('click', () => chooseTab('hardware'));
  $('flyout').addEventListener('pointerenter', cancelDismiss);
  $('flyout').addEventListener('pointerleave', queueDismiss);
  $('flyout').addEventListener('focusin', cancelDismiss);
  $('flyout').addEventListener('focusout', queueDismiss);
  document.addEventListener('keydown', event => { if (event.key === 'Escape') closeDetail(); });
  window.addEventListener('resize', () => { fitPill(); positionFlyout(); });
  chooseTab('ai');
  window.addEventListener('pagehide', () => { paused = true; clearTimeout(timer); controller?.abort(); });
  render(); collect();
})();
