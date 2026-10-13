const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const monitor = require('./codenotch/ui/machine-monitor.js');
const html = readFileSync(join(__dirname, 'codenotch/ui/notch.html'), 'utf8');
const carry = readFileSync(join(__dirname, 'codenotch/ui/carry.html'), 'utf8');
const resource = (id = 'cpu', patch = {}) => ({ id, kind: 'cpu', name: 'Synthetic CPU', subtitle: 'Fixture', status: 'ok', percent: 35, value: '35%', detail: '', ...patch });
const snapshot = (time, resources) => ({ schema_version: 1, supported: true, platform: 'windows', sampled_at: new Date(time).toISOString(), collection_ms: 1, resources, issues: [] });
const settings = patch => ({ enabled: true, refresh_seconds: 3, history_minutes: 5, order: [], hidden: [], ...patch });
const controller = () => monitor.createController({ invoke: () => Promise.resolve(), listen: () => Promise.resolve() });
function fn(source, name) {
  const start = source.indexOf(`function ${name}(`), end = source.indexOf('\n}', start);
  assert.ok(start >= 0 && end > start, name);
  return source.slice(start, end + 2);
}

test('missing, warming and unavailable metrics remain absent while real zero stays zero', () => {
  for (const percent of [null, undefined, NaN, '0']) assert.equal(monitor.metricValue(resource('cpu', { percent })), null);
  for (const status of ['warming', 'unavailable']) assert.equal(monitor.metricValue(resource('cpu', { percent: 0, status })), null);
  assert.equal(monitor.metricValue(resource('cpu', { percent: 0 })), 0);
  assert.equal(monitor.metricValue(resource('net', { kind: 'network', read_bps: null, write_bps: null })), null);
  assert.equal(monitor.metricValue(resource('net', { kind: 'network', read_bps: 0, write_bps: null })), 0);
  assert.equal(monitor.formatRate(null), '—');
});

test('fifteen-minute RAM retention, duplicate timestamps and gaps use sampled UTC time', () => {
  const c = controller(), start = Date.parse('2026-10-13T15:00:00Z');
  for (let seconds = 0; seconds <= 16 * 60; seconds += 3) c.acceptSnapshot(snapshot(start + seconds * 1000, [resource()]));
  const samples = c.history('cpu');
  assert.equal(samples.length, 301);
  assert.equal(samples[0].time, start + 60000);
  c.acceptSnapshot(snapshot(start + 16 * 60000, [resource('cpu', { percent: 99 })]));
  assert.equal(c.history('cpu').at(-1).value, 35, 'same sample cannot manufacture another measurement');
  c.acceptSnapshot(snapshot(start + 17 * 60000, [resource()]));
  assert.equal(c.history('cpu').at(-1).gap, true);
  const segments = monitor.chartSegments(c.history('cpu'), start + 16 * 60000, start + 17 * 60000, 100, 50);
  assert.equal(segments.length, 2, 'long pause is two distinct segments');
  assert.equal(segments.at(-1)[0].x, 100);
});

test('hidden resources retain history and absent saved IDs preserve their order on return', () => {
  const c = controller(), start = Date.parse('2026-10-13T15:00:00Z');
  const prefs = settings({ order: ['gpu-returning', 'memory', 'cpu'], hidden: ['cpu', 'disk-absent'] });
  c.acceptSettings(prefs);
  c.acceptSnapshot(snapshot(start, [resource('cpu'), resource('memory')]));
  assert.deepEqual(c.visible().map(item => item.id), ['memory']);
  c.acceptSnapshot(snapshot(start + 3000, [resource('cpu'), resource('memory'), resource('gpu-returning')]));
  assert.deepEqual(c.visible().map(item => item.id), ['gpu-returning', 'memory']);
  assert.equal(c.history('cpu').length, 2);
  assert.deepEqual(c.settings.order, prefs.order);
  assert.deepEqual(c.settings.hidden, prefs.hidden);
  c.acceptSettings(settings({ ...prefs, hidden: [] }));
  assert.equal(c.history('cpu').length, 2);
  c.acceptSettings(settings({ hidden: ['cpu', 'memory', 'gpu-returning'] }));
  assert.deepEqual(c.visible(), [], 'explicit all-hidden selection is not replaced by defaults');
});

test('switching off collection preserves buffers and adds a gap on re-enable', () => {
  const c = controller(), start = 1_790_000_000_000;
  c.acceptSnapshot(snapshot(start, [resource()]));
  c.acceptSettings(settings({ enabled: false }));
  assert.deepEqual(c.visible(), []);
  c.acceptSettings(settings());
  c.acceptSnapshot(snapshot(start + 3000, [resource()]));
  assert.equal(c.history('cpu').length, 2);
  assert.equal(c.history('cpu').at(-1).gap, true);
});

