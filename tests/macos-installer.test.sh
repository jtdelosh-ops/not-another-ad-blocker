#!/bin/bash
# Run on macOS: /bin/bash tests/macos-installer.test.sh
# Each helper invocation receives an isolated HOME; no real browser is touched.
set -Eeuo pipefail
# Keep diagnostics independent of per-invocation capture redirections. Bash can
# run an error/exit trap while a function's stderr still points at its log file.
# Sending that log back to the same descriptor can make cat copy into itself.
exec 3>&2

[[ "$(/usr/bin/uname -s)" == Darwin ]] || { printf 'These integration tests require macOS and its system plutil.\n' >&2; exit 1; }
REPOSITORY=$(cd -P -- "$(/usr/bin/dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
TEMP_BASE=$(cd -P -- "${TMPDIR:-/tmp}" && pwd -P)
TEST_ROOT=$(/usr/bin/mktemp -d "$TEMP_BASE/naab-installer-tests.XXXXXX")
cleanup() {
  # Recursive removal is limited to this exact mktemp directory, in this shell.
  case "$TEST_ROOT" in
    "$TEMP_BASE"/naab-installer-tests.??????)
      [[ -d "$TEST_ROOT" && ! -L "$TEST_ROOT" ]] && /bin/rm -rf -- "$TEST_ROOT" ;;
    *) printf 'Refusing to clean an unexpected temporary path: %s\n' "$TEST_ROOT" >&3 ;;
  esac
}
finish() {
  local result=$?
  local captured
  trap - EXIT
  # The fixture contains only synthetic IDs and paths. Show captured helper
  # output before deleting it so a platform-specific failure is actionable.
  set +e
  if [[ "$result" -ne 0 ]]; then
    printf '\nInstaller integration test exited %s. Captured fixture output:\n' "$result" >&3
    for captured in "$TEST_ROOT"/*.txt; do
      [[ -f "$captured" && ! -L "$captured" ]] || continue
      printf '\n--- %s ---\n' "${captured##*/}" >&3
      /bin/cat -- "$captured" >&3
    done
  fi
  cleanup
  exit "$result"
}
trap finish EXIT
trap 'printf "Unexpected test failure at line %s: %s\n" "$LINENO" "$BASH_COMMAND" >&3' ERR
trap 'exit 130' INT
trap 'exit 143' TERM

TEST_HOME="$TEST_ROOT/home with space 'quote' \"double\" \\backslash"
PACKAGE="$TEST_ROOT/package with space 'quote' \"double\" \\backslash"
HELPER="$PACKAGE/Install.command"
BINARY="$PACKAGE/companion/naab-companion"
CHROME_MANIFEST="$TEST_HOME/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.naab.companion.json"
EDGE_MANIFEST="$TEST_HOME/Library/Application Support/Microsoft Edge/NativeMessagingHosts/com.naab.companion.json"
ID_A='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
ID_B='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
/bin/mkdir -p -- "$TEST_HOME" "$PACKAGE/companion"
/bin/cp -- "$REPOSITORY/scripts/install-macos.command" "$HELPER"
cat > "$BINARY" <<'BINARY'
#!/bin/bash
printf 'Installer must not launch the native executable.\n' > "$0.ran"
exit 99
BINARY
/bin/chmod +x "$HELPER" "$BINARY"

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
pass() { printf 'PASS: %s\n' "$*"; }
run_helper() { /usr/bin/env HOME="$TEST_HOME" /bin/bash "$HELPER" "$@"; }
expect_failure() {
  if run_helper "$@" </dev/null > "$TEST_ROOT/failure.txt" 2>&1; then
    fail "Unexpected success: $*"
  fi
}
field() { /usr/bin/plutil -extract "$2" raw -o - "$1"; }
assert_identity() {
  local manifest=$1
  [[ -f "$manifest" ]] || fail "Missing manifest: $manifest"
  [[ "$(field "$manifest" name)" == com.naab.companion ]] || fail 'Wrong host name'
  [[ "$(field "$manifest" path)" == "$BINARY" ]] || fail 'Package path did not survive JSON serialization'
  [[ "$(field "$manifest" type)" == stdio ]] || fail 'Wrong native transport type'
  [[ "$(/usr/bin/plutil -extract allowed_origins raw -expect array -o - "$manifest")" == 1 ]] || fail 'Origin array must contain exactly one entry'
  [[ "$(field "$manifest" allowed_origins.0)" == "chrome-extension://$ID_A/" ]] || fail 'Wrong allowed extension origin'
  # Node is a CI test dependency only. Chrome consumes JSON, whereas plutil's
  # plist lint mode can reject valid JSON even when typed extraction succeeds.
  node --input-type=module - "$manifest" "$BINARY" "$ID_A" <<'NODE'
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const [manifestPath, binaryPath, extensionId] = process.argv.slice(2);
assert.deepEqual(JSON.parse(readFileSync(manifestPath, 'utf8')), {
  name: 'com.naab.companion',
  description: 'Not Another Ad Blocker local companion',
  path: binaryPath,
  type: 'stdio',
  allowed_origins: [`chrome-extension://${extensionId}/`]
});
NODE
}

