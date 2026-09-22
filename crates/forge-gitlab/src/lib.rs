//! GitLab REST client with OAuth PKCE and personal access token auth.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_is_built_by_the_workspace() {
        assert_eq!(env!("CARGO_PKG_NAME"), "forge-gitlab");
    }
}
