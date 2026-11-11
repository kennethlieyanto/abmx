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
demo dir:
    #!/usr/bin/env bash
    set -euo pipefail
    work=$(mktemp -d)
    mkdir -p "$work/mail"
    cp {{dir}}/*.eml "$work/mail/"
    cat > "$work/txsync.toml" <<EOF
    [general]
    state = "$work/state.db"

    [sink.file]
    path = "$work/ledger.jsonl"
    EOF
    cargo run -q -p txsync -- --config "$work/txsync.toml" plan --from-dir "$work/mail" -v
    cargo run -q -p txsync -- --config "$work/txsync.toml" sync --from-dir "$work/mail"
    echo "ledger: $work/ledger.jsonl"

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
