# Security model

## What a sandbox protects

| Threat | Protection |
|---|---|
| A program reads or damages your personal files | It only sees its own home and the folders you share; sensitive paths inside shared folders (`.git/hooks`, `.git/config`, `.vscode`, `.envrc`…) are mounted read-only |
| A program reaches services of your computer or devices of your network | Per-sandbox firewall, on by default: no access to your computer, only the gateway on your network ([Network firewall](network.md)) |
| A program gains privileges | Rootless Podman, all capabilities dropped except a minimal set, `no-new-privileges`, seccomp filter on by default |
| A program in one sandbox reaches another sandbox's data | Each sandbox has its own home; a home can belong to one sandbox only |
| A program controls Cardea or Podman | Neither Cardea's control socket nor Podman's socket is ever visible in a sandbox |
| A shared profile does something hidden | Profiles are declarative data, validated, never executed on your computer; risky items are highlighted before import and scripts are shown in full |
| A firewall failure leaves the network open | The sandbox does not start unless its rules are applied |

## What it does not protect

- **Shared kernel**: sandboxes are containers, not virtual machines. A kernel vulnerability can break out of any
  container. KVM and turning seccomp off widen this surface; both are risky settings.
- **Wayland display**: a sandbox with the Wayland capability can draw windows on your desktop, including a window
  that imitates a password prompt, and may use the clipboard.
- **Secrets inside a sandbox**: everything inside a sandbox can read what the sandbox stores, including its
  keyring when it is unlocked automatically. Keep unrelated credentials in separate sandboxes.
- **Programs without their own sandbox**: browsers and Electron apps run with their internal sandbox disabled
  inside the container; the container remains the boundary.
- **Allowed network traffic**: the firewall decides where a sandbox can connect, not what it sends there.

## Risky settings

These settings ask for an explicit confirmation that names the consequence, and stay flagged with a warning:

- firewall off, or the Host or LAN zone in blocklist mode;
- KVM, seccomp off;
- X11 through your desktop's X server;
- for an Untrusted sandbox: any setting loosened from its preset.

## Reporting a vulnerability

Please report security issues privately to the maintainers rather than in a public issue. Contact details will be
given in the repository once it is published.
