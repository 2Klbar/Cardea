//! Libpod specs of a sandbox's pod and app container, built from its configuration and what the daemon found on
//! the host. Pure: the daemon resolves paths and shared folders first.
//!
//! Only what is listed here is mounted: never the control socket, `podman.sock` or the runtime directory (R1, R2,
//! R15).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::firewall;
use crate::image::{self, Ids, LABEL};
use crate::model::{Config, KeyringMode, Theme, X11};

/// Capabilities given back to the container's root (used by `cardea exec --root`); the sandbox user runs with
/// none, under `no-new-privileges`.
pub const ROOT_CAPS: [&str; 7] = [
    "CHOWN",
    "DAC_OVERRIDE",
    "FOWNER",
    "FSETID",
    "KILL",
    "SETGID",
    "SETUID",
];

/// Every Linux capability: the app container drops all but `ROOT_CAPS`. Not `cap_drop: ALL` + `cap_add`: Podman
/// gives the capabilities named in `cap_add` to a non-root user too (effective and ambient), so the sandbox user
/// would hold them; what is only left in the bounding set goes to root alone.
const ALL_CAPS: [&str; 41] = [
    "CHOWN",
    "DAC_OVERRIDE",
    "DAC_READ_SEARCH",
    "FOWNER",
    "FSETID",
    "KILL",
    "SETGID",
    "SETUID",
    "SETPCAP",
    "LINUX_IMMUTABLE",
    "NET_BIND_SERVICE",
    "NET_BROADCAST",
    "NET_ADMIN",
    "NET_RAW",
    "IPC_LOCK",
    "IPC_OWNER",
    "SYS_MODULE",
    "SYS_RAWIO",
    "SYS_CHROOT",
    "SYS_PTRACE",
    "SYS_PACCT",
    "SYS_ADMIN",
    "SYS_BOOT",
    "SYS_NICE",
    "SYS_RESOURCE",
    "SYS_TIME",
    "SYS_TTY_CONFIG",
    "MKNOD",
    "LEASE",
    "AUDIT_WRITE",
    "AUDIT_CONTROL",
    "SETFCAP",
    "MAC_OVERRIDE",
    "MAC_ADMIN",
    "SYSLOG",
    "WAKE_ALARM",
    "BLOCK_SUSPEND",
    "AUDIT_READ",
    "PERFMON",
    "BPF",
    "CHECKPOINT_RESTORE",
];

/// IDs of an isolated user namespace: room for the system users of a distribution and the sandbox user (1000),
/// and about 15 isolated sandboxes at once with the usual 65536 sub-IDs.
pub const ISOLATED_IDS: u32 = 4096;

/// `nobody` and `nogroup` in every distribution (the kernel's overflow ID).
pub const NOBODY: u32 = 65534;

/// Container IDs below this are the distribution's system users (root, `_apt`, `messagebus`…).
const SYSTEM_IDS: u32 = 1000;

/// The sub-IDs a Work sandbox takes, counted from 1 in your user namespace: the system users and `nobody`.
pub fn work_ids(ids: Ids) -> u32 {
    ids.uid.max(ids.gid).min(SYSTEM_IDS) + 1
}

/// How many Untrusted sandboxes can run at once with `sub_ids` sub-IDs (Work sandboxes share their few).
pub fn untrusted_limit(sub_ids: u64, ids: Ids) -> u64 {
    sub_ids.saturating_sub(u64::from(work_ids(ids))) / u64::from(ISOLATED_IDS)
}

/// A Work mapping (instead of `keep-id`, which takes every sub-ID you have): you are the sandbox user (container
/// `id` = yours, 0 in your user namespace); the system users below and `nobody` take sub-IDs; nothing else. So
/// Work sandboxes overlap only each other (they are all you), and the rest is left to Untrusted sandboxes.
fn work_map(id: u32) -> Value {
    let system = id.min(SYSTEM_IDS);
    json!([
        { "container_id": 0, "host_id": 1, "size": system },
        { "container_id": id, "host_id": 0, "size": 1 },
        { "container_id": NOBODY, "host_id": system + 1, "size": 1 },
    ])
}

pub fn pod_name(id: &str) -> String {
    format!("cardea-{id}")
}

