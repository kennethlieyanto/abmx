#!/usr/bin/env bash
# Astro 7's native compiler binding requires AVX2. On older x86_64 hosts the
# binding SIGILLs at load time; switch to the WASI build instead.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ! -f node_modules/@astrojs/compiler-binding/package.json ]]; then
  echo "run npm ci first" >&2
  exit 1
fi

version=$(node -p "require('./node_modules/@astrojs/compiler-binding/package.json').version")
native=node_modules/@astrojs/compiler-binding-linux-x64-gnu
wasm=node_modules/@astrojs/compiler-binding-wasm32-wasi

if [[ -d "$native" ]]; then
  rm -rf "$native.disabled"
  mv "$native" "$native.disabled"
  echo "disabled the native compiler binding"
fi

if [[ ! -f "$wasm/package.json" ]]; then
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  npm pack "@astrojs/compiler-binding-wasm32-wasi@$version" --pack-destination "$tmp" >/dev/null 2>&1
  mkdir -p "$wasm"
  tar -xzf "$tmp"/*.tgz -C "$wasm" --strip-components=1
fi

node -e "require('@astrojs/compiler-binding')" >/dev/null 2>&1
touch node_modules/.abmx-wasi
echo "WASI compiler binding enabled."
