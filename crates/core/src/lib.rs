//! Cardea core: data model, validation, trust presets, profile risk analysis, Containerfile generation,
//! container spec building and nftables generation. Pure code, no I/O towards Podman.

pub mod apps;
pub mod builtin;
pub mod firewall;
pub mod image;
pub mod impact;
pub mod links;
pub mod load;
pub mod model;
pub mod packages;
pub mod profile;
pub mod risk;
pub mod services;
pub mod spec;
pub mod state;
pub mod trust;
pub mod validate;

/// Version of the Cardea workspace, shared by every binary.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::VERSION.is_empty());
    }
}
