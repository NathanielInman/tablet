#!/usr/bin/env bash
# End-to-end check of the real daemon, bar module and CLI against a fake
# openvpn3 (tools/fake-openvpn3.py) on a private bus. Needs dbus-run-session,
# jq and python-gobject. Never touches a real VPN.
#
#   tools/e2e.sh [path/to/tablet]      (default: target/debug/tablet)
#
# Every check polls for up to 8 seconds instead of sleeping a fixed time.
set -euo pipefail

if [[ -z ${TABLET_E2E_INNER:-} ]]; then
  bin=$(realpath "${1:-target/debug/tablet}")
  [[ -x $bin ]] || { echo "e2e: no tablet at $bin (cargo build first)" >&2; exit 1; }
  # A private bus, and nothing from the desktop this may run on.
  exec env -u HYPRLAND_INSTANCE_SIGNATURE -u WAYLAND_DISPLAY -u SWAYSOCK -u NIRI_SOCKET \
    TABLET_E2E_INNER=1 TABLET_BIN="$bin" \
    dbus-run-session -- "$0"
fi

here=$(cd "$(dirname "$0")" && pwd)
tablet=$TABLET_BIN
python=${PYTHON:-python3}
fake="$python $here/fake-openvpn3.py"
# The fake openvpn3 is the "system" bus as far as the daemon knows.
export DBUS_SYSTEM_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS
# Socket paths must stay short (sun_path), so not a deep workspace path.
root=$(mktemp -d /tmp/tb-e2e.XXXXXX)
export TABLET_RUNTIME_DIR=$root/run TABLET_STATE_DIR=$root/state TABLET_CONFIG_DIR=$root/config
mkdir -p "$TABLET_CONFIG_DIR"
# What the next `session-start` does (the daemon runs it, so not an
# environment variable of the command that asks).
export FAKE_OPENVPN3_MODE_FILE=$root/mode
mode() { echo "$1" > "$root/mode"; }
touch "$root/work.ovpn"

# A listener the probe can reach, and a config that points at it.
$python -c '
import socket, sys
s = socket.socket(); s.bind(("127.0.0.1", 0)); s.listen(16)
open(sys.argv[1], "w").write(str(s.getsockname()[1]))
while True:
    c, _ = s.accept(); c.close()
' "$root/port" &
pids=($!)
for _ in $(seq 50); do [[ -s $root/port ]] && break; sleep 0.1; done
port=$(cat "$root/port")

config() {
  cat > "$TABLET_CONFIG_DIR/config.toml" <<EOF
[checks]
every = 0
grace = 4

[openvpn3]
command = "$fake"

[doctor]
binaries = []

[[profile]]
name = "work"
ovpn = "$root/work.ovpn"
zones = ["corp.example.com"]
probes = ["$1"]
EOF
}
config "127.0.0.1:$port"

cleanup() {
  kill "${pids[@]}" 2>/dev/null || true
  pkill -f "^$tablet daemon" 2>/dev/null || true
  rm -rf "$root"
}
trap cleanup EXIT

passed=0
fail() {
  echo "e2e: FAIL: $1" >&2
  echo "--- bar" >&2; tail -5 "$root/bar.out" >&2 || true
  echo "--- status" >&2; "$tablet" status >&2 || true
  echo "--- daemon.log" >&2; cat "$TABLET_STATE_DIR/daemon.log" >&2 || true
  exit 1
}
expect() {
  for _ in $(seq 80); do
    if eval "$2" >/dev/null 2>&1; then
      passed=$((passed + 1))
      echo "ok   $1"
      return
    fi
    sleep 0.1
  done
  echo "--- the check said (phase=$(phase), lamp=$(lamp), $(date +%T.%N)):" >&2
  eval "$2" >&2 || true
  fail "$1"
}
phase() { "$tablet" status --json | jq -r '.profiles[] | select(.name == "work") | .phase'; }
lamp() { tail -1 "$root/bar.out" | jq -r '.class[0]'; }
path() { "$tablet" status --json | jq -r '.profiles[0].session.path'; }
run() { "$tablet" "$@" > "$root/run.out" 2>&1 || fail "tablet $* ($(cat "$root/run.out"))"; }

$fake serve &
pids+=($!)
expect "the fake openvpn3 owns its name" 'gdbus introspect --session --dest net.openvpn.v3.sessions --object-path /net/openvpn/v3/sessions'

"$tablet" bar > "$root/bar.out" &
pids+=($!)
expect "the bar starts the daemon and shows a dim lamp" '[[ $(lamp) == off ]]'
expect "the profile is off" '[[ $(phase) == off ]]'

