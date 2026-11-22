# Network firewall

Each sandbox has its own firewall, **on by default**. Its rules live in the sandbox's network namespace and are
held by a small companion container: programs in the sandbox, even running as root there, cannot change them.
Your system firewall is never modified and still applies on top.

## Addresses inside the sandbox

The sandbox sees fixed, neutral addresses, the same on every computer, never copies of yours:

| Inside | Value |
|---|---|
| Interface | `eth0` |
| Address and gateway (IPv4) | `10.249.0.2/30`, gateway `10.249.0.1` |
| Address and gateway (IPv6) | `fd63:6172:6465:6100::2`, gateway `fd63:6172:6465:6100::1` |
| DNS server | `169.254.1.1`, a relay to your computer's DNS (your router's address never shows) |
| DNS search domain | none |
| Your computer | `host.containers.internal` (`169.254.1.2`) |
| Network interfaces in `/sys` | hidden (your computer's names and hardware addresses) |

The gateway address inside never leads to your computer. Your computer is reached only through
`host.containers.internal`, and only when the firewall allows it. It leads to your computer's loopback, so a
service there is reachable when it listens on all addresses or on `127.0.0.1`, not when it listens only on your
LAN address.

## Zones

Each new outgoing connection is classified into a zone, in this order:

| Order | Zone | Contains |
|---|---|---|
| 1 | — | Loopback and already established connections: always allowed |
| 2 | — | DNS queries to the resolvers: allowed |
| 3 | **Host** | Your computer itself (all its addresses) |
| 4 | **LAN** | Your local networks and private address ranges (`10/8`, `172.16/12`, `192.168/16`, `169.254/16`, `100.64/10`, `fc00::/7`, `fe80::/10`), multicast and broadcast (`224.0.0.0/4`, `255.255.255.255`, `ff00::/8`) |
| 5 | **WAN** | Everything else (the Internet) |

IPv4 and IPv6 are both filtered.

## Allowlist or blocklist

Each zone is in one of two modes, and keeps a separate list for each:

- **allowlist**: everything is blocked except the entries;
- **blocklist**: everything is allowed except the entries.

Defaults for a new sandbox:

| Zone | Mode | Entries |
|---|---|---|
| Host | allowlist | none: no service of your computer is reachable |
| LAN | allowlist | none: no machine of your network is reachable, not even the gateway |
| WAN | blocklist | none: the Internet is open |

An entry is a **reference** or an IP address / range (`192.168.1.30`, `10.0.0.0/8`, `2001:db8::1`), with an
optional port and protocol and a comment.

| Reference | Resolves to |
|---|---|
| `host` | Your computer: every address of its network interfaces, and the address Podman gives it inside the sandbox (`host.containers.internal`) |
| `gateway` | Your computer's default gateways (your router) |
| `dns` | The resolvers the sandbox uses |

| Port | Protocol | Matches |
|---|---|---|
| none | none | Every connection to the destination |
| a port | none | TCP and UDP to that port |
| a port or none | TCP or UDP | Only that protocol |

References are resolved again every 5 seconds, and the rules follow when your network changes (for example when
you join another Wi-Fi network). A reference that cannot be resolved blocks: an allowlist entry is left out, and
a blocklist entry closes its whole zone.

Changes apply immediately, without restarting the sandbox.

## Rejected or dropped

Each sandbox chooses what blocked traffic gets:

| Setting | What the program sees | Typical use |
|---|---|---|
| **Reject** | An immediate refusal ("Connection refused" / "No route to host") | Trusted tools and agents, which can then tell why a request failed |
| **Drop** | Nothing: the connection times out | Untrusted programs, which learn nothing about what is there |

Blocked connections are counted per zone since the start. Destinations blocked in the last 10 minutes are listed
(with an **Allow** shortcut in the manager) and each new one adds a line to the sandbox log.

## Warnings

These settings expose your computer or your network, so they ask for confirmation and stay flagged with a warning
wherever the sandbox is shown:

- firewall turned **off**;
- **Host** zone in blocklist mode: every service of your computer is reachable except the listed ones;
- **LAN** zone in blocklist mode: every device of your network is reachable except the listed ones.

## Managing it

| Command | What it does |
|---|---|
| `cardea net <id>` | The zones and their lists, what the references resolve to, blocked counts |
| `cardea net <id> allow <zone> <destination> [--port <n>] [--proto tcp\|udp] [--comment <text>]` | Add an entry to a zone's allowlist (`<zone>`: `host`, `lan` or `wan`) |
| `cardea net <id> block <zone> <destination> …` | Add an entry to its blocklist |
| `cardea net <id> remove <zone> <destination> [--port <n>]` | Remove the matching entries from both lists |
| `cardea net <id> mode <zone> allow\|block [-y]` | Switch a zone's mode (blocklist for Host or LAN asks first) |
| `cardea net <id> reject` / `drop` | What blocked connections get |
| `cardea net <id> on` / `off [-y]` | Turn the firewall on or off (off asks first) |
| `cardea net <id> blocked` | Destinations blocked in the last 10 minutes |

In the manager, the sandbox's **Network** page does the same, with **Restore defaults**.

## Failure

The firewall container starts first and blocks everything; the sandbox's own container starts only once the real
rules are applied. If they cannot be applied, the sandbox does not start. If later rules cannot be applied, or the
firewall container stops while the sandbox runs, the last rules stay in force and the sandbox is marked in error:
stop it and start it again.
