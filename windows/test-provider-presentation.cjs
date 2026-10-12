const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const { test } = require('node:test');
const settings = readFileSync(join(__dirname, 'codenotch/ui/settings.html'), 'utf8');
const notch = readFileSync(join(__dirname, 'codenotch/ui/notch.html'), 'utf8');

function fn(source, name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert.ok(start >= 0, `${name} exists`);
  const end = source.indexOf('\n}', start);
  assert.ok(end > start);
  return source.slice(start, end + 2);
}
function settingsContext(ids = ['claude', 'codex', 'kiro']) {
  const context = vm.createContext({
    providerList: () => ids.map(id => ({ id, label: id, status: '' })),
    providerOrder: [], providerNicknames: {}, notchSlots: [], accountsBusy: false,
    editingName: null, nameDraft: '', FALLBACK_LABEL: { claude: 'Claude', codex: 'Codex', kiro: 'Kiro' },
    renderAccounts: () => {}, toast: () => {}, strip: () => {}, errText: String, ui: (_, fallback) => fallback,
  });
  vm.runInContext(['notchOn', 'arrangeProviders', 'providerName', 'joinedOrder', 'writeProviderOrder',
    'saveNotch', 'toggleNotch', 'moveProvider', 'saveProviderName'].map(name => fn(settings, name)).join('\n'), context);
  context.invoke = async (command, args) => {
    if (command === 'set_provider_order') return [...args.order];
    if (command === 'set_notch_slots') return args.slots;
    if (command === 'set_provider_nickname') {
      const next = { ...context.providerNicknames };
      if (args.name.trim()) next[args.provider] = args.name.trim();
      else delete next[args.provider];
      return next;
    }
    throw new Error(command);
  };
  return context;
}
const idsOf = rows => Array.from(rows, row => row.id);
const slotsOf = rows => Array.from(rows, row => row.provider);
function notchContext() {
  const absent = () => ({ status: 'absent', windows: [] });
  const context = vm.createContext({
    usage: { status: 'ok', windows: [] }, codexSnap: absent(), glmSnap: absent(), opencodeSnap: absent(),
    cursorSnap: absent(), grokSnap: absent(), copilotSnap: absent(), agSnap: absent(), kiroSnap: absent(),
    notchSlots: [], providerOrder: [], providerNicknames: {},
  });
  vm.runInContext(['slotFor', 'claudeCells', 'providerDisplayName', 'presentProviders', 'providers']
    .map(name => fn(notch, name)).join('\n'), context);
  return context;
}

test('Reordering all selected providers keeps visibility automatic and survives reload', async () => {
  const c = settingsContext();
  await c.moveProvider('kiro', 'claude');
  assert.deepEqual(Array.from(c.providerOrder), ['kiro', 'claude', 'codex']);
  await c.saveNotch(c.notchOn());
  assert.deepEqual(slotsOf(c.notchSlots), []);
  assert.deepEqual(Array.from(c.providerOrder), ['kiro', 'claude', 'codex']);
  const reloaded = settingsContext(['claude', 'codex', 'kiro', 'new-provider']);
  reloaded.providerOrder = JSON.parse(JSON.stringify(c.providerOrder));
  assert.deepEqual(idsOf(reloaded.arrangeProviders(reloaded.providerList(), reloaded.providerOrder)),
    ['kiro', 'claude', 'codex', 'new-provider']);
});

test('Switching a provider back on joins the end without shuffling the active order', async () => {
  const c = settingsContext();
  c.providerOrder = ['kiro', 'codex', 'claude'];
  c.notchSlots = [{ provider: 'codex' }, { provider: 'claude' }];
  await c.saveNotch([...c.notchOn(), { provider: 'kiro' }], 'kiro');
  assert.deepEqual(Array.from(c.providerOrder), ['codex', 'claude', 'kiro']);
  assert.deepEqual(slotsOf(c.notchSlots), []);
  c.notchSlots = [{ provider: 'kiro' }];
  await c.moveProvider('kiro', 'codex');
  assert.deepEqual(Array.from(c.providerOrder), ['codex', 'claude', 'kiro'], 'disabled target cannot move');
});

test('Visibility edits retain unavailable and unknown saved identities', async () => {
  const c = settingsContext();
  c.notchSlots = [{ provider: 'not-installed' }, { provider: 'codex' }];
  assert.deepEqual(slotsOf(c.notchOn()), ['not-installed', 'codex']);
  await c.saveNotch([...c.notchOn(), { provider: 'kiro' }], 'kiro');
  assert.deepEqual(slotsOf(c.notchSlots), ['not-installed', 'codex', 'kiro']);
  c.notchSlots = [{ provider: 'codex' }];
  let calls = 0;
  c.saveNotch = () => { calls += 1; };
  c.toggleNotch('codex');
  assert.equal(calls, 0, 'the last selected provider cannot be switched off');
});

