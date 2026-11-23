---
title: Install
description: Install abmx with the shell installer, Nix, Cargo, or the container image, and the credentials you need first.
---

## Quick install

Linux and macOS, no Rust or Nix required:

```bash
curl -fsSL https://kennethlieyanto.github.io/abmx/install.sh | sh
```

The installer downloads the latest release for your platform, verifies its
SHA-256, and installs:

- `abmx` to `~/.local/bin/abmx`
- the Actual helper to `~/.local/share/abmx/actual-helper/`

It then runs `npm ci` in the helper directory when Node 22+ is available.
`abmx --version` verifies the binary. If `~/.local/bin` is not on `PATH`, the
installer prints the line to add it.

The JSONL sink works without Node. The Actual sink needs **Node 22+** and the
helper's `node_modules`; install Node and rerun the installer, or run:

```bash
cd ~/.local/share/abmx/actual-helper && npm ci --omit=dev
```

Pin a version or choose a different prefix:

```bash
curl -fsSL https://kennethlieyanto.github.io/abmx/install.sh | ABMX_VERSION=v0.1.0 sh
curl -fsSL https://kennethlieyanto.github.io/abmx/install.sh | ABMX_INSTALL_DIR="$HOME/.local" sh
```

Prefer to read before running? Download it first:

```bash
curl -fsSLO https://kennethlieyanto.github.io/abmx/install.sh
less install.sh
sh install.sh
```

Rerun the same command to upgrade. Uninstall with:

```bash
rm ~/.local/bin/abmx
rm -rf ~/.local/share/abmx
```

Your config, state, and parsers live under `~/.config/abmx`,
`~/.local/state/abmx`, and `~/.cache/abmx`; the installer never touches them.

## Nix

The flake bundles the Actual helper and Node, so this is still the turnkey path
for a NixOS or home-manager host:

```bash
nix profile install github:kennethlieyanto/abmx
nix run github:kennethlieyanto/abmx -- --version
```

From a checkout:

```bash
nix build .#          # ./result/bin/abmx
nix develop           # rust, cargo, clippy, rustfmt, just, node, sqlite
```

`.envrc` also adds `target/debug` to `PATH`:

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
