const { test } = require('node:test');
const assert = require('node:assert');
const { spawnSync } = require('node:child_process');
const path = require('node:path');

const helper = path.join(__dirname, '..', 'helper.js');

function run(input) {
  return spawnSync(process.execPath, [helper], {
    input,
    encoding: 'utf8',
    timeout: 30_000,
    // Keep the test independent of the developer's shell.
    env: { ...process.env, ACTUAL_SERVER_URL: '', ACTUAL_API_KEY: '', ACTUAL_SERVER_PASSWORD: '' },
  });
}

test('version answers without a budget or credentials', () => {
  const result = run(JSON.stringify({ method: 'version' }));
  assert.equal(result.status, 0, result.stderr);

  // stdout must be exactly one JSON document: the library logs to stderr.
  const response = JSON.parse(result.stdout);
  assert.equal(response.ok, true);
  assert.match(response.data.api, /^\d+\.\d+\.\d+/);
});

test('a malformed request is reported as JSON, not a stack trace', () => {
  const result = run('not json');
  assert.equal(result.status, 0, result.stderr);

  const response = JSON.parse(result.stdout);
  assert.equal(response.ok, false);
  assert.ok(response.error.length > 0);
});
