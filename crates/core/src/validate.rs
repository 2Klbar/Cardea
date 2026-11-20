//! Validation of configs, settings and sources, with every problem reported at once (security R5, R8, R9,
//! R10, R15).
//!
//! Each [`Issue`] names the field (`folders[0].mount`) and says what is wrong and what is accepted.

use std::collections::HashSet;
use std::fmt;
use std::net::IpAddr;

use crate::builtin;
use crate::model::{
    Autostart, Capability, Config, Kind, Rule, SCHEMA, Settings, Source, Toolkit, X11,
};

/// One problem in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// Field path, such as `firewall.lan.allow[0].to`.
    pub path: String,
    pub message: String,
}

/// Every problem found in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid(pub Vec<Issue>);

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, issue) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "  - {}: {}", issue.path, issue.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for Invalid {}

/// Reference names accepted in firewall entries.
pub const REFERENCES: [&str; 3] = ["host", "gateway", "dns"];

/// Paths of the image that mount points cannot be, contain or be inside (R15), plus the runtime folder used
/// for the Wayland socket.
const RESERVED_MOUNTS: [&str; 12] = [
    "/usr",
    "/etc",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/proc",
    "/sys",
    "/dev",
    "/tmp/xdg-runtime",
];

#[derive(Default)]
struct Report(Vec<Issue>);

impl Report {
    fn add(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.0.push(Issue {
            path: path.into(),
            message: message.into(),
        });
    }

    fn check(&mut self, path: impl Into<String>, result: Result<(), String>) {
        if let Err(message) = result {
            self.add(path, message);
        }
    }

    fn finish(self) -> Result<(), Invalid> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Invalid(self.0))
        }
    }
}

