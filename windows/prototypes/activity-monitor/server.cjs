'use strict';

// Development preview only: all machine readings stay in process/browser memory.
const http = require('node:http');
const fs = require('node:fs/promises');
const path = require('node:path');
const { spawn } = require('node:child_process');

class CollectorBridge {
  constructor({ executable = process.platform === 'win32' ? 'pwsh.exe' : 'pwsh', timeoutMs = 12000 } = {}) {
    this.executable = executable;
    this.timeoutMs = timeoutMs;
    this.child = null;
    this.pending = null;
    this.last = null;
    this.lastRead = 0;
  }
  start() {
    if (this.child) return;
    const child = spawn(this.executable, ['-NoProfile', '-File', path.join(__dirname, 'collector.ps1'), '-Stream'], {
      windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'],
    });
    this.child = child;
    let buffer = '';
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', chunk => {
      buffer += chunk;
      if (buffer.length > 512 * 1024) { this.fail(new Error('Resposta do coletor excedeu o limite.')); return; }
      let newline;
      while ((newline = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, newline).trim();
        buffer = buffer.slice(newline + 1);
        if (!line) continue;
        try {
          const snapshot = JSON.parse(line);
          if (snapshot.schema_version !== 1 || !Array.isArray(snapshot.resources) || !Number.isFinite(Date.parse(snapshot.sampled_at))) {
            throw new Error('Contrato do coletor inválido.');
          }
          this.last = snapshot;
          this.lastRead = performance.now();
          const pending = this.pending;
          this.pending = null;
          if (pending) { clearTimeout(pending.timer); pending.resolve(snapshot); }
        } catch { this.fail(new Error('Resposta do coletor inválida.')); }
      }
    });
    // Drain stderr; never forward environment-specific diagnostics into browser responses.
    child.stderr.resume();
    child.stdin.on('error', () => this.fail(new Error('Coletor indisponível.')));
    child.on('error', () => this.fail(new Error('PowerShell 7 não pôde iniciar.')));
    child.on('exit', () => { if (this.child === child) this.fail(new Error('Coletor encerrou.')); });
  }
  sample() {
    if (this.pending) return this.pending.promise;
    // Protect the machine from repeated page reloads and concurrent preview clients.
    if (this.last && performance.now() - this.lastRead < 2000) return Promise.resolve(this.last);
    this.start();
    let resolve, reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    const timer = setTimeout(() => this.fail(new Error('Tempo de coleta excedido.')), this.timeoutMs);
    this.pending = { promise, resolve, reject, timer };
    this.child.stdin.write('sample\n');
    return promise;
  }
  fail(error) {
    const pending = this.pending;
    this.pending = null;
    if (pending) { clearTimeout(pending.timer); pending.reject(error); }
    const child = this.child;
    this.child = null;
    this.last = null;
    if (child) child.kill();
  }
  close() { this.fail(new Error('Prévia encerrada.')); }
}

const assets = new Map([
  ['/', ['index.html', 'text/html; charset=utf-8']],
  ['/monitor.css', ['monitor.css', 'text/css; charset=utf-8']],
  ['/monitor.js', ['monitor.js', 'text/javascript; charset=utf-8']],
  ...['claude', 'codex', 'kiro'].map(name => [`/glyphs/${name}.svg`, [`../../codenotch/glyphs/${name}.svg`, 'image/svg+xml']]),
]);

function isAllowedRequest(request, port) {
  const host = `127.0.0.1:${port}`;
  return request.method === 'GET' && request.headers.host === host &&
    (!request.headers.origin || request.headers.origin === `http://${host}`);
}

function createPreviewServer(bridge = new CollectorBridge()) {
  return http.createServer(async (request, response) => {
    const port = request.socket.localPort;
    const headers = { 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', 'Referrer-Policy': 'no-referrer' };
    const reply = (status, body, type = 'application/json; charset=utf-8') => {
      if (!response.destroyed) { response.writeHead(status, { ...headers, 'Content-Type': type }); response.end(body); }
    };
    if (!isAllowedRequest(request, port)) { reply(403, '{"error":"Pedido recusado."}'); return; }
    const route = request.url;
    if (route === '/api/snapshot') {
      try { reply(200, JSON.stringify(await bridge.sample())); }
      catch (error) { reply(503, JSON.stringify({ error: error.message })); }
      return;
    }
    const asset = assets.get(route);
    if (!asset) { reply(404, '{"error":"Página não encontrada."}'); return; }
    try { reply(200, await fs.readFile(path.resolve(__dirname, asset[0])), asset[1]); }
    catch { reply(500, '{"error":"Arquivo da prévia indisponível."}'); }
  });
}

if (require.main === module) {
  if (process.platform !== 'win32') { console.error('Este coletor de protótipo requer Windows.'); process.exit(1); }
  const port = Number(process.argv[2] ?? 0);
  if (!Number.isInteger(port) || port < 0 || port > 65535) { console.error('Porta inválida.'); process.exit(1); }
  const bridge = new CollectorBridge();
  const server = createPreviewServer(bridge);
  server.requestTimeout = 15000;
  server.headersTimeout = 10000;
  server.on('error', error => { console.error(`Não foi possível abrir a prévia: ${error.code || 'erro'}.`); bridge.close(); process.exitCode = 1; });
  server.listen(port, '127.0.0.1', () => {
    console.log(`ACTIVITY_MONITOR_URL=http://127.0.0.1:${server.address().port}/`);
    console.log('Prévia local; Ctrl+C encerra. Métricas apenas em memória.');
  });
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGBREAK']) {
    process.on(signal, () => { bridge.close(); server.close(); });
  }
}

module.exports = { CollectorBridge, createPreviewServer, isAllowedRequest };
