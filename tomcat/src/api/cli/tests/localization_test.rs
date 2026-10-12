use super::*;
use std::collections::BTreeSet;

#[test]
fn i18n_cli_help_covers_command_tree_without_orphan_keys() {
    fn visit(cmd: &Command, path: &str, keys: &mut BTreeSet<String>) {
        if cmd.get_name() == "help" {
            return;
        }
        if cmd.get_about().is_some() {
            keys.insert(path.to_owned());
        }
        if cmd.get_long_about().is_some() {
            keys.insert(format!("{path}.long"));
        }
        for arg in cmd.get_arguments() {
            if arg.get_help().is_some() && !matches!(arg.get_id().as_str(), "help" | "version") {
                keys.insert(format!("{path}.{}", arg.get_id()));
            }
        }
        for sub in cmd.get_subcommands() {
            visit(sub, &format!("{path}.{}", sub.get_name()), keys);
        }
    }
    let mut keys = BTreeSet::new();
    visit(&Cli::command(), "tomcat", &mut keys);
    assert_eq!(keys, english_help().keys().cloned().collect());
    for locale in [Locale::En, Locale::ZhCn] {
        let mut cmd = command(locale);
        cmd.debug_assert();
        cmd = command(locale);
        let help = cmd.render_long_help().to_string();
        if locale == Locale::En {
            assert!(help.contains(&tr_in(Locale::En, "cli.commands", &[])));
            assert!(help.contains(&tr_in(Locale::En, "cli.help_flag", &[])));
        }
        assert!(help.contains("--help"));
    }
}

#[test]
fn i18n_cli_parse_errors_preserve_tokens_and_kind() {
    for args in [
        vec!["tomcat", "--bad-arg"],
        vec!["tomcat", "session", "delete"],
        vec!["tomcat", "session", "list", "--scope", "wrong"],
        vec!["tomcat", "serve", "--stdio", "--ws"],
        vec!["tomcat", "audit", "list", "--limit", "abc"],
    ] {
        for locale in [Locale::En, Locale::ZhCn] {
            let error = command(locale).try_get_matches_from(&args).unwrap_err();
            assert_eq!(error.exit_code(), 2);
            let text = format_error_in(locale, &error).to_string();
            if locale == Locale::En {
                assert!(text.contains(&tr_in(Locale::En, "cli.error", &[])));
            }
            assert!(text.contains("--help"));
        }
    }
}