/// Validates a sandbox config or a profile.
pub fn config(c: &Config, kind: Kind) -> Result<(), Invalid> {
    let mut r = Report::default();
    let profile = kind == Kind::Profile;
    let local_only =
        |what: &str| format!("not allowed in a profile ({what} is local to a sandbox)");

    r.check("schema", schema(c.schema));
    r.check("id", id(&c.id));
    r.check("name", display_name(&c.name));
    if let Some(d) = &c.description {
        r.check("description", text(d, 500, true));
    }
    if let Some(icon) = &c.icon {
        let mut letters = icon.letter.chars();
        if !matches!((letters.next(), letters.next()), (Some(l), None) if l.is_alphanumeric()) {
            r.add(
                "icon.letter",
                format!("{:?} must be a single letter or digit", icon.letter),
            );
        }
        r.check("icon.color", color(&icon.color));
    }
    match (&c.profile, profile) {
        (Some(_), true) => r.add("profile", local_only("the source profile")),
        (Some(p), false) => r.check("profile", id(p)),
        (None, _) => {}
    }
    r.check("base_image", image_ref(&c.base_image));

    // Identity and home.
    r.check("identity.user", user(&c.identity.user));
    r.check("identity.home_mount", mount_point(&c.identity.home_mount));
    match (&c.identity.hostname, profile) {
        (Some(_), true) => r.add("identity.hostname", local_only("the hostname")),
        (Some(h), false) => r.check("identity.hostname", hostname(h)),
        (None, _) => {}
    }
    match (&c.home.path, profile) {
        (Some(_), true) => r.add("home.path", local_only("a host path")),
        (Some(p), false) => r.check("home.path", host_path(p)),
        (None, _) => {}
    }

    // Shared folders.
    if profile && !c.folders.is_empty() {
        r.add("folders", local_only("a shared folder"));
    }
    let mut mounts = HashSet::new();
    let mut names = HashSet::new();
    for (i, f) in c.folders.iter().enumerate() {
        let at = format!("folders[{i}]");
        r.check(format!("{at}.host"), host_path(&f.host));
        let ok = mount_point(&f.mount);
        let valid = ok.is_ok();
        r.check(format!("{at}.mount"), ok);
        if !valid {
            continue;
        }
        let home = &c.identity.home_mount;
        if !mounts.insert(f.mount.as_str()) {
            r.add(
                format!("{at}.mount"),
                format!("{:?} is already used by another shared folder", f.mount),
            );
        } else if within(home, &f.mount) {
            r.add(
                format!("{at}.mount"),
                format!("{:?} would cover the home folder {home:?}; mount it elsewhere or inside the home", f.mount),
            );
        }
        if !names.insert(f.name()) {
            r.add(
                format!("{at}.mount"),
                format!(
                    "another shared folder is also named {:?} (last part of the mount point); names must differ",
                    f.name()
                ),
            );
        }
    }
    let mut seen = HashSet::new();
    for (i, n) in c.protected_paths.names.iter().enumerate() {
        r.check(format!("protected_paths.names[{i}]"), relative_path(n));
        if !seen.insert(n) {
            r.add(
                format!("protected_paths.names[{i}]"),
                format!("{n:?} is listed twice"),
            );
        }
    }
    if c.protected_paths.depth > 10 {
        r.add(
            "protected_paths.depth",
            format!("{} is too deep: use 0 to 10", c.protected_paths.depth),
        );
    }

    // Security.
    if let Some(gb) = c
        .security
        .ram_limit_gb
        .filter(|gb| !(1..=1024).contains(gb))
    {
        r.add(
            "security.ram_limit_gb",
            format!("{gb} is out of range: use 1 to 1024, or null for no limit"),
        );
    }

    // Firewall.
    for (zone, z) in [
        ("host", &c.firewall.host),
        ("lan", &c.firewall.lan),
        ("wan", &c.firewall.wan),
    ] {
        for (list, rules) in [("allow", &z.allow), ("block", &z.block)] {
            for (i, rule) in rules.iter().enumerate() {
                firewall_rule(
                    &mut r,
                    &format!("firewall.{zone}.{list}[{i}]"),
                    rule,
                    profile,
                );
            }
        }
    }

    // Sources and packages.
    let mut source_ids: HashSet<&str> = builtin::SOURCE_IDS.into_iter().collect();
    for (i, s) in c.sources.iter().enumerate() {
        let at = format!("sources[{i}]");
        source_fields(&mut r, &at, s);
        if builtin::SOURCE_IDS.contains(&s.id.as_str()) {
            r.add(
                format!("{at}.id"),
                format!("{:?} is a built-in source; choose another id", s.id),
            );
        } else if !source_ids.insert(&s.id) {
            r.add(
                format!("{at}.id"),
                format!("{:?} is used by another source", s.id),
            );
        }
    }
    let mut repos = HashSet::new();
    for (i, a) in c.apt_repos.iter().enumerate() {
        let at = format!("apt_repos[{i}]");
        r.check(format!("{at}.name"), id(&a.name));
        if !repos.insert(&a.name) {
            r.add(
                format!("{at}.name"),
                format!("{:?} is used by another repository", a.name),
            );
        }
        r.check(format!("{at}.url"), https_url(&a.url));
        r.check(format!("{at}.key_url"), https_url(&a.key_url));
        r.check(format!("{at}.suite"), apt_word(&a.suite, true));
        if a.components.is_empty() {
            r.add(
                format!("{at}.components"),
                "at least one component is needed, such as \"main\"",
            );
        }
        for (j, comp) in a.components.iter().enumerate() {
            r.check(format!("{at}.components[{j}]"), apt_word(comp, false));
        }
        r.check(format!("{at}.fingerprint"), fingerprint(&a.fingerprint));
    }
    for (i, p) in c.packages.iter().enumerate() {
        let at = format!("packages[{i}]");
        if !source_ids.contains(p.source.as_str()) {
            let mut known: Vec<&str> = source_ids.iter().copied().collect();
            known.sort_unstable();
            r.add(
                format!("{at}.source"),
                format!("unknown source {:?}; known: {}", p.source, known.join(", ")),
            );
        }
        r.check(format!("{at}.name"), package_name(&p.name));
        if let Some(v) = &p.version {
            r.check(format!("{at}.version"), version(v));
        }
    }
    r.check("setup.build", script(&c.setup.build));
    r.check("setup.first_start", script(&c.setup.first_start));

    // Apps.
    let mut ids = HashSet::new();
    for (i, a) in c.apps.iter().enumerate() {
        let at = format!("apps[{i}]");
        r.check(format!("{at}.id"), id(&a.id));
        if !ids.insert(&a.id) {
            r.add(
                format!("{at}.id"),
                format!("{:?} is used by another app", a.id),
            );
        }
        r.check(format!("{at}.name"), display_name(&a.name));
        argv(&mut r, &format!("{at}.exec"), &a.exec);
        if a.exec.iter().any(|arg| arg.contains("{folder}")) {
            r.add(
                format!("{at}.exec"),
                "{folder} is only replaced in per-folder services",
            );
        }
        env(&mut r, &format!("{at}.env"), a.env.iter());
        let needs_x11 = a.toolkit == Toolkit::X11 || a.wants.contains(&Capability::X11);
        if needs_x11 && c.capabilities.x11 == X11::Off {
            r.add(
                format!("{at}.toolkit"),
                format!(
                    "app {:?} needs X11 but capabilities.x11 is \"off\"; set it to \"isolated\" or run the app on \
                     Wayland (see the desktop compatibility guide)",
                    a.id
                ),
            );
        }
    }

    // Services.
    let mut ids = HashSet::new();
    for (i, s) in c.services.iter().enumerate() {
        let at = format!("services[{i}]");
        r.check(format!("{at}.id"), id(&s.id));
        if !ids.insert(&s.id) {
            r.add(
                format!("{at}.id"),
                format!("{:?} is used by another service", s.id),
            );
        }
        r.check(format!("{at}.name"), display_name(&s.name));
        argv(&mut r, &format!("{at}.exec"), &s.exec);
        for (j, arg) in s.exec.iter().enumerate() {
            if arg.contains("{folder}") && (arg != "{folder}" || !s.per_folder) {
                let why = if s.per_folder {
                    "{folder} must be a whole argument, alone"
                } else {
                    "{folder} is only replaced when per_folder is true"
                };
                r.add(format!("{at}.exec[{j}]"), why);
            }
        }
        env(&mut r, &format!("{at}.env"), s.env.iter());
        match (&s.autostart, profile, s.per_folder) {
            (None, _, _) => {}
            (Some(_), true, _) => r.add(format!("{at}.autostart"), local_only("service autostart")),
            (Some(Autostart::Single(_)), false, true) => r.add(
                format!("{at}.autostart"),
                "a per-folder service autostarts a list of folder names, such as [\"my-project\"]",
            ),
            (Some(Autostart::Folders(_)), false, false) => r.add(
                format!("{at}.autostart"),
                "a single-instance service autostarts with true or false",
            ),
            (Some(Autostart::Folders(list)), false, true) => {
                for (j, name) in list.iter().enumerate() {
                    if !c.folders.iter().any(|f| f.name() == name) {
                        r.add(
                            format!("{at}.autostart[{j}]"),
                            format!("no shared folder is named {name:?}"),
                        );
                    }
                }
            }
            (Some(Autostart::Single(_)), false, false) => {}
        }
    }

    // Appearance.
    if c.appearance.languages.is_empty() {
        r.add(
            "appearance.languages",
            "at least one language is needed, such as \"en\"",
        );
    }
    for (i, l) in c.appearance.languages.iter().enumerate() {
        r.check(format!("appearance.languages[{i}]"), language(l));
    }
    if profile && c.autostart.is_some() {
        r.add("autostart", local_only("start at login"));
    }
    r.finish()
}

