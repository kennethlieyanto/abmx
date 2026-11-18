#!/usr/bin/env node
// One-shot bridge between abmx and the official @actual-app/api client.
//
// abmx spawns this process per operation, writes one JSON request to stdin,
// reads one JSON response from stdout, and waits for it to exit. Nothing stays
// running, so there is no port, no service, and no shared secret on the wire.
//
// The library logs breadcrumbs on stdout ("[Breadcrumb] ..."), so stdout is
// reserved for the protocol (written with writeSync) and the library's own
// output is silenced unless ABMX_ACTUAL_DEBUG=1.

const { mkdirSync, writeSync } = require('node:fs');
const path = require('node:path');

// The library narrates loads and syncs on stdout. Keep fd 1 for the protocol
// (written with writeSync) and silence or forward everything else.
if (process.env.ABMX_ACTUAL_DEBUG === '1') {
  process.stdout.write = process.stderr.write.bind(process.stderr);
} else {
  process.stdout.write = () => true;
}

const DATA_DIR = process.env.ACTUAL_DATA_DIR || '/tmp/abmx-actual';
const SERVER_URL = process.env.ACTUAL_SERVER_URL;
const API_KEY = process.env.ACTUAL_API_KEY;
const SERVER_PASSWORD = process.env.ACTUAL_SERVER_PASSWORD;

let api;
let initialized = false;

function requireApi() {
  if (!api) {
    api = require('@actual-app/api');
  }
  return api;
}

function apiVersion() {
  const entry = require.resolve('@actual-app/api');
  const manifest = path.join(path.dirname(entry), '..', 'package.json');
  return require(manifest).version;
}

async function init() {
  if (initialized) {
    return requireApi();
  }
  // @actual-app/api requires dataDir to exist; it does not create it.
  mkdirSync(DATA_DIR, { recursive: true, mode: 0o700 });
  const client = requireApi();
  await client.init({
    dataDir: DATA_DIR,
    serverURL: SERVER_URL,
    ...(API_KEY ? { sessionToken: API_KEY } : { password: SERVER_PASSWORD }),
  });
  initialized = true;
  return client;
}

// Load the budget, pulling anything the server has that the local cache does
// not. `downloadBudget` alone only loads the cached file, so sync explicitly.
async function openBudget(params) {
  const client = await init();
  if (!params.budgetSyncId) {
    throw Object.assign(new Error('budgetSyncId is required'), { code: 'missing-sync-id' });
  }
  await client.downloadBudget(
    params.budgetSyncId,
    params.budgetPassword ? { password: params.budgetPassword } : undefined,
  );
  await client.sync();
  return client;
}

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) {
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString('utf8');
}

function normalizeAccount(account) {
  return {
    id: account.id,
    name: account.name ?? '',
    offbudget: Boolean(account.offbudget),
    closed: Boolean(account.closed),
  };
}

async function handle(request) {
  const params = request.params || {};

  if (request.method === 'version') {
    return { api: apiVersion(), node: process.versions.node };
  }

  const client = await openBudget(params);

  switch (request.method) {
    case 'accounts': {
      const accounts = await client.getAccounts();
      return accounts.map(normalizeAccount);
    }

    case 'snapshot': {
      const rows = [];
      for (const accountId of params.accounts || []) {
        const transactions = await client.getTransactions(
          accountId,
          params.since,
          params.until,
        );
        for (const transaction of transactions) {
          rows.push({ ...transaction, account: transaction.account || accountId });
        }
      }
      return rows;
    }

    case 'import': {
      const options = params.options || {};
      const result = await client.importTransactions(
        params.account,
        params.transactions || [],
        options,
      );
      // shutdown() syncs best-effort and swallows failures, so persist here
      // where an error can still reach abmx.
      if (!options.dryRun) {
        await client.sync();
      }
      return {
        added: (result.added || []).length,
        updated: (result.updated || []).length,
      };
    }

    default:
      throw Object.assign(new Error(`unknown method: ${request.method}`), {
        code: 'unknown-method',
      });
  }
}

async function main() {
  let response;
  try {
    const request = JSON.parse(await readStdin());
    response = { ok: true, data: await handle(request) };
  } catch (error) {
    response = {
      ok: false,
      error: error && error.message ? error.message : String(error),
      ...(error && error.code ? { code: error.code } : {}),
    };
  } finally {
    if (initialized) {
      try {
        await api.shutdown();
      } catch {
        // The explicit syncs above already reported anything actionable.
      }
    }
  }

  // writeSync avoids interleaving with anything still buffered on stderr.
  writeSync(1, `${JSON.stringify(response)}\n`);
}

main().then(
  () => process.exit(0),
  (error) => {
    writeSync(1, `${JSON.stringify({ ok: false, error: String(error) })}\n`);
    process.exit(1);
  },
);
