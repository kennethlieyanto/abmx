---
title: Install
description: Build or install abmx from Nix, Cargo, or the container image, and the credentials you need first.
---

## Nix

The turnkey path. The flake bundles the Actual helper and Node.

```bash
nix build .#          # ./result/bin/abmx
nix develop           # rust, cargo, clippy, rustfmt, just, node, sqlite
```

From a checkout you can also try it in place; `.envrc` adds `target/debug` to
`PATH`:

```bash
direnv allow          # first time only
cargo build
abmx --version
```

## Cargo

From a clone of the repository:

```bash
cargo install --path crates/abmx
```

This builds the Rust binary only. The Actual sink also needs **Node 22+** and the
helper's `node_modules`:

```bash
just helper-install   # once, from a clone
```

Keep `crates/abmx-actual/helper/` where `abmx` can find it, or set
`ABMX_ACTUAL_HELPER`. The JSONL file sink needs neither Node nor the helper.

## Container

The repository's `Dockerfile` contains the Rust binary and the helper, so the
image needs nothing else at runtime:

```bash
docker build -t abmx .
docker run --rm abmx --version
```

See [Docker](/guides/deployment/docker/) for a full `docker run` invocation with
config, state, and secrets mounted.

## You need

1. **A Gmail OAuth client** — one Google Cloud project, `gmail.readonly`, desktop
   app client. See [Gmail](/guides/gmail/).
2. **An Actual server** — your existing Actual server (`:5006` by default), a
   budget, and either a budget-scoped API key or the server password. See
   [Actual Budget](/guides/actual/).
3. **Parsers** — nothing to install. `abmx samples` downloads evidence and an
   agent writes parsers under `~/.config/abmx/parsers/<id>/`. If you already have
   a corpus, point at it with `--parsers`, `$ABMX_PARSERS`, or
   `[general].parsers`.

## Where parsers live

`abmx` never reads the working directory. Parsers live in your own corpus at
`~/.config/abmx/parsers/`, or wherever `--parsers`, `$ABMX_PARSERS`, or
`[general].parsers` points. The repository ships one synthetic example under
`examples/parsers/` for documentation and tests; it is not installed.

Implicit discovery searches `~/.config/abmx/parsers/`, then
`/etc/abmx/parsers/`, and the first directory that exists wins. Run
`abmx parser paths` to see the chain and which directory won.
