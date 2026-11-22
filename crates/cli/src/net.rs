//! `cardea net <id> …`: the sandbox firewall.

use cardea_core::firewall::NetStatus;
use cardea_core::model::{Blocked, Firewall, Proto, Rule, Zone, ZoneMode};
use cardea_proto::{Method, Saved, Status};
use clap::{Subcommand, ValueEnum};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ZoneArg {
    /// Your computer
    Host,
    /// Your local network
    Lan,
    /// The Internet
    Wan,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ModeArg {
    /// Everything is blocked except the entries
    Allow,
    /// Everything is allowed except the entries
    Block,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ProtoArg {
    Tcp,
    Udp,
}

#[derive(Subcommand)]
pub enum NetCommand {
    /// The zones, their lists, what the references resolve to, blocked connections
    Show,
    /// Add an entry to a zone's allowlist
    Allow {
        zone: ZoneArg,
        /// `host`, `gateway`, `dns`, an IP address or a CIDR range
        to: String,
        #[arg(long)]
        port: Option<u16>,
        /// Only this protocol (default: TCP and UDP when a port is given, everything otherwise)
        #[arg(long)]
        proto: Option<ProtoArg>,
        #[arg(long)]
        comment: Option<String>,
    },
    /// Add an entry to a zone's blocklist
    Block {
        zone: ZoneArg,
        to: String,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        proto: Option<ProtoArg>,
        #[arg(long)]
        comment: Option<String>,
    },
    /// Remove an entry from a zone's lists
    Remove {
        zone: ZoneArg,
        to: String,
        /// Only the entry with this port
        #[arg(long)]
        port: Option<u16>,
    },
    /// Use a zone's allowlist or its blocklist
    Mode {
        zone: ZoneArg,
        mode: ModeArg,
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// The recently blocked destinations
    Blocked,
    /// Blocked connections are refused at once (the program knows it was refused)
    Reject,
    /// Blocked connections are dropped silently (the program waits until its timeout)
    #[command(name = "drop")]
    DropTraffic,
    /// Turn the firewall on
    On,
    /// Turn the firewall off: the sandbox reaches everything
    Off {
        #[arg(short, long)]
        yes: bool,
    },
}

const TITLES: [&str; 3] = [
    "Host (your computer)",
    "LAN (your local network)",
    "WAN (the Internet)",
];

pub fn run(id: String, command: NetCommand) -> Result<(), String> {
    let (_, mut c) = crate::connect()?;
    let status: Status = crate::call(&mut c, Method::Status { id: id.clone() })?;
    let mut fw = status.config.firewall.clone();
    match command {
        NetCommand::Show => {
            let net: NetStatus = crate::call(&mut c, Method::Net { id })?;
            show(&fw, &net);
            return Ok(());
        }
        NetCommand::Blocked => {
            let net: NetStatus = crate::call(&mut c, Method::Net { id: id.clone() })?;
            if net.blocked.is_empty() {
                out!(
                    "Nothing blocked in the last 10 minutes{}.",
                    running_note(&status)
                );
            }
            for b in &net.blocked {
                out!("{:>6} ago  {:<4} {}", ago(b.ago), b.zone, b.address);
            }
            return Ok(());
        }
        NetCommand::Allow {
            zone,
            to,
            port,
            proto,
            comment,
        } => {
            zone_mut(&mut fw, zone)
                .allow
                .push(rule(to, port, proto, comment));
        }
        NetCommand::Block {
            zone,
            to,
            port,
            proto,
            comment,
        } => {
            zone_mut(&mut fw, zone)
                .block
                .push(rule(to, port, proto, comment));
        }
        NetCommand::Remove { zone, to, port } => {
            let z = zone_mut(&mut fw, zone);
            let before = z.allow.len() + z.block.len();
            let keep = |r: &Rule| !(r.to == to && (port.is_none() || r.port == port));
            z.allow.retain(keep);
            z.block.retain(keep);
            if z.allow.len() + z.block.len() == before {
                return Err(format!(
                    "no entry {to:?} in that zone (see `cardea net {id} show`)"
                ));
            }
        }
        NetCommand::Mode { zone, mode, yes } => {
            let mode = match mode {
                ModeArg::Allow => ZoneMode::Allow,
                ModeArg::Block => ZoneMode::Block,
            };
            if mode == ZoneMode::Block && zone != ZoneArg::Wan && !yes {
                let what = if zone == ZoneArg::Host {
                    "every service on your computer becomes reachable, except the listed ones"
                } else {
                    "every device on your network becomes reachable, except the listed ones"
                };
                if !crate::confirm(&format!("Risky setting: {what}. Continue?"), false)? {
                    return Err("cancelled (without a terminal, confirm with --yes)".into());
                }
            }
            zone_mut(&mut fw, zone).mode = mode;
        }
        NetCommand::Reject => fw.blocked = Blocked::Reject,
        NetCommand::DropTraffic => fw.blocked = Blocked::Drop,
        NetCommand::On => fw.enabled = true,
        NetCommand::Off { yes } => {
            if !yes
                && !crate::confirm(
                    "Risky setting: the sandbox will reach your whole computer, your whole network and the whole \
                     Internet. Turn the firewall off?",
                    false,
                )?
            {
                return Err("cancelled (without a terminal, confirm with --yes)".into());
            }
            fw.enabled = false;
        }
    }
    let saved: Saved = crate::call(&mut c, Method::SetFirewall { id, firewall: fw })?;
    out!("{}", crate::saved_text(saved));
    Ok(())
}

fn zone_mut(fw: &mut Firewall, zone: ZoneArg) -> &mut Zone {
    match zone {
        ZoneArg::Host => &mut fw.host,
        ZoneArg::Lan => &mut fw.lan,
        ZoneArg::Wan => &mut fw.wan,
    }
}

fn rule(to: String, port: Option<u16>, proto: Option<ProtoArg>, comment: Option<String>) -> Rule {
    let proto = proto.map(|p| match p {
        ProtoArg::Tcp => Proto::Tcp,
        ProtoArg::Udp => Proto::Udp,
    });
    Rule {
        to,
        port,
        proto,
        comment,
    }
}

fn running_note(status: &Status) -> &'static str {
    if status.summary.state == cardea_core::state::State::Running {
        ""
    } else {
        " (the sandbox is not running)"
    }
}

pub fn entry(r: &Rule) -> String {
    let mut s = r.to.clone();
    match (r.port, r.proto) {
        (Some(p), Some(Proto::Tcp)) => s += &format!(" port {p}/tcp"),
        (Some(p), Some(Proto::Udp)) => s += &format!(" port {p}/udp"),
        (Some(p), None) => s += &format!(" port {p}"),
        (None, Some(Proto::Tcp)) => s += " (tcp)",
        (None, Some(Proto::Udp)) => s += " (udp)",
        (None, None) => {}
    }
    if let Some(c) = &r.comment {
        s += &format!("  # {c}");
    }
    s
}

fn ago(s: u64) -> String {
    if s < 60 {
        format!("{s} s")
    } else {
        format!("{} min", s / 60)
    }
}

fn show(fw: &Firewall, net: &NetStatus) {
    let answer = match fw.blocked {
        Blocked::Reject => "refused at once (reject)",
        Blocked::Drop => "dropped silently (drop)",
    };
    if fw.enabled {
        out!("Firewall on · blocked connections are {answer}");
    } else {
        out!(
            "Firewall OFF: the sandbox reaches your computer, your network and the Internet; the lists below do not apply"
        );
    }
    if !net.references.is_empty() {
        let refs: Vec<String> = net
            .references
            .iter()
            .map(|(name, addrs)| {
                format!(
                    "{name} → {}",
                    if addrs.is_empty() {
                        "unresolved (blocks)".to_owned()
                    } else {
                        addrs.join(", ")
                    }
                )
            })
            .collect();
        out!("References: {}", refs.join(" · "));
    }
    for (i, z) in [&fw.host, &fw.lan, &fw.wan].into_iter().enumerate() {
        let (mode, list, empty) = match z.mode {
            ZoneMode::Allow => ("allowlist", &z.allow, "empty: nothing is reachable"),
            ZoneMode::Block => ("blocklist", &z.block, "empty: everything is reachable"),
        };
        out!("");
        out!("{}: {mode} · {} blocked", TITLES[i], net.blocked_count[i]);
        if list.is_empty() {
            out!("  ({empty})");
        }
        for r in list {
            out!("  {}", entry(r));
        }
    }
}
