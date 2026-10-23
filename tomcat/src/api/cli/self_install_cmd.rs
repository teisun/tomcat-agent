//! Install only the current executable; bootstrap and service lifecycle stay untouched.
use crate::infra::{i18n::tr, platform};
use crate::AppError;

pub(super) fn run_self_install() -> Result<(), AppError> {
    let home = platform::home_dir()
        .filter(|home| home.is_absolute())
        .ok_or_else(|| AppError::Config(tr("cli.selfInstall.invalidHome", &[])))?;
    let bin_dir = platform::user_bin_dir(&home);
    let entry = bin_dir.join(format!("{}{}", crate::CLI_NAME, std::env::consts::EXE_SUFFIX));
    let path = entry.display().to_string();
    let installed = super::init::install_tomcat_command(&std::env::current_exe()?, &bin_dir)
        .map_err(|error| {
            AppError::Io(std::io::Error::new(
                error.kind(),
                tr(
                    "cli.selfInstall.failed",
                    &[("path", &path), ("detail", &error.to_string())],
                ),
            ))
        })?;
    // None also covers preserved user files and skipped test artifacts, not just identical builds.
    let key = if installed.is_some() {
        "cli.selfInstall.updated"
    } else {
        "cli.selfInstall.unchanged"
    };
    println!("{}", tr(key, &[("path", &path)]));
    Ok(())
}
