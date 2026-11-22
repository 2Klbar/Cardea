#!/bin/bash
# Cardea host self-test: creates throw-away sandboxes on this computer, checks every feature that can be checked
# without a person, deletes them and writes a Markdown report.
#
#   selftest.sh [-o report.md] [--quick] [--claude] [--keep]
#
#   --quick   skip the steps that rebuild an image (packages, services, keyring, GPU, X11, apt repository)
#   --claude  also build a full Claude sandbox and check its tools (long: 15-30 min, large download)
#   --keep    keep the test sandboxes at the end (to look at them)
#
# Needs: cardea installed (log out and back in once after installing), podman, python3, bash, coreutils,
# iproute2, util-linux (`script`). Uses `podman` directly only to inject failures (kill the sidecar, stop a
# container, hold a home) and to collect diagnostics. Takes 20 to 40 minutes without --claude.
# The link checks open one tab of https://example.com/?cardea-selftest in your default browser, and show the link
# window a few seconds (the test closes it).

set -u

REPORT="cardea-selftest-$(date +%Y%m%d-%H%M%S).md"
QUICK=0
CLAUDE=0
KEEP=0
while [ $# -gt 0 ]; do
    case $1 in
        -o) REPORT=$2; shift ;;
        --quick) QUICK=1 ;;
        --claude) CLAUDE=1 ;;
        --keep) KEEP=1 ;;
        -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

CARDEA=${CARDEA:-cardea}
A=selftest-a        # Work sandbox: most checks
B=selftest-b        # Untrusted sandbox
C=selftest-claude   # --claude
D=selftest-d        # from a saved profile, deleted with --keep-home
A2=selftest-a-2     # name collision
IDS="$A $B $C $D $A2"
PROFILES="selftest-prof selftest-import"
WORK=$(mktemp -d /tmp/cardea-selftest.XXXXXX)
RAW=$WORK/raw
mkdir -p "$RAW"
BODY=$WORK/body.md
SUMMARY=$WORK/summary.md
: >"$BODY"
: >"$SUMMARY"
PASS=0 FAIL=0 SKIP=0
STEP=""
N=0
LISTENER=""
DONE=0

say() { printf '%s\n' "$*" >&2; }

# --- Recording ---------------------------------------------------------------------------------------------

step() {
    STEP=$1
    say ""
    say "== $1"
    printf '\n## %s\n\n' "$1" >>"$BODY"
}

# result STATUS TITLE [EVIDENCE_FILE]
result() {
    local status=$1 title=$2 file=${3:-}
    case $status in
        PASS) PASS=$((PASS + 1)) ;;
        FAIL) FAIL=$((FAIL + 1)) ;;
        SKIP) SKIP=$((SKIP + 1)) ;;
    esac
    say "  $status  $title"
    printf '| %s | %s | %s |\n' "$status" "$STEP" "$title" >>"$SUMMARY"
    printf -- '- **%s** %s\n' "$status" "$title" >>"$BODY"
    if [ -n "$file" ] && [ -s "$file" ]; then
        printf '\n  ```\n' >>"$BODY"
        head -n 60 "$file" | sed 's/^/  /' >>"$BODY"
        [ "$(wc -l <"$file")" -gt 60 ] && printf '  … (%s lines)\n' "$(wc -l <"$file")" >>"$BODY"
        printf '  ```\n\n' >>"$BODY"
    fi
    write_report
}
ok() { result PASS "$@"; }
# A failure shows the last command's output unless it names its own evidence.
ko() { result FAIL "$1" "${2:-${OUT:-}}"; }
skip() { result SKIP "$@"; }
# expect CONDITION_EXIT_STATUS "title" [file]: PASS when the previous test command succeeded.
expect() { if [ "$1" -eq 0 ]; then ok "$2" "${3:-$OUT}"; else ko "$2" "${3:-$OUT}"; fi; }

# run TIMEOUT CMD… : runs a command; output in the file $OUT, exit code in $RC.
run() {
    local t=$1
    shift
    N=$((N + 1))
    OUT=$RAW/$N.txt
    printf '$ %s\n' "$*" >"$OUT"
    timeout "$t" "$@" </dev/null >>"$OUT" 2>&1
    RC=$?
    return 0
}

# Same, in a pseudo-terminal (TTY paths, password prompts); `input` is typed after 2 s.
run_tty() {
    local t=$1 input=$2
    shift 2
    N=$((N + 1))
    OUT=$RAW/$N.txt
    printf '$ (tty) %s\n' "$*" >"$OUT"
    { sleep 2; printf '%s' "$input"; sleep "$t"; } | timeout "$t" script -qec "$*" /dev/null >>"$OUT" 2>&1
    RC=$?
    return 0
}

in_box() {
    local id=$1
    shift
    run 300 "$CARDEA" exec "$id" -- "$@"
}

# Line 1 of $OUT is the command itself: never searched, so a pattern it contains does not match.
has() { tail -n +2 "$OUT" | grep -q -- "$1"; }
hasi() { tail -n +2 "$OUT" | grep -qi -- "$1"; }

# The daemon's control socket, as the manager and the tray use it: `ctl call METHOD JSON` prints the answer,
# `ctl events SECONDS` prints the events of that many seconds.
CTL_PY=$WORK/ctl.py
cat >"$CTL_PY" <<'EOF'
import json, os, socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(os.path.join(os.environ["XDG_RUNTIME_DIR"], "cardea", "control.sock"))
f = s.makefile("rw")
n = 0
def call(method, params):
    global n
    n += 1
    f.write(json.dumps({"id": n, "method": method, "params": params}) + "\n")
    f.flush()
    while True:
        m = json.loads(f.readline())
        if m.get("id") == n:
            return m
print(json.dumps(call("hello", {"version": 3})))
if sys.argv[1] == "call":
    print(json.dumps(call(sys.argv[2], json.loads(sys.argv[3]))))
else:
    call("subscribe", {})
    end = time.time() + float(sys.argv[2])
    s.settimeout(1)
    while time.time() < end:
        try:
            line = f.readline()
        except (TimeoutError, socket.timeout):
            continue
        if not line:
            break
        print(line.strip())
EOF
ctl() { run 60 python3 "$CTL_PY" "$@"; }

state_of() { "$CARDEA" status "$1" 2>/dev/null | head -n 1 | sed 's/.* — //'; }
exists() { "$CARDEA" status "$1" >/dev/null 2>&1; }

# wait_state ID PATTERN SECONDS
wait_state() {
    local i=0
    while [ $i -lt "$3" ]; do
        case $(state_of "$1") in $2) return 0 ;; esac
        sleep 1
        i=$((i + 1))
    done
    return 1
}

# Changes a sandbox's configuration through `cardea edit`, with python code acting on the dict `c`.
EDIT_PY=$WORK/edit.py
cat >"$EDIT_PY" <<'EOF'
import json, os, sys
p = sys.argv[1]
c = json.load(open(p))
exec(os.environ["SELFTEST_EDIT"])
open(p, "w").write(c if isinstance(c, str) else json.dumps(c, indent=2))
EOF
edit_config() {
    local id=$1 code=$2
    N=$((N + 1))
    OUT=$RAW/$N.txt
    printf '$ cardea edit %s  # %s\n' "$id" "$code" >"$OUT"
    SELFTEST_EDIT=$code VISUAL="python3 $EDIT_PY" timeout 60 "$CARDEA" edit "$id" </dev/null >>"$OUT" 2>&1
    RC=$?
}

# probe ID HOST PORT : a TCP connection from inside; sets PROBE (connected|refused|timeout) and MS.
probe() {
    local s e
    s=$(date +%s%N)
    run 30 "$CARDEA" exec "$1" -- bash -c 'timeout 6 bash -c "exec 3<>/dev/tcp/$0/$1" 2>&1; echo "rc=$?"' "$2" "$3"
    e=$(date +%s%N)
    MS=$(((e - s) / 1000000))
    if grep -q '^rc=0' "$OUT"; then
        PROBE=connected
    elif grep -q '^rc=124' "$OUT"; then
        PROBE=timeout
    else
        PROBE=refused
    fi
    echo "→ $PROBE in ${MS} ms (including exec)" >>"$OUT"
}
fast_refused() { [ "$PROBE" = refused ] && [ "$MS" -lt 4000 ]; }

logs_of() { "$CARDEA" logs "$1" 2>&1 | tail -n 200; }

# Restarts a sandbox, typing the keyring password when asked.
restart() { run 600 "$CARDEA" restart "$1"; wait_state "$1" 'running*' 10; }

cleanup() {
    [ -n "$LISTENER" ] && kill "$LISTENER" 2>/dev/null
    podman rm -f selftest-home-user >/dev/null 2>&1
    if [ $KEEP -eq 0 ]; then
        for id in $IDS; do
            if exists "$id"; then
                "$CARDEA" stop "$id" >/dev/null 2>&1
                "$CARDEA" delete "$id" -y >/dev/null 2>&1
            fi
        done
        for p in $PROFILES; do "$CARDEA" profile delete "$p" >/dev/null 2>&1; done
        rm -rf "$WORK"
    fi
}

# write_report [final]: writes the report so far. Called after each check, so a run that crashes or is stopped
# still leaves a report to hand back; `final` (at exit) adds the sandbox logs and the daemon journal.
write_report() {
    local final=${1:-}
    if [ -n "$final" ]; then
        for id in a b; do
            case $id in a) sb=$A ;; b) sb=$B ;; esac
            exists "$sb" && timeout 30 "$CARDEA" logs "$sb" 2>&1 | tail -n 200 >"$WORK/log-$id.txt"
        done
    fi
    {
        echo "# Cardea self-test report"
        echo
        if [ "$DONE" -eq 1 ]; then
            echo "**$PASS passed, $FAIL failed, $SKIP skipped.** Generated by selftest.sh on $(date -Is)."
        else
            echo "**Incomplete: the run stopped during step \"$STEP\".** $PASS passed, $FAIL failed, $SKIP skipped so far ($(date -Is))."
        fi
        echo
        echo "| Result | Step | Check |"
        echo "|---|---|---|"
        grep '^| FAIL' "$SUMMARY"
        grep -v '^| FAIL' "$SUMMARY"
        echo
        echo "# Environment"
        echo
        echo '```'
        cat "$WORK/env.txt" 2>/dev/null
        echo '```'
        cat "$BODY"
        [ -n "$final" ] || return 0
        for f in a b c; do
            if [ -s "$WORK/log-$f.txt" ]; then
                echo
                echo "# Log of the test sandbox $f (last 200 lines)"
                echo
                echo '```'
                cat "$WORK/log-$f.txt"
                echo '```'
            fi
        done
        echo
        echo "# Daemon journal (last 150 lines)"
        echo
        echo '```'
        journalctl --user -u cardea -n 150 --no-pager 2>&1
        echo '```'
    } | sed -e "s|$HOME|~|g" >"$REPORT"
}

