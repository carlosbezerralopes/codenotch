const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const { test } = require('node:test');
const source = readFileSync(join(__dirname, 'codenotch/ui/notch.html'), 'utf8');
const begin = '// BEGIN TESTABLE ACTIVITY PRIVACY';
const end = '// END TESTABLE ACTIVITY PRIVACY';
assert.equal(source.split(begin).length, 2);
assert.equal(source.split(end).length, 2);
const privacy = source.split(begin)[1].split(end)[0];
const insights = readFileSync(join(__dirname, 'codenotch/ui/usage-insights.js'), 'utf8');
const pace = source.split('// BEGIN TESTABLE WORK PACE')[1].split('// END TESTABLE WORK PACE')[0];
function fn(name) {
  const start = source.indexOf(`function ${name}(`);
  assert.ok(start >= 0, `${name} exists`);
  const stop = source.indexOf('\n}', start);
  assert.ok(stop > start, `${name} has a body`);
  return source.slice(start, stop + 2);
}
function oneLine(name) {
  const start = source.indexOf(`function ${name}(`);
  return source.slice(start, source.indexOf('\n', start));
}
function fixture(base = 'claude', lang = 'pt-BR') {
  let p = { id: base, base, name: 'Custom provider', snap: { status: 'ok', windows: [
    { id: 'primary', label: 'Quota', used: 0.25 },
  ] } };
  const classes = new Set();
  const toggle = { focus: () => { toggle.focused = true; } };
  const signIn = {};
  const card = { innerHTML: '', onclick: null,
    classList: { add: name => classes.add(name), remove: name => classes.delete(name), contains: name => classes.has(name) },
    querySelector: selector => selector === '#activity-details-toggle' && card.innerHTML.includes('id="activity-details-toggle"') ? toggle
      : selector === '#claude-sign-in' && card.innerHTML.includes('id="claude-sign-in"') ? signIn : null,
  };
  const escape = value => String(value ?? '').replace(/[&<>"']/g, ch => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[ch]));
  const c = vm.createContext({
    document: { getElementById: () => card }, card, uiLang: lang, providers: () => p ? [p] : [], hoverId: base,
    glyphHtml: () => '', staleOf: () => false, textCopy: value => value, esc: escape,
    ui: () => ({ title: name => name, andMore: count => `and ${count} more` }), resetCopy: () => '',
    tone: () => '#fff', usedCopy: () => '25% used', creditAmountsHtml: () => '',
    offersClaudeSignIn: row => row.id === 'claude' && row.snap.status === 'needsAuth',
    claudeAuthHtml: () => '<button id="claude-sign-in">Sign in</button>', claudeAuth: {}, claudeActionMessage: '',
    signInClaude: () => { c.signIns += 1; }, signIns: 0,
    activity: [], stateSnap: { sessions: [], agg: 'idle' }, SESSION_ROWS: 5,
    STATE_DOT: { running: '#fff', attention: '#aa0', done: '#0a0', idle: '#666' }, TRACK: '#666', WATCH: '#aa0', INK: '#fff',
    moreRow: count => count > 0 ? `<div>${count} more</div>` : '', placeCard: () => {}, reportHot: () => {},
    hideTimer: null, clearTimeout: () => {}, armWatchdog: () => {}, refreshClock: () => {},
  });
  vm.runInContext([insights, privacy, pace, fn('renderCard'), oneLine('showCard'), oneLine('hideCard')].join('\n'), c);
  return { c, card, toggle, signIn, setProvider: value => { p = value; c.hoverId = value?.id ?? null; } };
}
function click(card, control = 'card') {
  const event = { stopped: false, stopPropagation() { this.stopped = true; }, target: {
    closest(selector) {
      if (selector === '#activity-details-toggle') return control === 'toggle' ? {} : null;
      return control === 'login' || control === 'other-button' ? {} : null;
    },
  } };
  card.onclick(event);
  return event;
}
function sensitiveRows(c) {
  c.stateSnap = { agg: 'running', sessions: [
    { state: 'running', title: 'SecretProject C:\\private\\repo <img src=x onerror=alert(1)> SecretPrompt' },
  ] };
  c.activity = [{ provider: 'codex', state: 'waiting', name: 'SecretProject', detail: 'C:\\private\\repo <script>SecretPrompt</script>', since: 1 }];
}

test('Hover renders only aggregate activity and never inserts session/project/path/prompt details', () => {
  const { c, card } = fixture();
  sensitiveRows(c);
  c.showCard();
  assert.match(card.innerHTML, /Trabalhando/);
  assert.match(card.innerHTML, /Mostrar detalhes/);
  assert.match(card.innerHTML, /aria-expanded="false"/);
  assert.doesNotMatch(card.innerHTML, /SecretProject|private|SecretPrompt|id="activity-details"/);
  assert.match(card.innerHTML, /Custom provider/);
  assert.match(card.innerHTML, /width:25%|25% used/);
});

test('Explicit balloon/status click reveals the existing rows with escaped markup', () => {
  const { c, card } = fixture();
  sensitiveRows(c);
  c.showCard();
  assert.equal(click(card).stopped, true);
  assert.match(card.innerHTML, /SecretProject/);
  assert.match(card.innerHTML, /C:\\private\\repo/);
  assert.match(card.innerHTML, /&lt;img src=x onerror=alert\(1\)&gt;/);
  assert.doesNotMatch(card.innerHTML, /<img src=x/);
  assert.match(card.innerHTML, /Ocultar detalhes/);
  assert.match(card.innerHTML, /aria-expanded="true"/);
  click(card, 'toggle');
  assert.doesNotMatch(card.innerHTML, /SecretProject|SecretPrompt|private/);
});

test('Native keyboard button exposes disclosure semantics and restores its focus', () => {
  const { c, card, toggle } = fixture('codex', 'en');
  sensitiveRows(c);
  c.showCard();
  assert.match(card.innerHTML, /Waiting for your input/);
  assert.match(card.innerHTML, /<button id="activity-details-toggle"[^>]+aria-expanded="false"[^>]+>Show details<\/button>/);
  click(card, 'toggle');
  assert.equal(toggle.focused, true);
  assert.match(card.innerHTML, /Hide details/);
  assert.match(card.innerHTML, /&lt;script&gt;SecretPrompt&lt;\/script&gt;/);
  assert.doesNotMatch(card.innerHTML, /<script>/);
});

test('Hide removes sensitive DOM and reopening always starts collapsed', () => {
  const { c, card } = fixture();
  sensitiveRows(c);
  c.showCard();
  click(card, 'toggle');
  assert.match(card.innerHTML, /SecretPrompt/);
  c.hideCard();
  assert.equal(card.innerHTML, '');
  assert.equal(card.onclick, null);
  assert.equal(card.classList.contains('show'), false);
  c.showCard();
  assert.doesNotMatch(card.innerHTML, /SecretProject|SecretPrompt|private/);
  assert.match(card.innerHTML, /aria-expanded="false"/);
});

test('Changing provider resets disclosure, while same-provider refresh preserves an explicit expansion', () => {
  const { c, card, setProvider } = fixture();
  sensitiveRows(c);
  c.showCard();
  click(card, 'toggle');
  c.renderCard();
  assert.match(card.innerHTML, /SecretPrompt/);
  setProvider({ id: 'codex', base: 'codex', name: 'Codex', snap: { status: 'ok', windows: [] } });
  c.renderCard();
  assert.doesNotMatch(card.innerHTML, /SecretProject|SecretPrompt|private/);
  assert.match(card.innerHTML, /aria-expanded="false"/);
  assert.match(card.innerHTML, /Aguardando sua resposta/);
});

test('Login and other interactive controls never reveal activity details', () => {
  const { c, card, signIn, setProvider } = fixture();
  sensitiveRows(c);
  setProvider({ id: 'claude', base: 'claude', name: 'Claude', snap: { status: 'needsAuth', windows: [] } });
  c.showCard();
  assert.equal(click(card, 'login').stopped, false);
  assert.equal(click(card, 'other-button').stopped, false);
  signIn.onclick({ stopPropagation: () => {} });
  assert.equal(c.signIns, 1);
  assert.doesNotMatch(card.innerHTML, /SecretProject|SecretPrompt|private/);
});

test('Aggregate status uses observed states and never calls unsupported Kiro idle or finished', () => {
  const { c } = fixture();
  for (const state of ['running', 'attention', 'done', 'idle']) {
    c.stateSnap.agg = state;
    assert.equal(c.activityStatus('claude'), state);
  }
  c.stateSnap.agg = undefined;
  assert.equal(c.activityStatus('claude'), 'unmonitored');
  c.stateSnap.agg = 'done';
  c.activity = [{ provider: 'claude', state: 'busy' }];
  assert.equal(c.activityStatus('claude'), 'running', 'current cloud activity takes precedence over a finished local session');
  c.activity = [];
  for (const base of ['codex', 'cursor', 'gemini']) assert.equal(c.activityStatus(base), 'idle');
  for (const base of ['kiro', 'glm', 'grok', 'copilot', 'opencode']) assert.equal(c.activityStatus(base), 'unmonitored');
  c.activity = [{ provider: 'cursor', state: 'busy' }, { provider: 'cursor', state: 'waiting' }];
  assert.equal(c.activityStatus('cursor'), 'attention');
  c.activity = [{ provider: 'kiro', state: 'busy', name: 'SecretProject' }];
  assert.equal(c.activityStatus('kiro'), 'unmonitored');
  assert.match(c.activityPrivacyHtml({ base: 'kiro' }), /Atividade não monitorada/);
  assert.doesNotMatch(c.activityPrivacyHtml({ base: 'kiro' }), /Mostrar detalhes|SecretProject/);
});

test('No available provider remains safe after an expanded card disappears', () => {
  const { c, card, setProvider } = fixture();
  sensitiveRows(c);
  c.showCard();
  click(card, 'toggle');
  setProvider(null);
  assert.doesNotThrow(() => c.renderCard());
  assert.equal(card.innerHTML, '');
  assert.equal(c.hoverId, null);
  assert.equal(card.classList.contains('show'), false);
});
