# Network firewall

Each sandbox has its own firewall, **on by default**. Its rules live in the sandbox's network namespace and are
held by a small companion container: programs in the sandbox, even running as root there, cannot change them.
Your system firewall is never modified and still applies on top.

## Zones

Each new outgoing connection is classified into a zone, in this order:

| Order | Zone | Contains |
|---|---|---|
| 1 | — | Loopback and already established connections: always allowed |
| 2 | — | DNS queries to the resolvers: allowed |
| 3 | **Host** | Your computer itself (all its addresses) |
| 4 | **LAN** | Your local networks and private address ranges (`10/8`, `172.16/12`, `192.168/16`, `169.254/16`, `100.64/10`, `fc00::/7`, `fe80::/10`) |
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
| LAN | allowlist | the gateway only |
| WAN | blocklist | none: the Internet is open |

An entry is a **reference** (`host`, `gateway`, `dns`) or an IP address / range, with an optional port and
protocol and a comment. References are resolved on your computer and kept up to date when your network changes
(for example when you join another Wi-Fi network). A reference that cannot be resolved blocks.

Changes apply immediately, without restarting the sandbox.

## Rejected or dropped

Each sandbox chooses what blocked traffic gets:

| Setting | What the program sees | Typical use |
|---|---|---|
| **Reject** | An immediate refusal ("Connection refused" / "No route to host") | Trusted tools and agents, which can then tell why a request failed |
| **Drop** | Nothing: the connection times out | Untrusted programs, which learn nothing about what is there |

Blocked connections are counted per zone; recent blocked destinations are listed with an **Allow** shortcut.

## Warnings

These settings expose your computer or your network, so they ask for confirmation and stay flagged with a warning
wherever the sandbox is shown:

- firewall turned **off**;
- **Host** zone in blocklist mode: every service of your computer is reachable except the listed ones;
- **LAN** zone in blocklist mode: every device of your network is reachable except the listed ones.

## Failure

If the firewall rules cannot be applied, the sandbox does not start. If the firewall container stops while the
sandbox runs, the last rules stay in force and the sandbox is marked in error.
