default:
    @just --list

# The repository's parser corpus. Recipes pass it with --parsers, so nothing
# depends on the working directory or an environment variable.
corpus := justfile_directory() / "examples/parsers"

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

# Run the binary, e.g. `just run status`.
run *args:
    cargo run -q -p abmx -- {{args}}

# Build the container image locally.
image:
    docker build -t abmx:dev .

# Lint and render the Helm chart with its CI fixture values.
helm-lint:
    helm lint charts/abmx -f charts/abmx/ci/minimal-values.yaml
    helm lint examples/abmx-config

helm-template:
    helm template abmx charts/abmx -f charts/abmx/ci/minimal-values.yaml

# Exercise the whole pipeline against saved .eml files and the file sink.
# `just demo ./sample-mail`
demo dir parser="example.bank" parsers="examples/parsers":
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

    [receipts]
    prefix = "receipt/"

    [receipts.accounts]
    "{{parser}}" = "demo"
    EOF
    cargo run -q -p abmx -- --config "$work/abmx.toml" receipt sync --dry-run --from-dir "$work/mail" --label "receipt/{{parser}}"
    cargo run -q -p abmx -- --config "$work/abmx.toml" sync --from-dir "$work/mail" --label "receipt/{{parser}}"
    echo "ledger: $work/ledger.jsonl"
    cat "$work/ledger.jsonl"

# ---------------------------------------------------------------------------
# website
# ---------------------------------------------------------------------------

# Build the documentation site. On hosts where `npm run compiler:wasi` was
# needed, Pagefind cannot run either; skip the search index for the local build.
site:
    cd website && if [ -f node_modules/.abmx-wasi ]; then export ABMX_NO_PAGEFIND=1; fi; npm run build

# Run the Starlight dev server at http://localhost:4321/abmx/.
site-dev:
    cd website && npm run dev

# Type-check the site and build it, exactly what the website workflow runs.
site-check:
    cd website && npm run check
    cd website && if [ -f node_modules/.abmx-wasi ]; then export ABMX_NO_PAGEFIND=1; fi; npm run build

# ---------------------------------------------------------------------------
# parser corpus
# ---------------------------------------------------------------------------

# Parse a file or directory with one parser, as JSON on stdout.
parse path parser="example.bank":
    cargo run -q -p abmx -- parser parse {{path}} --parser {{parser}} --parsers "{{corpus}}"

# Parse a directory as one JSON object per line.
ndjson path parser="example.bank":
    cargo run -q -p abmx -- parser parse {{path}} --parser {{parser}} --format ndjson --parsers "{{corpus}}"

# Explain the block regions behind each field of one parser.
explain path parser:
    cargo run -q -p abmx -- parser explain {{path}} --parser {{parser}} --parsers "{{corpus}}"

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

# ---------------------------------------------------------------------------
# authoring a parser
# ---------------------------------------------------------------------------

# Download up to three sample emails per managed label.
samples *args:
    cargo run -q -p abmx -- samples {{args}}

# Rewrite real emails into publishable fixtures (needs redact.toml, gitignored).
# Copy the output into your parser's fixtures/ directory afterwards.
redact in="sample" out="build/redacted":
    @test -f redact.toml || { echo "missing redact.toml; copy redact.example.toml and fill it in"; exit 1; }
    cargo run -q -p xtask -- redact --in {{in}} --out {{out}}

# Scaffold a parser in the user corpus (~/.config/abmx/parsers/<id>/parser.rhai).
parser-init id:
    cargo run -q -p abmx -- parser init {{id}}

