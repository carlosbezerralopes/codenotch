const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const { test } = require('node:test');
const notch = readFileSync(join(__dirname, 'codenotch/ui/notch.html'), 'utf8');
const settings = readFileSync(join(__dirname, 'codenotch/ui/settings.html'), 'utf8');

function selector(name) {
  const begin = `// BEGIN TESTABLE ${name} SELECTOR`;
  const end = `// END TESTABLE ${name} SELECTOR`;
  assert.equal(notch.split(begin).length, 2);
  return notch.slice(notch.indexOf(begin) + begin.length, notch.indexOf(end));
}
function fn(source, name) {
  const start = source.indexOf(`function ${name}(`);
  assert.ok(start >= 0, `${name} exists`);
  const end = source.indexOf('\n}', start);
  assert.ok(end > start);
  return source.slice(start, end + 2);
}

test('Kiro draws only declared credits and never a weekly bonus ring', () => {
  const c = vm.createContext({});
  vm.runInContext(selector('HEADLINE') + selector('WEEKLY'), c);
  const windows = [{ id: 'bonus', used: 0.9 }, { id: 'overage', used: 1 }, { id: 'credits', used: 0.25 }];
  assert.equal(c.headlineOf({ windows }, 'kiro').id, 'credits');
  assert.equal(c.weeklyOf({ windows }, 'kiro'), null);
  assert.equal(c.headlineOf({ windows: windows.slice(0, 2) }, 'kiro'), null);
  assert.equal(c.headlineOf({ windows: [] }, 'kiro'), null);
});

test('Installation and sign-out are distinct; selection hides the Kiro cell', () => {
  const absent = () => ({ status: 'absent', windows: [] });
  const c = vm.createContext({ claudeCells: () => [], codexSnap: absent(), glmSnap: absent(), opencodeSnap: absent(), cursorSnap: absent(), grokSnap: absent(), copilotSnap: absent(), agSnap: absent(), kiroSnap: absent(), notchSlots: [], slotFor: id => c.notchSlots.find(s => s.provider === id) });
  vm.runInContext(fn(notch, 'providers'), c);
  assert.equal(c.providers().length, 0);
  c.kiroSnap = { status: 'needsAuth', windows: [], note: 'Run kiro-cli login' };
  assert.equal(c.providers()[0].id, 'kiro');
  c.codexSnap = { status: 'ok', windows: [{ id: 'primary', used: 0.2 }] };
  c.notchSlots = [{ provider: 'codex' }];
  assert.deepEqual(Array.from(c.providers(), p => p.id), ['codex']);
  c.notchSlots = [{ provider: 'kiro' }];
  assert.deepEqual(Array.from(c.providers(), p => p.id), ['kiro']);
  c.kiroSnap = absent();
  assert.ok(!c.providers().some(p => p.id === 'kiro'));
});

test('Accounts toggle Kiro off and on using the existing settings contract', () => {
  const ids = ['codex', 'kiro'];
  let saved;
  const c = vm.createContext({ providerList: () => ids.map(id => ({ id })), notchSlots: [], ui: (_, fallback) => fallback, toast: () => {}, saveNotch: slots => { saved = Array.from(slots, s => s.provider); c.notchSlots = slots; } });
  vm.runInContext(fn(settings, 'notchOn') + fn(settings, 'toggleNotch'), c);
  c.toggleNotch('kiro');
  assert.deepEqual(saved, ['codex']);
  c.toggleNotch('kiro');
  assert.deepEqual(saved, ['codex', 'kiro']);
  c.notchSlots = [{ provider: 'kiro' }];
  saved = null;
  c.toggleNotch('kiro');
  assert.equal(saved, null, 'the last selected provider remains available');
});

test('The official upstream Kiro loops are reused as artwork', () => {
  const svg = readFileSync(join(__dirname, 'codenotch/glyphs/kiro.svg'), 'utf8');
  const outline = readFileSync(join(__dirname, '../Sources/Providers/GlyphOutline.swift'), 'utf8');
  const start = outline.indexOf('static let kiro: [[CGPoint]]');
  const stop = outline.indexOf('\n    ]', start);
  const points = [...outline.slice(start, stop).matchAll(/CGPoint\(x: ([0-9.]+), y: ([0-9.]+)\)/g)];
  assert.ok(points.length > 100);
  for (const [, x, y] of points) assert.ok(svg.includes(`${x} ${y}`));
  assert.equal((svg.match(/ Z/g) || []).length, 3);
});