# At exit, normal or not: the full report, then where it is.
finish() {
    write_report final
    say ""
    [ "$DONE" -eq 1 ] || say "The run stopped during step \"$STEP\"; the report holds every check made until then."
    say "Report: $REPORT ($PASS passed, $FAIL failed, $SKIP skipped). Your home path is shown as ~; local IP addresses"
    say "are kept (they matter for the firewall checks)."
    [ $KEEP -eq 1 ] && say "Kept: the selftest-* sandboxes (delete them with: cardea delete <id> -y)"
    return 0
}
trap finish EXIT
# Without these, a signal ends bash without the EXIT trap, so without the final report.
trap 'exit 130' INT TERM HUP

# --- 0. Installation and environment -----------------------------------------------------------------------

step "0. Installation and environment"
{
    echo "date: $(date -Is)"
    echo "selftest options: quick=$QUICK claude=$CLAUDE keep=$KEEP"
    grep -E '^(PRETTY_NAME|VERSION_ID)=' /etc/os-release
    echo "kernel: $(uname -r)"
    echo "session: XDG_SESSION_TYPE=${XDG_SESSION_TYPE:-} XDG_CURRENT_DESKTOP=${XDG_CURRENT_DESKTOP:-} WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-} DISPLAY=${DISPLAY:-}"
    echo "cardea: $("$CARDEA" --version 2>&1)  package: $(dpkg-query -W -f '${Version}' cardea 2>/dev/null || rpm -q --qf '%{VERSION}-%{RELEASE}' cardea 2>/dev/null || echo none)  path: $(command -v "$CARDEA")"
    echo "podman: $(podman --version 2>&1)"
    echo "units: podman.socket=$(systemctl --user is-active podman.socket 2>&1) cardea=$(systemctl --user is-active cardea 2>&1)"
    echo "devices: $(ls -d /dev/dri /dev/kvm /dev/vhost-vsock 2>/dev/null | tr '\n' ' ')"
    echo "subuid: $(grep "^$(id -un):" /etc/subuid 2>/dev/null | cut -d: -f2-) subgid: $(grep "^$(id -un):" /etc/subgid 2>/dev/null | cut -d: -f2-)"
    echo "ipv4: $(ip -4 route get 1.1.1.1 2>/dev/null | head -n 1)"
    echo "ipv6 default route: $(ip -6 route show default 2>/dev/null | head -n 1)"
    podman info --format 'rootless={{.Host.Security.Rootless}} network={{.Host.NetworkBackend}} cgroup={{.Host.CgroupsVersion}} runtime={{.Host.OCIRuntime.Name}}' 2>&1 | head -n 3
} >"$WORK/env.txt" 2>&1
ok "environment collected" "$WORK/env.txt"

run 60 "$CARDEA" doctor
expect $RC "cardea doctor passes"
UNIT=$(systemctl --user is-active cardea 2>&1)
if [ "$UNIT" = active ]; then ok "the cardea user unit runs the daemon"; else ko "the cardea unit is $UNIT: log out and back in once after installing the package"; fi
OUTSIDE=""
for p in $(pgrep -u "$(id -u)" -f '(^|/)cardea daemon$'); do
    grep -q '/cardea\.service$' "/proc/$p/cgroup" 2>/dev/null || OUTSIDE="$OUTSIDE $p"
done
OUT="restarts: $(systemctl --user show -p NRestarts --value cardea 2>&1); daemons outside the unit:${OUTSIDE:- none}"
if [ -z "$OUTSIDE" ]; then ok "only the unit's daemon runs (no restart loop)"; else ko "a daemon runs outside the unit (PID$OUTSIDE): stop it, then: systemctl --user restart cardea"; fi
if [ "$(systemctl --user is-enabled podman.socket 2>&1)" = enabled ]; then ok "podman.socket enabled"; else ko "podman.socket not enabled"; fi
for f in /usr/share/applications/cardea.desktop /usr/share/icons/hicolor/scalable/apps/cardea.svg /usr/lib/systemd/user/cardea.service; do
    if [ -f "$f" ]; then ok "installed: $f"; else ko "missing: $f"; fi
done
if command -v desktop-file-validate >/dev/null && [ -f /usr/share/applications/cardea.desktop ]; then
    run 10 desktop-file-validate /usr/share/applications/cardea.desktop
    expect $RC "the menu entry is a valid .desktop file"
fi

for id in $IDS; do
    if exists "$id"; then
        say "A test sandbox $id already exists: delete it first (cardea delete $id -y)."
        ko "a previous test sandbox $id still exists; stopping"
        exit 1
    fi
done
trap 'finish; cleanup' EXIT

HOST_IP=$(ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p')
GATEWAY=$(ip -4 route show default 2>/dev/null | awk '{print $3; exit}')
PORT=$((18000 + $$ % 1000))
UIDN=$(id -u)

# --- 1. Files and profiles (no sandbox) --------------------------------------------------------------------

step "1. Configuration files and profiles"
cat >"$WORK/good.json" <<'EOF'
{ "schema": 1, "id": "good", "name": "Good", "base_image": "docker.io/library/debian:trixie-slim" }
EOF
cat >"$WORK/bad.json" <<'EOF'
{ "schema": 1, "id": "Bad Id", "name": "Bad", "base_image": "debian", "folders": [ { "host": "/tmp/../etc", "mount": "/usr/x", "mode": "rw" } ], "oops": 1 }
EOF
run 10 "$CARDEA" check sandbox "$WORK/good.json"
expect $RC "a valid sandbox.json passes cardea check"
run 10 "$CARDEA" check sandbox "$WORK/bad.json"
if [ $RC -ne 0 ] && has 'oops'; then ok "a broken sandbox.json is refused with the field named" ; else ko "the broken file was not refused clearly"; fi
cat >"$WORK/bad-full.json" <<'EOF'
{ "schema": 1, "id": "Bad Id", "name": "Bad", "base_image": "debian", "folders": [ { "host": "/tmp/../etc", "mount": "/usr/x", "mode": "rw" } ] }
EOF
run 10 "$CARDEA" check sandbox "$WORK/bad-full.json"
if [ $RC -ne 0 ] && has 'folders\[0\].mount' && has 'base_image' && has '\bid\b\|^  - id'; then ok "every problem listed at once (id, image, folder path and mount)"; else ko "not every problem listed"; fi
cat >"$WORK/st-home.json" <<'EOF'
{ "id": "st", "name": "Selftest home source", "scope": "home",
  "setup": "mkdir -p \"$HOME/.st\"",
  "install": "echo \"$1 ${2:-1.0}\" >> \"$HOME/.st/list\"",
  "remove": "grep -v \"^$1 \" \"$HOME/.st/list\" > \"$HOME/.st/new\"; mv \"$HOME/.st/new\" \"$HOME/.st/list\"",
  "update": "true",
  "list": "cat \"$HOME/.st/list\" 2>/dev/null; true" }
EOF
run 10 "$CARDEA" check source "$WORK/st-home.json"
expect $RC "a custom source file passes cardea check"
run 10 "$CARDEA" profile list
if has empty; then ok "built-in profiles listed" ; else ko "built-in profiles missing"; fi
run 10 "$CARDEA" profile show empty
if has 'base_image'; then ok "profile show empty"; else ko "profile show empty"; fi
run 10 "$CARDEA" profile export empty "$WORK/empty-profile.json"
expect $RC "profile export"
python3 -c "import json,sys; p=json.load(open(sys.argv[1])); p['id']='selftest-import'; p['name']='Selftest import'; json.dump(p, open(sys.argv[2],'w'))" "$WORK/empty-profile.json" "$WORK/import.json"
run 30 "$CARDEA" profile import "$WORK/import.json" -y
run 10 "$CARDEA" profile list
if has selftest-import; then ok "profile import under a new id"; else ko "imported profile not listed" ; fi
python3 -c "import json,sys; p=json.load(open(sys.argv[1])); p['folders']=[{'host':'/tmp','mount':'/work/t','mode':'rw'}]; json.dump(p, open(sys.argv[2],'w'))" "$WORK/import.json" "$WORK/import-bad.json"
run 30 "$CARDEA" profile import "$WORK/import-bad.json" --replace -y
if [ $RC -ne 0 ] && has folders; then ok "a profile with a host folder is refused (R10)"; else ko "a profile with a host folder was accepted"; fi
run 10 "$CARDEA" profile delete selftest-import
expect $RC "profile delete"
run 10 "$CARDEA" profile delete empty
if [ $RC -ne 0 ]; then ok "a built-in profile cannot be deleted"; else ko "a built-in profile was deleted"; fi
ctl call settings '{}'
SETTINGS=$(tail -n 1 "$OUT" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("result", {})))')
if [ "$SETTINGS" != "{}" ]; then ok "settings read" "$OUT"; else ko "settings not readable" "$OUT"; fi
NEW=$(python3 -c 'import json,sys; s=json.loads(sys.argv[1]); s["terminal"]=["xterm","-e"]; print(json.dumps({"settings": s}))' "$SETTINGS")
ctl call set_settings "$NEW"
ctl call settings '{}'
if has '"terminal": \["xterm", "-e"\]'; then ok "settings saved and read back"; else ko "settings change not kept"; fi
BAD=$(python3 -c 'import json,sys; s=json.loads(sys.argv[1]); s["sandbox_location"]="relative/path"; print(json.dumps({"settings": s}))' "$SETTINGS")
ctl call set_settings "$BAD"
if has '"error"'; then ok "invalid settings refused"; else ko "invalid settings accepted"; fi
ctl call set_settings "{\"settings\": $SETTINGS}"
run 30 "$CARDEA" tray-autostart off
AUTO=${XDG_CONFIG_HOME:-$HOME/.config}/autostart/cardea-tray.desktop
if [ ! -f "$AUTO" ]; then ok "tray-autostart off removes the entry"; else ko "tray-autostart off left $AUTO"; fi
run 30 "$CARDEA" tray-autostart on
if [ -f "$AUTO" ]; then ok "tray-autostart on writes the entry"; else ko "tray-autostart on wrote nothing"; fi

# --- 2. Build and start ------------------------------------------------------------------------------------