run connect
expect "connect starts a session and the lamp goes frost" '[[ $(lamp) == held ]]'
expect "status says held" '[[ $(phase) == held ]]'
expect "the device is known" '[[ $("$tablet" status --json | jq -r ".profiles[0].session.device") == tun9 ]]'
expect "history says connected" 'grep -q "\"what\":\"connected\"" "$TABLET_STATE_DIR/history.jsonl"'
"$tablet" check > "$root/check.out" || fail "check of a working tunnel exited $?"
expect "check reports the probe" 'grep -q "ok    127.0.0.1:$port" "$root/check.out"'
expect "why names the profile and exits 0" '"$tablet" why https://wiki.corp.example.com/x | grep -q "needs work"'

# The probe stops answering: one failed check is not enough, two are.
config "name.invalid:443"
sleep 0.5
set +e; "$tablet" check > "$root/check.out"; code=$?; set -e
[[ $code == 75 ]] || fail "a failing check should exit 75, not $code"
passed=$((passed + 1)); echo "ok   a failing check exits 75"
"$tablet" check > /dev/null || true
expect "two failed checks make it stale" '[[ $(phase) == stale && $(lamp) == stale ]]'
set +e; "$tablet" why wiki.corp.example.com > "$root/why.out"; code=$?; set -e
[[ $code == 75 ]] || fail "why on a stale profile should exit 75, not $code"
grep -q "This is the VPN, not the service" "$root/why.out" || fail "why does not blame the VPN: $(cat "$root/why.out")"
passed=$((passed + 1)); echo "ok   why says it is the VPN and exits 75"
config "127.0.0.1:$port"
sleep 0.5
run check
expect "a passing check brings it back" '[[ $(phase) == held ]]'
expect "history says recovered" 'grep -q "\"what\":\"recovered\"" "$TABLET_STATE_DIR/history.jsonl"'

# The server ends the session behind our back.
p=$(path)
$fake ctl remove "$p"
expect "a session that vanishes is dropped" '[[ $(phase) == dropped && $(lamp) == dropped ]]'
sleep 1
expect "and stays dropped" '[[ $(phase) == dropped ]]'
run ack
expect "ack puts the lamp out" '[[ $(phase) == off && $(lamp) == off ]]'

# A disconnect we asked for is not a drop.
run connect
expect "connected again" '[[ $(phase) == held ]]'
run disconnect
expect "disconnect turns it off, not dropped" '[[ $(phase) == off ]]'

# openvpn3 says connected without telling anyone: the watcher reads it.
mode quiet; run connect
expect "a quiet session is found by reading it" '[[ $(phase) == held ]]'
run disconnect
expect "off again" '[[ $(phase) == off ]]'

# A refused sign-in that lingers.
mode auth-failed; run connect
expect "a refused sign-in is failed" '[[ $(phase) == failed && $(lamp) == failed ]]'
expect "and says why" '"$tablet" status | grep -q "refused the sign-in"'
run ack
expect "ack disconnects the dead session" '[[ $(phase) == off && $("$tablet" status --json | jq ".profiles[0].session") == null ]]'

# A web sign-in.
mode auth-url; run connect
expect "a web sign-in pulses amber" '[[ $(phase) == signin ]] && tail -1 "$root/bar.out" | jq -e ".class == [\"signin\",\"pulse\"]"'
expect "the sign-in page is known" '[[ $("$tablet" status --json | jq -r ".profiles[0].signin_url") == https://login.example.com/* ]]'
expect "the log never has the sign-in URL" '! grep -q "login.example.com" "$TABLET_STATE_DIR/daemon.log"'
p=$(path)
$fake ctl status "$p" 2 7
expect "finishing the sign-in holds it" '[[ $(phase) == held ]]'
run disconnect

# openvpn3 cannot start: the failure says what openvpn3 said.
mode broken; run connect
expect "a start that fails is failed with openvpn3's words" '"$tablet" status | grep -q "New tunnel did not respond"'
run ack
mode connect

# An agent asks; nobody answers.
set +e; "$tablet" need work --reason "e2e" --timeout 1 -- true 2> "$root/need.err"; code=$?; set -e
[[ $code == 75 ]] || fail "an unanswered need should exit 75, not $code ($(cat "$root/need.err"))"
passed=$((passed + 1)); echo "ok   an unanswered need exits 75 without running the command"

# An agent asks and the click on the lamp says yes.
"$tablet" need work --reason "e2e" --timeout 20 -- touch "$root/ran" 2> "$root/need.err" &
needpid=$!
expect "the request lights the lamp" '[[ $(lamp) == asking ]]'
expect "the request is in the snapshot" '[[ $("$tablet" status --json | jq -r ".asks[0].reason") == e2e ]]'
run flip
wait $needpid || fail "need exited $? after the request was accepted ($(cat "$root/need.err"))"
expect "the command ran once the profile was held" '[[ -e $root/ran && $(phase) == held ]]'
expect "need with the profile held runs at once" '"$tablet" need work -- true'
run disconnect

# Every module state came with a tooltip.
expect "the bar line has a tooltip" 'tail -1 "$root/bar.out" | jq -e ".tooltip | length > 10"'

echo "e2e: $passed checks passed"