# Nuke every trace of abmx so the first-run UX can be tried from scratch: the
# state DB and Gmail token (setup reauthorizes), the config directory (samples,
# OAuth client, config.toml), the cached Actual budget, the disposable Actual
# server and its budget volume, and the gitignored dev/ sandbox files. Authored
# parsers are expensive to recreate, so the user corpus is offered a move to
# parsers.bak first.
# Usage: `just nuke` (prompts) or `just nuke yes`.
nuke yes="no":
    #!/usr/bin/env bash
    set -euo pipefail

    config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/abmx"
    state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/abmx"
    cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/abmx"
    parsers="$config_dir/parsers"
    repo="{{justfile_directory()}}"
    compose="$repo/dev/compose.yaml"

    ask=1
    if [[ "{{yes}}" == "yes" ]]; then
        ask=0
    elif [[ ! -t 0 ]]; then
        echo "no terminal to confirm; rerun as 'just nuke yes'" >&2
        exit 1
    fi

    echo "nuke removes:"
    echo "  $config_dir (everything but parsers.bak*)"
    echo "  $state_dir  (state DB, Gmail token, authoring samples)"
    echo "  $cache_dir  (cached Actual budgets)"
    echo "  dev/abmx.toml, dev/state.db*, dev/actual-cache/, dev/actual-api-key, dev/*.password"
    echo "  the dev Actual container and its budget volume"
    if [[ $ask -eq 1 ]]; then
        printf 'nuke? [y/N] '
        read -r answer
        case "$answer" in
            y|Y|yes|YES) ;;
            *) echo "aborted"; exit 1 ;;
        esac
    fi

    # 1. Parsers are the one expensive artifact: offer to keep them.
    backup=""
    if [[ -d "$parsers" ]]; then
        backup="$config_dir/parsers.bak"
        if [[ -e "$backup" ]]; then
            backup="$backup.$(date +%Y%m%dT%H%M%S)"
        fi
        keep=1
        if [[ $ask -eq 1 ]]; then
            printf 'keep authored parsers? move %s -> %s [Y/n] ' "$parsers" "$backup"
            read -r answer
            case "$answer" in
                n|N|no|NO) keep=0 ;;
            esac
        fi
        if [[ $keep -eq 1 ]]; then
            mv -- "$parsers" "$backup"
            echo "kept parsers at $backup"
        else
            echo "warning: $parsers will be deleted" >&2
            backup=""
        fi
    fi

    # 2. Config directory, keeping any parsers backup.
    if [[ -d "$config_dir" ]]; then
        shopt -s nullglob dotglob
        for entry in "$config_dir"/*; do
            case "${entry##*/}" in
                parsers.bak*) continue ;;
            esac
            rm -rf -- "$entry"
        done
        rmdir -- "$config_dir" 2>/dev/null || true
        echo "cleaned $config_dir"
    fi

    # 3. State and cache: state DB, Gmail token, downloaded samples.
    rm -rf -- "$state_dir" "$cache_dir"
    echo "removed $state_dir and $cache_dir"

    # 4. Gitignored dev sandbox files, which point at the volume below.
    shopt -s nullglob
    for path in "$repo"/dev/abmx.toml "$repo"/dev/state.db* \
                "$repo"/dev/actual-cache "$repo"/dev/actual-api-key \
                "$repo"/dev/*.password; do
        rm -rf -- "$path"
        echo "removed ${path#"$repo"/}"
    done

    # 5. The disposable Actual server and its budget volume.
    if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1 && [[ -f "$compose" ]]; then
        if ! docker compose -f "$compose" down -v --remove-orphans; then
            echo "warning: docker compose down failed; continuing" >&2
        fi
        # `down -v` can leave the volume when the containers were already gone.
        volume="$(basename "$(dirname "$compose")")_actual-data"
        if docker volume inspect "$volume" >/dev/null 2>&1; then
            docker volume rm "$volume"
        fi
        echo "removed the dev Actual container and volume"
    else
        echo "warning: docker unavailable or dev/compose.yaml missing; the dev container/volume were left alone" >&2
    fi

    echo
    echo "next: just run setup"
    if [[ -n "$backup" ]]; then
        echo "restore parsers later with: mv \"$backup\" \"$parsers\""
    fi

# ---------------------------------------------------------------------------
# actual helper
# ---------------------------------------------------------------------------

# Install the helper's pinned npm dependencies. Development only: the Nix
# package and the container bundle the helper for you.
helper-install:
    npm --prefix crates/abmx-actual/helper ci

# Exercise the helper's protocol without an Actual server.
helper-test:
    node --test crates/abmx-actual/helper/test/*.test.js

# Move the bundled @actual-app/api to a released Actual version, then refresh
# the Nix dependency hash and rerun the Rust tests.
helper-bump version:
    #!/usr/bin/env bash
    set -euo pipefail
    cd crates/abmx-actual/helper
    npm install --save-exact --package-lock-only --ignore-scripts "@actual-app/api@{{version}}"
    npm ci
    node --test test
    cd -
    echo "refresh npmDepsHash in flake.nix with:"
    echo "  nix run nixpkgs#prefetch-npm-deps -- crates/abmx-actual/helper/package-lock.json"
    cargo test -p abmx-actual

# Nix build.
nix-build:
    nix build .#

# List what `cargo package -p abmx` would ship.
package:
    cargo package -p abmx --list --allow-dirty

clean:
    cargo clean

# ---------------------------------------------------------------------------
# development sandbox (throwaway Actual Budget server)
# ---------------------------------------------------------------------------

# Start the disposable Actual server on http://localhost:5006.
dev-up:
    docker compose -f dev/compose.yaml up -d

# Stop the disposable server, keeping its budget volume.
dev-down:
    docker compose -f dev/compose.yaml down

# Stop the disposable server and delete its budget.
dev-reset:
    docker compose -f dev/compose.yaml down -v

# List accounts on the disposable server (needs dev/abmx.toml).
dev-accounts:
    cargo run -q -p abmx -- --config dev/abmx.toml accounts

# Verify statement parsers against the private corpus fixtures.
dev-check:
    cargo run -q -p abmx -- statement --parsers statement-samples/corpus check --password-file statement-samples/.password -v

# Statement sync against the disposable server. Dry run unless you pass args.
dev-statements *args:
    cargo run -q -p abmx -- --config dev/abmx.toml statement sync --dry-run {{args}}
