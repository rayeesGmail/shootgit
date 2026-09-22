//! Forge trait, neutral models, auth traits, SQLite cache and polling scheduler shared by the hosting providers.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_is_built_by_the_workspace() {
        assert_eq!(env!("CARGO_PKG_NAME"), "forge-core");
    }
}
