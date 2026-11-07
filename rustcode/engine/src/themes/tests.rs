use super::*;

fn legacy(name: &str) -> &'static str {
    let (_, content) = BUILTIN_THEMES
        .iter()
        .find(|(file, _)| file.trim_end_matches(".toml") == name)
        .unwrap();
    legacy_builtin(content).unwrap()
}

fn rgb(text: &str) -> Rgb {
    ColorSpec::Hex(text.to_owned()).rgb().unwrap()
}

#[test]
fn every_builtin_is_valid_and_sets_every_desktop_token() {
    assert!(
        BUILTIN_THEMES
            .iter()
            .any(|(file, _)| *file == "rustcode.toml")
    );
    for (file, content) in BUILTIN_THEMES {
        let (parsed, diagnostics) = validate(content);
        assert!(diagnostics.is_empty(), "{file}: {diagnostics:?}");
        let parsed = parsed.unwrap();
        assert_eq!(format!("{}.toml", parsed.name), *file);
        let desktop = parsed.desktop.as_ref().unwrap();
        for (key, _) in TOKENS {
            assert!(desktop.token(key).is_some(), "{file} lacks desktop.{key}");
        }
        assert!(desktop.mode.is_some() && desktop.radius.is_some(), "{file}");
        // The legacy part is a whole theme without a desktop table.
        let old: ThemeFile = toml::from_str(legacy_builtin(content).unwrap()).unwrap();
        assert!(old.desktop.is_none());
    }
}

#[test]
fn the_default_theme_is_the_neutral_rustcode_dark() {
    assert_eq!(DEFAULT_THEME_NAME, "rustcode");
    assert_eq!(crate::config::AppConfig::default().theme, "rustcode");
    let theme = find(&builtin_themes(), "rustcode").unwrap().clone();
    let tokens = &theme.desktop.tokens;
    assert_eq!(theme.desktop.mode, Mode::Dark);
    assert_eq!(tokens.bg, "#0b0b0c");
    assert_eq!(tokens.text, "#ededed");
    assert_eq!(tokens.text_muted, "#a1a1a1");
    assert_eq!(tokens.text_faint, "#6b6b6b");
    assert!(contrast(rgb(&tokens.text), rgb(&tokens.bg)) > 15.0);
}

#[test]
fn a_theme_without_a_desktop_table_derives_every_token() {
    let file: ThemeFile = toml::from_str(legacy("nord")).unwrap();
    let desktop = resolve_desktop(&file);
    let tokens = &desktop.tokens;
    assert_eq!(desktop.mode, Mode::Dark);
    assert_eq!(desktop.radius, Radius::Default);
    assert_eq!(tokens.bg, "#2e3440");
    assert_eq!(tokens.sidebar, "#3b4252", "panel");
    assert_eq!(tokens.surface_2, "#4c566a", "hover_bg");
    assert_eq!(tokens.surface_3, "#434c5e", "element");
    assert_eq!(tokens.border_strong, "#4c566a", "status_border");
    assert_eq!(tokens.text, "#eceff4");
    assert_eq!(tokens.text_muted, "#d8dee9");
    assert_eq!(tokens.accent, "#88c0d0", "primary");
    assert_eq!(tokens.focus, tokens.accent);
    assert_eq!(tokens.success, "#a3be8c", "green");
    assert_eq!(tokens.warning, "#ebcb8b", "tip");
    // Deterministic.
    assert_eq!(resolve_desktop(&file), desktop);

    // A terminal-background theme still gets a concrete desktop bg.
    let file: ThemeFile = toml::from_str(legacy("catppuccin-mocha")).unwrap();
    let desktop = resolve_desktop(&file);
    assert_eq!(desktop.mode, Mode::Dark);
    assert_eq!(desktop.tokens.bg, hex(mix(rgb("#181825"), BLACK, 0.25)));

    let file: ThemeFile = toml::from_str(legacy("light")).unwrap();
    let desktop = resolve_desktop(&file);
    assert_eq!(desktop.mode, Mode::Light);
    assert_eq!(desktop.tokens.danger, "#c62828");
}

