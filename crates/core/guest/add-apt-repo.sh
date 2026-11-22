#!/bin/sh
# Cardea guest helper, run as root at build time:
# add-apt-repo.sh NAME URL SUITE KEY_URL FINGERPRINT COMPONENT...
# Adds a third-party apt repository whose signing key must have the expected fingerprint (R8): a key with another
# fingerprint stops the build. $CARDEA_ROOT is only set by tests.
set -eu
name=$1 url=$2 suite=$3 key_url=$4 fpr=$5
shift 5
root=${CARDEA_ROOT:-}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl --proto '=https' --tlsv1.2 -fsSL "$key_url" -o "$tmp/key"
found=$(gpg --batch --homedir "$tmp" --with-colons --show-keys "$tmp/key" 2>/dev/null | awk -F: '$1 == "fpr" { print $10 }')
if ! printf '%s\n' "$found" | grep -qx "$fpr"; then
    echo "apt repository $name: the key at $key_url does not have the fingerprint $fpr (found: $(echo $found))" >&2
    exit 1
fi
gpg --batch --homedir "$tmp" --import "$tmp/key" 2>/dev/null
mkdir -p "$root/etc/apt/keyrings" "$root/etc/apt/sources.list.d"
gpg --batch --homedir "$tmp" --export "$fpr" > "$root/etc/apt/keyrings/cardea-$name.gpg"
echo "deb [signed-by=/etc/apt/keyrings/cardea-$name.gpg] $url $suite $*" > "$root/etc/apt/sources.list.d/cardea-$name.list"
[ -n "$root" ] || apt-get update
