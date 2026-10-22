#!/bin/sh
# Rune Kit installer for Linux/macOS. No root or Rust toolchain required.
# curl -fsSL https://raw.githubusercontent.com/shba007/rune-kit/main/install.sh | sh
# Overrides: RUNE_KIT_VERSION, RUNE_KIT_INSTALL_DIR, RUNE_KIT_BASE_URL,
# RUNE_KIT_TARGET (advanced). Mirrors require a pinned version and SHA-256 sidecar.
set -eu
say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }
repo=shba007/rune-kit
version=${RUNE_KIT_VERSION:-latest}
install_dir=${RUNE_KIT_INSTALL_DIR:-"$HOME/.local/bin"}
base=${RUNE_KIT_BASE_URL:-}

have curl || die 'curl is required.'
have tar || die 'tar is required.'
if [ -n "$base" ] && [ "$version" = latest ]; then die 'Set RUNE_KIT_VERSION when using a mirror.'; fi
if [ "$version" = latest ]; then
    location=$(curl -fsSL --retry 3 -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest") || die 'Could not find the latest release.'
    version=${location##*/}
fi
case "$version" in v*) ;; *) version="v$version" ;; esac
case "$version" in *[!A-Za-z0-9.+-]*|v) die 'Invalid release version.' ;; esac
if [ -z "$base" ]; then base="https://github.com/$repo/releases/download/$version"; fi
base=${base%/}

target=${RUNE_KIT_TARGET:-}
if [ -z "$target" ]; then
    os=$(uname -s); arch=$(uname -m)
    case "$os:$arch" in
        Linux:x86_64|Linux:amd64)
            target=x86_64-unknown-linux-gnu
            if ldd --version 2>&1 | grep -qi musl; then target=x86_64-unknown-linux-musl; fi ;;
        Darwin:x86_64) target=x86_64-apple-darwin ;;
        Darwin:arm64|Darwin:aarch64) target=aarch64-apple-darwin ;;
        *) die "No prebuilt package for $os/$arch. Build from source with cargo install --git https://github.com/$repo rune-kit-cli." ;;
    esac
fi
case "$target" in x86_64-unknown-linux-gnu|x86_64-unknown-linux-musl|x86_64-apple-darwin|aarch64-apple-darwin) ;;
    *) die "Unsupported target: $target" ;; esac

fetch() {
    if [ -d "$base" ]; then cp "$base/$1" "$2"; else curl -fsSL --retry 3 "$base/$1" -o "$2"; fi
}
sha256() {
    if have sha256sum; then sha256sum "$1" | cut -d' ' -f1
    elif have shasum; then shasum -a 256 "$1" | cut -d' ' -f1
    elif have openssl; then openssl dgst -sha256 "$1" | sed 's/^.*= //'
    else die 'A SHA-256 tool (sha256sum, shasum or openssl) is required.'; fi
}
# mktemp owns this private directory. No user directory is recursively removed.
tmp=$(mktemp -d "${TMPDIR:-/tmp}/rune-kit.XXXXXXXX") || die 'Cannot create temporary directory.'
stage=
cleanup() { [ -z "$stage" ] || rm -f -- "$stage"; rm -rf -- "$tmp"; }
trap cleanup EXIT
trap 'exit 1' INT TERM
archive="rune-kit-$version-$target.tar.gz"
say "Installing Rune Kit $version ($target)..."
if ! fetch "$archive" "$tmp/package.tar.gz"; then
    archive="rune-$version-$target.tar.gz"
    fetch "$archive" "$tmp/package.tar.gz" || die 'Package not available; the release may still be building.'
fi
fetch "$archive.sha256" "$tmp/checksum" || die 'No checksum sidecar. Choose a release with installer support or install this older release manually.'
expected=$(awk 'NR==1 {print tolower($1)}' "$tmp/checksum")
case "$expected" in *[!0-9a-f]*|'') die 'Invalid SHA-256 checksum.' ;; esac
[ "${#expected}" -eq 64 ] || die 'Invalid SHA-256 checksum.'
actual=$(sha256 "$tmp/package.tar.gz")
[ "$actual" = "$expected" ] || die 'Checksum mismatch; existing installation was not changed.'
# Extract one known root entry only: never unpack arbitrary archive paths.
entry=$(tar -tzf "$tmp/package.tar.gz" | grep -E '^(\./)?rune-kit$' | head -n 1 || true)
if [ -z "$entry" ]; then entry=$(tar -tzf "$tmp/package.tar.gz" | grep -E '^(\./)?rune$' | head -n 1 || true); fi
[ -n "$entry" ] || die 'Archive contains neither rune-kit nor legacy rune.'
tar -xOzf "$tmp/package.tar.gz" "$entry" > "$tmp/rune-kit" || die 'Cannot read executable from archive.'
chmod 755 "$tmp/rune-kit"
reported=$("$tmp/rune-kit" --version) || die 'Downloaded executable failed its version check.'
case "$reported" in "rune-kit ${version#v}"|"rune ${version#v}") ;; *) die "Unexpected binary version: $reported" ;; esac
mkdir -p "$install_dir" || die "Cannot create $install_dir. Set RUNE_KIT_INSTALL_DIR to a writable directory."
stage=$(mktemp "$install_dir/.rune-kit-install.XXXXXXXX") || die 'Cannot stage executable.'
cp "$tmp/rune-kit" "$stage" && chmod 755 "$stage" && mv -f "$stage" "$install_dir/rune-kit" || die 'Cannot replace executable.'
stage=
say "Installed: $install_dir/rune-kit ($reported)"
case ":$PATH:" in *":$install_dir:"*) ;;
    *) say "$install_dir is not on PATH. Add it to your shell profile, then open a new terminal:"
       # Single-quote paths safely, including apostrophes, before printing shell code.
       quoted=$(printf '%s' "$install_dir" | sed "s/'/'\\\\''/g")
       say "  export PATH='$quoted':\$PATH" ;;
esac
say 'Next: rune-kit --help'
