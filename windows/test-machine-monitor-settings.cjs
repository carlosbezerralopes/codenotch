const { test } = require('node:test');
const assert = require('node:assert/strict');
const { createController, mount } = require('./codenotch/ui/machine-monitor-settings.js');
const initial = () => ({ enabled: true, refresh_seconds: 3, history_minutes: 5,
  order: ['gpu:missing', 'cpu', 'gpu:intel:0', 'gpu:intel:1'], hidden: ['gpu:missing', 'gpu:intel:1'] });
const inventory = () => ({ supported: true, resources: [
  { id: 'cpu', name: 'CPU', kind: 'cpu' },
  { id: 'gpu:intel:0', name: 'GPU 0', subtitle: 'Intel Graphics', vendor: 'Intel', kind: 'gpu' },
  { id: 'gpu:intel:1', name: 'GPU 1', subtitle: 'Intel Graphics', vendor: 'Intel', kind: 'gpu' },
  { id: 'network:guid', name: 'Ethernet', kind: 'network' }
] });
function fixture() {
  let persisted = initial(), fail = false, writes = [];
  const invoke = async (command, args) => {
    if (command === 'get_machine_monitor_settings') return structuredClone(persisted);
    if (command === 'get_machine_snapshot') return inventory();
    if (command === 'set_machine_monitor_settings') {
      writes.push(structuredClone(args.settings));
      if (fail) throw Error('Disk write failed');
      persisted = structuredClone(args.settings); return structuredClone(persisted);
    }
    throw Error(command);
  };
  const controller = createController(invoke);
  return { invoke, controller, writes, fail: value => { fail = value; } };
}
test('device selection and order survive command roundtrip and recreation, including absent devices', async () => {
  const f = fixture(), c = f.controller;
  await c.reload(); c.acceptSnapshot(inventory());
  c.move('gpu:intel:1', -1); c.toggle('cpu', false); c.toggle('gpu:intel:0', false); c.toggle('network:guid', false);
  c.edit({ refresh_seconds: 10, history_minutes: 15, enabled: false });
  assert.equal(await c.save(), true);
  const next = createController(f.invoke); await next.reload(); next.acceptSnapshot(inventory());
  assert.deepEqual(next.state().draft, { enabled: false, refresh_seconds: 10, history_minutes: 15,
    order: ['gpu:missing', 'cpu', 'gpu:intel:1', 'gpu:intel:0', 'network:guid'],
    hidden: ['gpu:missing', 'gpu:intel:1', 'cpu', 'gpu:intel:0', 'network:guid'] });
});
test('failed save keeps dirty draft and persisted baseline; cancel and reload restore actual settings', async () => {
  const f = fixture(), c = f.controller; await c.reload(); c.acceptSnapshot(inventory());
  c.toggle('cpu', false); f.fail(true); assert.equal(await c.save(), false);
  assert.equal(c.state().saved, false); assert.equal(c.state().dirty, true);
  assert.match(c.state().error, /Disk write failed/); assert.deepEqual(c.state().persisted, initial());
  c.acceptSettings({ ...initial(), refresh_seconds: 5 });
  assert.equal(c.state().draft.refresh_seconds, 3); assert.ok(c.state().draft.hidden.includes('cpu'));
  c.cancel(); assert.deepEqual(c.state().draft, initial());
  c.toggle('cpu', false); await c.reload(); assert.deepEqual(c.state().draft, initial());
});
test('settings are not editable before loading, during save or on an unsupported platform', async () => {
  let resolve; const f = fixture(), c = f.controller;
  c.edit({ enabled: false }); assert.equal(c.state().draft, null);
  await c.reload(); c.acceptSnapshot({ supported: false, resources: [] });
  c.edit({ enabled: false }); assert.equal(c.state().dirty, false);
  c.acceptSnapshot(inventory()); c.edit({ enabled: false });
  const saving = createController(async command => command === 'get_machine_monitor_settings' ? initial() : new Promise(r => { resolve = r; }));
  await saving.reload(); saving.acceptSnapshot(inventory()); saving.edit({ enabled: false });
  const pending = saving.save(); saving.edit({ refresh_seconds: 10 }); saving.acceptSettings(initial());
  assert.equal(saving.state().draft.enabled, false); assert.equal(saving.state().draft.refresh_seconds, 3);
  assert.equal(saving.state().saved, false); resolve({ ...initial(), enabled: false }); await pending;
  assert.equal(saving.state().saved, true);
});
test('an initial getter cannot overwrite a newer event', async () => {
  let resolve; const c = createController(() => new Promise(r => { resolve = r; }));
  const loading = c.reload(); c.acceptSettings({ ...initial(), refresh_seconds: 10 });
  resolve(initial()); await loading; assert.equal(c.state().draft.refresh_seconds, 10);
});
class Element {
  constructor(tag, doc) { this.tagName = tag; this.ownerDocument = doc; this.children = []; this.style = {}; this.attributes = {}; this.listeners = {}; }
  append(...nodes) { nodes.forEach(node => this.insertBefore(node, null)); }
  insertBefore(node, before) {
    if (node.parent) node.remove();
    const index = before ? this.children.indexOf(before) : this.children.length;
    this.children.splice(index, 0, node); node.parent = this;
  }
  remove() { const p = this.parent; if (p) { p.children.splice(p.children.indexOf(this), 1); this.parent = null; } }
  setAttribute(key, value) { this.attributes[key] = value; }
  addEventListener(name, fn) { this.listeners[name] = fn; }
}
test('mount preserves device control nodes on live samples and dirty drafts on settings events', async () => {
  const f = fixture(), callbacks = {}, doc = { createElement: tag => new Element(tag, doc) }, host = new Element('div', doc);
  const pane = mount({ host, invoke: f.invoke, listen: async (event, callback) => { callbacks[event] = callback; return () => {}; } });
  await pane.ready;
  const list = host.children[3], firstRow = list.children[0], check = firstRow.children[0].children[0];
  let detaches = 0; const remove = firstRow.remove.bind(firstRow); firstRow.remove = () => { detaches++; remove(); };
  check.checked = false; check.listeners.change();
  callbacks.machine_snapshot({ payload: { ...inventory(), resources: inventory().resources.map(r => ({ ...r, percent: 80, value: 'changed' })) } });
  callbacks.machine_monitor_settings({ payload: initial() });
  assert.equal(list.children[0], firstRow); assert.equal(firstRow.children[0].children[0], check);
  assert.equal(detaches, 0); assert.equal(check.checked, false); assert.equal(pane.controller.state().dirty, true);
  assert.notEqual(list.children[1].children[0].children[2].textContent, list.children[2].children[0].children[2].textContent);
  pane.dispose();
});