pub fn netguard_name(id: &str) -> String {
    format!("cardea-{id}-netguard")
}

pub fn app_name(id: &str) -> String {
    format!("cardea-{id}-app")
}

/// A host path mounted into the app container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bind {
    pub source: PathBuf,
    pub destination: String,
    pub read_only: bool,
}

/// What the daemon resolved on the host for a start.
#[derive(Clone, Debug)]
pub struct Host {
    pub ids: Ids,
    /// The persistent home folder; `None` for a disposable home (a fresh volume at each start).
    pub home: Option<PathBuf>,
    /// `state/machine-id`.
    pub machine_id: PathBuf,
    /// Shared folders that exist, each followed by its protected paths (read-only).
    pub folders: Vec<Bind>,
    /// The host's Wayland socket, when the capability is on and the socket exists.
    pub wayland: Option<PathBuf>,
    /// Device nodes of the capabilities that are on and exist on the host (`/dev/dri`, `/dev/kvm`…).
    pub devices: Vec<String>,
    /// `x11: host`: the host's display (`:0`) and a copy of its cookie file.
    pub x11_host: Option<(String, Option<PathBuf>)>,
    /// `keyring.keep` with a disposable home: `state/keyrings`.
    pub keyrings: Option<PathBuf>,
    /// `links.outgoing`: the link FIFO and the guest opener.
    pub links: Option<(PathBuf, PathBuf)>,
}

/// The sandbox's runtime folder: a private tmpfs holding the Wayland socket, the session bus, the X cookie.
pub const XDG_RUNTIME: &str = "/tmp/xdg-runtime";
/// The session bus every app of a sandbox with a keyring shares.
pub const BUS: &str = "unix:path=/tmp/xdg-runtime/bus";
/// The display of the private Xwayland (`x11: isolated`).
pub const ISOLATED_DISPLAY: &str = ":1";

/// The address that leads to your computer (`host.containers.internal`), and the DNS forwarder: Podman's own.
pub const HOST_ALIAS: &str = "169.254.1.2";
pub const DNS_FORWARDER: &str = "169.254.1.1";

/// Pasta options: fixed neutral addresses and interface name instead of copies of the host's, so nothing inside
/// names the host's network. Podman adds `--no-map-gw` (the gateway address never reaches the host) and the DNS
/// forwarder. The host alias maps to the host's loopback: Podman's default `--map-guest-addr` leads to "the
/// guest's address on the host", which with a neutral address is no address of the host (self-test 2026-11-22).
/// shortcut: a host service listening only on a LAN address is not reached; upgrade if a user needs one.
pub const PASTA: [&str; 16] = [
    "--ns-ifname",
    "eth0",
    "-a",
    "10.249.0.2",
    "-n",
    "30",
    "-g",
    "10.249.0.1",
    "-a",
    "fd63:6172:6465:6100::2",
    "-g",
    "fd63:6172:6465:6100::1",
    "--map-guest-addr",
    "none",
    "--map-host-loopback",
    HOST_ALIAS,
];

/// The pod: hostname, user namespace, and its own network namespace through pasta, filtered by the netguard
/// sidecar.
pub fn pod(c: &Config, ids: Ids) -> Value {
    let mut pod = json!({
        "name": pod_name(&c.id),
        "hostname": c.identity.hostname.as_deref().unwrap_or(&c.id),
        "labels": { LABEL: c.id },
        "netns": { "nsmode": "pasta" },
        "network_options": { "pasta": PASTA },
        // Only the forwarder and no search domain: the host's servers and domain would name its network.
        "dns_server": [DNS_FORWARDER],
        "dns_search": ["."],
        // Podman would name the host by its LAN address once `--map-guest-addr` is off; first entry wins.
        "hostadd": [format!("host.containers.internal;host.docker.internal:{HOST_ALIAS}")],
        // The containers share the pod's IPC namespace, so its /dev/shm: their own size would be ignored.
        "shm_size": 1u64 << 30,
        "userns": if c.security.isolated_uid {
            // An explicit size: by default Podman sizes the range from the image's highest ID (nogroup, 65534),
            // more than a user's 65536 sub-IDs once they are shared.
            json!({ "nsmode": "auto", "value": format!("size={ISOLATED_IDS}") })
        } else {
            json!({ "nsmode": "private" })
        },
    });
    if !c.security.isolated_uid {
        pod["idmappings"] = json!({ "UIDMap": work_map(ids.uid), "GIDMap": work_map(ids.gid) });
    }
    pod
}

