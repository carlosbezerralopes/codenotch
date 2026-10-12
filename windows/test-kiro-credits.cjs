const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const { test } = require('node:test');
const source = readFileSync(join(__dirname, 'codenotch/ui/notch.html'), 'utf8');
const amounts = source.split('// BEGIN TESTABLE CREDIT AMOUNTS')[1].split('// END TESTABLE CREDIT AMOUNTS')[0];
const privacy = source.split('// BEGIN TESTABLE ACTIVITY PRIVACY')[1].split('// END TESTABLE ACTIVITY PRIVACY')[0];
const insights = readFileSync(join(__dirname, 'codenotch/ui/usage-insights.js'), 'utf8');
const pace = source.split('// BEGIN TESTABLE WORK PACE')[1].split('// END TESTABLE WORK PACE')[0];
const tables = source.slice(source.indexOf('const UI='), source.indexOf('function textCopy('));
function context(lang = 'pt-BR') {
  const c = vm.createContext({ uiLang: lang, Intl, esc: s => s });
  vm.runInContext(insights + tables + amounts + privacy + pace, c);
  return c;
}
const window = (consumed, allowance = 50) => ({ credits: { consumed, allowance } });

test('actual credit formatter preserves locale, decimals and integer allowance', () => {
  const c = context();
  assert.equal(c.creditAmountsCopy(window(12.5)), '12,50 de 50 créditos consumidos');
  assert.equal(c.creditAmountsCopy(window(0.17)), '0,17 de 50 créditos consumidos');
  assert.equal(c.creditAmountsCopy(window(0)), '0 de 50 créditos consumidos');
  assert.equal(c.creditAmountsCopy(window(0.123456)), '0,123456 de 50 créditos consumidos');
  assert.equal(c.creditAmountsCopy(window(0.25, 1000)), '0,25 de 1.000 créditos consumidos');
  const en = context('en');
  assert.equal(en.creditAmountsCopy(window(12.5)), '12.50 of 50 credits used');
  for (const lang of ['ko', 'uk', 'ru', 'zh', 'zh-Hant', 'unknown']) assert.ok(context(lang).creditAmountsCopy(window(0.17)));
});

test('unknown and malformed amounts are hidden without coercion or invented zeros', () => {
  const c = context();
  for (const w of [{}, { credits: null }, { credits: {} }, window(null), window('0.17'), window(NaN), window(Infinity), window(-1), window(1, 0), window(1, -50), window(1, '50')]) {
    assert.equal(c.creditAmountsCopy(w), '');
    assert.equal(c.creditAmountsHtml({ base: 'kiro' }, w), '');
  }
});

test('actual renderCard adds credits only for Kiro while preserving official percent and bar', () => {
  const c = context();
  const start = source.indexOf('function renderCard(');
  const end = source.indexOf('\n}', start) + 2;
  let p = { id: 'kiro', base: 'kiro', name: 'Kiro', snap: { status: 'ok', windows: [{ id: 'credits', label: 'Credits', used: 0, ...window(0.17) }] } };
  const card = { innerHTML: '', querySelector: () => null };
  Object.assign(c, { document: { getElementById: () => card }, providers: () => [p], hoverId: 'kiro', glyphHtml: () => '', staleOf: () => false, textCopy: s => s,
    resetCopy: () => '', tone: () => '#fff', usedCopy: w => `${Math.round(w.used * 100)}% used`, offersClaudeSignIn: () => false,
    activity: [], stateSnap: { sessions: [], agg: 'idle' }, STATE_DOT: {}, TRACK: '#666',
    placeCard: () => {}, reportHot: () => {}, sessionRows: () => {} });
  vm.runInContext(source.slice(start, end), c);
  c.renderCard();
  assert.match(card.innerHTML, /width:0%/);
  assert.match(card.innerHTML, /0% used/);
  assert.match(card.innerHTML, /0,17 de 50 créditos consumidos/);
  assert.ok(card.innerHTML.indexOf('0% used') < card.innerHTML.indexOf('w-credits'));
  for (const base of ['codex', 'claude', 'cursor', 'gemini', 'copilot', 'glm', 'opencode', 'grok']) {
    p = { ...p, id: base, base };
    c.renderCard();
    assert.doesNotMatch(card.innerHTML, /w-credits|créditos consumidos/);
  }
  p = { ...p, id: 'kiro', base: 'kiro', snap: { status: 'ok', windows: [{ id: 'credits', label: 'Credits', used: 0.25 }] } };
  c.renderCard();
  assert.match(card.innerHTML, /25% used/);
  assert.doesNotMatch(card.innerHTML, /w-credits/);
});
