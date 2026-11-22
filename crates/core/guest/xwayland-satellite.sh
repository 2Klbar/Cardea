# Installs xwayland-satellite on an apt base: the distribution's package when it has one, else built from its git
# repository $1 at commit $2 (pinned: a tag can move, a commit cannot). The build tools that were not there before
# are removed in the same step; the libraries the binary uses are kept.
set -eu
repo=$1
rev=$2
if [ -n "$(apt-cache policy xwayland-satellite 2>/dev/null | sed -n 's/^ *Candidate: //p' | grep -v '(none)')" ]; then
    exec apt-get install -y --no-install-recommends xwayland-satellite
fi
tools="ca-certificates cargo clang libclang-dev libxcb-cursor-dev libxcb1-dev pkg-config rustc"
# shellcheck disable=SC2086
had=$(dpkg-query -W -f '${db:Status-Abbrev} ${Package}\n' $tools 2>/dev/null | awk '$1 == "ii" { print $2 }')
# shellcheck disable=SC2086
apt-get install -y --no-install-recommends $tools
export CARGO_HOME=/tmp/cardea-cargo
cargo install --locked --root /usr/local --git "$repo" --rev "$rev" xwayland-satellite
rm -rf "$CARGO_HOME" /usr/local/.crates.toml /usr/local/.crates2.json
bin=/usr/local/bin/xwayland-satellite
keep=""
for lib in $(ldd "$bin" | awk '$2 == "=>" && $3 ~ /^\// { print $3 }'); do
    real=$(readlink -f "$lib")
    pkg=$(dpkg -S "$real" 2>/dev/null || dpkg -S "${real#/usr}" 2>/dev/null) || continue
    keep="$keep ${pkg%%:*}"
done
# shellcheck disable=SC2086
[ -z "$keep" ] || apt-mark manual $keep >/dev/null
remove=""
for t in $tools; do
    printf '%s\n' "$had" | grep -qx "$t" || remove="$remove $t"
done
# shellcheck disable=SC2086
[ -z "$remove" ] || apt-get purge -y --auto-remove $remove
if ldd "$bin" | grep -q 'not found'; then
    ldd "$bin" >&2
    echo "xwayland-satellite: libraries missing after the build" >&2
    exit 1
fi