#[test]
fn derived_tokens_follow_the_ones_the_file_sets() {
    let text = format!(
        "{}\n[desktop]\nbg = \"#202020\"\nradius = \"round\"\naccent = \"#ffffff\"\n",
        legacy("nord")
    );
    let (file, diagnostics) = validate(&text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let desktop = resolve_desktop(&file.unwrap());
    assert_eq!(desktop.tokens.bg, "#202020");
    assert_eq!(desktop.radius, Radius::Round);
    assert_eq!(
        desktop.tokens.surface,
        hex(mix(rgb("#202020"), rgb("#eceff4"), 0.04))
    );
    assert_eq!(desktop.tokens.accent_text, "#0b0b0c", "dark text on white");
    assert_eq!(desktop.tokens.focus, "#ffffff");
}

#[test]
fn validation_reports_errors_and_warnings() {
    let base = legacy("nord");
    let check = |text: String| validate(&text).1;

    let diagnostics = check(base.replace("panel = \"#3b4252\"", "panel = \"#3b42\""));
    assert!(has_errors(&diagnostics));
    assert!(
        diagnostics[0].message.contains("`panel`"),
        "{diagnostics:?}"
    );

    let diagnostics = check(format!(
        "{base}shiny = true\n[desktop]\nglow = \"#ffffff\"\n"
    ));
    assert!(!has_errors(&diagnostics), "unknown keys only warn");
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(diagnostics[0].message.contains("`shiny`"));
    assert!(diagnostics[1].message.contains("`desktop.glow`"));

    for bad in [
        "bg = \"reset\"",
        "bg = \"blue\"",
        "mode = \"dim\"",
        "radius = \"pill\"",
    ] {
        let diagnostics = check(format!("{base}\n[desktop]\n{bad}\n"));
        assert!(has_errors(&diagnostics), "{bad}: {diagnostics:?}");
    }

    let diagnostics = check(format!(
        "{base}\n[desktop]\nbg = \"#777777\"\ntext = \"#888888\"\n"
    ));
    assert!(!has_errors(&diagnostics));
    assert!(
        diagnostics[0].message.contains("contrast"),
        "{diagnostics:?}"
    );

    let diagnostics = check("name = \"x\"\n".to_owned());
    assert!(has_errors(&diagnostics), "missing terminal keys");
    assert!(has_errors(&check("not toml = = 1".to_owned())));
    assert!(has_errors(&check(format!("{base}desktop = 3\n"))));
}

#[test]
fn colours_parse_as_the_terminal_ui_always_did() {
    let hex = |text: &str| ColorSpec::Hex(text.to_owned());
    assert_eq!(hex("#0A0b0c").parse(), Ok(ThemeColor::Rgb(10, 11, 12)));
    assert_eq!(hex("reset").parse(), Ok(ThemeColor::Reset));
    assert_eq!(hex("#none").parse(), Ok(ThemeColor::Reset));
    assert_eq!(hex("Transparent").parse(), Ok(ThemeColor::Reset));
    assert!(hex("#fff").parse().is_err());
    assert_eq!(hex("#fff").color(), ThemeColor::Reset);
    assert_eq!(ColorSpec::Rgb([1, 2, 3]).color(), ThemeColor::Rgb(1, 2, 3));
}

#[test]
fn user_and_project_themes_override_builtins_by_name() {
    let user = ensure_user_dir().unwrap();
    let themes = load_themes(None);
    assert_eq!(themes[0].name(), DEFAULT_THEME_NAME, "the default first");
    let nord = find(&themes, "nord").unwrap();
    assert_eq!(nord.source, ThemeSource::Builtin, "an unedited copy");
    assert_eq!(nord.path.as_deref(), Some(user.join("nord.toml").as_path()));

    std::fs::write(
        user.join("nord.toml"),
        legacy("nord").replace("Arctic", "My arctic"),
    )
    .unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_themes = project.path().join(".rustcode/themes");
    std::fs::create_dir_all(&project_themes).unwrap();
    std::fs::write(
        project_themes.join("team.toml"),
        legacy("sky").replace("name = \"sky\"", "name = \"team\""),
    )
    .unwrap();
    std::fs::write(project_themes.join("broken.toml"), "name = 1").unwrap();

    let themes = load_themes(Some(project.path()));
    let nord = find(&themes, "NORD").unwrap();
    assert_eq!(nord.source, ThemeSource::User);
    assert_eq!(nord.description(), "My arctic nord text and border palette");
    assert_eq!(find(&themes, "team").unwrap().source, ThemeSource::Project);
    assert!(find(&themes, "broken").is_none());
    assert!(find(&load_themes(None), "team").is_none());
}

#[test]
fn older_builtin_copies_are_upgraded_and_edited_ones_kept() {
    let user = user_themes_dir().unwrap();
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("sky.toml"), legacy("sky")).unwrap();
    let edited = legacy("nord").replace("#88c0d0", "#88c0d1");
    std::fs::write(user.join("nord.toml"), &edited).unwrap();
    ensure_user_dir();
    let sky = std::fs::read_to_string(user.join("sky.toml")).unwrap();
    assert!(sky.contains("[desktop]"));
    assert_eq!(
        std::fs::read_to_string(user.join("nord.toml")).unwrap(),
        edited
    );
}

