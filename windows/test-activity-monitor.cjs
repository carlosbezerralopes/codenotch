'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { once } = require('node:events');
const http = require('node:http');
const {
  RETENTION_MS, trimHistory, metricValue, appendSample, chartSegments,
  reconcileOrder, moveIndicator, visibleIndicators,
} = require('./prototypes/activity-monitor/monitor.js');
const { createPreviewServer, isAllowedRequest } = require('./prototypes/activity-monitor/server.cjs');

test('indicator ordering survives sensor discovery and does not duplicate identities', () => {
  const prior = ['memory', 'cpu', 'memory', 'disconnected'];
  assert.deepEqual(reconcileOrder(prior, ['cpu', 'memory', 'disk']), ['memory', 'cpu', 'disk']);
  assert.deepEqual(prior, ['memory', 'cpu', 'memory', 'disconnected']);
});

test('indicator moves are bounded and preserve hidden item positions', () => {
  const order = ['cpu', 'memory', 'disk'];
  assert.deepEqual(moveIndicator(order, 'cpu', -1), order);
  assert.deepEqual(moveIndicator(order, 'disk', 1), order);
  assert.deepEqual(moveIndicator(order, 'absent', 1), order);
  assert.deepEqual(moveIndicator(order, 'disk', -1), ['cpu', 'disk', 'memory']);
  assert.deepEqual(order, ['cpu', 'memory', 'disk']);
});

test('visibility filters items without losing ordering or new sensors', () => {
  const items = [{ id: 'cpu' }, { id: 'memory' }, { id: 'disk' }];
  assert.deepEqual(visibleIndicators(items, ['memory', 'cpu'], new Set(['memory'])).map(item => item.id), ['cpu', 'disk']);
  assert.deepEqual(visibleIndicators(items, ['disk', 'cpu', 'memory'], new Set(['cpu', 'disk', 'memory'])), []);
});

test('history expires by elapsed time, and unknown readings never become idle', () => {
  assert.deepEqual(trimHistory([{ time: 0 }, { time: 1 }, { time: RETENTION_MS }, { time: RETENTION_MS + 2 }], RETENTION_MS + 1), [{ time: 1 }, { time: RETENTION_MS }]);
  assert.equal(metricValue({ kind: 'cpu', status: 'warming', percent: 0 }), null);
  assert.equal(metricValue({ kind: 'cpu', status: 'ok', percent: null }), null);
  assert.equal(metricValue({ kind: 'cpu', status: 'ok', percent: 0 }), 0);
  assert.equal(metricValue({ kind: 'network', status: 'ok', read_bps: null, write_bps: null }), null);
  assert.equal(metricValue({ kind: 'network', status: 'ok', read_bps: 1200, write_bps: 800 }), 1200);
});

test('pause, sensor failures and gaps break curves; refresh does not rescale elapsed time', () => {
  let samples = appendSample([], 1000, 20, 3000);
  samples = appendSample(samples, 4000, 25, 3000);
  samples = appendSample(samples, 7000, null, 3000);
  samples = appendSample(samples, 10000, 30, 3000);
  samples = appendSample(samples, 20000, 50, 3000);
  assert.deepEqual(chartSegments(samples, 0, 20000, 200, 100, 100).map(segment => segment.map(point => point.x)), [[10, 40], [100], [200]]);
  const afterResume = appendSample(samples, 23000, 40, 10000, true);
  assert.equal(afterResume.at(-1).gap, true);
  assert.deepEqual(appendSample(afterResume, 20000, 0, 3000), afterResume);
});

test('preview rejects unrelated browser origins, DNS hosts and state-changing requests', () => {
  const allowed = { method: 'GET', headers: { host: '127.0.0.1:12345' } };
  assert.equal(isAllowedRequest(allowed, 12345), true);
  assert.equal(isAllowedRequest({ ...allowed, headers: { ...allowed.headers, origin: 'http://127.0.0.1:12345' } }, 12345), true);
  assert.equal(isAllowedRequest({ ...allowed, headers: { ...allowed.headers, origin: 'https://example.com' } }, 12345), false);
  assert.equal(isAllowedRequest({ ...allowed, headers: { host: 'example.com:12345' } }, 12345), false);
  assert.equal(isAllowedRequest({ ...allowed, method: 'POST' }, 12345), false);
});

test('loopback server allows declared assets only and labels collector failure', async t => {
  const server = createPreviewServer({ sample: async () => { throw new Error('Sensor indisponível.'); } });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => new Promise(resolve => server.close(resolve)));
  const port = server.address().port;
  const request = route => new Promise((resolve, reject) => {
    http.get({ hostname: '127.0.0.1', port, path: route }, response => {
      let body = '';
      response.setEncoding('utf8');
      response.on('data', chunk => body += chunk);
      response.on('end', () => resolve({ status: response.statusCode, headers: response.headers, body }));
    }).on('error', reject);
  });
  assert.equal((await request('/')).status, 200);
  assert.equal((await request('/glyphs/codex.svg')).status, 200);
  assert.equal((await request('/collector.ps1')).status, 404);
  assert.equal((await request('/../../codenotch/src/usage.rs')).status, 404);
  const failure = await request('/api/snapshot');
  assert.equal(failure.status, 503);
  assert.equal(failure.headers['cache-control'], 'no-store');
  assert.equal(failure.headers['access-control-allow-origin'], undefined);
  assert.deepEqual(JSON.parse(failure.body), { error: 'Sensor indisponível.' });
});