/// Validates tool settings.
pub fn settings(s: &Settings) -> Result<(), Invalid> {
    let mut r = Report::default();
    r.check("schema", schema(s.schema));
    if let Some(loc) = &s.sandbox_location {
        let result = match loc.strip_prefix("~/") {
            Some(rest) => relative_path(rest),
            None => host_path(loc),
        };
        r.check("sandbox_location", result);
    }
    if let Some(term) = &s.terminal {
        argv(&mut r, "terminal", term);
    }
    r.finish()
}

/// Validates a source file (custom or built-in).
pub fn source(s: &Source) -> Result<(), Invalid> {
    let mut r = Report::default();
    source_fields(&mut r, "", s);
    r.finish()
}

fn source_fields(r: &mut Report, at: &str, s: &Source) {
    let field = |name: &str| {
        if at.is_empty() {
            name.to_owned()
        } else {
            format!("{at}.{name}")
        }
    };
    r.check(field("id"), id(&s.id));
    r.check(field("name"), display_name(&s.name));
    r.check(field("setup"), script(&s.setup));
    for (name, template) in [
        ("install", &s.install),
        ("remove", &s.remove),
        ("update", &s.update),
        ("list", &s.list),
    ] {
        if template.trim().is_empty() {
            r.add(field(name), "a command is needed");
        } else {
            r.check(field(name), script(template));
        }
    }
}

