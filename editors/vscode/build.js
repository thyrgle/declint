// Bundles extension.js (+ vscode-languageclient) into a single
// dist/extension.js so the .vsix is self-contained.
require('esbuild').buildSync({
  entryPoints: ['extension.js'],
  bundle: true,
  outfile: 'dist/extension.js',
  external: ['vscode'],
  platform: 'node',
  target: 'node18',
  format: 'cjs',
  sourcemap: false,
  minify: false,
});
