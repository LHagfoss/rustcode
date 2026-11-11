//! Pi takes no ACP `mcpServers`, so `pi-acp` drops the orchestrator server the
//! other harnesses get. `pi --extension` is how it gets those tools: `pi-acp`
//! starts `PI_ACP_PI_COMMAND`, here a small script that adds the extension,
//! and the extension starts `rustcode mcp-orchestrator` itself.

use std::path::{Path, PathBuf};

use crate::orchestration::hub::{
    Launch, ORCHESTRATION_VERSION_ENV, ORCHESTRATOR_TOKEN_ENV, SMART_UI_ENV,
};

const EXTENSION: &str = include_str!("pi_extension.ts");
/// Keep both script formats testable on every host platform.
fn wrapper(windows: bool) -> (&'static str, &'static str) {
    if windows {
        (
            "pi-with-thread-tools.cmd",
            "@echo off\r\npi --extension \"%RUSTCODE_PI_EXTENSION%\" %*\r\n",
        )
    } else {
        (
            "pi-with-thread-tools",
            "#!/bin/sh\nexec pi --extension \"$RUSTCODE_PI_EXTENSION\" \"$@\"\n",
        )
    }
}

/// Write the extension and its script into `dir`, and give the script's path.
fn install(dir: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(dir)?;
    let extension = dir.join("rustcode-thread-tools.ts");
    std::fs::write(&extension, EXTENSION)?;
    let (name, script) = wrapper(cfg!(windows));
    let wrapper = dir.join(name);
    std::fs::write(&wrapper, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok((wrapper, extension))
}

/// The environment that gives a Pi thread the orchestrator's tools. Empty, so
/// the thread runs without them, when the files cannot be written.
pub(crate) fn environment(launch: &Launch, dir: &Path) -> Vec<(String, String)> {
    let (wrapper, extension) = match install(dir) {
        Ok(paths) => paths,
        Err(error) => {
            crate::dbg_log!("harness pi: could not write the thread tools extension: {error}");
            return Vec::new();
        }
    };
    let mut env = vec![
        (
            "PI_ACP_PI_COMMAND".to_owned(),
            wrapper.to_string_lossy().into_owned(),
        ),
        (
            "RUSTCODE_PI_EXTENSION".to_owned(),
            extension.to_string_lossy().into_owned(),
        ),
        (
            "RUSTCODE_PI_MCP_COMMAND".to_owned(),
            launch.command.to_string_lossy().into_owned(),
        ),
        (
            "RUSTCODE_PI_MCP_ARGS".to_owned(),
            serde_json::to_string(&launch.args).unwrap_or_else(|_| "[]".to_owned()),
        ),
        (ORCHESTRATOR_TOKEN_ENV.to_owned(), launch.token.clone()),
    ];
    // The extension hands these to the server, and takes them out of the
    // agent's shell.
    env.push((
        ORCHESTRATION_VERSION_ENV.to_owned(),
        launch.version.to_string(),
    ));
    if launch.smart_ui {
        env.push((SMART_UI_ENV.to_owned(), "1".to_owned()));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_uses_windows_command_script_and_forwards_arguments() {
        assert_eq!(
            wrapper(true),
            (
                "pi-with-thread-tools.cmd",
                "@echo off\r\npi --extension \"%RUSTCODE_PI_EXTENSION%\" %*\r\n"
            )
        );
    }

    #[test]
    fn wrapper_preserves_unix_script_and_argument_quoting() {
        assert_eq!(
            wrapper(false),
            (
                "pi-with-thread-tools",
                "#!/bin/sh\nexec pi --extension \"$RUSTCODE_PI_EXTENSION\" \"$@\"\n"
            )
        );
    }

    #[test]
    fn environment_writes_the_files_and_passes_the_token_outside_the_args() {
        let dir = tempfile::tempdir().unwrap();
        let launch = Launch {
            command: "/bin/rustcode".into(),
            args: vec!["mcp-orchestrator".into(), "--session".into(), "t".into()],
            token: "secret".into(),
            smart_ui: false,
            version: crate::orchestration::VERSION,
        };
        let env = environment(&launch, dir.path());
        let get = |name: &str| {
            env.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(get(ORCHESTRATOR_TOKEN_ENV), Some("secret"));
        assert_eq!(
            get(ORCHESTRATION_VERSION_ENV),
            Some(crate::orchestration::VERSION.to_string().as_str())
        );
        assert!(!get("RUSTCODE_PI_MCP_ARGS").unwrap().contains("secret"));
        let installed_wrapper = Path::new(get("PI_ACP_PI_COMMAND").unwrap());
        let (name, script) = wrapper(cfg!(windows));
        assert_eq!(installed_wrapper.file_name().unwrap(), name);
        assert_eq!(std::fs::read_to_string(installed_wrapper).unwrap(), script);
        assert!(
            std::fs::read_to_string(get("RUSTCODE_PI_EXTENSION").unwrap())
                .unwrap()
                .contains("mcp__rustcode__")
        );
    }
}
