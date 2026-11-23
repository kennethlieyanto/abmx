#!/bin/sh
# Install abmx from a GitHub release.
#
#   curl -fsSL https://kennethlieyanto.github.io/abmx/install.sh | sh
#
# Environment:
#   ABMX_VERSION          latest (default) or vX.Y.Z
#   ABMX_INSTALL_DIR      prefix used for bin/ and share/ (default: ~/.local)
#   ABMX_SKIP_HELPER=1    install the binary and helper files, skip npm ci
#   ABMX_SKIP_CHECKSUM=1  skip SHA-256 verification (not recommended)
#   ABMX_RELEASES_URL     override the release base URL (used by tests)
#
# The binary goes to $ABMX_INSTALL_DIR/bin/abmx and the Actual helper to
# $ABMX_INSTALL_DIR/share/abmx/actual-helper/. The helper is optional: the
# JSONL sink works without Node, while the Actual sink needs Node 22+ and the
# helper's node_modules.

set -eu

RELEASES_URL="${ABMX_RELEASES_URL:-https://github.com/kennethlieyanto/abmx/releases}"
VERSION="${ABMX_VERSION:-latest}"
PREFIX="${ABMX_INSTALL_DIR:-${HOME}/.local}"
SKIP_HELPER="${ABMX_SKIP_HELPER:-0}"
SKIP_CHECKSUM="${ABMX_SKIP_CHECKSUM:-0}"

die() {
    printf 'abmx install: %s\n' "$*" >&2
    exit 1
}

warn() {
    printf 'abmx install: warning: %s\n' "$*" >&2
}

usage() {
    cat <<'EOF'
Install abmx from a GitHub release.

Usage: install.sh [options]

Options:
  --version VERSION   install VERSION (default: latest)
  --dir DIR           install prefix (default: ~/.local)
  --skip-helper       do not run npm ci for the Actual helper
  --no-checksum       skip SHA-256 verification
  -h, --help          show this help

Environment:
  ABMX_VERSION, ABMX_INSTALL_DIR, ABMX_SKIP_HELPER, ABMX_SKIP_CHECKSUM,
  ABMX_RELEASES_URL
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || die "--version needs a value"
            VERSION="$2"
            shift 2
            ;;
        --version=*)
            VERSION="${1#*=}"
            shift
            ;;
        --dir)
            [ "$#" -ge 2 ] || die "--dir needs a value"
            PREFIX="$2"
            shift 2
            ;;
        --dir=*)
            PREFIX="${1#*=}"
            shift
            ;;
        --skip-helper)
            SKIP_HELPER=1
            shift
            ;;
        --no-checksum)
            SKIP_CHECKSUM=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1 (try --help)"
            ;;
    esac
done

# Expand ~/ because the installer never runs through a shell that would.
first="${PREFIX%%/*}"
if [ "$first" = "~" ]; then
    PREFIX="${HOME}${PREFIX#\~}"
fi
PREFIX="${PREFIX%/}"
[ -n "$PREFIX" ] || die "ABMX_INSTALL_DIR must not be empty"

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
    Linux)
        case "$arch" in
            x86_64|amd64) target="x86_64-unknown-linux-musl" ;;
            aarch64|arm64) target="aarch64-unknown-linux-musl" ;;
            *) die "unsupported Linux architecture: $arch" ;;
        esac
        ;;
    Darwin)
        # An x86_64 shell under Rosetta should still get the arm64 build.
        if [ "$arch" = "x86_64" ]; then
            if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || printf '0')" = "1" ]; then
                arch="arm64"
            fi
        fi
        case "$arch" in
            x86_64|amd64) target="x86_64-apple-darwin" ;;
            aarch64|arm64) target="aarch64-apple-darwin" ;;
            *) die "unsupported macOS architecture: $arch" ;;
        esac
        ;;
    *)
        die "unsupported operating system: $os (Linux and macOS are supported)"
        ;;
esac

case "$VERSION" in
    latest|"")
        base_url="${RELEASES_URL}/latest/download"
        ;;
    *)
        tag="$VERSION"
        case "$tag" in
            v*) ;;
            *) tag="v${tag}" ;;
        esac
        base_url="${RELEASES_URL}/download/${tag}"
        ;;
esac

asset="abmx-${target}.tar.gz"
asset_url="${base_url}/${asset}"
checksum_url="${asset_url}.sha256"

for command in tar mktemp uname; do
    command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
done