test('Failed persistence leaves confirmed values visible and unlocks the controls', async () => {
  const c = settingsContext();
  c.providerOrder = ['codex', 'claude', 'kiro'];
  c.notchSlots = [{ provider: 'codex' }, { provider: 'kiro' }];
  c.providerNicknames = { codex: 'Work' };
  c.invoke = async () => { throw new Error('Disk unavailable'); };
  await c.moveProvider('kiro', 'codex');
  await c.saveNotch([{ provider: 'codex' }]);
  await c.saveProviderName('codex', 'Changed');
  assert.deepEqual(Array.from(c.providerOrder), ['codex', 'claude', 'kiro']);
  assert.deepEqual(slotsOf(c.notchSlots), ['codex', 'kiro']);
  assert.equal(c.providerNicknames.codex, 'Work');
  assert.equal(c.accountsBusy, false);
});

test('Names can be changed and cleared without changing provider identity or visibility', async () => {
  const c = settingsContext();
  await c.saveProviderName('codex', '  Work <account>  ');
  assert.equal(c.providerName({ id: 'codex' }), 'Work <account>');
  await c.saveProviderName('codex', '   ');
  assert.equal(c.providerName({ id: 'codex', label: 'Old cached nickname' }), 'Codex');
  assert.deepEqual(slotsOf(c.notchSlots), []);
  assert.deepEqual(Array.from(c.providerOrder), []);
  let calls = 0;
  c.invoke = async () => { calls += 1; };
  for (const name of ['x'.repeat(61), 'x\n', 'x\u0085']) await c.saveProviderName('kiro', name);
  assert.equal(calls, 0, 'invalid names never reach IPC');
});

test('Notch order is stable across new and unavailable providers, with no absent-only fallback', () => {
  const c = notchContext();
  c.codexSnap = { status: 'ok', windows: [{ id: 'primary', used: 0.2 }] };
  c.kiroSnap = { status: 'ok', windows: [{ id: 'credits', used: 0.1 }] };
  c.providerOrder = ['kiro', 'removed', 'claude', 'codex'];
  assert.deepEqual(idsOf(c.providers()), ['kiro', 'claude', 'codex']);
  c.cursorSnap = { status: 'ok', windows: [] };
  assert.deepEqual(idsOf(c.providers()), ['kiro', 'claude', 'codex', 'cursor']);
  c.notchSlots = [{ provider: 'opencode' }];
  assert.equal(c.providers().length, 0, 'explicit unavailable selection stays unavailable');
  c.notchSlots = [{ provider: 'unknown' }];
  assert.equal(c.providers().length, 0);
});

test('Claude accounts remain a stable group and retain profile distinctions after renaming', () => {
  const c = notchContext();
  c.usage.windows = [
    { id: 'session', group: 'Claude (Pro)', used: 0.1 },
    { id: 'session@work', group: 'Claude (work) · Pro', used: 0.2 },
  ];
  c.codexSnap = { status: 'ok', windows: [] };
  c.providerOrder = ['codex', 'claude'];
  c.providerNicknames = { claude: 'Assistant', codex: 'My Codex' };
  const rows = c.providers();
  assert.deepEqual(idsOf(rows), ['codex', 'claude', 'claude@work']);
  assert.equal(rows[0].name, 'My Codex');
  assert.equal(rows[1].name, 'Assistant (Pro)');
  assert.ok(rows[2].name.includes('work'));
  assert.equal(rows[2].snap.windows[0].used, 0.2, 'account quotas keep their identity');
  assert.equal(rows[2].snap.windows[0].id, 'session');
  c.providerNicknames = {};
  assert.equal(c.providers()[0].name, 'Codex');
});

test('An empty provider selection hides its open card safely', () => {
  let hidden = 0;
  const card = { innerHTML: 'Old content' };
  const c = vm.createContext({ document: { getElementById: () => card }, providers: () => [], hoverId: 'kiro', hideCard: () => { hidden += 1; } });
  vm.runInContext(fn(notch, 'renderCard'), c);
  c.renderCard();
  assert.equal(hidden, 1);
  assert.equal(card.innerHTML, '');
  assert.equal(c.hoverId, null);
});

test('Account rows expose keyboard reorder controls and escape custom names', () => {
  const c = settingsContext();
  Object.assign(c, { statusText: () => '', glyphHtml: () => '', hooks: {},
    esc: text => String(text).replace(/[&<>"']/g, ch => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[ch])),
  });
  vm.runInContext(fn(settings, 'accountBlock'), c);
  c.providerNicknames = { kiro: '<img onerror="alert(1)">' };
  const row = c.accountBlock({ id: 'kiro', label: 'Kiro' }, true, false, 1, 3);
  assert.ok(row.includes('aria-label="Move up:'));
  assert.ok(row.includes('aria-label="Move down:'));
  assert.ok(row.includes('data-drag="kiro"'));
  assert.ok(!row.includes('<img onerror='));
  const off = c.accountBlock({ id: 'kiro', label: 'Kiro' }, false, false);
  assert.ok(!off.includes('data-drag='));
  assert.ok(!off.includes('data-move='));
});
