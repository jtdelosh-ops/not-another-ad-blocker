#!/bin/sh
# Install the independent, root-owned offline recovery command on an isolated Mac.
set -eu

if [ "$(/usr/bin/id -u)" -ne 0 ]; then
  echo 'Run this installer with sudo.' >&2
  exit 1
fi
if [ "$#" -ne 1 ] || [ ! -f "$1" ] || [ -L "$1" ]; then
  echo 'Usage: sudo sh scripts/install-macos-dns-recovery.sh PATH_TO_naab-dns-macos' >&2
  exit 2
fi

directory='/Library/Application Support/NAAB-DNS-Preview'
parent='/Library/Application Support'
helper="$directory/recovery-helper"
plist='/Library/LaunchDaemons/com.naab.dns-recovery.plist'
label='com.naab.dns-recovery'
helper_tmp="$directory/recovery-helper.pending.$$"
plist_tmp="${plist}.pending.$$"

check_no_acl() {
  # -e prints each ACL entry on a separate line even when extended attributes
  # hide the ACL marker in the first line's permission field.
  listing=$(/bin/ls -lde "$1")
  lines=$(printf '%s\n' "$listing" | /usr/bin/wc -l | /usr/bin/tr -d '[:space:]')
  if [ "$lines" -ne 1 ]; then
    echo "Unexpected access-control list on protected path: $1" >&2
    exit 1
  fi
}

check_parent() {
  path=$1
  if [ -L "$path" ] || [ ! -d "$path" ]; then
    echo "Untrusted recovery parent: $path" >&2
    exit 1
  fi
  owner=$(/usr/bin/stat -f '%u' "$path")
  permissions=$(/usr/bin/stat -f '%Sp' "$path")
  group_write=$(printf '%s' "$permissions" | /usr/bin/cut -c6)
  other_write=$(printf '%s' "$permissions" | /usr/bin/cut -c9)
  if [ "$owner" != 0 ] || [ "$group_write" = w ] || [ "$other_write" = w ]; then
    echo "Unsafe recovery parent ownership or permissions: $path" >&2
    exit 1
  fi
  check_no_acl "$path"
}

check_parent /Library
check_parent "$parent"
check_parent /Library/LaunchDaemons
if [ -e "$directory" ] || [ -L "$directory" ]; then
  if [ -L "$directory" ] || [ ! -d "$directory" ] ||
     [ "$(/usr/bin/stat -f '%u:%Sp' "$directory")" != '0:drwx------' ]; then
    echo 'Existing Mac recovery directory has unsafe ownership or permissions.' >&2
    exit 1
  fi
  check_no_acl "$directory"
else
  /usr/bin/install -d -m 700 -o root -g wheel "$directory"
  check_no_acl "$directory"
fi
if [ -e "$directory/recovery.json" ] || [ -L "$directory/recovery.json" ]; then
  echo 'Pending or invalid Mac DNS recovery record; recover it before upgrading.' >&2
  exit 1
fi
if [ -e "$helper_tmp" ] || [ -L "$helper_tmp" ] ||
   [ -e "$plist_tmp" ] || [ -L "$plist_tmp" ]; then
  echo 'Stale Mac recovery installation files need inspection.' >&2
  exit 1
fi
trap '/bin/rm -f "$helper_tmp" "$plist_tmp"' EXIT HUP INT TERM
if [ -e "$helper" ] || [ -L "$helper" ]; then
  if [ -L "$helper" ] || [ ! -f "$helper" ] ||
     [ "$(/usr/bin/stat -f '%u:%Sp' "$helper")" != '0:-rwx------' ]; then
    echo 'Existing Mac recovery helper has unsafe ownership or permissions.' >&2
    exit 1
  fi
  check_no_acl "$helper"
fi
if [ -e "$plist" ] || [ -L "$plist" ]; then
  if [ -L "$plist" ] || [ ! -f "$plist" ] ||
     [ "$(/usr/bin/stat -f '%u:%Sp' "$plist")" != '0:-rw-r--r--' ]; then
    echo 'Existing Mac recovery task has unsafe ownership or permissions.' >&2
    exit 1
  fi
  check_no_acl "$plist"
fi

/usr/bin/install -m 700 -o root -g wheel "$1" "$helper_tmp"
check_no_acl "$helper_tmp"
/bin/cat > "$plist_tmp" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.naab.dns-recovery</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Library/Application Support/NAAB-DNS-Preview/recovery-helper</string>
    <string>recover-if-needed</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>StartInterval</key><integer>60</integer>
</dict>
</plist>
PLIST
/usr/sbin/chown root:wheel "$plist_tmp"
/bin/chmod 644 "$plist_tmp"
check_no_acl "$plist_tmp"
/usr/bin/plutil -lint "$plist_tmp" >/dev/null
/bin/mv -f "$helper_tmp" "$helper"
/bin/mv -f "$plist_tmp" "$plist"
check_no_acl "$helper"
check_no_acl "$plist"
if ! /bin/launchctl print "system/$label" >/dev/null 2>&1; then
  /bin/launchctl bootstrap system "$plist"
fi
/bin/launchctl enable "system/$label"
echo "Installed protected Mac DNS recovery helper: $helper"
echo 'Recovery runs at startup and every minute when a protected record exists.'