#[test]
fn a_previous_builtin_copy_is_upgraded_in_a_temp_dir() {
    let dir = tempfile::tempdir().unwrap();
    // The copies `ensure_user_dir` wrote before the surfaces moved.
    for (filename, old) in PREVIOUS_BUILTINS {
        std::fs::write(dir.path().join(filename), old).unwrap();
    }
    sync_builtins(dir.path());
    for (filename, content) in BUILTIN_THEMES {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(filename)).unwrap(),
            *content,
            "{filename} was not upgraded"
        );
    }
    let mut themes = std::collections::BTreeMap::new();
    load_dir(dir.path(), ThemeSource::User, &mut themes);
    assert_eq!(
        themes["rustcode"].source,
        ThemeSource::Builtin,
        "an upgraded copy is the built-in, not a user theme"
    );
    assert_eq!(themes["rustcode"].desktop.tokens.surface, "#1d1d20");
    // A copy the user edited at some point is theirs: it never byte-matched a
    // built-in, so it is neither upgraded nor counted as one.
    let (_, old) = PREVIOUS_BUILTINS
        .iter()
        .find(|(filename, _)| *filename == "light.toml")
        .unwrap();
    let edited = old.replace("#e8e8ea", "#e8e8eb");
    assert_ne!(edited, *old);
    std::fs::write(dir.path().join("light.toml"), &edited).unwrap();
    sync_builtins(dir.path());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("light.toml")).unwrap(),
        edited
    );
    let mut themes = std::collections::BTreeMap::new();
    load_dir(dir.path(), ThemeSource::User, &mut themes);
    assert_eq!(themes["light"].source, ThemeSource::User);
}

#[test]
fn the_snapshot_has_the_acp_shape() {
    let value = serde_json::to_value(snapshot(None)).unwrap();
    assert_eq!(value["active"], DEFAULT_THEME_NAME);
    let themes = value["themes"].as_array().unwrap();
    assert!(themes.len() >= BUILTIN_THEMES.len());
    let first = themes[0].as_object().unwrap();
    let mut keys: Vec<&str> = first.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        ["description", "mode", "name", "radius", "source", "tokens"],
        "no path for a built-in without a file"
    );
    assert_eq!(first["source"], "builtin");
    assert_eq!(first["mode"], "dark");
    assert_eq!(first["radius"], "default");
    let tokens = first["tokens"].as_object().unwrap();
    assert_eq!(tokens.len(), TOKENS.len());
    for (key, _) in TOKENS {
        let value = tokens[*key].as_str().unwrap();
        assert!(value.len() == 7 && value.starts_with('#'), "{key}: {value}");
    }
}

#[test]
fn setting_a_theme_writes_the_user_config() {
    let snapshot = set_active("Dracula", None).unwrap();
    assert_eq!(snapshot.active, "dracula");
    assert_eq!(crate::config::load_config().2.theme, "dracula");
    assert!(set_active("no-such-theme", None).is_err());
    assert_eq!(crate::config::load_config().2.theme, "dracula");
}

#[test]
fn a_new_theme_is_a_valid_commented_copy_of_its_base() {
    let path = create("my-theme", Some("nord"), None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# RustCode theme"));
    let (file, diagnostics) = validate(&text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let file = file.unwrap();
    assert_eq!(file.name, "my-theme");
    let nord = find(&builtin_themes(), "nord").unwrap().clone();
    assert_eq!(resolve_desktop(&file), nord.desktop);
    let theme = find(&load_themes(None), "my-theme").unwrap().clone();
    assert_eq!(theme.source, ThemeSource::User);

    assert!(create("my-theme", None, None).is_err(), "exists");
    assert!(create("nord", None, None).is_err(), "a built-in name");
    assert!(create("Bad Name", None, None).is_err());
    assert!(create("../x", None, None).is_err());
    assert!(create("ok", Some("missing"), None).is_err());
}

#[test]
fn cli_commands_report_and_fail_clearly() {
    let listed = cli::list(None);
    assert!(listed.starts_with("* rustcode"), "{listed}");
    assert!(listed.contains("nord"));
    let shown = cli::show("rustcode", false, None).unwrap();
    assert!(shown.contains("bg             #0b0b0c"), "{shown}");
    let json: ThemeEntry =
        serde_json::from_str(&cli::show("rustcode", true, None).unwrap()).unwrap();
    assert_eq!(json.tokens.bg, "#0b0b0c");
    assert!(cli::show("missing", false, None).is_err());
    assert!(
        cli::path(Some("rustcode"), None).is_err(),
        "built in, no file yet"
    );
    assert!(
        cli::path(None, None)
            .unwrap()
            .trim_end()
            .ends_with("themes")
    );

    let (report, ok) = cli::validate_command(None, None);
    assert!(ok, "{report}");
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.toml");
    std::fs::write(&bad, legacy("nord").replace("#3b4252", "oops")).unwrap();
    let (report, ok) = cli::validate_command(bad.to_str(), None);
    assert!(!ok);
    assert!(report.contains("error: `panel`"), "{report}");
    let (_, ok) = cli::validate_command(Some("nord"), None);
    assert!(ok);
}
