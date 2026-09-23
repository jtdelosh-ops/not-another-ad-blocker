#!/bin/sh
set -eu

name=${1:-ads.example.test}
expected=${2:-}
server=${NAAB_DNS_SERVER:-127.0.0.1}
port=${NAAB_DNS_PORT:-5354}

if ! command -v dig >/dev/null 2>&1; then
    printf '%s\n' 'dig is required. It is included with macOS or can be installed with Homebrew.' >&2
    exit 1
fi

if ! response=$(dig -r +noedns +time=3 +tries=1 +noall +comments +answer "@$server" -p "$port" "$name" A); then
    printf 'No DNS response from %s:%s. Confirm naab-dns-dev is running and the port is correct.\n' "$server" "$port" >&2
    exit 1
fi
status=$(printf '%s\n' "$response" | sed -n 's/.*status: \([^,]*\).*/\1/p' | head -n 1)

if [ -z "$status" ]; then
    printf 'No DNS response from %s:%s. Confirm naab-dns-dev is running and the port is correct.\n' "$server" "$port" >&2
    exit 1
fi

printf 'Name: %s\nServer: %s:%s\nResponseCode: %s\n' "$name" "$server" "$port" "$status"
if [ -n "$expected" ] && [ "$status" != "$expected" ]; then
    printf 'Expected %s but received %s.\n' "$expected" "$status" >&2
    exit 1
fi
