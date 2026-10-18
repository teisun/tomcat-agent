use super::{detect_package_manager, PackageManager};

fn detect(os: &str, termux: bool, programs: &[&str]) -> Option<PackageManager> {
    detect_package_manager(os, termux, |program| programs.contains(&program))
}

#[test]
fn macos_prefers_brew_then_falls_back_to_linux_managers() {
    assert_eq!(
        detect("macos", false, &["brew", "apt-get"]),
        Some(PackageManager::Brew)
    );
    assert_eq!(
        detect("macos", false, &["apt-get"]),
        Some(PackageManager::AptGet)
    );
}

#[test]
fn windows_prefers_winget_then_falls_back_to_pacman() {
    assert_eq!(
        detect("windows", false, &["winget", "pacman"]),
        Some(PackageManager::Winget)
    );
    assert_eq!(
        detect("windows", false, &["pacman"]),
        Some(PackageManager::Pacman)
    );
}

#[test]
fn brew_and_winget_only_count_on_their_own_os() {
    assert_eq!(detect("linux", false, &["brew", "winget"]), None);
    assert_eq!(
        detect("linux", false, &["dnf", "pacman"]),
        Some(PackageManager::Dnf)
    );
}

#[test]
fn termux_uses_pkg_and_plain_android_has_no_manager() {
    assert_eq!(
        detect("android", true, &["pkg", "apt-get"]),
        Some(PackageManager::TermuxPkg)
    );
    assert_eq!(detect("android", false, &["pkg", "apt-get"]), None);
}