/// The netguard sidecar: applies the drop-all ruleset as soon as it starts, then waits; the daemon pushes the real
/// rules into `rules_dir` (`state/netguard/`) and runs `nft` here. The only container with `NET_ADMIN` (R7).
pub fn netguard(c: &Config, rules_dir: &Path) -> Value {
    json!({
        "name": netguard_name(&c.id),
        "pod": pod_name(&c.id),
        "image": firewall::NETGUARD_TAG,
        "command": ["sh", "-c", "nft -f /etc/netguard/drop-all.nft && exec sleep 2147483647"],
        "user": "0:0",
        "labels": { LABEL: c.id },
        "cap_drop": ["ALL"],
        "cap_add": ["NET_ADMIN"],
        "no_new_privileges": true,
        "read_only_filesystem": true,
        "stop_timeout": 2,
        "mounts": [{
            "type": "bind",
            "source": rules_dir,
            "destination": firewall::RULES_DIR,
            "options": ["rbind", "ro"],
        }],
    })
}

/// The app container: a long-running `sleep` under an init, where apps and shells are executed.
pub fn app(c: &Config, h: &Host) -> Value {
    let home = &c.identity.home_mount;
    let bind = |source: &PathBuf, destination: &str, read_only: bool| {
        json!({
            "type": "bind",
            "source": source,
            "destination": destination,
            "options": ["rbind", if read_only { "ro" } else { "rw" }],
        })
    };
    let mut mounts = vec![bind(&h.machine_id, "/etc/machine-id", true)];
    let mut volumes = vec![];
    match &h.home {
        Some(path) => {
            let mut m = bind(path, home, false);
            if c.security.isolated_uid {
                // `U`: owned by the isolated user. shortcut: the host user then cannot delete these files without
                // `podman unshare`; upgrade with idmapped mounts when every supported Podman has them.
                m["options"] = json!(["rbind", "rw", "U"]);
            }
            mounts.push(m);
        }
        // An anonymous volume, removed with the container; `U` gives it to the sandbox user.
        None => {
            volumes.push(json!({ "Name": "", "Dest": home, "Options": ["U"], "IsAnonymous": true }))
        }
    }
    mounts.extend(
        h.folders
            .iter()
            .map(|b| bind(&b.source, &b.destination, b.read_only)),
    );
    let hm = home.trim_end_matches('/');
    // The home's tool folders first: what home sources install (npm, cargo, mise, the Claude Code installer).
    // Login shells get the same folders back from /etc/profile.d/cardea.sh (guest/add-user.sh).
    let path = format!(
        "{hm}/.local/bin:{hm}/.cargo/bin:{hm}/.npm-global/bin:{hm}/.local/share/mise/shims:/usr/local/sbin:/usr/local/bin:\
         /usr/sbin:/usr/bin:/sbin:/bin"
    );
    let locales = image::locales(c);
    let mut env = json!({
        "HOME": home,
        "USER": c.identity.user,
        "XDG_RUNTIME_DIR": XDG_RUNTIME,
        "PATH": path,
        "LANG": locales.first().map_or("C.UTF-8", String::as_str),
        "LANGUAGE": c.appearance.languages.join(":"),
    });
    if c.appearance.theme == Theme::Dark {
        env["GTK_THEME"] = json!("Adwaita:dark");
    }
    mounts.push(json!({
        "type": "tmpfs",
        "source": "tmpfs",
        "destination": XDG_RUNTIME,
        // `U`: Podman gives it to the container's user (it refuses `uid=`/`gid=` options).
        "options": ["rw", "nosuid", "nodev", "mode=0700", "U"],
    }));
    if let Some(socket) = &h.wayland {
        mounts.push(bind(socket, &format!("{XDG_RUNTIME}/wayland-0"), false));
        env["WAYLAND_DISPLAY"] = json!("wayland-0");
    }
    match (c.capabilities.x11, &h.x11_host) {
        (X11::Isolated, _) => env["DISPLAY"] = json!(ISOLATED_DISPLAY),
        (X11::Host, Some((display, cookie))) => {
            mounts.push(bind(
                &PathBuf::from("/tmp/.X11-unix"),
                "/tmp/.X11-unix",
                true,
            ));
            env["DISPLAY"] = json!(display);
            if let Some(cookie) = cookie {
                mounts.push(bind(cookie, &format!("{XDG_RUNTIME}/Xauthority"), true));
                env["XAUTHORITY"] = json!(format!("{XDG_RUNTIME}/Xauthority"));
            }
        }
        _ => {}
    }
    if c.capabilities.keyring.mode != KeyringMode::Off {
        env["DBUS_SESSION_BUS_ADDRESS"] = json!(BUS);
    }
    if let Some(dir) = &h.keyrings {
        mounts.push(bind(
            dir,
            &format!("{}/.local/share/keyrings", home.trim_end_matches('/')),
            false,
        ));
    }
    if let Some((fifo, opener)) = &h.links {
        mounts.push(bind(fifo, crate::links::FIFO_MOUNT, false));
        mounts.push(bind(opener, crate::links::OPENER_MOUNT, true));
        env["BROWSER"] = json!(crate::links::OPENER_MOUNT);
    }
    let mut spec = json!({
        "name": app_name(&c.id),
        "pod": pod_name(&c.id),
        "image": image::tag(&c.id),
        "command": ["sleep", "infinity"],
        "init": true,
        "user": format!("{}:{}", h.ids.uid, h.ids.gid),
        "work_dir": home,
        "env": env,
        "labels": { LABEL: c.id },
        "cap_drop": ALL_CAPS.iter().filter(|c| !ROOT_CAPS.contains(c)).collect::<Vec<_>>(),
        // Rootless, /sys is the host's: hide its network interfaces (names, MAC addresses). Apps list theirs
        // through netlink, which shows the sandbox's own.
        "mask": ["/sys/class/net", "/sys/devices/virtual/net"],
        "no_new_privileges": true,
        "timezone": "local",
        "stop_timeout": 5,
        "mounts": mounts,
        "volumes": volumes,
    });
    if let Some(gb) = c.security.ram_limit_gb {
        spec["resource_limits"] = json!({ "memory": { "limit": u64::from(gb) << 30 } });
    }
    if !h.devices.is_empty() {
        spec["devices"] = json!(
            h.devices
                .iter()
                .map(|d| json!({ "path": d }))
                .collect::<Vec<_>>()
        );
    }
    if h.devices.iter().any(|d| d == "/dev/dri") {
        // The host user's video and render groups, for the GPU: what `--group-add keep-groups` does in the CLI
        // (the API takes no `keep-groups` group: it looks it up as a name). crun only.
        spec["annotations"] = json!({ "run.oci.keep_original_groups": "1" });
    }
    if !c.security.seccomp {
        spec["seccomp_profile_path"] = json!("unconfined");
    }
    spec
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin;
    use crate::model::Trust;

    fn host(home: Option<&str>) -> Host {
        Host {
            ids: Ids {
                uid: 1000,
                gid: 1000,
            },
            home: home.map(PathBuf::from),
            machine_id: "/s/box/state/machine-id".into(),
            folders: vec![
                Bind {
                    source: "/p/site".into(),
                    destination: "/work/site".into(),
                    read_only: false,
                },
                Bind {
                    source: "/p/site/.git/hooks".into(),
                    destination: "/work/site/.git/hooks".into(),
                    read_only: true,
                },
            ],
            wayland: Some("/run/user/1000/wayland-0".into()),
            devices: vec![],
            x11_host: None,
            keyrings: None,
            links: None,
        }
    }

    fn config() -> Config {
        let mut c = builtin::profile("empty").unwrap();
        c.id = "box".into();
        c
    }

    #[test]
    fn work_sandbox() {
        let c = config();
        let pod = pod(&c, host(None).ids);
        assert_eq!(pod["name"], "cardea-box");
        assert_eq!(pod["hostname"], "box");
        assert_eq!(pod["shm_size"], 1u64 << 30);
        assert_eq!(pod["netns"]["nsmode"], "pasta");
        assert_eq!(pod["network_options"]["pasta"], json!(PASTA));
        assert_eq!(pod["dns_search"], json!(["."]));
        assert_eq!(pod["dns_server"], json!(["169.254.1.1"]));
        assert_eq!(
            pod["hostadd"],
            json!(["host.containers.internal;host.docker.internal:169.254.1.2"])
        );
        let pasta = &pod["network_options"]["pasta"];
        assert!(
            pasta
                .as_array()
                .unwrap()
                .windows(2)
                .any(|w| w[0] == "--map-host-loopback" && w[1] == "169.254.1.2")
        );
        assert!(
            pasta
                .as_array()
                .unwrap()
                .windows(2)
                .any(|w| w[0] == "--map-guest-addr" && w[1] == "none")
        );
        // Only the IDs a Work sandbox uses: 0–999, you (1000 = your own UID), nobody.
        assert_eq!(pod["userns"]["nsmode"], "private");
        assert_eq!(
            pod["idmappings"]["UIDMap"],
            json!([
                { "container_id": 0, "host_id": 1, "size": 1000 },
                { "container_id": 1000, "host_id": 0, "size": 1 },
                { "container_id": 65534, "host_id": 1001, "size": 1 },
            ])
        );
        assert_eq!(pod["idmappings"]["GIDMap"], pod["idmappings"]["UIDMap"]);
        let app = app(&c, &host(Some("/s/box/home")));
        assert_eq!(app["user"], "1000:1000");
        assert_eq!(app["image"], "localhost/cardea/box");
        // Nothing added (it would reach the user); everything but root's set dropped.
        assert!(app.get("cap_add").is_none());
        let dropped = app["cap_drop"].as_array().unwrap();
        assert_eq!(dropped.len(), ALL_CAPS.len() - ROOT_CAPS.len());
        assert!(dropped.iter().any(|c| c == "NET_ADMIN"));
        assert_eq!(app["no_new_privileges"], true);
        assert_eq!(
            app["mask"],
            json!(["/sys/class/net", "/sys/devices/virtual/net"])
        );
        let mounts: Vec<(String, String, String)> = app["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["source"].as_str().unwrap().into(),
                    m["destination"].as_str().unwrap().into(),
                    m["options"][1].as_str().unwrap().into(),
                )
            })
            .collect();
        let m = |s: &str, d: &str, o: &str| (s.to_owned(), d.to_owned(), o.to_owned());
        assert_eq!(
            mounts,
            [
                m("/s/box/state/machine-id", "/etc/machine-id", "ro"),
                m("/s/box/home", "/home/sandbox", "rw"),
                m("/p/site", "/work/site", "rw"),
                m("/p/site/.git/hooks", "/work/site/.git/hooks", "ro"),
                m("tmpfs", "/tmp/xdg-runtime", "nosuid"),
                m(
                    "/run/user/1000/wayland-0",
                    "/tmp/xdg-runtime/wayland-0",
                    "rw"
                ),
            ]
        );
        assert_eq!(app["volumes"], json!([]));
        assert!(app.get("seccomp_profile_path").is_none() && app.get("resource_limits").is_none());
    }

    #[test]
    fn untrusted_disposable_limited() {
        let mut c = config();
        c.trust = Trust::Untrusted;
        c.security.isolated_uid = true;
        c.security.seccomp = false;
        c.security.ram_limit_gb = Some(4);
        c.identity.hostname = Some("lab".into());
        assert_eq!(pod(&c, host(None).ids)["userns"]["nsmode"], "auto");
        assert_eq!(pod(&c, host(None).ids)["userns"]["value"], "size=4096");
        assert!(pod(&c, host(None).ids).get("idmappings").is_none());
        assert_eq!(pod(&c, host(None).ids)["hostname"], "lab");
        let app = app(&c, &host(None));
        assert_eq!(app["volumes"][0]["Dest"], "/home/sandbox");
        assert!(
            app["mounts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["destination"] != "/home/sandbox")
        );
        assert_eq!(app["resource_limits"]["memory"]["limit"], 4u64 << 30);
        assert_eq!(app["seccomp_profile_path"], "unconfined");
    }

    #[test]
    fn capabilities() {
        let mut c = config();
        c.capabilities.keyring.mode = KeyringMode::Auto;
        c.capabilities.x11 = X11::Host;
        let mut h = host(None);
        h.devices = vec!["/dev/dri".into(), "/dev/kvm".into()];
        h.x11_host = Some((":0".into(), Some("/s/box/state/xauthority".into())));
        h.keyrings = Some("/s/box/state/keyrings".into());
        h.links = Some((
            "/run/user/1000/cardea/links-box.fifo".into(),
            "/s/box/state/xdg-open".into(),
        ));
        let app = app(&c, &h);
        let link = app["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["destination"] == "/run/cardea/links")
            .unwrap();
        assert_eq!(link["options"][1], "rw");
        assert_eq!(app["env"]["BROWSER"], "/usr/local/bin/xdg-open");
        let dest = |d: &str| {
            app["mounts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["destination"] == d)
                .cloned()
        };
        assert_eq!(dest("/tmp/xdg-runtime").unwrap()["type"], "tmpfs");
        // Podman refuses uid=/gid= on a tmpfs: the owner comes from `U`.
        assert_eq!(
            dest("/tmp/xdg-runtime").unwrap()["options"],
            json!(["rw", "nosuid", "nodev", "mode=0700", "U"])
        );
        assert_eq!(
            dest("/tmp/xdg-runtime/wayland-0").unwrap()["source"],
            "/run/user/1000/wayland-0"
        );
        assert_eq!(dest("/tmp/.X11-unix").unwrap()["options"][1], "ro");
        assert_eq!(
            dest("/home/sandbox/.local/share/keyrings").unwrap()["source"],
            "/s/box/state/keyrings"
        );
        assert_eq!(app["env"]["WAYLAND_DISPLAY"], "wayland-0");
        assert_eq!(app["env"]["LANG"], "C.UTF-8");
        assert!(
            app["env"]["PATH"]
                .as_str()
                .unwrap()
                .starts_with("/home/sandbox/.local/bin:")
        );
        assert_eq!(app["env"]["DISPLAY"], ":0");
        assert_eq!(app["env"]["DBUS_SESSION_BUS_ADDRESS"], BUS);
        assert_eq!(
            app["devices"],
            json!([{ "path": "/dev/dri" }, { "path": "/dev/kvm" }])
        );
        assert_eq!(app["annotations"]["run.oci.keep_original_groups"], "1");
        // The runtime folder never is the host's.
        assert!(!app.to_string().contains("\"/run/user/1000\""));
    }

    #[test]
    fn only_the_sidecar_has_net_admin() {
        let c = config();
        let app = app(&c, &host(Some("/s/box/home")));
        assert!(
            app["cap_drop"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c == "NET_ADMIN")
        );
        let ng = netguard(&c, Path::new("/s/box/state/netguard"));
        assert_eq!(ng["cap_add"], json!(["NET_ADMIN"]));
        assert_eq!(ng["pod"], "cardea-box");
        assert_eq!(ng["mounts"][0]["options"], json!(["rbind", "ro"]));
        assert!(
            ng["command"][2]
                .as_str()
                .unwrap()
                .starts_with("nft -f /etc/netguard/drop-all.nft && ")
        );
    }

    #[test]
    fn no_socket_is_ever_mounted() {
        let app = app(&config(), &host(Some("/s/box/home"))).to_string();
        for bad in ["podman.sock", "control.sock"] {
            assert!(!app.contains(bad), "{bad}");
        }
        // From the runtime folder, only the Wayland socket file (R15).
        assert_eq!(app.matches("/run/user/").count(), 1);
        assert!(app.contains("\"/run/user/1000/wayland-0\""));
    }

    #[test]
    fn work_mapping_leaves_room_for_untrusted_sandboxes() {
        let ids = Ids {
            uid: 1000,
            gid: 1000,
        };
        assert_eq!(work_ids(ids), 1001);
        // The usual 65536 sub-IDs: 15 Untrusted sandboxes besides any number of Work ones.
        assert_eq!(untrusted_limit(65536, ids), 15);
        assert_eq!(untrusted_limit(0, ids), 0);
        // A UID below 1000: the system range stops below it.
        assert_eq!(
            work_map(500),
            json!([
                { "container_id": 0, "host_id": 1, "size": 500 },
                { "container_id": 500, "host_id": 0, "size": 1 },
                { "container_id": 65534, "host_id": 501, "size": 1 },
            ])
        );
    }
}