run_helper --help > "$TEST_ROOT/help.txt"
expect_failure --extension-id invalid
expect_failure --extension-id ''
expect_failure --extension-id "$ID_A" --extension-id "$ID_B"
expect_failure --extension-id
expect_failure --extension-id "$ID_A" --browser safari
expect_failure --extension-id "$ID_A" --unexpected
expect_failure
[[ ! -e "$TEST_HOME/Library" ]] || fail 'Invalid input changed browser directories'
pass 'invalid IDs, missing input and unknown options fail before registration'

run_helper --preview --extension-id "$ID_A" </dev/null > "$TEST_ROOT/preview.txt" 2>&1
[[ ! -e "$TEST_HOME/Library" ]] || fail 'Preview created a browser registration directory'
if /usr/bin/grep -q 'Paste the 32-character' "$TEST_ROOT/preview.txt"; then fail 'Explicit ID prompted again'; fi
/usr/bin/grep -q 'Preview only' "$TEST_ROOT/preview.txt" || fail 'Preview was not labeled'
pass 'explicit ID does not prompt; preview leaves native registration untouched'

if printf '%s\n%s\n' invalid "$ID_A" | run_helper > "$TEST_ROOT/invalid-prompt.txt" 2>&1; then
  fail 'Invalid prompted ID should fail instead of reading a second value'
fi
[[ "$(/usr/bin/grep -c 'Paste the 32-character' "$TEST_ROOT/invalid-prompt.txt")" == 1 ]] || fail 'ID was requested more than once'
[[ ! -e "$CHROME_MANIFEST" ]] || fail 'Invalid prompted ID created a manifest'
printf '%s\n' "$ID_A" | run_helper > "$TEST_ROOT/prompt.txt" 2>&1
[[ "$(/usr/bin/grep -c 'Paste the 32-character' "$TEST_ROOT/prompt.txt")" == 1 ]] || fail 'Successful install did not prompt exactly once'
assert_identity "$CHROME_MANIFEST"
[[ ! -e "$BINARY.ran" ]] || fail 'Installer launched the companion'
pass 'one-prompt Chrome install preserves paths with spaces, quotes and backslashes'

/bin/cp -- "$CHROME_MANIFEST" "$TEST_ROOT/original.json"
run_helper --extension-id "$ID_A" </dev/null > "$TEST_ROOT/repeat.txt" 2>&1
/usr/bin/cmp -s "$CHROME_MANIFEST" "$TEST_ROOT/original.json" || fail 'Idempotent install rewrote its manifest'
/usr/bin/grep -q 'Already registered' "$TEST_ROOT/repeat.txt" || fail 'Idempotent success was not reported'
if /usr/bin/grep -q 'Paste the 32-character' "$TEST_ROOT/repeat.txt"; then fail 'Explicit ID prompted again'; fi
pass 'repeat installation is idempotent'

run_helper --browser edge --extension-id "$ID_A" </dev/null > "$TEST_ROOT/edge.txt"
assert_identity "$EDGE_MANIFEST"
/usr/bin/cmp -s "$CHROME_MANIFEST" "$TEST_ROOT/original.json" || fail 'Edge install changed Chrome registration'
pass 'Edge uses its separate current-user manifest directory'

expect_failure --extension-id "$ID_B"
expect_failure --extension-id "$ID_B" --uninstall
/usr/bin/cmp -s "$CHROME_MANIFEST" "$TEST_ROOT/original.json" || fail 'Mismatched ID changed an existing manifest'
for field_name in name path type; do
  /bin/cp -- "$TEST_ROOT/original.json" "$CHROME_MANIFEST"
  /usr/bin/plutil -replace "$field_name" -string "different-$field_name" "$CHROME_MANIFEST"
  /bin/cp -- "$CHROME_MANIFEST" "$TEST_ROOT/conflict.json"
  expect_failure --extension-id "$ID_A"
  expect_failure --extension-id "$ID_A" --uninstall
  /usr/bin/cmp -s "$CHROME_MANIFEST" "$TEST_ROOT/conflict.json" || fail "Conflicting $field_name was changed"
