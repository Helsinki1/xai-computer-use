//! Platform and installation checks for the reserved native computer-use relay.

use std::path::{Path, PathBuf};

pub const APP_BUNDLE_NAME: &str = "Grok Computer Use.app";
pub const RELAY_RELATIVE_PATH: &str = "Contents/MacOS/grok-computer-use-mcp";
pub const APP_SIGNING_IDENTIFIER: &str = "com.xai.grok.computer-use";
pub const RELAY_SIGNING_IDENTIFIER: &str = "com.xai.grok.computer-use.mcp";
pub const LOCAL_DEVELOPMENT_ENV: &str = "GROK_COMPUTER_USE_LOCAL_DEV";

pub fn app_bundle_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Applications").join(APP_BUNDLE_NAME))
}

pub fn relay_path() -> Option<PathBuf> {
    app_bundle_path().map(|app| app.join(RELAY_RELATIVE_PATH))
}

/// Every existing component from the relay through the filesystem root must
/// be a real directory/file rather than a symbolic-link redirect.
pub fn path_chain_has_no_symlinks(app: &Path, relay: &Path) -> bool {
    app.is_absolute()
        && relay.is_absolute()
        && relay.starts_with(app)
        && relay.ancestors().all(|path| {
            std::fs::symlink_metadata(path)
                .map(|metadata| !metadata.file_type().is_symlink())
                .unwrap_or(false)
        })
}

/// Local ad-hoc trust is available only in debug Grok builds and must be
/// explicitly enabled for each process invocation.
pub fn local_development_enabled() -> bool {
    cfg!(debug_assertions)
        && std::env::var_os(LOCAL_DEVELOPMENT_ENV).is_some_and(|value| value == "1")
}

#[cfg(target_os = "macos")]
pub fn platform_supported() -> bool {
    if !cfg!(target_arch = "aarch64") && !local_development_enabled() {
        return false;
    }
    std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|version| version.trim().split('.').next()?.parse::<u32>().ok())
        .is_some_and(|major| major >= 14)
}

#[cfg(not(target_os = "macos"))]
pub fn platform_supported() -> bool {
    false
}

#[cfg(target_os = "macos")]
pub fn signature_valid(app: &Path) -> bool {
    let codesign = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "--verbose=2"])
        .arg(app)
        .output()
        .is_ok_and(|output| output.status.success());
    let gatekeeper = std::process::Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "execute"])
        .arg(app)
        .output()
        .is_ok_and(|output| output.status.success());
    let Some(host) = std::env::current_exe()
        .ok()
        .and_then(|path| code_signature_identity(&path))
    else {
        return false;
    };
    let Some(app_identity) = code_signature_identity(app) else {
        return false;
    };
    let Some(relay_identity) = code_signature_identity(&app.join(RELAY_RELATIVE_PATH)) else {
        return false;
    };
    if local_development_enabled() {
        return codesign
            && app_identity.identifier == APP_SIGNING_IDENTIFIER
            && relay_identity.identifier == RELAY_SIGNING_IDENTIFIER;
    }
    codesign
        && gatekeeper
        && app_identity.identifier == APP_SIGNING_IDENTIFIER
        && relay_identity.identifier == RELAY_SIGNING_IDENTIFIER
        && !host.team_identifier.is_empty()
        && app_identity.team_identifier == host.team_identifier
        && relay_identity.team_identifier == host.team_identifier
}

#[cfg(target_os = "macos")]
struct CodeSignatureIdentity {
    identifier: String,
    team_identifier: String,
}

#[cfg(target_os = "macos")]
fn code_signature_identity(path: &Path) -> Option<CodeSignatureIdentity> {
    let output = std::process::Command::new("/usr/bin/codesign")
        .args(["-d", "--verbose=4"])
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    // `codesign -d` writes its report to stderr.
    let report = String::from_utf8(output.stderr).ok()?;
    let value = |key: &str| {
        report
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    Some(CodeSignatureIdentity {
        identifier: value("Identifier=")?,
        team_identifier: value("TeamIdentifier=")?,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn signature_valid(_app: &Path) -> bool {
    false
}

/// Return the one permitted relay executable only when the current install is
/// usable. Call this at session creation, not only when the user enables the
/// feature, so replacing the app after enable cannot silently gain trust.
pub fn trusted_relay_path() -> Option<PathBuf> {
    if !platform_supported() {
        return None;
    }
    let app = app_bundle_path()?;
    let relay = app.join(RELAY_RELATIVE_PATH);
    if !app.is_dir()
        || !relay.is_file()
        || !path_chain_has_no_symlinks(&app, &relay)
        || !signature_valid(&app)
    {
        return None;
    }
    Some(relay)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn secure_path_check_covers_ancestors_above_app_bundle() {
        let root = tempfile::tempdir().unwrap();
        let real_home = root.path().join("real-home");
        let linked_home = root.path().join("linked-home");
        let app = linked_home.join("Applications").join(APP_BUNDLE_NAME);
        let relay = app.join(RELAY_RELATIVE_PATH);
        let real_relay = real_home
            .join("Applications")
            .join(APP_BUNDLE_NAME)
            .join(RELAY_RELATIVE_PATH);
        std::fs::create_dir_all(real_relay.parent().unwrap()).unwrap();
        std::fs::write(real_relay, b"relay").unwrap();
        std::os::unix::fs::symlink(&real_home, &linked_home).unwrap();

        assert!(!path_chain_has_no_symlinks(&app, &relay));
    }
}
