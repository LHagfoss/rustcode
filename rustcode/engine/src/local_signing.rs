//! Command arguments shared by the CLI and desktop macOS updaters.
pub const IDENTITY: &str = "RustCode Local Signing";
pub const CLI_IDENTIFIER: &str = "org.rustcode.cli";

pub fn sign_args(identifier: &str) -> [&str; 5] {
    ["--force", "--sign", IDENTITY, "--identifier", identifier]
}

#[cfg(test)]
mod tests {
    #[test]
    fn stable_cli_arguments() {
        assert_eq!(
            super::sign_args(super::CLI_IDENTIFIER),
            [
                "--force",
                "--sign",
                "RustCode Local Signing",
                "--identifier",
                "org.rustcode.cli"
            ]
        );
    }
}
