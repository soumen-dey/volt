#!/bin/sh
set -eu

repo="soumen-dey/volt"
binary="volt"
base_url="https://github.com/$repo/releases"

fail() {
  printf 'Volt installer: %s\n' "$*" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || fail "curl is required."
command -v tar >/dev/null 2>&1 || fail "tar is required."
command -v awk >/dev/null 2>&1 || fail "awk is required."

os=$(uname -s)
architecture=$(uname -m)
case "$os:$architecture" in
  Linux:x86_64|Linux:amd64)
    target="x86_64-unknown-linux-gnu"
    ;;
  Linux:aarch64|Linux:arm64)
    target="aarch64-unknown-linux-gnu"
    ;;
  Darwin:x86_64|Darwin:amd64)
    target="x86_64-apple-darwin"
    ;;
  Darwin:arm64|Darwin:aarch64)
    target="aarch64-apple-darwin"
    ;;
  *)
    fail "unsupported platform: $os $architecture. Supported targets are Linux and macOS on x86_64 or arm64."
    ;;
esac

if [ -n "${VOLT_INSTALL_DIR:-}" ]; then
  install_dir=$VOLT_INSTALL_DIR
else
  [ -n "${HOME:-}" ] || fail "HOME is not set; specify VOLT_INSTALL_DIR."
  install_dir="$HOME/.local/bin"
fi

version=${VOLT_VERSION:-latest}
if [ "$version" = "latest" ]; then
  release_url=$(curl --fail --silent --show-error --location \
    --proto '=https' --tlsv1.2 \
    --output /dev/null --write-out '%{url_effective}' \
    "$base_url/latest") ||
    fail "could not resolve the latest GitHub release."
  tag=${release_url##*/}
else
  tag=$version
  case "$tag" in
    v*) ;;
    *) tag="v$tag" ;;
  esac
fi

case "$tag" in
  v[0-9]*)
    case "$tag" in
      *[!A-Za-z0-9._+-]*) fail "invalid release version: $tag." ;;
    esac
    ;;
  *)
    fail "could not resolve a valid release version."
    ;;
esac

asset="$binary-$tag-$target.tar.gz"
release_url="$base_url/download/$tag"
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/volt-install.XXXXXX") ||
  fail "could not create a temporary directory."
trap 'rm -rf "$tmp_dir"' 0
trap 'exit 1' HUP INT TERM

curl --fail --silent --show-error --location \
  --proto '=https' --tlsv1.2 \
  "$release_url/SHA256SUMS" -o "$tmp_dir/SHA256SUMS" ||
  fail "could not download release checksums for $tag."
curl --fail --silent --show-error --location \
  --proto '=https' --tlsv1.2 \
  "$release_url/$asset" -o "$tmp_dir/$asset" ||
  fail "could not download $asset."

expected_checksum=$(awk -v file="$asset" \
  '$2 == file { print $1; count++ } END { if (count != 1) exit 1 }' \
  "$tmp_dir/SHA256SUMS") ||
  fail "release checksums do not contain exactly one entry for $asset."
case "$expected_checksum" in
  *[!0-9a-f]*|'') fail "invalid checksum for $asset." ;;
esac
[ "${#expected_checksum}" -eq 64 ] || fail "invalid checksum for $asset."
if command -v sha256sum >/dev/null 2>&1; then
  actual_checksum=$(sha256sum "$tmp_dir/$asset" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
  actual_checksum=$(shasum -a 256 "$tmp_dir/$asset" | awk '{print $1}')
else
  fail "sha256sum or shasum is required to verify the download."
fi
[ "$actual_checksum" = "$expected_checksum" ] ||
  fail "checksum verification failed for $asset."

mkdir -p "$install_dir" || fail "could not create install directory: $install_dir."
tar -xzf "$tmp_dir/$asset" -C "$tmp_dir" "$binary" ||
  fail "could not extract $binary from $asset."
install -m 0755 "$tmp_dir/$binary" "$install_dir/$binary" ||
  fail "could not install $binary to $install_dir."

printf 'Installed Volt %s to %s/%s\n' "$tag" "$install_dir" "$binary"
case ":${PATH:-}:" in
  *:"$install_dir":*) ;;
  *) printf 'Add %s to your PATH to run volt.\n' "$install_dir" ;;
esac
