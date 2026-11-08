# Common tasks. Run `just` to list them.

default:
    @just --list

# ---------------------------------------------------------------------------
# build / run
# ---------------------------------------------------------------------------

# Build the release binary at target/release/txp
build:
    @cargo build --release --bin txp

# Build the debug binary (fast loop)
build-debug:
    @cargo build --bin txp

# Run the CLI (parsers resolved from --parsers / TXP_PARSERS / ./parsers / XDG / /etc)
txp *args: build-debug
    @cargo run -q --bin txp -- {{args}}

# Parse a file or directory to JSON on stdout
parse path: build-debug
    @cargo run -q --bin txp -- parse {{path}}

# Parse a directory as one JSON object per line
ndjson path: build-debug
    @cargo run -q --bin txp -- parse {{path}} --format ndjson

# Show the normalized blocks of an email (parser authoring aid)
dump path: build-debug
    @cargo run -q --bin txp -- dump {{path}}

# Explain which parser claims an email, and why
route path: build-debug
    @cargo run -q --bin txp -- route {{path}}

# List the sender addresses the resolved corpus covers
senders: build-debug
    @cargo run -q --bin txp -- senders

# Show the parser search order and which directory won
paths: build-debug
    @cargo run -q --bin txp -- paths

# List the loaded parsers and the file each came from
specs: build-debug
    @cargo run -q --bin txp -- specs --format table

# Print the JSON Schema of the transaction contract
schema: build-debug
    @cargo run -q --bin txp -- schema

# ---------------------------------------------------------------------------
# verification
# ---------------------------------------------------------------------------

# Verify every parser against the committed fixtures
fixtures: build-debug
    @cargo run -q --bin txp -- check -v

# Only check one parser, e.g. `just fixtures-one example.card_debit`
fixtures-one id: build-debug
    @cargo run -q --bin txp -- check --parser {{id}} -v

# Summarise how a directory of emails parses (defaults to the fixture corpus)
corpus dir="fixtures": build-debug
    @cargo run -q --bin txp -- corpus --dir {{dir}}

# Unit + integration tests
test:
    cargo test --workspace

# Type-check without codegen
check:
    cargo check --workspace --all-targets

# Clippy, warnings are errors
lint:
    cargo clippy --all-targets -- -D warnings

# Format the workspace
fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

# Everything CI runs
ci: fmt-check lint test fixtures

# ---------------------------------------------------------------------------
# authoring a parser
# ---------------------------------------------------------------------------

# Rewrite real emails into publishable fixtures (needs redact.toml, gitignored)
redact in="sample" out="build/redacted":
    @test -f redact.toml || { echo "missing redact.toml; copy redact.example.toml and fill it in"; exit 1; }
    cargo run -q -p xtask -- redact --in {{in}} --out {{out}}

# Draft a parser spec from a directory of emails, e.g. `just infer sample/example/promo example.promo`
infer dir id: build-debug
    @cargo run -q --bin txp -- infer {{dir}} --id {{id}}

# Scaffold parsers/<id>.toml and fixtures/<id>/
init id: build-debug
    @cargo run -q --bin txp -- init {{id}}

# ---------------------------------------------------------------------------
# release
# ---------------------------------------------------------------------------

# List what `cargo package -p txp` would ship
package:
    cargo package -p txp --list --allow-dirty

# Test the crate as a consumer would see it
publish-check: ci package
    @echo "ok: workspace is clean and txp packages"

clean:
    cargo clean
