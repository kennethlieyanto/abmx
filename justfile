default:
    @just --list

# The repository's parser corpus. Recipes pass it with --parsers, so nothing
# depends on the working directory or an environment variable.
corpus := justfile_directory() / "parsers"

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
demo dir parsers="parsers":
    #!/usr/bin/env bash
    set -euo pipefail
    work=$(mktemp -d)
    mkdir -p "$work/mail"
    cp {{dir}}/*.eml "$work/mail/"
    cat > "$work/abmx.toml" <<EOF
    [general]
    state = "$work/state.db"
    parsers = ["$(realpath {{parsers}})"]

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
    cargo run -q -p abmx -- parser parse {{path}} --parsers "{{corpus}}"

# Parse a directory as one JSON object per line.
ndjson path:
    cargo run -q -p abmx -- parser parse {{path}} --format ndjson --parsers "{{corpus}}"

# Explain which parser claims an email, and why.
route path:
    cargo run -q -p abmx -- parser route {{path}} --parsers "{{corpus}}"

# List the sender addresses the resolved corpus covers.
senders:
    cargo run -q -p abmx -- parser senders --parsers "{{corpus}}"

# Show the parser search order and which directory won.
paths:
    cargo run -q -p abmx -- parser paths --parsers "{{corpus}}"

# List the loaded parsers and the file each came from.
specs:
    cargo run -q -p abmx -- parser specs --format table --parsers "{{corpus}}"

# Print the JSON Schema of the transaction contract.
schema:
    cargo run -q -p abmx -- parser schema

# Verify every parser against its colocated fixtures.
fixtures:
    cargo run -q -p abmx -- parser check -v --parsers "{{corpus}}"

# Only check one parser, e.g. `just check-parser example.card_debit`.
check-parser id:
    cargo run -q -p abmx -- parser check --parser {{id}} -v --parsers "{{corpus}}"

# Summarise how a directory of emails parses.
corpus dir="sample":
    cargo run -q -p abmx -- parser corpus --dir {{dir}} --parsers "{{corpus}}"

# ---------------------------------------------------------------------------
# authoring a parser
# ---------------------------------------------------------------------------

# Scan Gmail and emit an authoring prompt on stdout.
author *args:
    cargo run -q -p abmx -- author {{args}}

# Rewrite real emails into publishable fixtures (needs redact.toml, gitignored).
# Copy the output into parsers/<id>/fixtures/ afterwards.
redact in="sample" out="build/redacted":
    @test -f redact.toml || { echo "missing redact.toml; copy redact.example.toml and fill it in"; exit 1; }
    cargo run -q -p xtask -- redact --in {{in}} --out {{out}}

# Draft a parser spec from a directory of emails, e.g. `just infer sample/example example.promo`.
infer dir id:
    cargo run -q -p abmx -- parser infer {{dir}} --id {{id}}

# Scaffold parsers/<id>/config.toml and parsers/<id>/fixtures/
parser-init id:
    cargo run -q -p abmx -- parser init {{id}} --parsers "{{corpus}}"

# ---------------------------------------------------------------------------
# installing a parser
# ---------------------------------------------------------------------------

# Check one parser, then copy it into $XDG_CONFIG_HOME/abmx/parsers/<id>.
install-parser id:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo run -q -p abmx -- parser check --parser {{id}} -v --parsers "{{corpus}}"
    dest="${XDG_CONFIG_HOME:-$HOME/.config}/abmx/parsers/{{id}}"
    mkdir -p "$dest"
    cp -R "{{corpus}}/{{id}}/." "$dest/"
    echo "installed {{id}} -> $dest"

# Check all parsers, then install them and router.toml into the XDG corpus.
install-parsers:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo run -q -p abmx -- parser check -v --parsers "{{corpus}}"
    dest="${XDG_CONFIG_HOME:-$HOME/.config}/abmx/parsers"
    mkdir -p "$dest"
    for dir in "{{corpus}}"/*/; do
      id="$(basename "$dir")"
      mkdir -p "$dest/$id"
      cp -R "$dir." "$dest/$id/"
    done
    cp "{{corpus}}/router.toml" "$dest/router.toml"
    echo "installed parsers -> $dest"

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
