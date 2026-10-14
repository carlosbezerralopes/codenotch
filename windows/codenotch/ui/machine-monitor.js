/* The native collector owns sampling; this window keeps only a bounded, volatile chart buffer. */
(function (root) {
  'use strict';
  const RETENTION_MS = 15 * 60 * 1000;
  const finite = value => typeof value === 'number' && Number.isFinite(value);
  const escape = value => String(value ?? '').replace(/[&<>"']/g, ch => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[ch]));
  const isHardware = id => typeof id === 'string' && id.startsWith('machine:');
  const resourceID = id => isHardware(id) ? id.slice(8) : null;
  function metricValue(resource) {
    if (!resource || resource.status !== 'ok') return null;
    if (resource.kind === 'network') {
      const rates = [resource.read_bps, resource.write_bps].filter(finite);
      return rates.length ? Math.max(...rates, 0) : null;
    }
    return finite(resource.percent) ? Math.max(0, Math.min(100, resource.percent)) : null;
  }
  function trimHistory(samples, now) {
    return samples.filter(sample => finite(sample.time) && sample.time >= now - RETENTION_MS && sample.time <= now);
  }
  function appendSample(samples, time, resource, interval, forceGap = false) {
    const previous = samples[samples.length - 1];
    if (previous && time <= previous.time) return trimHistory(samples, previous.time);
    const ok = resource?.status === 'ok';
    return trimHistory([...samples, { time, value: metricValue(resource),
      read: ok && finite(resource.read_bps) ? Math.max(0, resource.read_bps) : null,
      write: ok && finite(resource.write_bps) ? Math.max(0, resource.write_bps) : null,
      gap: forceGap || !!(previous && time - previous.time > interval * 2) }], time);
  }
  function chartSegments(samples, start, end, width, height, ceiling = 100, key = 'value') {
    const segments = []; let segment = [];
    const flush = () => { if (segment.length) segments.push(segment); segment = []; };
    for (const sample of samples) {
      if (sample.time < start || sample.time > end) continue;
      if (sample.gap || !finite(sample[key])) flush();
      if (!finite(sample[key])) continue;
      segment.push({ x: (sample.time - start) / Math.max(1, end - start) * width,
        y: height - Math.max(0, Math.min(1, sample[key] / Math.max(1, ceiling))) * height });
    }
    flush(); return segments;
  }
  function formatRate(value) {
    if (!finite(value)) return '—';
    const units = ['B/s', 'KB/s', 'MB/s', 'GB/s']; let n = Math.max(0, value), i = 0;
    while (n >= 1000 && i < units.length - 1) { n /= 1000; i++; }
    return `${n.toLocaleString('pt-BR', { maximumFractionDigits: i ? 1 : 0 })} ${units[i]}`;
  }
  function visibleIndicators(items, settings) {
    if (!settings.enabled) return [];
    const byID = new Map(items.map(item => [item.id, item]));
    const order = [...new Set([...(settings.order || []), ...byID.keys()])];
    const hidden = new Set(settings.hidden || []);
    return order.filter(id => byID.has(id) && !hidden.has(id)).map(id => byID.get(id));
  }
  const titles = { cpu: 'CPU', memory: 'RAM', disk: 'Disco', gpu: 'GPU', network: 'Rede' };
  const paths = {
    cpu: 'M7 7h10v10H7z M10 10h4v4h-4z M8 3v4m4-4v4m4-4v4M8 17v4m4-4v4m4-4v4M3 8h4m-4 4h4m-4 4h4m10-8h4m-4 4h4m-4 4h4',
    memory: 'M3 7h18v10H3z M6 10h3v4H6z M12 10h3v4h-3z M18 10v4 M5 17v3m4-3v3m4-3v3m4-3v3',
    disk: 'M5 3h14v18H5z M8 6h8 M8 9h8 M8 17h2m4 0h2',
    gpu: 'M3 7h17v11H3z M20 10h2m-2 5h2 M7 10h5v5H7z M15 10v5 M6 18v3m4-3v3m4-3v3',
    network: 'M4 8c5-5 11-5 16 0 M7 11c3-3 7-3 10 0 M10 14c1-1 3-1 4 0 M12 18h.01',
    ethernet: 'M5 4h14v10H5z M8 7v3m4-3v3m4-3v3 M12 14v4 M5 18h14 M5 16v4m14-4v4'
  };
  function iconHTML(resource) {
    const vendor = String(resource.vendor || '').toLowerCase();
    if (resource.kind === 'gpu' && /nvidia|intel|amd/.test(vendor)) {
      return `<span class="machine-vendor">${vendor.includes('nvidia') ? 'NVIDIA' : vendor.includes('intel') ? 'intel' : 'AMD'}</span>`;
    }
    const wired = resource.kind === 'network' && /ethernet/i.test(`${resource.name} ${resource.subtitle}`);
    return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${paths[wired ? 'ethernet' : resource.kind] || paths.cpu}"/></svg>`;
  }
  function resourceTitle(resource, all) {
    if (resource.kind === 'disk') return resource.name;
    if (resource.kind === 'gpu') return `GPU ${all.filter(item => item.kind === 'gpu').findIndex(item => item.id === resource.id)} · ${resource.vendor || 'GPU'}`;
    if (resource.kind === 'network') return /ethernet/i.test(`${resource.name} ${resource.subtitle}`) ? 'Ethernet' : /wi-?fi|wireless|802\.11/i.test(`${resource.name} ${resource.subtitle}`) ? 'Wi-Fi' : 'Rede';
    return titles[resource.kind] || resource.name;
  }
  function cellLabel(resource, all) {
    if (resource.kind === 'gpu') return `GPU ${all.filter(item => item.kind === 'gpu').findIndex(item => item.id === resource.id)}`;
    if (resource.kind === 'disk') return /\bSSD\b/i.test(`${resource.name} ${resource.subtitle}`) ? 'SSD' : /\bHDD\b/i.test(`${resource.name} ${resource.subtitle}`) ? 'HDD' : 'Disco';
    return resource.kind === 'network' ? resourceTitle(resource, all) : titles[resource.kind] || resource.name;
  }
  function cellHTML(resource) {
    return `<div class="cell machine-cell" data-p="${escape('machine:' + resource.id)}" tabindex="0" role="button" aria-controls="card"><div class="ringwrap"><svg class="ring" viewBox="0 0 56 56"></svg><svg class="reading" viewBox="0 0 56 56"></svg><div class="glyph machine-glyph">${iconHTML(resource)}</div></div><div class="pct">—</div><div class="machine-label"></div></div>`;
  }
  function createController({ invoke, listen, onChange = () => {}, onError = () => {} }) {
    let snapshot = { supported: false, resources: [], issues: [] };
    let settings = { enabled: true, refresh_seconds: 3, history_minutes: 5, order: [], hidden: [] };
    const histories = new Map(); let time = 0, forceGap = false, request = null, snapshotVersion = 0, settingsVersion = 0;
    function acceptSnapshot(next) {
      if (!next || next.schema_version !== 1 || !Array.isArray(next.resources)) return;
      const at = Date.parse(next.sampled_at);
      if (!finite(at) || at < time) return;
      snapshot = next;
      if (at > time) {
        const seen = new Set();
        for (const resource of next.resources) {
          seen.add(resource.id);
          histories.set(resource.id, appendSample(histories.get(resource.id) || [], at, resource, settings.refresh_seconds * 1000, forceGap));
        }
        for (const [id, samples] of histories) {
          if (!seen.has(id)) histories.set(id, appendSample(samples, at, null, settings.refresh_seconds * 1000, forceGap));
        }
        time = at; forceGap = false;
      }
      onChange();
    }
    function acceptSettings(next) {
      if (!next || typeof next.enabled !== 'boolean') return;
      if (next.enabled !== settings.enabled || next.refresh_seconds !== settings.refresh_seconds) forceGap = true;
      settings = { ...next, order: [...(next.order || [])], hidden: [...(next.hidden || [])] }; onChange();
    }
    function refresh() {
      if (request) return request;
      const version = snapshotVersion;
      request = Promise.resolve().then(() => invoke('get_machine_snapshot')).then(next => {
        if (version === snapshotVersion) acceptSnapshot(next);
      }).catch(error => onError('Monitor: ' + String(error))).finally(() => { request = null; });
      return request;
    }
    async function start() {
      try {
        await listen('machine_snapshot', event => { snapshotVersion++; acceptSnapshot(event.payload); });
        await listen('machine_monitor_settings', event => { settingsVersion++; acceptSettings(event.payload); });
        const version = settingsVersion;
        const next = await invoke('get_machine_monitor_settings');
        if (version === settingsVersion) acceptSettings(next);
        await refresh();
      } catch (error) { onError('Monitor: ' + String(error)); }
    }
    function selected(id) { return visible().find(resource => resource.id === resourceID(id)) || null; }
    function visible() { return snapshot.supported ? visibleIndicators(snapshot.resources, settings) : []; }
    function ceiling(resource) {
      if (resource.kind !== 'network') return 100;
      const values = (histories.get(resource.id) || []).filter(sample => sample.time >= time - settings.history_minutes * 60000).flatMap(sample => [sample.read, sample.write]).filter(finite);
      return Math.max(1, ...values) * 1.1;
    }
    function ringValue(resource) {
      const value = metricValue(resource);
      return value === null ? null : resource.kind === 'network' ? Math.min(100, value / ceiling(resource) * 100) : value;
    }
    function cardHTML(resource) {
      const samples = histories.get(resource.id) || [], start = time - settings.history_minutes * 60000;
      const network = resource.kind === 'network', max = ceiling(resource), width = 210, height = 56;
      const series = network ? [['read', 'var(--ample)'], ['write', 'var(--watch)']] : [['value', 'var(--ample)']];
      let graph = `<svg class="machine-chart" viewBox="0 0 ${width} ${height}" role="img" aria-label="${escape(resourceTitle(resource, snapshot.resources))}: histórico de ${settings.history_minutes} minutos">`;
      for (const y of [0, height / 2, height]) graph += `<path d="M0 ${y}H${width}" stroke="var(--card-rule)"/>`;
      for (const [key, color] of series) for (const points of chartSegments(samples, start, time, width, height, max, key)) {
        if (points.length === 1) graph += `<circle cx="${points[0].x.toFixed(2)}" cy="${points[0].y.toFixed(2)}" r="1.5" fill="${color}"/>`;
        else graph += `<polyline points="${points.map(point => `${point.x.toFixed(2)},${point.y.toFixed(2)}`).join(' ')}" fill="none" stroke="${color}" stroke-width="1.6"/>`;
      }
      graph += '</svg>';
      const value = metricValue(resource), label = network ? formatRate(value) : value === null ? '—' : `${Math.round(value)}%`;
      const status = resource.status === 'warming' ? 'Aquecendo contadores' : resource.status !== 'ok' ? 'Leitura indisponível' : '';
      const rates = network || resource.kind === 'disk' ? `<div class="machine-rates"><span>↓ ${formatRate(resource.status === 'ok' ? resource.read_bps : null)}</span><span>↑ ${formatRate(resource.status === 'ok' ? resource.write_bps : null)}</span></div>` : '';
      const context = [resource.name, resource.subtitle].filter(Boolean).join(' · ');
      return `<div class="machine-head"><span>${iconHTML(resource)}</span><strong>${escape(resourceTitle(resource, snapshot.resources))}</strong></div><div class="machine-context" title="${escape(context)}">${escape(context)}</div><div class="machine-value">${label}${finite(resource.temperature_c) ? `<small> · ${resource.temperature_c.toFixed(0)} °C</small>` : ''}<small>${escape(status)}</small></div>${graph}<div class="machine-axis"><span>−${settings.history_minutes} min</span><span>${network ? escape(formatRate(max)) : '100%'} · agora</span></div>${rates}<div class="machine-note" title="${escape(resource.detail || '')}">${escape(resource.detail || 'Intervalos sem leitura ficam em branco.')}</div>`;
    }
    return { start, refresh, acceptSnapshot, acceptSettings, selected, visible, ringValue, cardHTML,
      get snapshot() { return snapshot; }, get settings() { return settings; }, history: id => histories.get(id) || [] };
  }
  const api = { RETENTION_MS, finite, escape, isHardware, resourceID, metricValue, trimHistory, appendSample, chartSegments,
    formatRate, visibleIndicators, iconHTML, resourceTitle, cellLabel, cellHTML, createController };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  root.MachineMonitor = api;
})(typeof window !== 'undefined' ? window : globalThis);
