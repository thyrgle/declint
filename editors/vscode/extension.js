// declint — VS Code LSP client for `declint serve`.
//
// Thin by design: the linter itself is the declint binary; this file
// spawns it over stdio and hands diagnostics and quickfixes to VS Code.

const vscode = require('vscode');
const {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} = require('vscode-languageclient/node');

let client;

const DEFAULT_FILETYPES = [
  'python',
  'javascript',
  'html',
  'vue',
  'svelte',
  'erb',
  'handlebars',
  'ini',
  'markdown',
  'json',
  'toml',
  'yaml',
  'sh',
  'dockerfile',
];

function activate(context) {
  const config = vscode.workspace.getConfiguration('declint');
  const command = config.get('path') || 'declint';
  const args = config.get('args') || ['serve'];
  const filetypes = config.get('filetypes') || DEFAULT_FILETYPES;

  const serverOptions = {
    command,
    args,
  };

  const clientOptions = {
    documentSelector: filetypes.map((language) => ({
      scheme: 'file',
      language,
    })),
    diagnosticCollectionName: 'declint',
  };

  client = new LanguageClient(
    'declint',
    'declint',
    serverOptions,
    clientOptions,
  );

  client
    .start()
    .then(() => {
      const channel = client.outputChannel;
      if (channel) {
        channel.appendLine('declint language server started');
      }
    })
    .catch((err) => {
      vscode.window.showErrorMessage(
        `declint: failed to start '${command}' (${err.message}). ` +
          'Install it with `cargo install declint`, or set declint.path ' +
          'to the binary location.',
      );
    });
}

function deactivate() {
  if (client) {
    return client.stop();
  }
  return undefined;
}

module.exports = { activate, deactivate };
