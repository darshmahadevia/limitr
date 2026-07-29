#!/usr/bin/env sh
set -eu

repository=${LIMITR_REPOSITORY:-darshmahadevia/limitr}
install_dir=${LIMITR_INSTALL_DIR:-"$HOME/.local/bin"}

fail() {
  echo "limitr installer: $*" >&2
  exit 1
}

for command_name in curl tar; do
  command -v "$command_name" >/dev/null 2>&1 ||
    fail "required command not found: $command_name"
done

case "$(uname -s):$(uname -m)" in
  Darwin:arm64)
    target=aarch64-apple-darwin
    ;;
  Darwin:x86_64)
    target=x86_64-apple-darwin
    ;;
  Linux:x86_64)
    target=x86_64-unknown-linux-musl
    ;;
  Linux:aarch64 | Linux:arm64)
    target=aarch64-unknown-linux-musl
    ;;
  *)
    fail "unsupported platform: $(uname -s) $(uname -m); Windows installs require Cargo"
    ;;
esac

version=${LIMITR_VERSION:-}
if [ -z "$version" ]; then
  release_json=$(curl -fsSL "https://api.github.com/repos/$repository/releases/latest") ||
    fail "could not determine the latest release"
  version=$(printf '%s\n' "$release_json" |
    sed -n 's/.*"tag_name":[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' |
    head -n 1)
  [ -n "$version" ] || fail "latest release did not contain a version tag"
fi
version=${version#v}

case "$version" in
  *[!0-9A-Za-z.-]* | "")
    fail "invalid release version: $version"
    ;;
esac

archive="limitr-v$version-$target.tar.gz"
release_url="https://github.com/$repository/releases/download/v$version"
temporary_dir=$(mktemp -d "${TMPDIR:-/tmp}/limitr-install.XXXXXX")
trap 'rm -rf "$temporary_dir"' EXIT HUP INT TERM

curl -fsSL "$release_url/$archive" -o "$temporary_dir/$archive" ||
  fail "could not download $archive"
curl -fsSL "$release_url/$archive.sha256" -o "$temporary_dir/$archive.sha256" ||
  fail "could not download the checksum for $archive"

checksum_line=$(cat "$temporary_dir/$archive.sha256")
case "$checksum_line" in
  [0-9a-fA-F][0-9a-fA-F]*"  $archive")
    ;;
  *)
    fail "release checksum has an unexpected format"
    ;;
esac

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$temporary_dir" && sha256sum -c "$archive.sha256" >/dev/null) ||
    fail "checksum verification failed"
elif command -v shasum >/dev/null 2>&1; then
  (cd "$temporary_dir" && shasum -a 256 -c "$archive.sha256" >/dev/null) ||
    fail "checksum verification failed"
else
  fail "required checksum command not found: sha256sum or shasum"
fi

tar -xzf "$temporary_dir/$archive" -C "$temporary_dir" limitr ||
  fail "could not extract limitr"
mkdir -p "$install_dir"
install -m 755 "$temporary_dir/limitr" "$install_dir/limitr"

echo "Installed limitr v$version to $install_dir/limitr"
case ":$PATH:" in
  *":$install_dir:"*)
    ;;
  *)
    echo "Add $install_dir to PATH to run limitr."
    ;;
esac
