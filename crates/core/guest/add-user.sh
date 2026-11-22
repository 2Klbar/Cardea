#!/bin/sh
# Cardea guest helper, run as root at build time: add-user.sh USER UID GID HOME.
# Creates the sandbox user with the host user's IDs, replacing any entry of the base image that uses the same
# name or IDs (the Ubuntu image has a user 1000). Writes the account files directly: no useradd needed.
set -eu
user=$1 uid=$2 gid=$3 home=$4

drop() { # drop FILE NAME ID: removes the lines whose name or numeric ID match.
    [ -f "$1" ] || return 0
    awk -F: -v n="$2" -v id="$3" '$1 != n && $3 != id' "$1" > "$1.cardea"
    cat "$1.cardea" > "$1"
    rm -f "$1.cardea"
}
drop /etc/passwd "$user" "$uid"
drop /etc/group "$user" "$gid"
if [ -f /etc/shadow ]; then
    awk -F: -v n="$user" '$1 != n' /etc/shadow > /etc/shadow.cardea
    cat /etc/shadow.cardea > /etc/shadow
    rm -f /etc/shadow.cardea
    echo "$user:!:1::::::" >> /etc/shadow
fi

shell=/bin/sh
[ -x /bin/bash ] && shell=/bin/bash
echo "$user:x:$uid:$gid::$home:$shell" >> /etc/passwd
echo "$user:x:$gid:" >> /etc/group
mkdir -p "$home"
chown "$uid:$gid" "$home"
chmod 700 "$home"

# Login shells (`cardea exec`): Debian's /etc/profile resets PATH, so put back the home's tool folders that the
# container's PATH has (same list and order as `spec::app`).
mkdir -p /etc/profile.d
cat > /etc/profile.d/cardea.sh <<'PROFILE'
for d in "$HOME/.local/share/mise/shims" "$HOME/.npm-global/bin" "$HOME/.cargo/bin" "$HOME/.local/bin"; do
    case ":$PATH:" in *":$d:"*) ;; *) PATH="$d:$PATH" ;; esac
done
export PATH
PROFILE