fn firewall_rule(r: &mut Report, at: &str, rule: &Rule, profile: bool) {
    let to = rule.to.as_str();
    if !REFERENCES.contains(&to) {
        match ip_or_cidr(to) {
            Ok(()) if profile => r.add(
                format!("{at}.to"),
                format!(
                    "{to:?} is an IP address; profiles only use references ({})",
                    REFERENCES.join(", ")
                ),
            ),
            Ok(()) => {}
            Err(why) => r.add(
                format!("{at}.to"),
                format!(
                    "{to:?} is not a reference ({}), an IP address or a CIDR: {why}",
                    REFERENCES.join(", ")
                ),
            ),
        }
    }
    if rule.port == Some(0) {
        r.add(
            format!("{at}.port"),
            "0 is not a port: use 1 to 65535, or leave it out for every port",
        );
    }
    if let Some(c) = &rule.comment {
        r.check(format!("{at}.comment"), text(c, 200, false));
    }
}

fn argv(r: &mut Report, at: &str, args: &[String]) {
    match args.first() {
        None => r.add(
            at,
            "the command is empty: give the program and its arguments, such as [\"freecad\"]",
        ),
        Some(p) if p.trim().is_empty() => r.add(format!("{at}[0]"), "the program name is empty"),
        Some(_) => {}
    }
    for (i, a) in args.iter().enumerate() {
        if a.chars().any(char::is_control) {
            r.add(
                format!("{at}[{i}]"),
                "control characters (newline, tab…) are not allowed in arguments",
            );
        }
    }
}

fn env<'a>(r: &mut Report, at: &str, vars: impl Iterator<Item = (&'a String, &'a String)>) {
    for (k, v) in vars {
        let valid = k
            .chars()
            .enumerate()
            .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()));
        if k.is_empty() || !valid {
            r.add(
                format!("{at}.{k}"),
                format!("{k:?} is not a variable name: use letters, digits and '_'"),
            );
        }
        if v.contains('\0') || v.contains('\n') {
            r.add(
                format!("{at}.{k}"),
                "the value cannot contain a newline or a NUL character",
            );
        }
    }
}

fn schema(n: u32) -> Result<(), String> {
    if n == SCHEMA {
        Ok(())
    } else {
        Err(format!(
            "schema {n} is not supported (this version reads schema {SCHEMA})"
        ))
    }
}

/// Ids of sandboxes, profiles, apps, services, sources and repositories.
pub fn id(s: &str) -> Result<(), String> {
    let chars_ok = s
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if s.is_empty() || s.len() > 32 || !chars_ok || s.starts_with('-') || s.ends_with('-') {
        return Err(format!(
            "{s:?} is not a valid id: use 1 to 32 lowercase letters, digits and '-', not starting or ending with '-'"
        ));
    }
    Ok(())
}

fn display_name(s: &str) -> Result<(), String> {
    if s.trim().is_empty() {
        return Err("the name is empty".into());
    }
    if s.trim() != s {
        return Err(format!("{s:?} starts or ends with spaces"));
    }
    text(s, 64, false)
}

fn text(s: &str, max: usize, multiline: bool) -> Result<(), String> {
    if s.chars().count() > max {
        return Err(format!("too long: at most {max} characters"));
    }
    if s.chars()
        .any(|c| c.is_control() && !(multiline && c == '\n'))
    {
        return Err("control characters are not allowed".into());
    }
    Ok(())
}