done
/bin/cp -- "$TEST_ROOT/original.json" "$CHROME_MANIFEST"
/usr/bin/plutil -insert allowed_origins.1 -string "chrome-extension://$ID_B/" "$CHROME_MANIFEST"
/bin/cp -- "$CHROME_MANIFEST" "$TEST_ROOT/conflict.json"
expect_failure --extension-id "$ID_A"
expect_failure --extension-id "$ID_A" --uninstall
/usr/bin/cmp -s "$CHROME_MANIFEST" "$TEST_ROOT/conflict.json" || fail 'Extra allowed origin was changed'
/bin/cp -- "$TEST_ROOT/original.json" "$CHROME_MANIFEST"
/usr/bin/plutil -replace allowed_origins -json '{"0":"chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"}' "$CHROME_MANIFEST"
/bin/cp -- "$CHROME_MANIFEST" "$TEST_ROOT/conflict.json"
expect_failure --extension-id "$ID_A"
expect_failure --extension-id "$ID_A" --uninstall
/usr/bin/cmp -s "$CHROME_MANIFEST" "$TEST_ROOT/conflict.json" || fail 'Origin dictionary was accepted as an array'
printf 'not valid JSON\n' > "$CHROME_MANIFEST"
expect_failure --extension-id "$ID_A"
expect_failure --extension-id "$ID_A" --uninstall
[[ "$(/bin/cat "$CHROME_MANIFEST")" == 'not valid JSON' ]] || fail 'Malformed manifest was modified'
pass 'conflicting identity, extra origins, wrong array type and malformed manifests are preserved'

/bin/rm -- "$CHROME_MANIFEST"
/bin/ln -s "$TEST_ROOT/original.json" "$CHROME_MANIFEST"
expect_failure --extension-id "$ID_A"
expect_failure --extension-id "$ID_A" --uninstall
[[ -L "$CHROME_MANIFEST" ]] || fail 'Symlinked manifest was removed'
/bin/rm -- "$CHROME_MANIFEST"
/bin/cp -- "$TEST_ROOT/original.json" "$CHROME_MANIFEST"
pass 'symlinked manifest is never overwritten or removed'

run_helper --uninstall --preview --extension-id "$ID_A" </dev/null > "$TEST_ROOT/uninstall-preview.txt"
assert_identity "$CHROME_MANIFEST"
run_helper --uninstall --extension-id "$ID_A" </dev/null > "$TEST_ROOT/uninstall.txt"
[[ ! -e "$CHROME_MANIFEST" ]] || fail 'Matching Chrome manifest was not removed'
assert_identity "$EDGE_MANIFEST"
run_helper --uninstall --extension-id "$ID_A" </dev/null > "$TEST_ROOT/uninstall-repeat.txt"
/usr/bin/grep -q 'Nothing to remove' "$TEST_ROOT/uninstall-repeat.txt" || fail 'Repeated uninstall was not idempotent'
pass 'preview uninstall preserves files; actual uninstall removes only the matching browser registration'

# Missing binaries do not prevent unregistering an already-verified package,
# but they must prevent installation. Symlinks may never select another binary.
/bin/chmod -x "$BINARY"
expect_failure --extension-id "$ID_A"
/bin/chmod +x "$BINARY"
/bin/mv -- "$BINARY" "$TEST_ROOT/saved-binary"
expect_failure --extension-id "$ID_A"
/bin/ln -s "$TEST_ROOT/saved-binary" "$BINARY"
expect_failure --extension-id "$ID_A"
/bin/rm -- "$BINARY"
run_helper --uninstall --browser edge --extension-id "$ID_A" </dev/null > "$TEST_ROOT/uninstall-edge.txt"
[[ ! -e "$EDGE_MANIFEST" ]] || fail 'Edge registration was not removed after its binary was lost'
/bin/mv -- "$TEST_ROOT/saved-binary" "$BINARY"
/bin/mv -- "$PACKAGE/companion" "$TEST_ROOT/other-companion"
/bin/ln -s "$TEST_ROOT/other-companion" "$PACKAGE/companion"
expect_failure --extension-id "$ID_A"
[[ ! -e "$CHROME_MANIFEST" ]] || fail 'Invalid companion created a manifest'
[[ ! -e "$TEST_ROOT/other-companion/naab-companion.ran" ]] || fail 'Installer launched the companion'
pass 'missing/nonexecutable binaries and symlink escapes are rejected without launching native code'

printf 'All macOS installer integration tests passed.\n'
