default:
    @just --list

# ---------------------------------------------------------------------------
# build / run
# ---------------------------------------------------------------------------

# Build every crate.
build:
    cargo build --workspace

# Formatting and lints, exactly what CI runs.
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# Unit, integration, and end-to-end tests. No network, no credentials.
test:
    cargo test --workspace

# Everything CI runs, including the parser corpus.
ci: check test fixtures

# Run the binary, e.g. `just run plan --from-dir ./mail`.
run *args:
    cargo run -q -p abmx -- {{args}}

# Exercise the whole pipeline against saved .eml files and the file sink.
# `just demo ./sample-mail`
demo dir parsers="./parsers":
    #!/usr/bin/env bash
    set -euo pipefail
    work=$(mktemp -d)
    mkdir -p "$work/mail"
    cp {{dir}}/*.eml "$work/mail/"
    export ABMX_PARSERS="$(realpath {{parsers}})"
    cat > "$work/abmx.toml" <<EOF
    [general]
    state = "$work/state.db"

    [sink.file]
    path = "$work/ledger.jsonl"
    EOF
    # Exit 3 means "rows are waiting for review", which is expected here: the
    # demo has no [accounts] mapping.
    cargo run -q -p abmx -- --config "$work/abmx.toml" plan --from-dir "$work/mail" -v > /dev/null || true
    cargo run -q -p abmx -- --config "$work/abmx.toml" sync --from-dir "$work/mail"
    cargo run -q -p abmx -- --config "$work/abmx.toml" review || true
    echo "ledger: $work/ledger.jsonl"
    cat "$work/ledger.jsonl"

# ---------------------------------------------------------------------------
# parser corpus
# ---------------------------------------------------------------------------

# Parse a file or directory to JSON on stdout.
parse path:
    cargo run -q -p abmx -- parser parse {{path}}

# Parse a directory as one JSON object per line.
ndjson path:
    cargo run -q -p abmx -- parser parse {{path}} --format ndjson

# Explain which parser claims an email, and why.
route path:
    cargo run -q -p abmx -- parser route {{path}}

# List the sender addresses the resolved corpus covers.
senders:
    cargo run -q -p abmx -- parser senders

# Show the parser search order and which directory won.
paths:
    cargo run -q -p abmx -- parser paths

# List the loaded parsers and the file each came from.
specs:
    cargo run -q -p abmx -- parser specs --format table

# Print the JSON Schema of the transaction contract.
schema:
    cargo run -q -p abmx -- parser schema

# Verify every parser against the committed fixtures.
fixtures:
    cargo run -q -p abmx -- parser check -v

# Only check one parser, e.g. `just fixtures-one example.card_debit`.
fixtures-one id:
    cargo run -q -p abmx -- parser check --parser {{id}} -v

# Summarise how a directory of emails parses (defaults to the fixture corpus).
corpus dir="fixtures":
    cargo run -q -p abmx -- parser corpus --dir {{dir}}

# ---------------------------------------------------------------------------
# authoring a parser
# ---------------------------------------------------------------------------

# Rewrite real emails into publishable fixtures (needs redact.toml, gitignored).
redact in="sample" out="build/redacted":
    @test -f redact.toml || { echo "missing redact.toml; copy redact.example.toml and fill it in"; exit 1; }
    cargo run -q -p xtask -- redact --in {{in}} --out {{out}}

# Draft a parser spec from a directory of emails, e.g. `just infer sample/example example.promo`.
infer dir id:
    cargo run -q -p abmx -- parser infer {{dir}} --id {{id}}

# Scaffold parsers/<id>.toml and fixtures/<id>/
parser-init id:
    cargo run -q -p abmx -- parser init {{id}}

# ---------------------------------------------------------------------------
# actual-http-api
# ---------------------------------------------------------------------------

# Start the actual-http-api wrapper on :5007. Needs docker and a .env.
actual-api:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -f .env ]]; then
      echo "no .env: cp .env.example .env, then fill in ACTUAL_SERVER_PASSWORD and ABMX_ACTUAL_API_KEY" >&2
      exit 1
    fi
    missing=()
    grep -qE '^ACTUAL_SERVER_PASSWORD=.+' .env || missing+=("ACTUAL_SERVER_PASSWORD")
    grep -qE '^ABMX_ACTUAL_API_KEY=.+' .env || missing+=("ABMX_ACTUAL_API_KEY")
    if (( ${#missing[@]} )); then
      printf 'fill in .env first: %s\n' "${missing[*]}" >&2
      exit 1
    fi
    # Owned by the caller, and the container runs as the caller, so the budget
    # cache is never root-owned.
    mkdir -p var/actual-http-api
    export ACTUAL_HTTP_API_UID="$(id -u)"
    export ACTUAL_HTTP_API_GID="$(id -g)"
    docker compose up -d --pull missing
    port="${ACTUAL_HTTP_API_PORT:-5007}"
    for _ in $(seq 1 40); do
      curl -fsS -o /dev/null "http://localhost:${port}/api-docs/" && break
      sleep 0.25
    done
    printf '\nactual-http-api: http://localhost:%s\n' "$port"
    printf 'swagger:         http://localhost:%s/api-docs/\n\n' "$port"
    printf 'In abmx.toml:\n\n'
    printf '  [sink.actual]\n'
    printf '  url = "http://localhost:%s"\n' "$port"
    printf '  budget_sync_id = "<Settings -> Show advanced settings -> Sync ID>"\n\n'
    printf 'Then: abmx accounts   # prints the account UUIDs to map\n'

# Follow the wrapper's logs.
actual-api-logs:
    docker compose logs -f actual-http-api

# Stop the wrapper. The budget cache in ./var is kept.
actual-api-down:
    docker compose down

# Is the wrapper up and serving its docs?
actual-api-status:
    #!/usr/bin/env bash
    set -euo pipefail
    docker compose ps
    port="${ACTUAL_HTTP_API_PORT:-5007}"
    printf '\n'
    ready=""
    for _ in $(seq 1 20); do
      if curl -fsS -o /dev/null "http://localhost:${port}/api-docs/"; then
        ready=1
        break
      fi
      sleep 0.25
    done
    if [[ -n "$ready" ]]; then
      echo "swagger reachable on :${port}"
    else
      echo "swagger not reachable on :${port}; start it with: just actual-api" >&2
      docker compose logs --tail=20 actual-http-api >&2 || true
      exit 1
    fi

# Nix build.
nix-build:
    nix build .#

# List what `cargo package -p abmx` would ship.
package:
    cargo package -p abmx --list --allow-dirty

clean:
    cargo clean