fn script(s: &str) -> Result<(), String> {
    if s.contains('\0') {
        return Err("NUL characters are not allowed".into());
    }
    if s.len() > 64 * 1024 {
        return Err("too long: at most 64 KiB".into());
    }
    Ok(())
}

fn color(s: &str) -> Result<(), String> {
    match s.strip_prefix('#') {
        Some(hex) if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) => Ok(()),
        _ => Err(format!(
            "{s:?} is not a colour: use #rrggbb, such as \"#c8643b\""
        )),
    }
}

fn user(s: &str) -> Result<(), String> {
    let valid = s.chars().enumerate().all(|(i, c)| {
        c.is_ascii_lowercase() || c == '_' || (i > 0 && (c.is_ascii_digit() || c == '-'))
    });
    if s.is_empty() || s.len() > 32 || !valid {
        return Err(format!(
            "{s:?} is not a valid user name: use 1 to 32 lowercase letters, digits, '_' and '-', starting with a letter"
        ));
    }
    if s == "root" {
        return Err("the sandbox user cannot be root".into());
    }
    Ok(())
}

fn hostname(s: &str) -> Result<(), String> {
    let valid = s
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if s.is_empty() || s.len() > 63 || !valid || s.starts_with('-') || s.ends_with('-') {
        return Err(format!(
            "{s:?} is not a valid hostname: use 1 to 63 lowercase letters, digits and '-', not starting or ending \
             with '-'"
        ));
    }
    Ok(())
}

/// A clean absolute path: no `.`, `..`, empty component, trailing `/`, `:` or control character.
fn absolute_path(s: &str) -> Result<(), String> {
    if !s.starts_with('/') {
        return Err(format!(
            "{s:?} is not an absolute path (it must start with '/')"
        ));
    }
    if s == "/" {
        return Err("\"/\" (the whole file system) is not allowed".into());
    }
    if s.len() > 4096 || s.chars().any(|c| c.is_control() || c == ':') {
        return Err(format!(
            "{s:?} contains a forbidden character (':' or a control character)"
        ));
    }
    if s[1..]
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!(
            "{s:?} is not a clean path: no '.', '..', '//' or trailing '/'"
        ));
    }
    Ok(())
}

fn host_path(s: &str) -> Result<(), String> {
    absolute_path(s)
}

/// A mount point inside the sandbox (R15).
fn mount_point(s: &str) -> Result<(), String> {
    absolute_path(s)?;
    if let Some(sys) = RESERVED_MOUNTS
        .iter()
        .find(|sys| within(s, sys) || within(sys, s))
    {
        return Err(format!(
            "{s:?} is, contains or is inside the system folder {sys}; mount points cannot touch it"
        ));
    }
    Ok(())
}

/// `path` is `base` or inside it.
fn within(path: &str, base: &str) -> bool {
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn relative_path(s: &str) -> Result<(), String> {
    if s.is_empty() || s.starts_with('/') {
        return Err(format!(
            "{s:?} must be a relative path, such as \".git/hooks\""
        ));
    }
    if s.chars().any(char::is_control)
        || s.split('/').any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(format!(
            "{s:?} is not a clean relative path: no '.', '..', '//' or control characters"
        ));
    }
    Ok(())
}

fn ip_or_cidr(s: &str) -> Result<(), String> {
    let (addr, prefix) = match s.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (s, None),
    };
    let ip: IpAddr = addr
        .parse()
        .map_err(|_| format!("{addr} is not a valid IP address"))?;
    if let Some(p) = prefix {
        let max = if ip.is_ipv4() { 32 } else { 128 };
        match p.parse::<u8>() {
            Ok(n) if n <= max => {}
            _ => return Err(format!("/{p} is not a prefix length (0 to {max})")),
        }
    }
    Ok(())
}

