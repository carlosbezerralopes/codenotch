const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const workflow = fs.readFileSync(path.join(root, '.github/workflows/windows-package.yml'), 'utf8');

test('The rolling download is restricted to validated repository pushes', () => {
  const validation = workflow.indexOf('- name: Validate the Windows build before distribution');
  const smoke = workflow.indexOf('- name: Install, run doctor, uninstall');
  const publish = workflow.indexOf('- name: Publish the fixed-name CI download');
  assert.ok(validation >= 0 && smoke > validation && publish > smoke);
  const checks = workflow.slice(validation, workflow.indexOf('- name: Build the hook'));
  assert.match(checks, /\$PSNativeCommandUseErrorActionPreference = \$true/);
  assert.match(checks, /node --test/);
  assert.match(checks, /cargo test --locked/);
  const upload = workflow.slice(publish, workflow.indexOf('- name: Explain how to open it'));
  assert.match(upload, /if: github\.event_name == 'push'/);
  assert.doesNotMatch(upload, /always\(\)|continue-on-error/);
  assert.match(upload, /name: Codenotch-Windows\s/);
  assert.match(upload, /if-no-files-found: error/);
});

test('README separates the latest tested branch build from stable releases', () => {
  const readme = fs.readFileSync(path.join(root, 'README.md'), 'utf8');
  assert.match(workflow, /branches: \[main, 'feat\/kiro-windows'\]/);
  assert.ok(readme.includes('https://nightly.link/carlosbezerralopes/codenotch/workflows/windows-package.yml/feat%2Fkiro-windows/Codenotch-Windows.zip'));
  assert.ok(readme.includes('https://github.com/carlosbezerralopes/codenotch/releases/latest/download/Codenotch-Setup.exe'));
});
