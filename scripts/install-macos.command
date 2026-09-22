#!/bin/bash
# Register the companion shipped beside this helper for the current user only.
set -Eeuo pipefail

HOST_NAME='com.naab.companion'
BROWSER='chrome'
EXTENSION_ID=''
ID_SUPPLIED=false
UNINSTALL=false
PREVIEW=false
TEMP_MANIFEST=''

fail() { printf 'NAAB: %s\n' "$*" >&2; exit 1; }
usage() {
  cat <<'USAGE'
Usage: Install.command [--extension-id ID] [--browser chrome|edge]
                       [--preview] [--uninstall]

Registers this package's companion for your user account. Chrome is the default.
If --extension-id is omitted, paste the extension ID once when prompted.
--preview prints the proposed manifest without changing browser registration.
--uninstall removes only a registration matching this package and extension ID.
USAGE
}
cleanup() {
  if [[ -n "$TEMP_MANIFEST" && -f "$TEMP_MANIFEST" ]]; then
    /bin/rm -f -- "$TEMP_MANIFEST"
  fi
}
trap cleanup EXIT
trap 'printf "NAAB: Installation stopped at line %s (exit %s).\n" "$LINENO" "$?" >&2' ERR
trap 'exit 130' INT
trap 'exit 143' TERM

while [[ $# -gt 0 ]]; do
  case "$1" in
    --extension-id)
      [[ $# -ge 2 && "$ID_SUPPLIED" == false ]] || fail 'Supply --extension-id once, followed by its value.'
      EXTENSION_ID=$2; ID_SUPPLIED=true; shift 2 ;;
    --browser)
      [[ $# -ge 2 ]] || fail '--browser requires chrome or edge.'
      BROWSER=$2; shift 2 ;;
    --uninstall) UNINSTALL=true; shift ;;
    --preview) PREVIEW=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *) fail "Unknown option: $1. Use --help for usage." ;;
  esac
done

[[ "$(/usr/bin/uname -s)" == Darwin ]] || fail 'This helper runs on macOS only.'
MACOS_VERSION=$(/usr/bin/sw_vers -productVersion)
MACOS_MAJOR=${MACOS_VERSION%%.*}
[[ "$MACOS_MAJOR" =~ ^[0-9]+$ && "$MACOS_MAJOR" -ge 12 ]] || fail 'This helper requires macOS 12 or later.'
[[ "$EUID" -ne 0 ]] || fail 'Run this helper as your normal user, without sudo.'
case "$BROWSER" in
  chrome) BROWSER_DIRECTORY='Google/Chrome'; BROWSER_LABEL='Chrome' ;;
  edge) BROWSER_DIRECTORY='Microsoft Edge'; BROWSER_LABEL='Microsoft Edge' ;;
  *) fail 'Browser must be chrome or edge.' ;;
esac

