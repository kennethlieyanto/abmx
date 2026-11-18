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

# Run the binary, e.g. `just run status`.
run *args:
    cargo run -q -p abmx -- {{args}}

# Exercise the whole pipeline against saved .eml files and the file sink.
# `just demo ./sample-mail`
demo dir parser="example.journal_transfer" parsers="parsers":
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

    [labels]
    prefix = "receipt/"

    [labels.accounts]
    "{{parser}}" = "demo"
    EOF
    cargo run -q -p abmx -- --config "$work/abmx.toml" sync --dry-run --from-dir "$work/mail" --label "receipt/{{parser}}"
    cargo run -q -p abmx -- --config "$work/abmx.toml" sync --from-dir "$work/mail" --label "receipt/{{parser}}"
    echo "ledger: $work/ledger.jsonl"
    cat "$work/ledger.jsonl"

# ---------------------------------------------------------------------------
# parser corpus
# ---------------------------------------------------------------------------

# Parse a file or directory with one parser, as JSON on stdout.
parse path parser="example.journal_transfer":
    cargo run -q -p abmx -- parser parse {{path}} --parser {{parser}} --parsers "{{corpus}}"

# Parse a directory as one JSON object per line.
ndjson path parser="example.journal_transfer":
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
# Copy the output into parsers/<id>/fixtures/ afterwards.
redact in="sample" out="build/redacted":
    @test -f redact.toml || { echo "missing redact.toml; copy redact.example.toml and fill it in"; exit 1; }
    cargo run -q -p xtask -- redact --in {{in}} --out {{out}}

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

# Check all parsers, then install them into the XDG corpus.
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
    echo "installed parsers -> $dest"

# Remove the user config (~/.config/abmx) and every installed parser, so the
# first-run UX can be tried again. The Gmail token and state DB live in
# ~/.local/state/abmx and are kept; `abmx reset --purge` replays them.
# Usage: `just nuke` (asks) or `just nuke yes`.
nuke yes="no":
    #!/usr/bin/env bash
    set -euo pipefail
    dir="${XDG_CONFIG_HOME:-$HOME/.config}/abmx"
    if [[ ! -e "$dir" ]]; then
        echo "nothing to remove: $dir"
        exit 0
    fi
    if [[ "{{yes}}" != "yes" ]]; then
        printf 'remove %s? [y/N] ' "$dir"
        read -r answer
        case "$answer" in
            y|Y|yes|YES) ;;
            *) echo "aborted"; exit 1 ;;
        esac
    fi
    rm -rf "$dir"
    echo "removed $dir"

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
