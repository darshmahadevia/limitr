#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT

fake_bin="$test_root/fake-bin"
fixtures="$test_root/fixtures"
mkdir -p "$fake_bin" "$fixtures"

cat >"$fake_bin/uname" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  -s) printf '%s\n' "$TEST_UNAME_S" ;;
  -m) printf '%s\n' "$TEST_UNAME_M" ;;
  *) exit 1 ;;
esac
EOF

cat >"$fake_bin/curl" <<'EOF'
#!/usr/bin/env bash
output=
url=
while (($#)); do
  case "$1" in
    -o)
      output=$2
      shift 2
      ;;
    -*)
      shift
      ;;
    *)
      url=$1
      shift
      ;;
  esac
done
cp "$TEST_FIXTURES/${url##*/}" "$output"
printf '%s\n' "$url" >>"$TEST_CURL_LOG"
EOF

chmod +x "$fake_bin/uname" "$fake_bin/curl"

make_fixture() {
  local target=$1
  local archive="limitr-v9.8.7-$target.tar.gz"
  local staging="$test_root/staging-$target"
  mkdir -p "$staging"
  cat >"$staging/limitr" <<'EOF'
#!/usr/bin/env sh
printf 'limitr fixture\n'
EOF
  chmod +x "$staging/limitr"
  tar -C "$staging" -czf "$fixtures/$archive" limitr
  if command -v sha256sum >/dev/null 2>&1; then
    digest=$(sha256sum "$fixtures/$archive" | awk '{print $1}')
  else
    digest=$(shasum -a 256 "$fixtures/$archive" | awk '{print $1}')
  fi
  printf '%s  %s\n' "$digest" "$archive" >"$fixtures/$archive.sha256"
}

assert_install() {
  local os=$1
  local arch=$2
  local target=$3
  local install_dir="$test_root/install-$target"
  local curl_log="$test_root/curl-$target.log"

  make_fixture "$target"
  TEST_UNAME_S="$os" \
    TEST_UNAME_M="$arch" \
    TEST_FIXTURES="$fixtures" \
    TEST_CURL_LOG="$curl_log" \
    LIMITR_VERSION=9.8.7 \
    LIMITR_INSTALL_DIR="$install_dir" \
    PATH="$fake_bin:$PATH" \
    bash "$repo_root/install.sh"

  test "$("$install_dir/limitr")" = "limitr fixture"
  grep -q "limitr-v9.8.7-$target.tar.gz" "$curl_log"
}

assert_install Darwin arm64 aarch64-apple-darwin
assert_install Darwin x86_64 x86_64-apple-darwin
assert_install Linux x86_64 x86_64-unknown-linux-musl
assert_install Linux aarch64 aarch64-unknown-linux-musl

unsupported_output="$test_root/unsupported.txt"
if TEST_UNAME_S=Windows_NT \
  TEST_UNAME_M=x86_64 \
  PATH="$fake_bin:$PATH" \
  bash "$repo_root/install.sh" >"$unsupported_output" 2>&1; then
  echo "installer unexpectedly accepted Windows" >&2
  exit 1
fi
grep -qi "unsupported" "$unsupported_output"

echo "installer behavior tests passed"