# Resolve the package directory physically. The fixed companion location may
# not be redirected by a symlink to another package or executable.
PACKAGE_DIRECTORY=$(cd -P -- "$(/usr/bin/dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
COMPANION_DIRECTORY="$PACKAGE_DIRECTORY/companion"
[[ -d "$COMPANION_DIRECTORY" && ! -L "$COMPANION_DIRECTORY" ]] || fail 'The companion folder is missing or is a symbolic link. Keep the extracted package together.'
[[ "$(cd -P -- "$COMPANION_DIRECTORY" && pwd -P)" == "$COMPANION_DIRECTORY" ]] || fail 'The companion folder must stay inside this package.'
COMPANION_BINARY="$COMPANION_DIRECTORY/naab-companion"
[[ ! -L "$COMPANION_BINARY" ]] || fail 'The companion executable must not be a symbolic link.'
if [[ "$UNINSTALL" == false ]]; then
  [[ -f "$COMPANION_BINARY" && -x "$COMPANION_BINARY" ]] || fail 'The companion executable is missing or not executable. Extract a fresh copy of the package.'
fi
case "${HOME:-}" in
  /*) [[ -d "$HOME" ]] || fail 'Your home directory is unavailable.' ;;
  *) fail 'Your home directory must be an absolute path.' ;;
esac
USER_DIRECTORY=$(cd -P -- "$HOME" && pwd -P)
MANIFEST_DIRECTORY="$USER_DIRECTORY/Library/Application Support/$BROWSER_DIRECTORY/NativeMessagingHosts"
MANIFEST_PATH="$MANIFEST_DIRECTORY/$HOST_NAME.json"

if [[ "$ID_SUPPLIED" == false ]]; then
  printf 'Paste the 32-character NAAB extension ID from %s, then press Return: ' "$BROWSER_LABEL" >&2
  IFS= read -r EXTENSION_ID || fail 'No extension ID was received. Run again or use --extension-id ID.'
fi
[[ "$EXTENSION_ID" =~ ^[a-p]{32}$ ]] || fail 'The extension ID must contain exactly 32 lowercase letters from a through p.'
ALLOWED_ORIGIN="chrome-extension://$EXTENSION_ID/"

matches_identity() {
  local candidate=$1
  [[ -f "$candidate" && ! -L "$candidate" ]] || return 1
  [[ "$(/usr/bin/plutil -extract name raw -expect string -o - "$candidate" 2>/dev/null)" == "$HOST_NAME" ]] || return 1
  [[ "$(/usr/bin/plutil -extract path raw -expect string -o - "$candidate" 2>/dev/null)" == "$COMPANION_BINARY" ]] || return 1
  [[ "$(/usr/bin/plutil -extract type raw -expect string -o - "$candidate" 2>/dev/null)" == stdio ]] || return 1
  [[ "$(/usr/bin/plutil -extract allowed_origins raw -expect array -o - "$candidate" 2>/dev/null)" == 1 ]] || return 1
  [[ "$(/usr/bin/plutil -extract allowed_origins.0 raw -expect string -o - "$candidate" 2>/dev/null)" == "$ALLOWED_ORIGIN" ]]
}
conflict() {
  printf 'NAAB: A different or unreadable registration already exists:\n  %s\n' "$MANIFEST_PATH" >&2
  printf 'It was left unchanged. Use the original package and extension ID with --uninstall first, or inspect this file before changing it.\n' >&2
  exit 1
}

EXISTS=false
if [[ -e "$MANIFEST_PATH" || -L "$MANIFEST_PATH" ]]; then
  matches_identity "$MANIFEST_PATH" || conflict
  EXISTS=true
fi

if [[ "$UNINSTALL" == true ]]; then
  if [[ "$PREVIEW" == true ]]; then
    printf 'Preview only: would remove the matching registration, if present:\n  %s\n' "$MANIFEST_PATH"
  elif [[ "$EXISTS" == true ]]; then
    # One identity-checked file only; retain browser settings, lists and package.
    /bin/rm -- "$MANIFEST_PATH"
    printf 'Registration removed for %s:\n  %s\n' "$BROWSER_LABEL" "$MANIFEST_PATH"
  else
    printf 'No matching registration is installed for %s. Nothing to remove.\n' "$BROWSER_LABEL"
  fi
  exit 0
fi

if [[ "$PREVIEW" == false && "$EXISTS" == true ]]; then
  printf 'Already registered for %s. No files changed.\n' "$BROWSER_LABEL"
  printf 'Open NAAB settings in the browser and click Check companion.\n'
  exit 0
fi

# plutil serializes every string, including package paths containing quotes,
# backslashes or spaces. The executable is never launched during registration.
if [[ "$PREVIEW" == true ]]; then
  TEMP_MANIFEST=$(/usr/bin/mktemp "${TMPDIR:-/tmp}/naab-manifest.XXXXXX")
else
  [[ ! -L "$MANIFEST_DIRECTORY" ]] || fail 'The NativeMessagingHosts directory is a symbolic link; registration was not changed.'
  /bin/mkdir -p -- "$MANIFEST_DIRECTORY"
  TEMP_MANIFEST=$(/usr/bin/mktemp "$MANIFEST_DIRECTORY/.naab-manifest.XXXXXX")
fi
printf '{}\n' > "$TEMP_MANIFEST"
/usr/bin/plutil -insert name -string "$HOST_NAME" "$TEMP_MANIFEST"
/usr/bin/plutil -insert description -string 'Not Another Ad Blocker local companion' "$TEMP_MANIFEST"
/usr/bin/plutil -insert path -string "$COMPANION_BINARY" "$TEMP_MANIFEST"
/usr/bin/plutil -insert type -string stdio "$TEMP_MANIFEST"
/usr/bin/plutil -insert allowed_origins -array "$TEMP_MANIFEST"
/usr/bin/plutil -insert allowed_origins.0 -string "$ALLOWED_ORIGIN" "$TEMP_MANIFEST"
/usr/bin/plutil -convert json -r "$TEMP_MANIFEST"
matches_identity "$TEMP_MANIFEST" || fail 'The generated registration could not be verified.'

if [[ "$PREVIEW" == true ]]; then
  printf 'Preview only. Registration target:\n  %s\n' "$MANIFEST_PATH"
  /bin/cat -- "$TEMP_MANIFEST"
  printf '\n'
else
  # The temporary file is on the same filesystem. Linking publishes it without
  # overwriting an existing regular file, including a concurrent installation.
  [[ ! -e "$MANIFEST_PATH" && ! -L "$MANIFEST_PATH" ]] || conflict
  /bin/ln -h -- "$TEMP_MANIFEST" "$MANIFEST_PATH" || fail 'Could not create the registration; an existing file was not overwritten.'
  printf 'Registered NAAB for %s:\n  %s\n' "$BROWSER_LABEL" "$MANIFEST_PATH"
  printf 'Keep this package in its current location. Open NAAB settings and click Check companion.\n'
fi
