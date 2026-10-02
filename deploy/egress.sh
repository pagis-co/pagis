#!/bin/sh
# The egress policy of the Computers on this Docker host (ADR-0014).
#
# A Computer reaches the public internet, the Media Relay's UDP range on
# this host, and the TCP port of the daemon's exit listener on this host,
# which the Exit Proxy of a Computer in Home mode sends its connections
# to (ADR-0029). It reaches no other address of this host, no link-local
# address and no private address, except the blocks in
# PAGIS_COMPUTER_ALLOW. The rules are in the host's DOCKER-USER chain
# (traffic that the host forwards) and INPUT chain (traffic to the host
# itself), so root inside a Computer cannot remove them.
#
# The `egress` service of `compose.yaml` runs this script with the host's
# network and NET_ADMIN before the daemon starts. Each run replaces the
# Pagis chains and their jumps in one iptables-restore transaction, so a
# second run leaves one copy of each rule.
#
# The rules only take reach away. The traffic that they let through goes
# on to the rules of Docker and of the host: Docker's rules still keep
# the Tenant Networks of two Workspaces apart, and the host's firewall
# still applies.
#
# Settings, from the environment:
#
#   PAGIS_MEDIA_PORT_FIRST, PAGIS_MEDIA_PORT_LAST
#       The Media Relay's UDP range.
#   PAGIS_EXIT_PORT
#       The TCP port of the daemon's exit listener: `[computer] exit_port`
#       of the daemon, PAGIS_COMPUTER_EXIT_PORT.
#   PAGIS_COMPUTER_ALLOW
#       The private IPv4 blocks that a Computer reaches, comma-separated,
#       such as `192.168.1.0/24,10.0.5.7`. Empty by default. The list
#       opens no address of this host.
#   PAGIS_EGRESS_BRIDGES
#       The interfaces of the Computers' networks, as an iptables
#       interface match. `br-+`, every user-defined Docker bridge, by
#       default. On a Headless Server each bridged container is a Computer.

# No word of a setting is a file pattern.
set -euf

fail() {
  echo "pagis-egress: $*" >&2
  exit 1
}

is_port() {
  case $1 in
    '' | *[!0-9]*) return 1 ;;
  esac
  [ "$1" -ge 1 ] && [ "$1" -le 65535 ]
}

# An IPv4 address, or an IPv4 address and a prefix length.
is_block() {
  echo "$1" | grep -Eq '^[0-9]{1,3}(\.[0-9]{1,3}){3}(/[0-9]{1,2})?$' || return 1
  case $1 in
    */*) [ "${1#*/}" -le 32 ] || return 1 ;;
  esac
  for octet in $(echo "${1%/*}" | tr . ' '); do
    [ "$octet" -le 255 ] || return 1
  done
}

first=${PAGIS_MEDIA_PORT_FIRST:-}
last=${PAGIS_MEDIA_PORT_LAST:-}
exit_port=${PAGIS_EXIT_PORT:-}
bridges=${PAGIS_EGRESS_BRIDGES:-br-+}

is_port "$first" || fail "PAGIS_MEDIA_PORT_FIRST is \"$first\"; set it to the first port of the Media Relay's range"
is_port "$last" || fail "PAGIS_MEDIA_PORT_LAST is \"$last\"; set it to the last port of the Media Relay's range"
[ "$first" -le "$last" ] || fail "PAGIS_MEDIA_PORT_LAST ($last) is before PAGIS_MEDIA_PORT_FIRST ($first)"
is_port "$exit_port" || fail "PAGIS_EXIT_PORT is \"$exit_port\"; set it to the port of the daemon's exit listener"
case $bridges in
  '' | *[!A-Za-z0-9_.+-]*) fail "PAGIS_EGRESS_BRIDGES is \"$bridges\"; set it to an interface name, or to a prefix and +" ;;
esac

allowed=
for block in $(echo "${PAGIS_COMPUTER_ALLOW:-}" | tr ',' ' '); do
  is_block "$block" ||
    fail "PAGIS_COMPUTER_ALLOW holds \"$block\"; each entry is an IPv4 address or an IPv4 CIDR block"
  allowed="$allowed $block"
done

# The resolvers that Docker gives to containers. Docker writes the same
# list into the resolv.conf of a container with the host's network, as
# this one is. A resolver on loopback is out of a Computer's reach.
resolvers=$(awk '$1 == "nameserver" && $2 ~ /^[0-9.]+$/ && $2 !~ /^127\./ { print $2 }' /etc/resolv.conf)

# Docker's rules and these rules must be in one backend: the one whose
# tables hold Docker's DOCKER-USER chain.
if iptables-nft -S DOCKER-USER >/dev/null 2>&1; then
  iptables=iptables-nft
elif iptables-legacy -S DOCKER-USER >/dev/null 2>&1; then
  iptables=iptables-legacy
else
  fail "this host has no DOCKER-USER chain; Docker must manage the firewall with iptables"
fi

# The jumps of an earlier run, as deletions. The settings of that run
# can differ, so the jumps are found by their target.
earlier_jumps() {
  "$iptables" -S "$1" 2>/dev/null | grep -e " -j $2\$" | sed 's/^-A /-D /' || true
}

{
  echo '*filter'
  # A chain that exists is emptied, and a chain that does not is made.
  echo ':PAGIS-FORWARD - [0:0]'
  echo ':PAGIS-INPUT - [0:0]'
  earlier_jumps DOCKER-USER PAGIS-FORWARD
  earlier_jumps INPUT PAGIS-INPUT
  echo "-I DOCKER-USER 1 -i $bridges -j PAGIS-FORWARD"
  echo "-I INPUT 1 -i $bridges -j PAGIS-INPUT"

  # Traffic that the host forwards from a Computer.
  echo '-A PAGIS-FORWARD -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN'
  # Traffic to a Docker bridge is Docker's to filter: a Computer reaches
  # its own Tenant Network, and Docker keeps it out of every other one.
  echo "-A PAGIS-FORWARD -o $bridges -j RETURN"
  for resolver in $resolvers; do
    echo "-A PAGIS-FORWARD -d $resolver/32 -p udp --dport 53 -j RETURN"
    echo "-A PAGIS-FORWARD -d $resolver/32 -p tcp --dport 53 -j RETURN"
  done
  for block in $allowed; do
    echo "-A PAGIS-FORWARD -d $block -j RETURN"
  done
  for block in 169.254.0.0/16 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 100.64.0.0/10; do
    echo "-A PAGIS-FORWARD -d $block -j DROP"
  done

  # Traffic from a Computer to an address of the host itself.
  echo '-A PAGIS-INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN'
  for resolver in $resolvers; do
    echo "-A PAGIS-INPUT -d $resolver/32 -p udp --dport 53 -j RETURN"
    echo "-A PAGIS-INPUT -d $resolver/32 -p tcp --dport 53 -j RETURN"
  done
  echo "-A PAGIS-INPUT -p udp --dport $first:$last -j RETURN"
  echo "-A PAGIS-INPUT -p tcp --dport $exit_port -j RETURN"
  echo '-A PAGIS-INPUT -j DROP'
  echo 'COMMIT'
} | "$iptables-restore" --noflush

echo "pagis-egress: the rules are in place with $iptables on $bridges;" \
  "media range $first-$last; exit port $exit_port; allowed:${allowed:- none};" \
  "resolvers: $(echo $resolvers)"
