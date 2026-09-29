#!/usr/bin/env node
// Raw-LSP smoke test: speaks real JSON-RPC to `declint serve` without
// VS Code, proving the protocol path the extension depends on.
//
// Usage: node scripts/lsp-smoke.js [path-to-declint]   (default: declint)

const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const {
  createMessageConnection,
  StreamMessageReader,
  StreamMessageWriter,
} = require('vscode-jsonrpc/node');

const declint = process.argv[2] || 'declint';

const fail = (message) => {
  console.error(`SMOKE FAIL: ${message}`);
  process.exit(1);
};

const withTimeout = (promise, ms, label) =>
  Promise.race([
    promise,
    new Promise((_, reject) =>
      setTimeout(
        () => reject(new Error(`timed out waiting for ${label}`)),
        ms,
      ),
    ),
  ]);

// The server discovers its config from the working directory, so the
// fixture project is the spawn cwd.
const project = fs.mkdtempSync(path.join(os.tmpdir(), 'declint-smoke-'));
fs.writeFileSync(
  path.join(project, '.declint.yaml'),
  "version: 1\nlanguages: [ini]\nrules:\n  - id: no-tabs\n    pattern: '\\t+'\n    message: 'Tab found'\n    severity: warning\n",
);
fs.writeFileSync(path.join(project, 'sample.ini'), 'key = value\t\n');

const proc = spawn(declint, ['serve'], { cwd: project });
proc.on('error', (err) => fail(`cannot spawn ${declint}: ${err.message}`));

const connection = createMessageConnection(
  new StreamMessageReader(proc.stdout),
  new StreamMessageWriter(proc.stdin),
);

const finish = (code, message) => {
  try {
    connection.dispose();
    proc.kill();
  } catch {}
  if (message) {
    console.log(message);
  }
  process.exit(code);
};

(async () => {
  connection.listen();

  const rootUri = `file://${project.split(path.sep).join('/')}`;
  const capabilities = await withTimeout(
    connection.sendRequest('initialize', {
      processId: process.pid,
      capabilities: {},
      rootUri,
    }),
    10_000,
    'initialize response',
  );
  if (!capabilities || !capabilities.capabilities) {
    fail('initialize returned no capabilities');
  }
  connection.sendNotification('initialized', {});

  const uri = `${rootUri}/sample.ini`;
  connection.sendNotification('textDocument/didOpen', {
    textDocument: {
      uri,
      languageId: 'ini',
      version: 0,
      text: 'key = value\t\n',
    },
  });

  const published = await withTimeout(
    new Promise((resolve) => {
      connection.onNotification('textDocument/publishDiagnostics', (p) => {
        if (p.uri === uri) {
          resolve(p);
        }
      });
    }),
    10_000,
    'publishDiagnostics',
  );

  const diags = published.diagnostics || [];
  if (diags.length !== 1) {
    fail(`expected 1 diagnostic, got ${diags.length}`);
  }
  const d = diags[0];
  if (d.code !== 'no-tabs') {
    fail(`expected code no-tabs, got ${d.code}`);
  }
  if (d.range.start.line !== 0) {
    fail(`expected line 0, got ${d.range.start.line}`);
  }

  // Fix the tab; expect an empty publish.
  connection.sendNotification('textDocument/didChange', {
    textDocument: { uri, version: 1 },
    contentChanges: [
      {
        range: {
          start: { line: 0, character: 11 },
          end: { line: 0, character: 12 },
        },
        text: '',
      },
    ],
  });

  const cleared = await withTimeout(
    new Promise((resolve) => {
      connection.onNotification('textDocument/publishDiagnostics', (p) => {
        if (p.uri === uri && p.version === 1) {
          resolve(p);
        }
      });
    }),
    10_000,
    'publishDiagnostics after fix',
  );
  if ((cleared.diagnostics || []).length !== 0) {
    fail('diagnostics did not clear after the fix');
  }

  await connection.sendRequest('shutdown');
  connection.sendNotification('exit');
  finish(0, 'SMOKE PASS');
})().catch((err) => {
  fail(err.message);
});
