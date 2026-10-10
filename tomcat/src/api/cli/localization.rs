//! Translate display metadata, never command names or parsing rules.
use super::Cli;
use crate::infra::i18n::{current_locale, tr_in, Locale};
use clap::{
    builder::StyledStr,
    error::{ContextKind, ErrorFormatter, ErrorKind},
    Command, CommandFactory, FromArgMatches,
};
use std::collections::BTreeMap;
use std::sync::OnceLock;

fn english_help() -> &'static BTreeMap<String, String> {
    static HELP: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    HELP.get_or_init(|| {
        serde_json::from_str(include_str!("../../../assets/i18n/cli_help.en.json"))
            .expect("embedded CLI help")
    })
}

pub(super) fn command(locale: Locale) -> Command {
    let mut cmd = Cli::command();
    // Build once so generated help/version metadata participates in the same localization walk.
    cmd.build();
    localize(cmd, "tomcat", locale)
}

fn localize(mut cmd: Command, path: &str, locale: Locale) -> Command {
    if cmd.get_name() == "help" {
        return cmd.about(tr_in(locale, "cli.help_command", &[]));
    }
    if locale == Locale::En {
        if let Some(about) = english_help().get(path) {
            cmd = cmd.about(about.clone());
        }
        if let Some(about) = english_help().get(&format!("{path}.long")) {
            cmd = cmd.long_about(about.clone());
        }
    }
    // Use unheaded {usage}; Clap still owns its token formatting and all parsing.
    let template = format!("{{before-help}}{{name}} {{version}}\n{{about-with-newline}}\n{}: {{usage}}\n\n{{all-args}}{{after-help}}", tr_in(locale, "cli.usage", &[]));
    cmd = cmd
        .help_template(template)
        .subcommand_help_heading(tr_in(locale, "cli.commands", &[]))
        .mut_args(|arg| {
            let id = arg.get_id().as_str();
            let help = match id {
                "help" => Some(tr_in(locale, "cli.help_flag", &[])),
                "version" => Some(tr_in(locale, "cli.version_flag", &[])),
                _ if locale == Locale::En => english_help().get(&format!("{path}.{id}")).cloned(),
                _ => None,
            };
            let heading = if arg.is_positional() {
                "cli.arguments"
            } else {
                "cli.options"
            };
            let arg = arg.help_heading(tr_in(locale, heading, &[]));
            match help {
                Some(help) => arg.help(help).long_help(None::<&str>),
                None => arg,
            }
        });
    cmd.mut_subcommands(|sub| {
        let child_path = format!("{path}.{}", sub.get_name());
        localize(sub, &child_path, locale)
    })
}

pub(super) fn parse() -> Cli {
    let args: Vec<_> = std::env::args_os().collect();
    let is_serve = args.get(1).is_some_and(|arg| arg == "serve");
    let host = is_serve
        .then(|| std::env::var("TOMCAT_HOST_LOCALE").ok())
        .flatten();
    crate::infra::config::ui::bootstrap_language(host.as_deref());
    let matches = command(current_locale())
        .try_get_matches_from(args)
        .unwrap_or_else(|error| error.apply::<ProductErrorFormatter>().exit());
    Cli::from_arg_matches(&matches)
        .unwrap_or_else(|error| error.apply::<ProductErrorFormatter>().exit())
}

struct ProductErrorFormatter;
impl ErrorFormatter for ProductErrorFormatter {
    fn format_error(error: &clap::error::Error<Self>) -> StyledStr {
        format_error_in(current_locale(), error)
    }
}

fn format_error_in<F: ErrorFormatter>(locale: Locale, error: &clap::error::Error<F>) -> StyledStr {
    let key = match error.kind() {
        ErrorKind::UnknownArgument => "cli.error.unknown_arg",
        ErrorKind::InvalidSubcommand => "cli.error.unknown_command",
        ErrorKind::MissingRequiredArgument => "cli.error.missing_arg",
        ErrorKind::MissingSubcommand => "cli.error.missing_command",
        ErrorKind::ArgumentConflict => "cli.error.conflict",
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => "cli.error.invalid_value",
        ErrorKind::NoEquals => "cli.error.equals",
        ErrorKind::TooManyValues => "cli.error.too_many",
        ErrorKind::TooFewValues | ErrorKind::WrongNumberOfValues => "cli.error.value_count",
        ErrorKind::InvalidUtf8 => "cli.error.utf8",
        _ => "cli.error.invalid_args",
    };
    let mut out = StyledStr::new();
    out.push_str(&format!(
        "{}: {}\n",
        tr_in(locale, "cli.error", &[]),
        tr_in(locale, key, &[])
    ));
    for (context, label) in [
        (ContextKind::InvalidArg, "cli.arguments"),
        (ContextKind::InvalidSubcommand, "cli.commands"),
        (ContextKind::InvalidValue, "cli.value"),
        (ContextKind::PriorArg, "cli.error.conflicting"),
        (ContextKind::ValidValue, "cli.possible_values"),
        (ContextKind::ValidSubcommand, "cli.commands"),
        (ContextKind::SuggestedSubcommand, "cli.suggestion"),
        (ContextKind::SuggestedArg, "cli.suggestion"),
        (ContextKind::SuggestedValue, "cli.suggestion"),
    ] {
        if let Some(value) = error.get(context) {
            out.push_str(&format!("  {}: {value}\n", tr_in(locale, label, &[])));
        }
    }
    if let Some(usage) = error.get(ContextKind::Usage) {
        // Normalize only the heading of Clap's structured usage, never classify an error by prose.
        let usage = usage.to_string();
        let tokens = usage.strip_prefix("Usage:").unwrap_or(&usage).trim();
        out.push_str(&format!(
            "\n{}: {tokens}\n",
            tr_in(locale, "cli.usage", &[])
        ));
    }
    out.push_str(&format!("\n{}\n", tr_in(locale, "cli.try_help", &[])));
    out
}

#[cfg(test)]
mod tests {
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
                if arg.get_help().is_some() && !matches!(arg.get_id().as_str(), "help" | "version")
                {
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
}