/// A fully qualified image reference: `registry/path[:tag][@sha256:digest]`.
fn image_ref(s: &str) -> Result<(), String> {
    let bad = |why: &str| {
        Err(format!(
            "{s:?} is not a valid image reference ({why}); write it in full, such as \"{}\"",
            crate::model::DEFAULT_BASE_IMAGE
        ))
    };
    let (name_tag, digest) = match s.split_once('@') {
        Some((n, d)) => (n, Some(d)),
        None => (s, None),
    };
    if let Some(d) = digest {
        let ok = d
            .strip_prefix("sha256:")
            .is_some_and(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()));
        if !ok {
            return bad("the digest must be sha256: followed by 64 hexadecimal characters");
        }
    }
    let last_slash = name_tag.rfind('/').unwrap_or(0);
    let (name, tag) = match name_tag.rfind(':') {
        Some(i) if i > last_slash => (&name_tag[..i], Some(&name_tag[i + 1..])),
        _ => (name_tag, None),
    };
    if let Some(t) = tag {
        let ok = !t.is_empty()
            && t.len() <= 128
            && !t.starts_with(['.', '-'])
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
        if !ok {
            return bad("invalid tag");
        }
    }
    let mut parts = name.split('/');
    let registry = parts.next().unwrap_or_default();
    let path: Vec<&str> = parts.collect();
    let registry_ok = (registry.contains('.') || registry.contains(':') || registry == "localhost")
        && registry
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c));
    if !registry_ok || path.is_empty() {
        return bad("the registry is missing");
    }
    let component_ok = |p: &&str| {
        !p.is_empty()
            && p.starts_with(|c: char| c.is_ascii_alphanumeric())
            && p.ends_with(|c: char| c.is_ascii_alphanumeric())
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
    };
    if !path.iter().all(component_ok) {
        return bad("invalid repository name");
    }
    Ok(())
}

fn https_url(s: &str) -> Result<(), String> {
    let rest = s
        .strip_prefix("https://")
        .ok_or_else(|| format!("{s:?} must be an https:// address"))?;
    let host = rest.split('/').next().unwrap_or_default();
    if host.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_graphic() && !"\"'`\\<>{}|^".contains(c))
    {
        return Err(format!("{s:?} is not a valid https:// address"));
    }
    Ok(())
}

fn apt_word(s: &str, slash: bool) -> Result<(), String> {
    let valid = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c) || (slash && c == '/'));
    if s.is_empty() || !valid || s.starts_with('-') {
        return Err(format!(
            "{s:?} is not valid: use letters, digits, '.', '_' and '-'"
        ));
    }
    Ok(())
}

fn fingerprint(s: &str) -> Result<(), String> {
    if s.len() != 40 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "{s:?} is not a key fingerprint: give the 40 hexadecimal characters, without spaces (it is required for \
             every third-party repository)"
        ));
    }
    Ok(())
}

fn package_name(s: &str) -> Result<(), String> {
    let valid = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._+:/@-".contains(c));
    let first_ok = s.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '@');
    if s.is_empty() || s.len() > 214 || !valid || !first_ok || s.contains("..") || s.contains("//")
    {
        return Err(format!(
            "{s:?} is not a valid package name: use letters, digits and . _ + : / @ -, starting with a letter, a \
             digit or '@'"
        ));
    }
    Ok(())
}

fn version(s: &str) -> Result<(), String> {
    let valid = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._+~:^-".contains(c));
    if s.is_empty()
        || s.len() > 128
        || !valid
        || !s.starts_with(|c: char| c.is_ascii_alphanumeric())
    {
        return Err(format!(
            "{s:?} is not a valid version: use letters, digits and . _ + ~ : ^ -, starting with a letter or digit"
        ));
    }
    Ok(())
}

