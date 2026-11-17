#!/bin/sh
# The network monitor end to end on Linux, without root: in a user and
# network namespace of its own (unshare -rn), `netstatus --watch` follows
# what this script does to the namespace's links and routes:
#
#   offline (only a down loopback)
#   -> online: dummy0 up with a default route (metric 100)
#   -> online, a new network: dummy1's default route (metric 50) wins
#   -> online, a new network again: dummy1 deleted, back to dummy0
#   -> offline: dummy0 deleted
#
# Usage: network-linux.sh target/debug/examples/netstatus
set -eu
bin=$1
if [ -z "${NETWORK_NETNS:-}" ]; then
    exec env NETWORK_NETNS=1 unshare -rn sh "$0" "$@"
fi

out=$(mktemp)
"$bin" --watch 9 >"$out" &
pid=$!
sleep 1.5
ip link add dummy0 type dummy
ip link set dummy0 up
ip addr add 10.9.0.2/24 dev dummy0
ip route add default via 10.9.0.1 metric 100
sleep 1.5
ip link add dummy1 type dummy
ip link set dummy1 up
ip addr add 10.8.0.2/24 dev dummy1
ip route add default via 10.8.0.1 metric 50
sleep 1.5
ip link del dummy1
sleep 1.5
ip link del dummy0
wait "$pid"
cat "$out"

# Each status line's online flag and generation, in order.
awk '
    /^netstatus: / {
        online = ""; gen = ""
        for (i = 2; i <= NF; i++) {
            split($i, kv, "=")
            if (kv[1] == "online") online = kv[2]
            if (kv[1] == "generation") gen = kv[2]
        }
        n++; on[n] = online; g[n] = gen
    }
    END {
        want = "0 1 1 1 0"
        got = ""
        for (i = 1; i <= n; i++) got = got (i > 1 ? " " : "") on[i]
        if (got != want) { print "network-linux: online went " got ", wanted " want; exit 1 }
        for (i = 3; i <= n; i++)
            if (g[i] + 0 <= g[i - 1] + 0) { print "network-linux: generation " i " did not grow"; exit 1 }
        print "network-linux: ok (" n " changes, generations " g[1] ".." g[n] ")"
    }
' "$out"
