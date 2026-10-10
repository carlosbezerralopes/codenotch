const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..', '..');

test('Windows updates remain in the fork for API, feed and manual download', () => {
  const updater = fs.readFileSync(path.join(root, 'windows/codenotch/src/updater.rs'), 'utf8');
  const config = JSON.parse(fs.readFileSync(path.join(root, 'windows/codenotch/tauri.conf.json'), 'utf8'));
  assert.match(updater, /https:\/\/api\.github\.com\/repos\/carlosbezerralopes\/codenotch\/releases\/latest/);
  assert.match(updater, /https:\/\/github\.com\/carlosbezerralopes\/codenotch\/releases\/download/);
  assert.deepEqual(config.plugins.updater.endpoints,
    ['https://github.com/carlosbezerralopes/codenotch/releases/latest/download/latest.json']);
  assert.doesNotMatch(updater, /https:\/\/(?:api\.)?github\.com\/(?:repos\/)?vinzdg\//);
});

test('Packaging feed targets the repository that owns the workflow', () => {
  const workflow = fs.readFileSync(path.join(root, '.github/workflows/windows-package.yml'), 'utf8');
  assert.match(workflow, /https:\/\/github\.com\/\$env:GITHUB_REPOSITORY\/releases\/download/);
  assert.match(workflow, /github\.repository == 'carlosbezerralopes\/codenotch'/);
});