fn language(s: &str) -> Result<(), String> {
    let (lang, region) = match s.split_once('_') {
        Some((l, r)) => (l, Some(r)),
        None => (s, None),
    };
    let lang_ok = (2..=3).contains(&lang.len()) && lang.chars().all(|c| c.is_ascii_lowercase());
    let region_ok =
        region.is_none_or(|r| r.len() == 2 && r.chars().all(|c| c.is_ascii_uppercase()));
    if !lang_ok || !region_ok {
        return Err(format!(
            "{s:?} is not a language code: use \"fr\", \"en\", \"pt_BR\"…"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Folder, Mode, Package, Trust};

    fn sandbox() -> Config {
        let mut c = Config::defaults(Trust::Work);
        c.id = "test".into();
        c.name = "Test".into();
        c
    }

    fn paths(c: &Config, kind: Kind) -> Vec<String> {
        match config(c, kind) {
            Ok(()) => Vec::new(),
            Err(Invalid(issues)) => issues.into_iter().map(|i| i.path).collect(),
        }
    }

    #[test]
    fn defaults_are_valid() {
        assert_eq!(paths(&sandbox(), Kind::Sandbox), Vec::<String>::new());
        assert_eq!(paths(&sandbox(), Kind::Profile), Vec::<String>::new());
    }

    #[test]
    fn hostile_ids_and_names() {
        for bad in [
            "",
            "Claude",
            "a;b",
            "$(rm)",
            "a\nb",
            "../x",
            "a:b",
            "-a",
            &"a".repeat(33),
        ] {
            assert!(id(bad).is_err(), "{bad:?}");
        }
        assert!(id("my-box-2").is_ok());
        assert!(display_name("a\nb").is_err());
        assert!(display_name(" a").is_err());
    }

    #[test]
    fn hostile_paths() {
        for bad in [
            "relative", "/", "/a/../b", "/a/./b", "/a//b", "/a/", "/a:b", "/a\nb",
        ] {
            assert!(host_path(bad).is_err(), "{bad:?}");
        }
        for bad in [
            "/usr",
            "/usr/local/x",
            "/etc",
            "/proc/1",
            "/dev",
            "/tmp/xdg-runtime",
            "/tmp",
        ] {
            assert!(mount_point(bad).is_err(), "{bad:?}");
        }
        assert!(mount_point("/work/site").is_ok());
        assert!(mount_point("/usrdata").is_ok());
        assert!(relative_path("../x").is_err());
        assert!(relative_path(".git/hooks").is_ok());
    }

    #[test]
    fn folders_cannot_collide_or_cover_home() {
        let mut c = sandbox();
        let f = |mount: &str| Folder {
            host: "/home/me/p".into(),
            mount: mount.into(),
            mode: Mode::Rw,
        };
        c.folders = vec![
            f("/work/a"),
            f("/work/a"),
            f("/home"),
            f("/home/sandbox/b"),
            f("/other/b"),
        ];
        assert_eq!(
            paths(&c, Kind::Sandbox),
            [
                "folders[1].mount",
                "folders[1].mount",
                "folders[2].mount",
                "folders[4].mount"
            ]
        );
    }

    #[test]
    fn profile_rejects_local_fields() {
        let mut c = sandbox();
        c.profile = Some("claude".into());
        c.identity.hostname = Some("h".into());
        c.home.path = Some("/home/me/h".into());
        c.folders.push(Folder {
            host: "/home/me/p".into(),
            mount: "/work/p".into(),
            mode: Mode::Ro,
        });
        c.firewall.lan.allow[0].to = "192.168.1.10".into();
        c.autostart = Some(true);
        assert_eq!(
            paths(&c, Kind::Profile),
            [
                "profile",
                "identity.hostname",
                "home.path",
                "folders",
                "firewall.lan.allow[0].to",
                "autostart"
            ]
        );
        assert!(paths(&c, Kind::Sandbox).is_empty());
    }

    #[test]
    fn firewall_targets() {
        for good in [
            "host",
            "gateway",
            "dns",
            "10.0.0.1",
            "10.0.0.0/8",
            "fe80::/10",
            "::1",
        ] {
            assert!(
                REFERENCES.contains(&good) || ip_or_cidr(good).is_ok(),
                "{good:?}"
            );
        }
        for bad in [
            "lan",
            "10.0.0.0/33",
            "fe80::/129",
            "example.com",
            "10.0.0.1; rm",
        ] {
            assert!(
                !REFERENCES.contains(&bad) && ip_or_cidr(bad).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn image_references() {
        for good in [
            "docker.io/library/debian:trixie-slim",
            "registry.fedoraproject.org/fedora:41",
            "localhost/cardea/claude",
            "localhost:5000/x/y:1",
            &format!("docker.io/library/alpine@sha256:{}", "a".repeat(64)),
        ] {
            assert!(image_ref(good).is_ok(), "{good:?}");
        }
        for bad in [
            "debian",
            "debian:trixie",
            "docker.io/Library/x",
            "docker.io/x:",
            "docker.io/x@sha256:12",
            "a b",
        ] {
            assert!(image_ref(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn packages_and_repos() {
        for bad in ["-rf", "a;b", "$(x)", "a b", "../x", ""] {
            assert!(package_name(bad).is_err(), "{bad:?}");
        }
        for good in [
            "claude-desktop",
            "@scope/pkg",
            "github.com/x/y/cmd/z",
            "g++",
            "libc6:amd64",
        ] {
            assert!(package_name(good).is_ok(), "{good:?}");
        }
        assert!(version("--force").is_err());
        assert!(https_url("http://x.org").is_err());
        assert!(https_url("https://x.org/a b").is_err());
        assert!(fingerprint("31DD DE24").is_err());
        assert!(fingerprint("31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE").is_ok());

        let mut c = sandbox();
        c.packages.push(Package {
            source: "brew".into(),
            name: "x".into(),
            version: None,
            when: None,
        });
        assert_eq!(paths(&c, Kind::Sandbox), ["packages[0].source"]);
    }

    #[test]
    fn folder_placeholder_and_autostart() {
        let mut c: Config = serde_json::from_value(serde_json::json!({
            "schema": 1, "id": "t", "name": "T", "base_image": crate::model::DEFAULT_BASE_IMAGE, "trust": "work",
            "identity": { "user": "sandbox", "home_mount": "/home/sandbox" }, "home": { "disposable": false },
            "folders": [ { "host": "/home/me/site", "mount": "/work/site", "mode": "rw" } ],
            "protected_paths": { "names": [], "depth": 5 },
            "capabilities": { "wayland": true, "x11": "off", "gpu": false, "kvm": false,
                              "keyring": { "mode": "auto", "keep": false } },
            "security": { "seccomp": true, "isolated_uid": false, "ram_limit_gb": null },
            "firewall": { "enabled": true, "blocked": "reject",
                          "host": { "mode": "allow", "allow": [], "block": [] },
                          "lan": { "mode": "allow", "allow": [], "block": [] },
                          "wan": { "mode": "block", "allow": [], "block": [] } },
            "sources": [], "apt_repos": [], "packages": [],
            "setup": { "build": "", "first_start": "", "on_failure": "stop" },
            "apps": [ { "id": "xt", "name": "XTerm", "exec": ["xterm"], "toolkit": "x11" } ],
            "services": [
                { "id": "a", "name": "A", "exec": ["rc", "--name={folder}"], "per_folder": true, "autostart": ["site"] },
                { "id": "b", "name": "B", "exec": ["rc", "{folder}"], "autostart": true },
                { "id": "c", "name": "C", "exec": ["rc"], "per_folder": true, "autostart": ["nope"] }
            ],
            "appearance": { "theme": "dark", "languages": ["fr", "EN"] }
        }))
        .unwrap();
        assert_eq!(
            paths(&c, Kind::Sandbox),
            [
                "apps[0].toolkit",
                "services[0].exec[1]",
                "services[1].exec[1]",
                "services[2].autostart[0]",
                "appearance.languages[1]"
            ]
        );
        c.capabilities.x11 = X11::Isolated;
        assert!(!paths(&c, Kind::Sandbox).contains(&"apps[0].toolkit".to_owned()));
    }

    #[test]
    fn settings_validation() {
        let mut s = Settings::default();
        assert!(settings(&s).is_ok());
        s.sandbox_location = Some("~/Sandboxes".into());
        assert!(settings(&s).is_ok());
        s.sandbox_location = Some("Sandboxes".into());
        s.terminal = Some(vec![]);
        let Err(Invalid(issues)) = settings(&s) else {
            panic!()
        };
        assert_eq!(issues.len(), 2);
    }
}
