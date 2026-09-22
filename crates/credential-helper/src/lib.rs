//! Implements the git credential protocol, returning signed-in forge tokens from the OS keychain.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_is_built_by_the_workspace() {
        assert_eq!(env!("CARGO_PKG_NAME"), "credential-helper");
    }
}