step "2. Build and start (Empty profile)"
s=$(date +%s)
run 1800 "$CARDEA" create --profile empty --name "$A"
if [ $RC -eq 0 ] && wait_state $A stopped 5; then
    ok "build of $A in $(($(date +%s) - s)) s"
else
    ko "build of $A failed (exit $RC)"
    A_FAILED=1
fi
if [ -z "${A_FAILED:-}" ]; then
    s=$(date +%s)
    run 600 "$CARDEA" start $A
    if [ $RC -eq 0 ] && wait_state $A running 5; then
        ok "start of $A in $(($(date +%s) - s)) s"
    else
        ko "start of $A failed (exit $RC): $(state_of $A)"
        A_FAILED=1
    fi
fi
run 10 "$CARDEA" list
ok "cardea list" "$OUT"

if [ -n "${A_FAILED:-}" ]; then
    step "3-12. Checks on the Work sandbox"
    skip "skipped: $A does not run (see its log at the end)"
else

    # --- 3. Identity and isolation ---------------------------------------------------------------------------

    step "3. Identity and isolation"
    in_box $A id -un
    if grep -qx sandbox "$OUT"; then ok "user is sandbox"; else ko "user is not sandbox"; fi
    in_box $A id -u
    if grep -qx "$UIDN" "$OUT"; then ok "UID is the host user's ($UIDN)"; else ko "UID differs from $UIDN"; fi
    in_box $A cat /proc/self/uid_map
    MAPPED=$(tail -n +2 "$OUT" | awk '{ s += $3 } END { print s }')
    if [ "$MAPPED" = $(( (UIDN < 1000 ? UIDN : 1000) + 2 )) ] && has '^ *65534 '; then ok "a Work sandbox maps only the IDs it uses: system users, you, nobody ($MAPPED)"; else ko "Work ID mapping unexpected ($MAPPED IDs)"; fi
    in_box $A hostname
    if grep -qx $A "$OUT"; then ok "hostname is $A"; else ko "hostname is not $A"; fi
    in_box $A sh -c 'exit 3'
    if [ $RC -eq 3 ]; then ok "exec returns the command's exit code (3)"; else ko "exec returned $RC instead of 3"; fi
    in_box $A sh -c 'echo to-stdout; echo to-stderr >&2'
    if has to-stdout && has to-stderr; then ok "stdout and stderr come back"; else ko "output lost"; fi
    run 60 "$CARDEA" exec -w /tmp $A -- pwd
    if grep -qx /tmp "$OUT"; then ok "exec -w sets the working folder"; else ko "exec -w ignored"; fi
    run_tty 30 "" "$CARDEA" exec $A -- stty size
    if grep -qE '^[0-9]+ [0-9]+' "$OUT"; then ok "exec with a terminal gets a TTY with a size" ; else ko "no TTY in exec"; fi
    in_box $A sh -c 'grep -E "^(CapEff|NoNewPrivs|Seccomp):" /proc/self/status'
    if has 'CapEff:.*0000000000000000' && has 'NoNewPrivs:.*1' && has 'Seccomp:.*2'; then
        ok "no capability, no-new-privileges, seccomp filter on"
    else
        ko "capabilities / no-new-privileges / seccomp not as expected"
    fi
    run 60 "$CARDEA" exec --root $A -- id -u
    if grep -qx 0 "$OUT"; then ok "exec --root runs as root"; else ko "exec --root is not root"; fi
    run 300 "$CARDEA" exec --root $A -- apt-get update
    expect $RC "as root: apt-get update works (network, DNS, root capabilities)"
    in_box $A sh -c 'for p in /run /tmp /var/run /home; do find "$p" -xdev \( -name podman.sock -o -name control.sock \) 2>/dev/null; done; ls -la /tmp/xdg-runtime; echo "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-}"; stat -c "%U %a" /tmp/xdg-runtime'
    if has 'podman.sock' || has 'control.sock'; then ko "a control socket is visible in the sandbox (R1/R2)"; else ok "no podman.sock or control.sock visible"; fi
    if has 'sandbox 700'; then ok "/tmp/xdg-runtime belongs to the user, mode 700"; else ko "/tmp/xdg-runtime owner or mode wrong"; fi
    if [ "${XDG_SESSION_TYPE:-}" = wayland ] || [ -n "${WAYLAND_DISPLAY:-}" ]; then
        if has 'wayland-0'; then ok "Wayland socket mounted as /tmp/xdg-runtime/wayland-0"; else ko "no Wayland socket in the sandbox"; fi
    else
        skip "Wayland socket: not a Wayland session"
    fi
    in_box $A cat /etc/machine-id
    if grep -qE '^[0-9a-f]{32}$' "$OUT"; then ok "own machine-id"; else ko "machine-id missing or malformed"; fi
    in_box $A sh -c 'date +%Z; df -k /dev/shm | tail -n 1'
    if has "^$(date +%Z)\$"; then ok "timezone follows the host ($(date +%Z))"; else ko "timezone differs from the host"; fi
    if has ' 1048576 '; then ok "/dev/shm is 1 GiB"; else ko "/dev/shm is not 1 GiB"; fi

    run 20 python3 "$CTL_PY" events 6
    if grep '"type":"stats"' "$OUT" | grep -q "\"$A\""; then ok "live CPU/RAM stats of the sandbox stream to the clients"; else ko "no stats event for $A in 6 s"; fi
    ctl call net "{\"id\": \"$A\"}"
    if has '"references"'; then ok "live firewall state readable by the clients"; else ko "no firewall state"; fi
    ctl call disk "{\"id\": \"$A\"}"
    if grep -q '"result"' "$OUT" && ! grep -q '"error"' "$OUT"; then ok "disk use of the sandbox" ; else ko "disk method failed"; fi
    run 5 "$CARDEA" logs -f $A
    if has 'Start finished'; then ok "logs -f follows the log"; else ko "logs -f showed nothing"; fi

    # --- 4. Home ---------------------------------------------------------------------------------------------

    step "4. Home"
    in_box $A sh -c 'echo kept > ~/selftest-file'
    restart $A
    in_box $A cat /home/sandbox/selftest-file
    if grep -qx kept "$OUT"; then ok "persistent home keeps a file across a restart"; else ko "the file was lost across a restart"; fi
    FOLDER=$("$CARDEA" status $A | sed -n 's/^  Folder *//p')
    FOLDER=${FOLDER/#\~/$HOME}
    if [ -f "$FOLDER/home/selftest-file" ] && [ "$(stat -c %u "$FOLDER/home/selftest-file")" = "$UIDN" ]; then
        ok "the file is on the host in the sandbox folder, owned by you"
    else
        ko "the file is not in $FOLDER/home on the host, or not yours"
    fi
    if [ -s "$FOLDER/state/machine-id" ]; then ok "state/machine-id kept on the host"; else ko "no state/machine-id"; fi

    # --- 5. Shared folders -----------------------------------------------------------------------------------

    step "5. Shared folders and protected paths"
    SHARE=$WORK/share
    mkdir -p "$SHARE/.git/hooks" "$SHARE/sub/.vscode" "$SHARE/node_modules/x/.vscode" "$SHARE/real"
    echo x >"$SHARE/.envrc"
    ln -s ../real "$SHARE/sub/.devcontainer"
    RO=$WORK/share-ro
    mkdir -p "$RO"
    echo ro >"$RO/readme"
    MISSING=$WORK/missing-folder
    run 30 "$CARDEA" folders $A add "$SHARE" --mount /work/share --rw
    expect $RC "add a read-write folder"
    run 30 "$CARDEA" folders $A add "$RO" --mount /work/ro --ro
    run 30 "$CARDEA" folders $A add "$MISSING" --mount /work/missing --rw
    run 30 "$CARDEA" folders $A add "$SHARE" --mount /usr/share/x
    if [ $RC -ne 0 ] && has '/usr'; then ok "a mount point in /usr is refused (R15)"; else ko "a mount point in /usr was accepted"; fi
    run 30 "$CARDEA" folders $A list
    if has MISSING; then ok "a missing folder is listed as MISSING"; else ko "the missing folder is not flagged"; fi
    run 10 "$CARDEA" list
    if grep "^$A " "$OUT" | grep -q 'restart'; then ok "a running sandbox shows restart required after a folder change"; else ko "no restart required flag"; fi
    restart $A
    in_box $A sh -c 'echo w > /work/share/new && echo "write ok"; for p in .git/hooks/x .envrc sub/.vscode/x real/x node_modules/x/.vscode/x; do (echo w > /work/share/$p) 2>&1 | sed "s|^|$p: |" | head -n 1; done; (echo w > /work/ro/x) 2>&1 | sed "s|^|ro: |"; cat /work/ro/readme'
    if has 'write ok' && [ -f "$SHARE/new" ]; then ok "read-write folder: a write reaches the host"; else ko "write to the shared folder failed"; fi
    if has '^.git/hooks/x:.*ead-only' && has '^.envrc:.*ead-only' && has '^sub/.vscode/x:.*ead-only'; then ok ".git/hooks, .envrc, sub/.vscode read-only (R12)"; else ko "a protected path is writable"; fi
    if has '^real/x:.*ead-only'; then ok "a protected path that is a link protects its target inside the folder"; else ko "the target of a linked protected path is writable"; fi
    if [ -f "$SHARE/node_modules/x/.vscode/x" ]; then ok "node_modules is not searched (writable inside)"; else ko "node_modules was searched or not writable"; fi
    if has '^ro:.*ead-only' && has '^ro$'; then ok "read-only folder: readable, not writable"; else ko "read-only folder wrong"; fi
    logs_of $A >"$WORK/log-a.txt"
    if grep -q 'missing' "$WORK/log-a.txt"; then ok "the start logged the missing folder"; else ko "no log line for the missing folder"; fi
    run 30 "$CARDEA" folders $A remove /work/missing
    expect $RC "remove a folder"

    # --- 6. Configuration changes ----------------------------------------------------------------------------

    step "6. Configuration changes"
    edit_config $A 'c = "{ not json"'
    if [ $RC -ne 0 ]; then ok "an invalid edit is refused"; else ko "an invalid edit was accepted"; fi
    edit_config $A "c['identity']['hostname'] = 'st-host'; c['security']['ram_limit_gb'] = 1"
    expect $RC "edit: hostname and RAM limit"
    run 10 "$CARDEA" list
    if grep "^$A " "$OUT" | grep -q restart; then ok "restart required flagged"; else ko "no restart required flag"; fi
    restart $A
    in_box $A sh -c 'hostname; cat /sys/fs/cgroup/memory.max'
    if has '^st-host$'; then ok "new hostname after restart"; else ko "hostname not changed"; fi
    if has '^1073741824$'; then ok "RAM limit 1 GiB applied"; else ko "RAM limit not applied"; fi
    KVM=False
    [ -e /dev/kvm ] && KVM=True
    edit_config $A "c['identity']['hostname'] = None; c['security']['ram_limit_gb'] = None; c['security']['seccomp'] = False; c['capabilities']['kvm'] = $KVM"
    restart $A
    in_box $A sh -c 'grep Seccomp: /proc/self/status; ls -la /dev/kvm 2>&1'
    if has 'Seccomp:.*0'; then ok "seccomp off applied"; else ko "seccomp still on"; fi
    if [ $KVM = True ]; then if has 'crw'; then ok "/dev/kvm visible with KVM on"; else ko "/dev/kvm missing with KVM on"; fi; else skip "no /dev/kvm on this computer"; fi
    run 10 "$CARDEA" status $A
    if hasi 'seccomp off'; then ok "the risky setting is a warning in cardea status"; else ko "no warning for seccomp off"; fi
    edit_config $A "c['security']['seccomp'] = True; c['capabilities']['kvm'] = False"
    restart $A

    # --- 7. Firewall -----------------------------------------------------------------------------------------

    step "7. Firewall"
    run 30 "$CARDEA" net $A
    if has 'gateway → [0-9]'; then ok "references resolved"; else ko "references not resolved"; fi
    probe $A example.com 443
    if [ $PROBE = connected ]; then ok "Internet reachable (DNS + example.com:443)"; else ko "Internet not reachable: $PROBE"; fi
    if ip -6 route show default 2>/dev/null | grep -q .; then
        probe $A ipv6.google.com 443
        if [ $PROBE = connected ]; then ok "Internet reachable over IPv6"; else ko "IPv6 Internet not reachable: $PROBE"; fi
        probe $A fd00::1 80
        if fast_refused; then ok "IPv6 private address refused at once"; else ko "IPv6 private address not refused: $PROBE"; fi
    else
        skip "no IPv6 default route on this computer"
    fi
    probe $A 10.255.255.1 80
    if fast_refused; then ok "LAN address refused at once (reject, ${MS} ms)"; else ko "LAN address not refused at once: $PROBE in ${MS} ms"; fi
    # Neutral addressing (pasta): nothing inside names this computer's network.
    in_box $A sh -c 'cat /proc/net/fib_trie /proc/net/if_inet6 /proc/net/route /etc/resolv.conf /proc/net/dev 2>&1; echo "sysfs:"; ls /sys/class/net'
    cp "$OUT" "$WORK/netns-a.txt"
    LEAK=
    for a in $(ip -o addr show scope global 2>/dev/null | awk '{sub(/\/.*/, "", $4); print $4}') $GATEWAY; do
        # /proc/net/if_inet6 prints IPv6 addresses as 32 hex digits.
        n=$(python3 -c 'import ipaddress,sys; a=ipaddress.ip_address(sys.argv[1]); print(a.exploded.replace(":","") if a.version==6 else a)' "$a")
        if tail -n +2 "$WORK/netns-a.txt" | grep -qiFw -- "$n"; then LEAK="$LEAK $a"; fi
    done
    if [ -z "$LEAK" ]; then ok "no address of this computer or its gateway inside the sandbox" "$WORK/netns-a.txt"; else ko "this computer's addresses visible inside:$LEAK" "$WORK/netns-a.txt"; fi
    if has '10.249.0.2' && has '^ *eth0:'; then ok "neutral address 10.249.0.2 on eth0"; else ko "no neutral address / interface name" "$WORK/netns-a.txt"; fi
    HOST_IFACES=$(ls /sys/class/net | grep -vx lo)
    SEEN=$(sed -n '/^sysfs:$/,$p' "$WORK/netns-a.txt" | grep -xF "$HOST_IFACES" | tr '\n' ' ')
    if [ -z "$SEEN" ]; then ok "this computer's network interfaces are hidden in /sys"; else ko "this computer's interfaces visible in /sys: $SEEN" "$WORK/netns-a.txt"; fi
    if has '^search\|^domain'; then ko "a DNS search domain is set inside" "$WORK/netns-a.txt"; else ok "no DNS search domain inside"; fi
    lan_blocked() { run 30 "$CARDEA" net $A; LAN_BLOCKED=$(grep '^LAN' "$OUT" | sed -n 's/.* \([0-9]*\) blocked.*/\1/p'); }
    if [ -n "$GATEWAY" ]; then
        probe $A "$GATEWAY" 80
        sleep 6
        run 30 "$CARDEA" net $A blocked
        if grep -q "$GATEWAY\$" "$OUT"; then ok "the gateway ($GATEWAY) is blocked by default"; else ko "the gateway ($GATEWAY) was not blocked (probe: $PROBE)"; fi
        # A LAN machine (here the gateway, standing in for a homelab) is reachable once in the allowlist.
        run 30 "$CARDEA" net $A allow lan "$GATEWAY" --comment selftest
        sleep 2
        lan_blocked
        BEFORE=$LAN_BLOCKED
        probe $A "$GATEWAY" 80
        sleep 6
        lan_blocked
        # The connection decides: other LAN traffic (multicast, IPv6 router solicitations) moves the counter.
        if [ "$PROBE" = connected ]; then ok "a LAN address in the allowlist gets through (blocked LAN packets: $BEFORE → $LAN_BLOCKED)"; else ko "LAN allowlist entry still blocked (probe: $PROBE)"; fi
        run 30 "$CARDEA" net $A remove lan "$GATEWAY"
    else
        skip "no default gateway found"
    fi
    if command -v python3 >/dev/null && [ -n "$HOST_IP" ]; then
        python3 -m http.server "$PORT" --bind 0.0.0.0 --directory "$WORK" >/dev/null 2>&1 &
        LISTENER=$!
        sleep 1
        probe $A "$HOST_IP" "$PORT"
        if [ $PROBE != connected ]; then ok "a service of this computer ($HOST_IP:$PORT) is blocked ($PROBE)"; else ko "a service of this computer is reachable with the default firewall"; fi
        probe $A host.containers.internal "$PORT"
        if [ $PROBE != connected ]; then ok "host.containers.internal:$PORT is blocked ($PROBE)"; else ko "host.containers.internal:$PORT is reachable with the default firewall"; fi
        # With pasta the sandbox has this computer's address too: the host is reached as host.containers.internal.
        run 30 "$CARDEA" net $A allow host host --port "$PORT" --proto tcp --comment selftest
        sleep 2
        probe $A host.containers.internal "$PORT"
        if [ $PROBE = connected ]; then ok "'allow host host --port' applies at once, without restart"; else ko "still blocked after the allow entry: $PROBE"; fi
        run 30 "$CARDEA" net $A remove host host
        sleep 2
        probe $A host.containers.internal "$PORT"
        if [ $PROBE != connected ]; then ok "removing the entry blocks again"; else ko "still reachable after removing the entry"; fi
        run 30 "$CARDEA" net $A off -y
        sleep 2
        probe $A host.containers.internal "$PORT"
        if [ $PROBE = connected ]; then ok "firewall off: this computer is reachable"; else ko "firewall off but still blocked: $PROBE"; fi
        # Even with the firewall off, the sandbox's gateway address never leads to this computer (no gateway mapping).
        probe $A 10.249.0.1 "$PORT"
        if [ $PROBE != connected ]; then ok "the sandbox's gateway address does not reach this computer ($PROBE)"; else ko "the gateway address reaches this computer's ports"; fi
        run 30 "$CARDEA" list
        if grep "^$A " "$OUT" | grep -q 'warning'; then ok "firewall off shows a warning in cardea list"; else ko "no warning in cardea list with the firewall off"; fi
        run 30 "$CARDEA" net $A on
    else
        skip "host service probes: python3 or the host address missing"
    fi
    run 30 "$CARDEA" net $A off
    if [ $RC -ne 0 ]; then ok "net off without a terminal needs -y"; else ko "net off went through without confirmation"; fi
    run 30 "$CARDEA" net $A mode lan block -y
    sleep 2
    probe $A 10.255.255.1 80
    if ! fast_refused; then ok "LAN blocklist: an unlisted LAN address is let out ($PROBE)"; else ko "LAN blocklist still refuses unlisted addresses"; fi
    run 30 "$CARDEA" net $A block lan 10.255.255.0/24
    sleep 2
    probe $A 10.255.255.1 80
    if fast_refused; then ok "LAN blocklist: a listed range is refused"; else ko "listed range not refused: $PROBE"; fi
    run 30 "$CARDEA" net $A mode lan allow
    run 30 "$CARDEA" net $A remove lan 10.255.255.0/24
    run 30 "$CARDEA" net $A mode wan allow
    sleep 2
    probe $A example.com 443
    if fast_refused; then ok "WAN allowlist (empty): the Internet is refused"; else ko "WAN allowlist did not block: $PROBE"; fi
    run 30 "$CARDEA" net $A mode wan block
    run 30 "$CARDEA" net $A drop
    sleep 2
    probe $A 10.255.255.1 80
    if [ $PROBE = timeout ]; then ok "drop: the LAN connection times out (${MS} ms)"; else ko "drop: expected a timeout, got $PROBE in ${MS} ms"; fi
    sleep 6
    run 30 "$CARDEA" net $A blocked
    if has '10.255.255.1'; then ok "blocked destination listed" ; else ko "10.255.255.1 not in the blocked list"; fi
    run 30 "$CARDEA" net $A
    if grep -q 'LAN.* [1-9][0-9]* blocked' "$OUT"; then ok "blocked connections counted per zone"; else ko "no blocked count for the LAN"; fi
    logs_of $A >"$WORK/log-a.txt"
    if grep -q 'Firewall blocked a connection to 10.255.255.1' "$WORK/log-a.txt"; then ok "a new blocked destination adds a log line"; else ko "no log line for a blocked destination"; fi
    run 30 "$CARDEA" net $A reject
    podman exec "cardea-$A-netguard" nft list ruleset >"$WORK/nft-a.txt" 2>&1
    ok "ruleset in the sidecar (for reference)" "$WORK/nft-a.txt"
    podman inspect "cardea-$A-app" --format 'app: {{json .HostConfig.CapAdd}} {{json .HostConfig.CapDrop}}' >"$WORK/caps-a.txt" 2>&1
    podman inspect "cardea-$A-netguard" --format 'netguard: {{json .HostConfig.CapAdd}}' >>"$WORK/caps-a.txt" 2>&1
    if grep '^app:' "$WORK/caps-a.txt" | grep -q NET_ADMIN; then ko "the app container has NET_ADMIN" "$WORK/caps-a.txt"; else ok "only the sidecar has NET_ADMIN" "$WORK/caps-a.txt"; fi
    in_box $A sh -c 'nft list ruleset 2>&1; ip link add x type dummy 2>&1'
    ok "the app cannot touch the network configuration (for reference)"

    # --- 8. Failures -----------------------------------------------------------------------------------------

    step "8. Failures"
    podman kill "cardea-$A-netguard" >/dev/null 2>&1
    if wait_state $A 'error*' 20; then ok "sidecar killed → sandbox in Error: $(state_of $A)"; else ko "sidecar killed but the state is $(state_of $A)"; fi
    run 120 "$CARDEA" stop $A
    if wait_state $A "stopped*" 10; then ok "stop from Error resets to Stopped"; else ko "stop from Error left $(state_of $A)"; fi
    run 600 "$CARDEA" start $A
    podman stop -t 1 "cardea-$A-app" >/dev/null 2>&1
    if wait_state $A 'error*' 20; then ok "app container stopped → sandbox in Error: $(state_of $A)"; else ko "app container stopped but the state is $(state_of $A)"; fi
    run 120 "$CARDEA" stop $A
    run 600 "$CARDEA" start $A
    if wait_state $A "running*" 5; then ok "start after an error works"; else ko "start after an error failed: $(state_of $A)"; fi
    run 30 "$CARDEA" start $A
    if [ $RC -ne 0 ]; then ok "start of a running sandbox is refused" ; else ko "start of a running sandbox accepted"; fi
    podman run --rm -d --name selftest-home-user -v "$FOLDER/home:/x:ro" docker.io/library/debian:trixie-slim sleep 300 >/dev/null 2>&1
    run 300 "$CARDEA" restart $A
    if has 'already used' || logs_of $A | grep -q 'already used'; then ok "another container on the same home blocks the start (R13)"; else ko "start not refused while another container uses the home"; fi
    podman rm -f selftest-home-user >/dev/null 2>&1
    run 120 "$CARDEA" stop $A
    run 600 "$CARDEA" start $A
    cat >"$WORK/d-home.json" <<EOF
{ "schema": 1, "id": "$D", "name": "$D", "base_image": "docker.io/library/debian:trixie-slim", "home": { "path": "$FOLDER/home" } }
EOF
    run 60 "$CARDEA" create --from-config "$WORK/d-home.json"
    if [ $RC -ne 0 ] && has 'already used\|home'; then ok "a second sandbox on the same home is refused at creation (R13)"; else ko "a second sandbox on the same home was created"; fi

    # --- 9. Daemon restart, start at login --------------------------------------------------------------------

    step "9. Daemon restart and start at login"
    if [ "$UNIT" = active ]; then
        FILTER_PID=$(cat "${XDG_RUNTIME_DIR:-/run/user/$UIDN}/cardea/wayland-$A.pid" 2>/dev/null)
        run 60 systemctl --user restart cardea
        sleep 4
        if wait_state $A running 10; then ok "after a daemon restart, the running sandbox is seen as running"; else ko "after a daemon restart, $A is $(state_of $A)"; fi
        if [ -z "${WAYLAND_DISPLAY:-}" ]; then
            skip "Wayland filter across a daemon restart: not a Wayland session"
        elif [ -n "$FILTER_PID" ] && grep -q wayland-filter "/proc/$FILTER_PID/cmdline" 2>/dev/null; then
            ok "the Wayland filter (and so the windows) survives a daemon restart"
        else
            ko "the Wayland filter stopped with the daemon (PID ${FILTER_PID:-none})"
        fi
        in_box $A true
        expect $RC "exec still works after a daemon restart"
        edit_config $A "c['autostart'] = True"
        run 120 "$CARDEA" stop $A
        run 60 systemctl --user restart cardea
        if wait_state $A running 120; then ok "a sandbox with autostart starts when the daemon starts"; else ko "autostart: $A is $(state_of $A)"; fi
        edit_config $A "c['autostart'] = False"
    else
        skip "the daemon does not run from its unit: log out and back in once, then run again"
    fi
    wait_state $A running 5 || run 600 "$CARDEA" start $A

    # --- 10. Apps and menu entries ---------------------------------------------------------------------------

    step "10. Apps and menu entries"
    edit_config $A 'c["apps"].append({"id": "st-app", "name": "Selftest", "exec": ["sh", "-c", "echo launched \"$@\" > $HOME/launched", "sh"], "toolkit": "other", "desktop_entry": True, "pinned": True})'
    expect $RC "app added through cardea edit"
    run 60 "$CARDEA" app $A
    if has st-app; then ok "cardea app lists it"; else ko "the app is not listed"; fi
    run 60 "$CARDEA" app $A st-app -- one two
    sleep 3
    in_box $A cat /home/sandbox/launched
    if grep -qx 'launched one two' "$OUT"; then ok "cardea app launches it in the background with its arguments"; else ko "the app did not run"; fi
    run 60 "$CARDEA" exec --app st-app $A
    ok "exec --app runs an app in the caller's terminal" "$OUT"
    ENTRY=${XDG_DATA_HOME:-$HOME/.local/share}/applications/cardea-$A-st-app.desktop
    if [ -f "$ENTRY" ] && grep -q "Name=Selftest ($A)" "$ENTRY" && grep -q "app $A st-app" "$ENTRY"; then
        cp "$ENTRY" "$WORK/entry.txt"
        ok "menu entry \"Selftest ($A)\" written" "$WORK/entry.txt"
    else
        ko "no menu entry $ENTRY"
    fi
    if command -v desktop-file-validate >/dev/null && [ -f "$ENTRY" ]; then
        run 10 desktop-file-validate "$ENTRY"
        expect $RC "the menu entry is a valid .desktop file"
    fi
    edit_config $A 'c["apps"] = [a for a in c["apps"] if a["id"] != "st-app"]'
    if [ ! -f "$ENTRY" ]; then ok "removing the app removes its menu entry"; else ko "the menu entry stayed after removing the app"; fi
    edit_config $A 'c["apps"].append({"id": "st-term", "name": "Term", "exec": ["sh"], "toolkit": "terminal"})'
    run 60 "$CARDEA" app $A st-term
    ok "a terminal app from a non-terminal context opens a host terminal (exit $RC; see output)" "$OUT"
    edit_config $A 'c["apps"] = [a for a in c["apps"] if a["id"] != "st-term"]'

    # --- 11. Home packages (custom source) --------------------------------------------------------------------

    step "11. Home packages"
    run 60 "$CARDEA" sources $A add "$WORK/st-home.json"
    expect $RC "custom home source added"
    run 300 "$CARDEA" packages $A add st foo --version 2.0
    expect $RC "home package added (installs at once)"
    in_box $A cat /home/sandbox/.st/list
    if has '^foo 2.0$'; then ok "the source's install template ran with name and version as arguments"; else ko "the home package was not installed"; fi
    run 60 "$CARDEA" packages $A list
    if has 'foo' && has '2.0'; then ok "packages list shows the installed version"; else ko "packages list does not show foo 2.0"; fi
    run 10 "$CARDEA" list
    if grep "^$A " "$OUT" | grep -q rebuild; then ko "a home package flagged a rebuild"; else ok "a home package needs no rebuild"; fi
    restart $A
    in_box $A sh -c 'grep -c "^foo " ~/.st/list'
    if grep -qx 1 "$OUT"; then ok "not installed again at restart"; else ko "installed again at restart"; fi
    run 300 "$CARDEA" packages $A remove st foo
    in_box $A sh -c 'grep -c "^foo " ~/.st/list'
    if grep -qx 0 "$OUT"; then ok "removed at once"; else ko "still there after remove"; fi
    run 60 "$CARDEA" packages $A add nosuchsource x
    if [ $RC -ne 0 ]; then ok "an unknown source is refused"; else ko "an unknown source was accepted"; fi

    # --- 12. Image, services, keyring, GPU, X11 ---------------------------------------------------------------

    step "12. Image, services, keyring, GPU"
    if [ $QUICK -eq 1 ]; then
        skip "skipped (--quick)"
    else
        cat >"$WORK/st-image.json" <<'EOF'
{ "id": "sti", "name": "Selftest image source", "scope": "image",
  "setup": "mkdir -p /opt/sti",
  "install": "echo \"$1\" >> /opt/sti/list",
  "remove": "true", "update": "true",
  "list": "cat /opt/sti/list 2>/dev/null; true" }
EOF
        run 60 "$CARDEA" sources $A add "$WORK/st-image.json"
        run 60 "$CARDEA" packages $A add sti bar
        run 30 "$CARDEA" packages $A add system htop
        run 30 "$CARDEA" packages $A add system wayland-utils
        run 30 "$CARDEA" service $A add st-svc -- sh -c 'echo started; sleep 4; exit 3'
        run 30 "$CARDEA" service $A add st-pf --per-folder -- sh -c 'echo "folder $0 in $PWD"; exec sleep 100000' '{folder}'
        GPU=False
        [ -e /dev/dri ] && GPU=True
        edit_config $A "c['capabilities']['keyring']['mode'] = 'auto'; c['capabilities']['gpu'] = $GPU; c['setup']['build'] = 'echo built > /opt/build-marker'; c['setup']['first_start'] = 'echo ran >> \$HOME/first-start-runs'"
        run 10 "$CARDEA" list
        if grep "^$A " "$OUT" | grep -q rebuild; then ok "image changes flag a rebuild"; else ko "no rebuild flagged" "$OUT"; fi
        run 30 "$CARDEA" service $A autostart st-pf on share
        s=$(date +%s)
        run 1800 "$CARDEA" rebuild $A
        if [ $RC -eq 0 ] && wait_state $A 'running*' 60; then ok "rebuild of a running sandbox, then restart, in $(($(date +%s) - s)) s"; else ko "rebuild failed (exit $RC): $(state_of $A)"; fi
        in_box $A sh -c 'command -v htop tmux dbus-daemon gnome-keyring-daemon; cat /opt/sti/list /opt/build-marker; cat ~/first-start-runs'
        if has /htop; then ok "system package installed" ; else ko "htop missing after rebuild"; fi
        if has '^bar$'; then ok "custom image source installed its package at build"; else ko "image source package missing"; fi
        if has '^built$'; then ok "setup.build ran in the image"; else ko "setup.build did not run"; fi
        if [ "$(grep -c '^ran$' "$OUT")" = 1 ]; then ok "setup.first_start ran once"; else ko "first start ran $(grep -c '^ran$' "$OUT") times"; fi
        restart $A
        in_box $A cat /home/sandbox/first-start-runs
        if [ "$(grep -c '^ran$' "$OUT")" = 1 ]; then ok "first start not run again at restart"; else ko "first start ran again"; fi
        in_box $A sh -c 'echo "bus=$DBUS_SESSION_BUS_ADDRESS"; for p in /proc/[0-9]*; do tr "\0" " " < $p/cmdline 2>/dev/null; echo; done | grep -E "dbus-daemon|gnome-keyring" | grep -v grep'
        if has 'gnome-keyring-daemon' && has 'bus=unix:path=/tmp/xdg-runtime/bus'; then ok "keyring auto: session bus and unlocked keyring daemon"; else ko "keyring session not running"; fi
        if [ -n "${WAYLAND_DISPLAY:-}" ]; then
            in_box $A wayland-info
            if has 'wl_compositor'; then ok "a Wayland client in the sandbox talks to your compositor" ; else ko "wayland-info could not connect"; fi
            grep -oE 'zwlr_data_control_manager_v1|ext_data_control_manager_v1|zwlr_screencopy_manager_v1|ext_image_copy_capture_manager_v1|org_kde_kwin_fake_input|zwp_virtual_keyboard_manager_v1|kde_screencast' "$OUT" | sort -u >"$WORK/protocols.txt"
            if [ -s "$WORK/protocols.txt" ]; then
                ko "the Wayland filter let background clipboard / screen capture / input protocols through" "$WORK/protocols.txt"
            else
                ok "no clipboard-manager, screen-capture or fake-input protocol offered to sandbox apps"
            fi
            if has 'wl_data_device_manager'; then ok "clipboard shared: copy and paste offered"; else ko "clipboard shared but no wl_data_device_manager"; fi
            FILTER_PID=$(cat "${XDG_RUNTIME_DIR:-/run/user/$UIDN}/cardea/wayland-$A.pid" 2>/dev/null)
            if [ -n "$FILTER_PID" ] && grep -q wayland-filter "/proc/$FILTER_PID/cmdline" 2>/dev/null; then ok "the sandbox's Wayland filter runs (PID $FILTER_PID)"; else ko "no Wayland filter process for $A"; fi
            edit_config $A "c['capabilities']['clipboard'] = 'off'"
            restart $A
            in_box $A wayland-info
            if has 'wl_compositor' && ! has 'wl_data_device_manager' && ! has 'primary_selection'; then ok "clipboard off: no clipboard protocol, the display still works"; else ko "clipboard off: clipboard still offered or display broken"; fi
            edit_config $A "c['capabilities']['clipboard'] = 'shared'"
            restart $A
        else
            skip "Wayland client: not a Wayland session"
        fi
        if [ "$GPU" = True ]; then
            in_box $A sh -c 'ls -la /dev/dri; id'
            if has 'renderD'; then ok "/dev/dri visible with GPU on" ; else ko "/dev/dri not visible"; fi
        else
            skip "GPU: no /dev/dri on this computer"
        fi
        run 30 "$CARDEA" service $A list
        if has 'st-pf \[share\].*running'; then ok "per-folder service autostarted in its folder" ; else ko "per-folder service not running"; fi
        in_box $A tmux -L cardea capture-pane -p -t svc-st-pf-share
        if has 'folder share in /work/share'; then ok "{folder} became the folder name, the folder is the working directory"; else ko "per-folder service arguments or folder wrong"; fi
        run 30 "$CARDEA" service $A start st-svc
        sleep 10
        run 30 "$CARDEA" service $A list
        if has 'st-svc.*EXITED (code 3)'; then ok "service exit detected with its code"; else ko "service exit not detected"; fi
        run 30 "$CARDEA" list
        if grep "^$A " "$OUT" | grep -q warning; then ok "the exited service is a warning on the sandbox"; else ko "no warning for the exited service"; fi
        logs_of $A >"$WORK/log-a.txt"
        if grep -q 'exited (code 3)' "$WORK/log-a.txt"; then ok "the exit is in the sandbox log"; else ko "no log line for the exit"; fi
        in_box $A tmux -L cardea capture-pane -p -S - -t svc-st-svc
        if has started; then ok "the service's last output is kept"; else ko "no output kept for the service"; fi
        run_tty 8 "" "$CARDEA" service $A attach st-pf share
        if has 'folder share'; then ok "service attach shows the instance in a terminal"; else ko "attach showed nothing"; fi
        run 30 "$CARDEA" service $A stop st-pf share
        run 30 "$CARDEA" service $A list
        if has 'st-pf \[share\].*stopped'; then ok "service stop"; else ko "service still running after stop"; fi
        run 60 "$CARDEA" app $A --found
        if has '^htop'; then ok "installed app found from its .desktop file"; else ko "htop not found among the apps" ; fi
        run 60 "$CARDEA" app $A --add htop
        sleep 8
        ICON=${XDG_CACHE_HOME:-$HOME/.cache}/cardea/icons/$A/htop.png
        if [ -f "$ICON" ] && head -c 8 "$ICON" | grep -q PNG; then ok "its icon was copied out as a PNG"; else ko "no PNG icon in $ICON"; fi
        if grep -q "Icon=" "${XDG_DATA_HOME:-$HOME/.local/share}/applications/cardea-$A-htop.desktop" 2>/dev/null; then ok "its menu entry points to the icon"; else ko "the menu entry has no icon"; fi
        edit_config $A "c['capabilities']['keyring']['mode'] = 'ask'"
        restart $A
        if [ $RC -ne 0 ] && hasi 'password'; then ok "keyring ask: a start without a terminal asks for the password"; else ko "keyring ask: start went through without a password"; fi
        run_tty 600 $'selftest-pw\n' "$CARDEA" restart $A
        if wait_state $A running 10; then ok "keyring ask: start with the password typed in the terminal"; else ko "keyring ask: start failed: $(state_of $A)"; fi
        in_box $A sh -c 'for p in /proc/[0-9]*; do tr "\0" " " < $p/cmdline 2>/dev/null; echo; done | grep gnome-keyring | grep -v grep; ls -la ~/.local/share/keyrings'
        if has gnome-keyring-daemon && has login.keyring; then ok "keyring created and unlocked with the password"; else ko "keyring not unlocked"; fi
        edit_config $A "c['capabilities']['keyring']['mode'] = 'auto'"
        run 30 "$CARDEA" repo $A add st-bad --url https://downloads.claude.ai/claude-desktop/apt/stable --suite stable --key-url https://downloads.claude.ai/claude-desktop/key.asc --fingerprint 0000000000000000000000000000000000000000
        run 1800 "$CARDEA" rebuild $A
        if [ $RC -ne 0 ] && logs_of $A | grep -q -i 'fingerprint'; then ok "a repository key with a wrong fingerprint fails the build (R8)"; else ko "the build did not fail on the wrong fingerprint"; fi
        run 30 "$CARDEA" repo $A remove st-bad
        run 1800 "$CARDEA" rebuild $A
        if [ $RC -eq 0 ]; then ok "rebuild after removing the repository"; else ko "rebuild after removing the repository failed"; fi
        s=$(date +%s)
        run 1800 "$CARDEA" rebuild $A --fresh
        if [ $RC -eq 0 ] && ! has 'Using cache'; then ok "rebuild --fresh: no cached step ($(($(date +%s) - s)) s)"; else ko "rebuild --fresh failed or used the cache"; fi
        wait_state $A 'running*' 5 || run 600 "$CARDEA" start $A
    fi

    step "13. X11 isolated (xwayland-satellite)"
    if [ $QUICK -eq 1 ]; then
        skip "skipped (--quick)"
    else
        edit_config $A "c['capabilities']['x11'] = 'isolated'"
        run 30 "$CARDEA" packages $A add system x11-utils
        run 1800 "$CARDEA" rebuild $A
        if [ $RC -eq 0 ]; then
            ok "image with Xwayland and xwayland-satellite built"
            in_box $A sh -c 'command -v xwayland-satellite; for t in cargo rustc clang; do command -v $t && echo "left: $t"; done; true'
            if has 'xwayland-satellite' && ! has '^left:'; then ok "xwayland-satellite installed, no build tool left in the image"; else ko "xwayland-satellite missing or build tools left"; fi
            wait_state $A 'running*' 60
            sleep 3
            in_box $A sh -c 'echo "DISPLAY=$DISPLAY"; ls -la /tmp/.X11-unix 2>&1; for p in /proc/[0-9]*; do tr "\0" " " < $p/cmdline 2>/dev/null; echo; done | grep -i xwayland | grep -v grep'
            if has 'X1'; then ok "private X server :1 running"; else ko "no private X server"; fi
            in_box $A sh -c 'xdpyinfo | head -n 12'
            if has 'name of display'; then ok "an X11 client in the sandbox talks to the private X server" ; else ko "xdpyinfo could not connect to :1"; fi
        else
            ko "build with x11 isolated failed (package xwayland-satellite?)"
            edit_config $A "c['capabilities']['x11'] = 'off'"
        fi
        edit_config $A "c['capabilities']['x11'] = 'host'"
        run 10 "$CARDEA" status $A
        if hasi 'host X11'; then ok "x11 host is flagged as a risky setting"; else ko "no warning for x11 host"; fi
        edit_config $A "c['capabilities']['x11'] = 'off'"
    fi
fi

# --- 14. Untrusted sandbox ---------------------------------------------------------------------------------

step "14. Untrusted sandbox"
cat >"$WORK/b.json" <<EOF
{ "schema": 1, "id": "$B", "name": "$B", "base_image": "docker.io/library/debian:trixie-slim", "trust": "untrusted" }
EOF
run 1800 "$CARDEA" create --from-config "$WORK/b.json"
if [ $RC -ne 0 ]; then
    ko "build of the Untrusted sandbox failed"
else
    run 600 "$CARDEA" start $B
    A_STOPPED=0
    if ! wait_state $B running 5 && has 'unused IDs' && wait_state $A 'running*' 1; then
        # A Work sandbox must leave the sub-UIDs to Untrusted ones: check it is not the one holding them.
        run 120 "$CARDEA" stop $A
        A_STOPPED=1
        run 600 "$CARDEA" start $B
        if wait_state $B running 5; then
            ko "an Untrusted sandbox starts only while no Work sandbox runs: the Work sandbox holds the sub-UIDs ($(grep "^$(id -un):" /etc/subuid | cut -d: -f3) available)"
        fi
    fi
    if wait_state $B running 5; then
        ok "Untrusted sandbox running"
        if [ $A_STOPPED -eq 0 ] && wait_state $A 'running*' 1; then ok "an Untrusted sandbox runs alongside a Work sandbox"; fi
        in_box $B cat /proc/self/uid_map
        if awk -v u="$UIDN" '$2 == u' "$OUT" | grep -q .; then ko "the host UID is mapped (expected an isolated user)"; else ok "isolated user: the host UID is not mapped"; fi
        in_box $B sh -c 'echo gone > ~/disposable-file; id -un'
        if has '^sandbox$'; then ok "the isolated user is the sandbox user"; else ko "unexpected user in the Untrusted sandbox"; fi
        restart $B
        in_box $B sh -c 'cat ~/disposable-file 2>&1 || echo absent'
        if has absent; then ok "disposable home: wiped at restart"; else ko "the home kept a file across a restart"; fi
        probe $B 10.255.255.1 80
        if [ $PROBE = timeout ]; then ok "Untrusted preset drops blocked traffic (timeout)"; else ko "expected drop (timeout), got $PROBE"; fi
        edit_config $B "c['setup']['first_start'] = 'echo failing; exit 7'; c['setup']['on_failure'] = 'stop'"
        restart $B
        if wait_state $B 'error*' 30; then ok "first start failing with on_failure: stop → Error"; else ko "on_failure stop: $B is $(state_of $B)"; fi
        edit_config $B "c['setup']['on_failure'] = 'continue'"
        run 120 "$CARDEA" stop $B
        run 600 "$CARDEA" start $B
        if wait_state $B running 10 && logs_of $B | grep -qi 'failing\|exit\|failed'; then ok "on_failure: continue → running, failure in the log"; else ko "on_failure continue: $B is $(state_of $B)"; fi
        edit_config $B "c['security']['isolated_uid'] = False"
        run 10 "$CARDEA" status $B
        if hasi 'own user ID'; then ok "loosening an Untrusted default is a warning"; else ko "no warning for an Untrusted sandbox with your user ID"; fi
    else
        ko "the Untrusted sandbox did not start: $(state_of $B)"
    fi
    logs_of $B >"$WORK/log-b.txt"
    if [ $A_STOPPED -eq 1 ]; then
        run 120 "$CARDEA" stop $B
        run 600 "$CARDEA" start $A
    fi
fi

# --- 15. Creation, profiles from a sandbox, deletion -------------------------------------------------------

step "15. Creation, profiles and deletion"
if [ -z "${A_FAILED:-}" ]; then
    run 1800 "$CARDEA" create --profile empty --name "$A"
    if has "($A2)"; then ok "a taken name gets the next free id ($A2)"; else ko "no free id for a taken name"; fi
    run 30 "$CARDEA" delete $A2 -y
    run 30 "$CARDEA" profile save $A selftest-prof --name "Selftest profile"
    expect $RC "save a sandbox as a profile"
    run 10 "$CARDEA" profile show selftest-prof
    if has '/work/share'; then ko "the saved profile kept a host folder"; else ok "the saved profile has no host folder (R10)"; fi
    run 1800 "$CARDEA" create --profile selftest-prof --name "$D"
    expect $RC "create a sandbox from the saved profile"
    DFOLDER=$("$CARDEA" status $D 2>/dev/null | sed -n 's/^  Folder *//p')
    DFOLDER=${DFOLDER/#\~/$HOME}
    run 60 "$CARDEA" delete $D --keep-home -y
    if [ $RC -eq 0 ] && ! exists $D && [ -f "$DFOLDER/sandbox.json" ]; then ok "delete --keep-home keeps the folder"; else ko "delete --keep-home did not keep $DFOLDER"; fi
    rm -rf "$DFOLDER"
    run 30 "$CARDEA" profile delete selftest-prof
else
    skip "skipped: $A does not run"
fi

# --- 16. Claude profile (optional) -------------------------------------------------------------------------

step "16. Claude profile"
if [ $CLAUDE -eq 0 ]; then
    skip "not asked (--claude)"
else
    # The Claude profile is not built in: import it from next to this script, the repository, else the package's copy.
    P=""
    for f in "$(dirname "$0")/claude.json" "$(dirname "$0")/../profiles/claude.json" "$(dirname "$0")/profiles/claude.json" /usr/share/cardea/profiles/claude.json; do
        [ -f "$f" ] && { P=$f; break; }
    done
    if [ -z "$P" ]; then
        ko "no claude.json found (next to this script, in ../profiles, profiles/ or /usr/share/cardea/profiles)"
    else
    run 30 "$CARDEA" profile import "$P" --replace -y
    expect $RC "import the Claude profile ($P)"
    s=$(date +%s)
    run 3600 "$CARDEA" create --profile claude --name "$C"
    if [ $RC -eq 0 ]; then ok "Claude image built in $(($(date +%s) - s)) s"; else ko "Claude build failed"; fi
    run 3600 "$CARDEA" start $C
    if wait_state $C running 5; then
        ok "Claude sandbox started (first start included) in $(($(date +%s) - s)) s"
        in_box $C bash -lc 'for t in claude node go cargo mise; do printf "%s: " $t; $t --version 2>&1 | head -n 1; done; locale | head -n 2; echo "GTK_THEME=$GTK_THEME"'
        if has '^claude: [0-9]'; then ok "Claude Code installed"; else ko "Claude Code missing"; fi
        if has '^node: v'; then ok "node installed"; else ko "node missing"; fi
        if has 'fr_FR.UTF-8\|en_US.UTF-8'; then ok "locale set"; else ko "locale not set"; fi
        # Each toolchain of the profile answers from a login shell (mise shims and ~/.npm-global/bin on the PATH).
        in_box $C bash -lc 'for t in php composer dotnet valgrind codeburn java mvn gradle rustc python3; do if command -v $t >/dev/null; then echo "found: $t"; else echo "missing: $t"; fi; done; dotnet --version 2>&1 | head -n 1'
        for t in php composer dotnet valgrind codeburn java mvn gradle rustc python3; do
            if has "^found: $t\$"; then ok "Claude profile: $t installed"; else ko "Claude profile: $t missing"; fi
        done
        run 60 "$CARDEA" app $C
        ok "apps of the Claude sandbox" "$OUT"
        for a in claude-desktop firefox; do
            E="$HOME/.local/share/applications/cardea-$C-$a.desktop"
            run 10 grep -E '^(Exec|Icon|StartupWMClass)=' "$E"
            if has '^Icon=' && has '^StartupWMClass='; then ok "Claude profile: $a has its own icon and window class in the menu (its windows are not grouped under Cardea)" "$OUT"; else ko "Claude profile: $a menu entry lacks its icon or window class" "$OUT"; fi
        done
        run 10 pgrep -af "wayland-filter .*--sandbox $C\$"
        if [ $RC -eq 0 ]; then ok "Claude profile: its Wayland filter gives windows the menu entry's name (taskbar icon)"; else ko "Claude profile: its Wayland filter runs without --sandbox (windows keep their own app ID)"; fi
        # What the image's own entries say: the window class must match the app ID the windows send.
        in_box $C sh -c 'for f in /usr/share/applications/*.desktop; do grep -qiE "claude|firefox" "$f" && { echo "== $f"; grep -E "^(Exec|Icon|StartupWMClass|NoDisplay)=" "$f"; }; done'
        ok "Claude profile: desktop files of the image (see output)" "$OUT"
        run 60 "$CARDEA" packages $C list
        ok "packages of the Claude sandbox" "$OUT"
        run 60 "$CARDEA" service $C list
        ok "services of the Claude sandbox" "$OUT"
        run 30 "$CARDEA" links $C
        if has '^  claude:' && has 'to your browser: *on'; then ok "Claude profile: claude:// links reach Claude Desktop, outgoing links on" "$OUT"; else ko "Claude profile: links not as declared" "$OUT"; fi
    else
        ko "Claude sandbox did not start: $(state_of $C)"
    fi
    logs_of $C >"$WORK/log-c.txt"
    fi
fi

# --- 17. Desktop -------------------------------------------------------------------------------------------

step "17. Desktop integration"
if pgrep -f '(^|/)cardea-tray( |$)' >/dev/null; then ok "cardea-tray is running"; else ko "cardea-tray is not running (log out and back in, or run cardea-tray)"; fi
run 10 busctl --user get-property org.kde.StatusNotifierWatcher /StatusNotifierWatcher org.kde.StatusNotifierWatcher RegisteredStatusNotifierItems
if [ $RC -ne 0 ]; then
    skip "no StatusNotifierWatcher on the session bus"
else
    TRAYPID=$(pgrep -f '(^|/)cardea-tray( |$)' | head -n 1)
    # Items register under a unique (:1.56) or a well-known (org.kde.StatusNotifierItem-<pid>-1) bus name.
    OWNERS=$(grep -o '"[^"/]*' "$OUT" | tr -d '"' | while read -r n; do busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus GetConnectionUnixProcessID s "$n" 2>/dev/null | awk '{print $2}'; done)
    if [ -n "$TRAYPID" ] && echo "$OWNERS" | grep -qx "$TRAYPID"; then ok "the Cardea tray item is registered" "$OUT"; else ko "no tray item from cardea-tray" "$OUT"; fi
    for item in $(grep -o '"[^"]*"' "$OUT" | tr -d '"'); do
        svc=${item%%/*}
        path=/${item#*/}
        [ "$svc" = "$item" ] && path=/StatusNotifierItem
        pid=$(busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus GetConnectionUnixProcessID s "$svc" 2>/dev/null | awk '{print $2}')
        [ "$pid" = "$TRAYPID" ] || continue
        menu=$(busctl --user get-property "$svc" "$path" org.kde.StatusNotifierItem Menu 2>/dev/null | awk '{print $2}' | tr -d '"')
        # `--`: the -1 argument is not an option.
        run 10 busctl --user call -- "$svc" "${menu:-/MenuBar}" com.canonical.dbusmenu GetLayout iias 0 -1 0
        if exists $A && has "$A"; then ok "the tray menu lists the test sandbox" ; elif [ $RC -eq 0 ]; then ko "the tray menu does not list $A"; else ko "the tray menu could not be read"; fi
        run 10 busctl --user get-property "$svc" "$path" org.kde.StatusNotifierItem Status
        ok "tray status (for reference)" "$OUT"
    done
fi
if command -v cardea-gui >/dev/null && [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
    GUILOG=$WORK/gui.txt
    if pgrep -f '(^|/)cardea-gui( |$)' >/dev/null; then
        skip "the manager is already open: close it to test its start"
    else
        cardea-gui >"$GUILOG" 2>&1 &
        GUIPID=$!
        sleep 6
        if kill -0 $GUIPID 2>/dev/null; then ok "the manager window starts and stays up" "$GUILOG"; else ko "the manager exited at start" "$GUILOG"; fi
        s=$(date +%s%N)
        run 20 cardea-gui --sandbox "$A"
        if [ $RC -eq 0 ] && kill -0 $GUIPID 2>/dev/null; then ok "a second launch hands over to the open manager and exits ($((($(date +%s%N) - s) / 1000000)) ms)"; else ko "the second launch did not hand over (exit $RC)"; fi
        kill $GUIPID 2>/dev/null
        wait $GUIPID 2>/dev/null
    fi
else
    skip "no graphical session for the manager"
fi

# --- 18. Links ---------------------------------------------------------------------------------------------

step "18. Links"
cat >"$WORK/bad-links.json" <<'EOF'
{ "schema": 1, "id": "bl", "name": "Bad links", "base_image": "docker.io/library/debian:trixie-slim",
  "apps": [ { "id": "w", "name": "W", "exec": ["w"], "toolkit": "other", "schemes": ["https", "Bad"] } ],
  "links": { "outgoing": true, "allowed": ["GitHub.com", "1.2.3.4", "a.com:443"] } }
EOF
run 10 "$CARDEA" check sandbox "$WORK/bad-links.json"
if [ $RC -ne 0 ] && has 'schemes\[0\]' && has 'schemes\[1\]' && has 'links.allowed\[0\]' && has 'links.allowed\[1\]' && has 'links.allowed\[2\]'; then
    ok "reserved or malformed schemes and allowed hosts are refused, each named"
else
    ko "bad schemes or allowed hosts were not all refused"
fi
if ! wait_state $A 'running*' 1; then
    skip "skipped: $A does not run"
else
    AFOLDER=$("$CARDEA" status $A 2>/dev/null | sed -n 's/^  Folder *//p')
    AFOLDER=${AFOLDER/#\~/$HOME}
    LINKLOG=$AFOLDER/state/links.log
    RUNTIME=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/cardea

    # Incoming links (R4).
    edit_config $A 'c["apps"].append({"id": "st-link", "name": "Link app", "exec": ["sh", "-c", "printf %s \"$1\" > $HOME/link", "sh"], "toolkit": "other", "schemes": ["cardea-selftest"]})'
    expect $RC "an app declares the scheme cardea-selftest"
    ENTRY=${XDG_DATA_HOME:-$HOME/.local/share}/applications/cardea-$A-st-link.desktop
    if grep -q '^MimeType=x-scheme-handler/cardea-selftest;' "$ENTRY" 2>/dev/null && grep -q -- '--url %u' "$ENTRY"; then
        cp "$ENTRY" "$WORK/link-entry.txt"
        ok "its menu entry declares the scheme and passes the link with --url %u" "$WORK/link-entry.txt"
    else
        ko "no scheme handler entry $ENTRY"
    fi
    if command -v xdg-mime >/dev/null; then
        run 10 xdg-mime query default x-scheme-handler/cardea-selftest
        if has "cardea-$A-st-link.desktop"; then ok "a scheme nobody handled gets this sandbox's app as default handler"; else ko "the default handler was not set"; fi
    else
        skip "xdg-mime missing: default handler not checked"
    fi
    run 60 "$CARDEA" app $A st-link --url 'cardea-selftest://hello?x=1'
    sleep 3
    in_box $A cat /home/sandbox/link
    if has '^cardea-selftest://hello?x=1$'; then ok "a link of the app's scheme reaches it as its last argument"; else ko "the link did not reach the app"; fi
    run 30 "$CARDEA" app $A st-link --url 'https://example.com'
    if [ $RC -ne 0 ] && hasi 'opens only'; then ok "a link of another scheme is refused (R4)"; else ko "a link of another scheme was accepted"; fi
    run 30 "$CARDEA" app $A st-link --url "$(printf 'cardea-selftest://a\nb')"
    if [ $RC -ne 0 ] && hasi 'control character'; then ok "a link with a control character is refused (R4)"; else ko "a link with a control character was accepted"; fi
    run 10 "$CARDEA" links $A
    if has 'cardea-selftest:'; then ok "cardea links shows the scheme and who handles it" "$OUT"; else ko "cardea links does not show the scheme"; fi

    # Outgoing links: off by default, nothing mounted (R3).
    in_box $A sh -c '[ -e /run/cardea/links ] && echo channel || echo no-channel; echo "BROWSER=$BROWSER"'
    if has '^no-channel$' && has '^BROWSER=$'; then ok "links off (default): no link channel in the sandbox"; else ko "a link channel exists with links off"; fi
    run 30 "$CARDEA" link-send $A 'https://example.com/'
    if [ $RC -ne 0 ] && hasi 'cannot leave'; then ok "links off: the daemon refuses a link"; else ko "links off: a link was not refused"; fi

    run 30 "$CARDEA" links $A outgoing on
    if hasi 'restart'; then ok "turning outgoing links on asks for a restart"; else ko "no restart asked for outgoing links"; fi
    restart $A
    in_box $A sh -c 'stat -c "%F" /run/cardea/links; command -v xdg-open; echo "BROWSER=$BROWSER"'
    if has 'fifo' && has '^/usr/local/bin/xdg-open$' && has '^BROWSER=/usr/local/bin/xdg-open$'; then ok "links on: the FIFO and the opener are in the sandbox, BROWSER points at it"; else ko "the link channel is incomplete"; fi
    run 10 stat -c '%a %U' "$RUNTIME/links-$A.fifo"
    if has "^622 $(id -un)\$"; then ok "the FIFO is yours, mode 622, in the daemon's runtime folder"; else ko "unexpected FIFO mode or owner"; fi
    if pgrep -f "link-relay --fifo $RUNTIME/links-$A.fifo" >/dev/null; then ok "the link relay runs apart from the daemon"; else ko "no link relay process"; fi
    in_box $A sh -c 'xdg-open file:///etc/passwd; echo "rc=$?"; xdg-open mailto:a@b.c; echo "rc=$?"'
    if [ "$(grep -c '^rc=1$' "$OUT")" -eq 2 ] && hasi 'only http and https'; then ok "the opener refuses file: and mailto: in the sandbox"; else ko "the opener let another scheme through"; fi
    # One second apart: the relay logs at most one dropped line a second.
    in_box $A sh -c 'f=/run/cardea/links; echo file:///etc/passwd > $f; sleep 1.2; echo "javascript:alert(1)" > $f; sleep 1.2; { head -c 5000 /dev/zero | tr "\0" a; echo; } > $f'
    sleep 1
    cp "$LINKLOG" "$WORK/links-log.txt" 2>/dev/null
    if grep -q 'Dropped a link: file' "$WORK/links-log.txt" 2>/dev/null && grep -q 'Dropped a link: javascript' "$WORK/links-log.txt" && grep -q 'longer than 4096' "$WORK/links-log.txt"; then
        ok "the relay drops other schemes and over-long lines written straight to the FIFO" "$WORK/links-log.txt"
    else
        ko "the relay did not drop every bad line" "$WORK/links-log.txt"
    fi

    # Opened without asking once `ask` is off (one browser tab).
    run 30 "$CARDEA" links $A ask off
    in_box $A xdg-open 'https://example.com/?cardea-selftest'
    sleep 3
    if logs_of $A | grep -q 'Opened a link to example.com'; then ok "ask off: a link from the sandbox opens in your browser"; else ko "ask off: the link did not open"; fi
    # Links off again: refused at once by the daemon (the channel stays until the restart); rate limit in the relay.
    run 30 "$CARDEA" links $A outgoing off
    sleep 3
    in_box $A sh -c 'printf "https://example.com/1\nhttps://example.com/2\n" > /run/cardea/links'
    sleep 2
    cp "$LINKLOG" "$WORK/links-log.txt" 2>/dev/null
    if grep -q 'less than 3 s' "$WORK/links-log.txt" 2>/dev/null; then ok "the relay forwards at most one link per 3 s" "$WORK/links-log.txt"; else ko "no rate limit in the relay" "$WORK/links-log.txt"; fi
    if logs_of $A | grep -q 'refused: links cannot leave'; then ok "links turned off: refused at once, before the restart"; else ko "links turned off: not refused"; fi

    # The link window: a suspicious link always asks, even with ask off.
    run 30 "$CARDEA" links $A outgoing on
    if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ] && command -v cardea-gui >/dev/null; then
        "$CARDEA" link-send $A 'http://192.168.1.1:8443/' >"$WORK/link-send.txt" 2>&1 &
        SENDPID=$!
        sleep 5
        if pgrep -f 'cardea-gui --link' >/dev/null; then ok "a suspicious link opens the link window, even with ask off"; else ko "no link window for a suspicious link" "$WORK/link-send.txt"; fi
        pkill -f 'cardea-gui --link'
        wait $SENDPID 2>/dev/null
        if grep -qx cancelled "$WORK/link-send.txt"; then ok "closing the window cancels the link"; else ko "closing the window did not cancel" "$WORK/link-send.txt"; fi
    else
        skip "no graphical session: link window not checked"
    fi
    edit_config $A 'c["apps"] = [a for a in c["apps"] if a["id"] != "st-link"]'
    if [ ! -f "$ENTRY" ]; then ok "removing the app removes its scheme entry"; else ko "the scheme entry stayed"; fi
    if command -v xdg-mime >/dev/null; then
        run 10 xdg-mime query default x-scheme-handler/cardea-selftest
        if has "cardea-$A-st-link.desktop"; then ko "the default handler still names the removed entry"; else ok "the default handler that named it is reset"; fi
    fi
fi
if wait_state $B 'running*' 1; then
    edit_config $B 'c["links"] = {"outgoing": True, "ask": False, "allowed": ["example.com"]}'
    say "  (a red link window opens and closes by itself: do not answer it)"
    "$CARDEA" link-send $B 'https://example.com/' >"$WORK/link-send-b.txt" 2>&1 &
    SENDPID=$!
    sleep 3
    # The window up, or no display (cancelled) means it asked; opened before anyone could answer means it did not.
    if pgrep -f 'cardea-gui --link' >/dev/null || ! grep -qx opened "$WORK/link-send-b.txt"; then
        ok "Untrusted: a link always asks, despite ask off and an allowed host" "$WORK/link-send-b.txt"
    else
        ko "Untrusted: a link did not ask" "$WORK/link-send-b.txt"
    fi
    pkill -f 'cardea-gui --link'
    wait $SENDPID 2>/dev/null
else
    skip "the Untrusted sandbox does not run: its link checks skipped"
fi

DONE=1
