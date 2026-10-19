#!/bin/bash
# Smoke test for a Linux wirest-tui package, run as root inside a throwaway
# container with CAP_NET_ADMIN (see scripts/linux.sh test). Uses a split-tunnel
# test config (10.123.45.0/24 only, TEST-NET endpoint), so nothing else is routed.
#   tests/linux-smoke.sh dist/wirest-tui-linux-aarch64.tar.gz
set -euo pipefail
PKG="$1"
pass() { echo "PASS: $*"; }
fail() { echo "FAIL: $*"; exit 1; }

WORK=$(mktemp -d)
tar -xzf "$PKG" -C "$WORK"
DIR=$(echo "$WORK"/wirest-tui-*)
TUI="$DIR/wirest-tui"

# Static binaries: no dynamic loader needed.
for b in wirest-tui wirest-helper wg bash boringtun-cli; do
    if readelf -l "$DIR/$b" 2>/dev/null | grep -q "Requesting program interpreter"; then
        fail "$b is dynamically linked"
    fi
done
pass "all binaries are static"

key() { head -c 32 /dev/urandom | base64; }
CONF=$WORK/office.conf
cat > "$CONF" <<EOF
[Interface]
PrivateKey = $(key)
Address = 10.123.45.2/32

[Peer]
PublicKey = $(key)
AllowedIPs = 10.123.45.0/24
Endpoint = 203.0.113.10:51820
PersistentKeepalive = 25
EOF
FULL=$WORK/everything.conf
sed 's#10.123.45.0/24#0.0.0.0/0, ::/0#' "$CONF" > "$FULL"

gone() { # iface: no interface, no route, no boringtun left behind
    ! ip link show "$1" >/dev/null 2>&1 || fail "interface $1 still exists"
    ! ip route | grep -q 10.123.45 || fail "route to 10.123.45.0/24 still present"
    ! pgrep -f "boringtun-cli.* $1\$" >/dev/null || fail "boringtun for $1 still running"
}

echo "== root: direct mode"
"$TUI" add "$CONF" >/dev/null
"$TUI" list | grep -q '^office' || fail "list doesn't show office"
"$TUI" connect office
ip -4 addr show office | grep -q 10.123.45.2 || fail "office has no address"
ip route | grep -q "10.123.45.0/24 dev office" || fail "no route via office"
"$TUI" status | grep -q "Waiting for server (office)" || fail "status should be waiting (no real server)"
pass "connect as root: interface, address, route, status"
"$TUI" disconnect
gone office
pass "disconnect as root cleans up"

echo "== SSH guard"
"$TUI" add "$FULL" everything >/dev/null
if SSH_CONNECTION="1.2.3.4 5 6.7.8.9 22" "$TUI" connect everything 2>/dev/null; then
    fail "full-tunnel connect over SSH should need --force"
fi
pass "full-tunnel connect over SSH refused without --force"
printf '[Interface]\nPrivateKey = %s\nPostUp = touch /tmp/pwned\n' "$(key)" > "$WORK/hook.conf"
"$TUI" add "$WORK/hook.conf" hook >/dev/null
! "$TUI" connect hook 2>/dev/null || fail "config with PostUp should be refused"
[ ! -e /tmp/pwned ] || fail "PostUp ran"
pass "configs with script hooks are refused"

echo "== normal user via the helper"
# What install.sh's systemd unit does, minus systemd (containers have none).
B=/usr/local/lib/wirest/bin
mkdir -p $B
cp "$DIR"/{wg,wg-quick,bash,boringtun-cli,wireguard-go,wirest-helper} $B/
id tester >/dev/null 2>&1 || useradd -m tester 2>/dev/null || adduser -D tester
GID=$(id -g tester)
$B/wirest-helper --socket-gid "$GID" &
HELPER=$!
for _ in $(seq 50); do [ -S /run/wirest.sock ] && break; sleep 0.1; done
[ -S /run/wirest.sock ] || fail "helper socket missing"
[ "$(stat -c %a /run/wirest.sock)" = 660 ] || fail "socket mode isn't 660"
cp -r "$DIR" /home/tester/pkg && cp "$CONF" /home/tester/ && chown -R tester /home/tester
as_tester() { su tester -s /bin/sh -c "cd ~ && $*"; }
as_tester "pkg/wirest-tui helper status" | grep -q "installed and current" || fail "helper not current for tester"
as_tester "pkg/wirest-tui add office.conf" >/dev/null
as_tester "pkg/wirest-tui connect office"
ip -4 addr show office | grep -q 10.123.45.2 || fail "helper connect: no address"
as_tester "pkg/wirest-tui status" | grep -q "office" || fail "tester status doesn't show office"
pass "connect as a normal user through the helper"
as_tester "pkg/wirest-tui disconnect"
gone office
pass "disconnect through the helper cleans up"
kill $HELPER

echo "ALL PASSED"