download() {
    url="$1"
    out="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 --retry-delay 1 -o "$out" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q --tries=3 -O "$out" "$url"
    else
        die "curl or wget is required to download abmx"
    fi
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$1" | awk '{print $NF}'
    else
        printf 'abmx install: no sha256 tool found (sha256sum, shasum, or openssl)\n' >&2
        return 1
    fi
}

tmp="$(mktemp -d "${TMPDIR:-/tmp}/abmx-install.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT INT TERM HUP

printf 'Downloading %s\n' "$asset_url"
download "$asset_url" "${tmp}/${asset}" || die "failed to download ${asset_url}"

if [ "$SKIP_CHECKSUM" != "1" ]; then
    download "$checksum_url" "${tmp}/${asset}.sha256" || die "failed to download ${checksum_url}"
    expected="$(awk 'NR == 1 {print $1}' "${tmp}/${asset}.sha256")"
    [ -n "$expected" ] || die "checksum file is empty: ${checksum_url}"
    actual="$(sha256_of "${tmp}/${asset}")" || exit 1
    if [ "$expected" != "$actual" ]; then
        die "checksum mismatch for ${asset}
  expected: ${expected}
  actual:   ${actual}"
    fi
    printf 'Verified SHA-256 %s\n' "$actual"
fi

tar -xzf "${tmp}/${asset}" -C "$tmp"
[ -x "${tmp}/abmx/bin/abmx" ] || die "release archive has no abmx binary"
[ -f "${tmp}/abmx/share/abmx/actual-helper/helper.js" ] || die "release archive has no Actual helper"

bin_dir="${PREFIX}/bin"
share_dir="${PREFIX}/share/abmx/actual-helper"

mkdir -p "$bin_dir" "${PREFIX}/share/abmx"

# Replace atomically so a failed copy cannot leave a truncated binary.
tmp_bin="${bin_dir}/.abmx.tmp.$$"
cp "${tmp}/abmx/bin/abmx" "$tmp_bin"
chmod 0755 "$tmp_bin"
mv -f "$tmp_bin" "${bin_dir}/abmx"

mkdir -p "$share_dir"
for file in helper.js package.json package-lock.json; do
    [ -f "${tmp}/abmx/share/abmx/actual-helper/${file}" ] || die "release archive is missing ${file}"
    cp "${tmp}/abmx/share/abmx/actual-helper/${file}" "${share_dir}/${file}"
done
chmod 0755 "${share_dir}/helper.js"

node_major=0
if command -v node >/dev/null 2>&1; then
    node_major="$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || printf '0')"
    case "$node_major" in
        ''|*[!0-9]*) node_major=0 ;;
    esac
fi

helper_status="installed"
if [ "$SKIP_HELPER" = "1" ]; then
    helper_status="skipped"
elif ! command -v npm >/dev/null 2>&1 || [ "$node_major" -lt 22 ]; then
    helper_status="missing dependencies"
    warn "Node 22+ and npm were not found; the Actual sink needs them."
    warn "Install Node 22+, then run: cd ${share_dir} && npm ci --omit=dev"
else
    lock_hash="$(sha256_of "${share_dir}/package-lock.json")" || exit 1
    marker="${share_dir}/.installed-lock"
    if [ -f "$marker" ] && [ "$(cat "$marker")" = "$lock_hash" ] && [ -d "${share_dir}/node_modules" ]; then
        helper_status="already installed"
    elif (cd "$share_dir" && npm ci --omit=dev --no-audit --no-fund); then
        printf '%s\n' "$lock_hash" > "$marker"
    else
        helper_status="missing dependencies"
        warn "npm ci failed; the Actual sink needs the helper dependencies."
        warn "Fix the error above, then run: cd ${share_dir} && npm ci --omit=dev"
    fi
fi

installed_version="$("${bin_dir}/abmx" --version 2>&1)" || die "installed binary failed to run"
printf '\n%s installed to %s\n' "$installed_version" "${bin_dir}/abmx"
printf 'Actual helper: %s (%s)\n' "$helper_status" "$share_dir"

case ":${PATH:-}:" in
    *":${bin_dir}:"*) ;;
    *)
        printf '\n%s is not on PATH yet. Add it:\n' "$bin_dir"
        case "${SHELL:-/bin/sh}" in
            fish|*/fish) printf '  fish_add_path %s\n' "$bin_dir" ;;
            *) printf "  export PATH=\"%s:\$PATH\"\n" "$bin_dir" ;;
        esac
        ;;
esac

printf '\nnext: abmx setup\n'