test('individual hardware card escapes names, keeps GPU identity and shows traffic separately', () => {
  const c = controller(), start = 1_790_000_000_000;
  const rows = [resource('nvidia-a', { kind: 'gpu', vendor: 'NVIDIA', name: '<img src=x onerror=alert(1)>' }),
    resource('nvidia-b', { kind: 'gpu', vendor: 'NVIDIA', name: 'Other GPU secret' }),
    resource('ssd', { kind: 'disk', name: 'Physical SSD 0', read_bps: null, write_bps: 1200 }),
    resource('net', { kind: 'network', name: 'Ethernet', read_bps: 12000000, write_bps: 3000000 })];
  c.acceptSnapshot(snapshot(start, rows));
  const detail = c.cardHTML(rows[0]);
  assert.match(detail, /GPU 0 · NVIDIA/);
  assert.match(detail, /&lt;img/);
  assert.doesNotMatch(detail, /Other GPU secret|<img|Physical SSD|Ethernet/);
  assert.equal(monitor.resourceTitle(rows[1], rows), 'GPU 1 · NVIDIA');
  assert.equal(monitor.cellLabel(rows[2], rows), 'SSD');
  const traffic = c.cardHTML(rows[3]);
  assert.match(traffic, /↓ 12 MB\/s/); assert.match(traffic, /↑ 3 MB\/s/);
  assert.doesNotMatch(traffic, /100%/);
  assert.equal(monitor.chartSegments([{ time: start, value: null }], start - 60000, start, 100, 50).length, 0);
});

test('hardware refresh, rendering and context menu never enter provider quota or login paths', async () => {
  const c = controller(); c.acceptSnapshot(snapshot(1_790_000_000_000, [resource()]));
  let refreshed = 0, places = 0; const commands = [], handlers = {};
  c.refresh = () => { refreshed++; return Promise.resolve(); };
  const card = { innerHTML: '', classList: { add() {}, remove() {} } };
  const context = vm.createContext({ MachineMonitor: monitor, machineMonitor: c, hoverId: 'machine:cpu',
    document: { getElementById: () => card, addEventListener: (name, callback) => { handlers[name] = callback; } },
    resetActivityDetails() {}, resetQuotaDetails() {}, placeCard: () => { places++; }, providers: () => { throw new Error('Provider paths forbidden'); },
    invoke: command => { commands.push(command); return Promise.resolve(); }, cellAt: () => 'machine:cpu', pointerInHot: () => true,
    dragging: false, menuOpen: false, notice: assert.fail, scheduleFold() {} });
  vm.runInContext(fn(html, 'renderCard') + '\n' + fn(html, 'refreshRing'), context);
  context.renderCard(); context.refreshRing('machine:cpu');
  assert.equal(refreshed, 1); assert.equal(places, 1); assert.match(card.innerHTML, /Synthetic CPU/);
  const start = html.indexOf("document.addEventListener('contextmenu',e=>{");
  const end = html.indexOf('\n});', start);
  vm.runInContext(html.slice(start, end + 4), context);
  handlers.contextmenu({ preventDefault() {}, clientX: 1, clientY: 1 });
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(commands, ['open_monitor_settings']);
});

test('controller starts via native events/IPC only, deduplicates refresh and respects newer settings events', async () => {
  const commands = [], listeners = {}; let replySettings, replySnapshot;
  const c = monitor.createController({ listen: async (name, listener) => { listeners[name] = listener; },
    invoke: command => { commands.push(command); return command === 'get_machine_monitor_settings' ? new Promise(resolve => { replySettings = resolve; }) : new Promise(resolve => { replySnapshot = resolve; }); } });
  const started = c.start();
  await new Promise(resolve => setImmediate(resolve));
  listeners.machine_monitor_settings({ payload: settings({ hidden: ['cpu'] }) });
  replySettings(settings());
  await new Promise(resolve => setImmediate(resolve));
  const refresh = c.refresh();
  replySnapshot(snapshot(1_790_000_000_000, [resource()]));
  await Promise.all([started, refresh]);
  assert.deepEqual(commands, ['get_machine_monitor_settings', 'get_machine_snapshot']);
  assert.deepEqual(c.visible(), []);
  assert.doesNotMatch(readFileSync(join(__dirname, 'codenotch/ui/machine-monitor.js'), 'utf8'), /localStorage|indexedDB|fetch\(|writeFile|console\.log/);
});

test('carry supports measured heterogeneous offsets and retains old uniform-pitch callers', () => {
  const context = vm.createContext({}); vm.runInContext(fn(carry, 'cellOffset'), context);
  const from = { pitch: 84, offsets: [-100, -28, 47] }, to = { pitch: 58, offsets: [-74, -16, 25] };
  assert.equal(context.cellOffset(from, to, 0, 3, 0), -100);
  assert.equal(context.cellOffset(from, to, 2, 3, 1), 25);
  assert.equal(context.cellOffset(from, to, 1, 3, .5), -22);
  assert.equal(context.cellOffset({ pitch: 84 }, { pitch: 58 }, 0, 3, .5), -71);
});

test('fit retains all selected indicators on each edge, compacting hardware before providers', () => {
  for (const vertical of [true, false]) {
    const values = {}; const providers = 10, hardware = 8;
    const length = () => 38 + providers * (vertical ? 68 : 44) * Number(values['--provider-scale'] || 1)
      + hardware * (vertical ? 62 : 36) * Number(values['--hardware-scale'] || 1) + 1 + (providers + hardware) * parseFloat(values['--cell-gap'] || 14);
    const pill = { style: { setProperty: (name, value) => { values[name] = value; } }, children: Array.from({ length: 19 }, () => ({})),
      get offsetHeight() { return length(); }, get offsetWidth() { return length(); } };
    const context = vm.createContext({ pill, innerHeight: 650, innerWidth: 650, document: { documentElement: { style: { zoom: '1' } } }, edgeIsVertical: () => vertical });
    vm.runInContext(fn(html, 'fitPill'), context); context.fitPill();
    assert.equal(pill.children.length, 19);
    assert.ok(length() <= 550.01, `${vertical ? 'upright' : 'flat'} length ${length()}`);
    assert.ok(Number(values['--hardware-scale']) < Number(values['--provider-scale']));
  }
});
