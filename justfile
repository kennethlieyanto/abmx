default:
    @just --list

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

ci: check test

# Run the binary, e.g. `just run plan --from-dir ./mail`.
run *args:
    cargo run -q -p txsync -- {{args}}

# Exercise the whole pipeline against saved .eml files and the file sink.
# `just demo ./sample-mail ../txp/parsers`
demo dir parsers="../txp/parsers":
    #!/usr/bin/env bash
    set -euo pipefail
    work=$(mktemp -d)
    mkdir -p "$work/mail"
    cp {{dir}}/*.eml "$work/mail/"
    export TXP_PARSERS="$(realpath {{parsers}})"
    cat > "$work/txsync.toml" <<EOF
    [general]
    state = "$work/state.db"

    [sink.file]
    path = "$work/ledger.jsonl"
    EOF
    # Exit 3 means "rows are waiting for review", which is expected here: the
    # demo has no [accounts] mapping.
    cargo run -q -p txsync -- --config "$work/txsync.toml" plan --from-dir "$work/mail" -v > /dev/null || true
    cargo run -q -p txsync -- --config "$work/txsync.toml" sync --from-dir "$work/mail"
    cargo run -q -p txsync -- --config "$work/txsync.toml" review || true
    echo "ledger: $work/ledger.jsonl"
    cat "$work/ledger.jsonl"

# Start the actual-http-api wrapper on :5007. Needs docker and a .env.
actual-api:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -f .env ]]; then
      echo "no .env: cp .env.example .env, then fill in ACTUAL_SERVER_PASSWORD and TXSYNC_ACTUAL_API_KEY" >&2
      exit 1
    fi
    missing=()
    grep -qE '^ACTUAL_SERVER_PASSWORD=.+' .env || missing+=("ACTUAL_SERVER_PASSWORD")
    grep -qE '^TXSYNC_ACTUAL_API_KEY=.+' .env || missing+=("TXSYNC_ACTUAL_API_KEY")
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
    printf 'In txsync.toml:\n\n'
    printf '  [sink.actual]\n'
    printf '  url = "http://localhost:%s"\n' "$port"
    printf '  budget_sync_id = "<Settings -> Show advanced settings -> Sync ID>"\n\n'
    printf 'Then: txsync accounts   # prints the account UUIDs to map\n'

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

# txp is consumed from a sibling checkout. Use a git tag instead.
txp-git tag="v0.1.0":
    #!/usr/bin/env bash
    set -euo pipefail
    sed -i 's|^txp = .*|txp = { git = "ssh://git@github.com/kennethlieyanto/txp", tag = "'"{{tag}}"'" }|' Cargo.toml
    grep -q '^txp-core' Cargo.toml && sed -i 's|^txp-core = .*|txp-core = { git = "ssh://git@github.com/kennethlieyanto/txp", tag = "'"{{tag}}"'" }|' Cargo.toml
    cargo update -p txp -p txp-core

# Point the workspace back at ../txp.
txp-local:
    #!/usr/bin/env bash
    set -euo pipefail
    sed -i 's|^txp = .*|txp = { path = "../txp/crates/txp" }|' Cargo.toml
    sed -i 's|^txp-core = .*|txp-core = { path = "../txp/crates/txp-core" }|' Cargo.toml
    cargo update -p txp -p txp-core
