//! Dev CLI over git-engine for manual testing and debugging.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_is_built_by_the_workspace() {
        assert_eq!(env!("CARGO_PKG_NAME"), "git-engine-cli");
    }
}
