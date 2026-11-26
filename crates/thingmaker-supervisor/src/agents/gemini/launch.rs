//! Launch contract for Antigravity's official CLI, `agy`.
//!
//! `agy -p= --input-format stream-json --output-format stream-json` is one
//! process for a whole conversation: a user event per line on stdin, events
//! per line on stdout (docs/research/multi-provider-viability.md §7, S3).
//! In stream-json mode everything about a session is a flag: the
//! model, the effort, the permission mode and the conversation to resume. A
//! change to any of them is a relaunch with `--conversation`, which the
//! bridge does between turns.
//! Native ACP sessions instead receive model/effort/mode after attachment
//! and their session-local MCP servers through session/new or session/load.
//!
//! Permission stances, from the live probe:
//!
//! - `ReadOnly` is `--mode plan`: workspace writes are refused (soft: the turn
//!   still succeeds and says what it was denied).
//! - `AcceptEdits` is `--mode accept-edits --dangerously-skip-permissions`.
//!   `accept-edits` alone refuses every shell command in headless mode, and
//!   the only other way to allow them is rules in the user's own settings,
//!   which the desktop does not edit. Skipping the prompts is what the
//!   desktop already answers for Claude and Codex in a trusted workspace:
//!   every request is allowed and journalled. Chosen by the user on
//!   1 October 2026.

use std::path::{Path, PathBuf};

use crate::{acp::launch::SessionId, agents::PermissionStance};

/// Environment override for the `agy` binary.
pub const PROGRAM_ENV: &str = "THINGMAKER_AGY";

/// Asks `agy` not to update itself. Left to itself it relaunches in a
/// terminal of its own, which on Windows is a console window the desktop
/// cannot hide (it is a window `agy` opens, not a child the desktop spawns).
/// ThingMaker offers the update as a button instead, the way it does for the
/// Claude adapter and Codex, so every run but that one holds it back.
pub const DISABLE_AUTO_UPDATE: &str = "AGY_CLI_DISABLE_AUTO_UPDATE";

/// The environment `agy` is asked questions in: the trusted-local profile
/// with its self-update held back. `versions::update` deliberately does not
/// use it, so `agy update` can still do its job.
pub fn environment() -> crate::security::EnvironmentProfile {
    crate::security::EnvironmentProfile::trusted_local().with_set(DISABLE_AUTO_UPDATE, "1")
}

/// The account's own home. `prepare_file_auth` selects private file storage;
/// neither the agent nor its tools receive a synthetic SSH environment.
/// The default account keeps its ordinary local/keyring authentication.
pub fn account_environment(folder: Option<&Path>) -> Vec<(String, String)> {
    let mut env = vec![(DISABLE_AUTO_UPDATE.into(), "1".into())];
    if let Some(folder) = folder {
        let path = folder.to_string_lossy().into_owned();
        if let Some(home) = std::env::home_dir() {
            let mut git = home.join(".gitconfig");
            if !git.is_file() {
                let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("git/config");
                if config.is_file() { git = config; }
            }
            for (key, default) in [("RUSTUP_HOME", home.join(".rustup")), ("CARGO_HOME", home.join(".cargo")), ("GIT_CONFIG_GLOBAL", git)] {
                let path = std::env::var_os(key).map(PathBuf::from).unwrap_or(default);
                env.push((key.into(), path.to_string_lossy().into_owned()));
            }
        }
        env.extend([
            ("HOME".into(), path.clone()), ("USERPROFILE".into(), path),
            ("APPDATA".into(), folder.join("AppData/Roaming").to_string_lossy().into_owned()),
            ("LOCALAPPDATA".into(), folder.join("AppData/Local").to_string_lossy().into_owned()),
        ]);
    }
    env
}

pub const ISOLATION_ERROR: &str = "Antigravity did not save an isolated sign-in token. This account is not separate from the Antigravity app; do not use it. Sign in again with a supported CLI version.";

pub fn token_path(folder: &Path) -> PathBuf {
    folder.join(".gemini/antigravity-cli/antigravity-oauth-token")
}

pub fn legacy_token_path(folder: &Path) -> PathBuf {
    folder.join(".gemini/jetski-standalone-oauth-token")
}

const LEGACY_AUTH_ERROR: &str = "This prototype Gemini profile contains legacy standalone credentials. Sign it out with the CLI before removing it, or add a new account profile. Legacy and current sign-ins must not be mixed.";

/// agy 1.5.9 chooses its file-only backend for the process lifetime when this
/// private cache entry is fresh at startup. This is an internal CLI contract:
/// callers must verify the supported version and empty-profile behavior before
/// using it. Refresh immediately before EVERY process launch, including resume.
pub fn prepare_file_auth(folder: &Path) -> Result<(), String> {
    if legacy_token_path(folder).exists() { return Err(LEGACY_AUTH_ERROR.into()); }
    let gemini = folder.join(".gemini");
    let cli = gemini.join("antigravity-cli");
    let cache = gemini.join("cache");
    let cli_cache = cli.join("cache");
    for path in [folder, &gemini, &cache, &cli, &cli_cache] {
        if let Ok(metadata) = path.symlink_metadata() {
            if !metadata.is_dir() { return Err(ISOLATION_ERROR.into()); }
            #[cfg(windows)]
            if super::super::accounts::is_junction(path).map_err(|e| e.to_string())? { return Err(ISOLATION_ERROR.into()); }
        } else { std::fs::create_dir_all(path).map_err(|e| e.to_string())?; }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        }
    }
    for directory in [cache, cli_cache] {
        let marker = directory.join("antigravity-keyring-unavailable");
        if marker.symlink_metadata().is_ok_and(|metadata| !metadata.is_file()) { return Err(ISOLATION_ERROR.into()); }
        let file = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(marker).map_err(|e| e.to_string())?;
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now())).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// ACP's terminal login is authentication only: it prints a browser URL and
/// accepts the returned code, then exits without starting an agent session.
pub fn login_arguments(folder: Option<&Path>) -> Vec<String> {
    if let Some(folder) = folder {
        if token_path(folder).is_file() { return vec!["--mode".into(), "plan".into()]; }
        let mut args = acp_arguments(folder);
        args.push("--login".into());
        args
    } else { Vec::new() }
}

pub fn acp_arguments(folder: &Path) -> Vec<String> {
    vec!["acp".into(), "--gemini-dir".into(), folder.join(".gemini").to_string_lossy().into_owned(),
        "--app-data-dir".into(), "antigravity-cli".into()]
}

/// Fail closed even when `agy models` can answer through a shared keychain.
pub fn require_isolated_token(folder: &Path) -> Result<(), String> {
    if legacy_token_path(folder).exists() { return Err(LEGACY_AUTH_ERROR.into()); }
    let root = folder.symlink_metadata().map_err(|_| ISOLATION_ERROR.to_string())?;
    let directory = folder.join(".gemini").symlink_metadata().map_err(|_| ISOLATION_ERROR.to_string())?;
    let cli = folder.join(".gemini/antigravity-cli").symlink_metadata().map_err(|_| ISOLATION_ERROR.to_string())?;
    let token = token_path(folder).symlink_metadata().map_err(|_| ISOLATION_ERROR.to_string())?;
    if !root.is_dir() || !directory.is_dir() || !cli.is_dir() || !token.is_file() || token.len() == 0 {
        return Err(ISOLATION_ERROR.into());
    }
    Ok(())
}

pub fn environment_for_account(folder: Option<&Path>) -> crate::security::EnvironmentProfile {
    account_environment(folder).into_iter().fold(environment(), |profile, (name, value)| profile.with_set(name, value))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiLaunchOptions {
    /// Canonical workspace root; the process's working directory.
    pub root: PathBuf,
    /// The `agy` binary.
    pub program: PathBuf,
    /// The id the desktop addresses this attachment by until `agy` names its
    /// conversation in `init`.
    pub reserved: SessionId,
    /// A model slug from `agy models` (`gemini-3.1-pro-high`, …).
    pub model: Option<String>,
    /// `low` … `max`.
    pub effort: Option<String>,
    /// The conversation to resume.
    pub resume: Option<String>,
    pub permission_mode: PermissionStance,
    pub account_home: Option<PathBuf>,
    /// Native ACP receives session-local MCP servers; ordinary sessions retain stream-json.
    pub native_acp: bool,
    /// Guidance sent with the first prompt of each attachment, including resume.
    pub initial_prompt_prefix: Option<String>,
}

impl GeminiLaunchOptions {
    pub fn new(root: impl Into<PathBuf>, program: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            program: program.into(),
            reserved: SessionId::reserve(),
            model: None,
            effort: None,
            resume: None,
            permission_mode: PermissionStance::ReadOnly,
            account_home: None,
            native_acp: false,
            initial_prompt_prefix: None,
        }
    }

    /// The command line for the first launch.
    pub fn arguments(&self) -> Vec<String> {
        if self.native_acp {
            // The selected account's home remains the credential anchor. Never
            // create a second home or rewrite its shared MCP configuration.
            let mut args = match self.account_home.as_deref() {
                Some(home) => acp_arguments(home),
                None => vec!["acp".into()],
            };
            // Also required by agy for editor-provided HTTP MCP servers.
            args.push("--process-per-session".into());
            return args;
        }
        arguments(self.model.as_deref(), self.effort.as_deref(), self.permission_mode, self.resume.as_deref())
    }
}

/// The command line for a launch with these settings. `-p` takes its value
/// attached: `-p` followed by a separate argument would read that argument as
/// the prompt.
pub fn arguments(model: Option<&str>, effort: Option<&str>, stance: PermissionStance, conversation: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = ["-p=", "--input-format", "stream-json", "--output-format", "stream-json"].map(str::to_string).to_vec();
    if let Some(model) = model.filter(|model| !model.is_empty()) {
        args.extend(["--model".into(), model.into()]);
    }
    if let Some(effort) = effort.filter(|effort| !effort.is_empty() && *effort != "default") {
        args.extend(["--effort".into(), effort.into()]);
    }
    args.extend(["--mode".into(), mode_id(stance).into()]);
    if stance == PermissionStance::AcceptEdits {
        args.push("--dangerously-skip-permissions".into());
    }
    if let Some(conversation) = conversation.filter(|id| !id.is_empty()) {
        args.extend(["--conversation".into(), conversation.into()]);
    }
    args
}

/// `agy`'s own name for a stance.
pub fn mode_id(stance: PermissionStance) -> &'static str {
    match stance {
        PermissionStance::AcceptEdits => "accept-edits",
        PermissionStance::ReadOnly => "plan",
    }
}

/// Where `agy` keeps a conversation: one database per id. A session with no
/// file has nothing to resume.
pub fn conversation_path(home: &Path, conversation: &str) -> PathBuf {
    home.join(".gemini/antigravity-cli/conversations").join(format!("{conversation}.db"))
}

/// CLI model variants include a thinking suffix; ACP advertises the base
/// model and thinking level separately. Exact advertised IDs win first.
pub fn native_model_selection(wanted: &str, offered: &[String]) -> Option<(String, Option<String>)> {
    if offered.iter().any(|model| model == wanted) { return Some((wanted.to_string(), None)); }
    offered.iter().filter_map(|model| {
        let level = wanted.strip_prefix(model.as_str())?.strip_prefix('-')?;
        matches!(level, "low" | "medium" | "high" | "xhigh" | "max").then(|| (model.clone(), Some(level.to_string())))
    }).max_by_key(|(model, _)| model.len())
}

/// `agy` where its installer puts it, on PATH, or in the usual prefixes. A
/// desktop app launched from the Dock has no shell PATH, so the installer's
/// directory is checked first.
pub fn find_program(home: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "agy.exe" } else { "agy" };
    let mut candidates = vec![home.join(".local/bin").join(name)];
    let path = std::env::var_os("PATH").unwrap_or_default();
    candidates.extend(std::env::split_paths(&path).map(|dir| dir.join(name)));
    candidates.extend([PathBuf::from("/opt/homebrew/bin").join(name), PathBuf::from("/usr/local/bin").join(name)]);
    candidates.into_iter().find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_auth_refreshes_private_cache_without_changing_credentials_or_environment() {
        let dir = tempfile::tempdir().unwrap();
        prepare_file_auth(dir.path()).unwrap();
        let marker = dir.path().join(".gemini/antigravity-cli/cache/antigravity-keyring-unavailable");
        let token = token_path(dir.path());
        std::fs::write(&token, "account-owned-token").unwrap();
        std::fs::OpenOptions::new().write(true).open(&marker).unwrap().set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH)).unwrap();
        prepare_file_auth(dir.path()).unwrap();
        assert!(marker.metadata().unwrap().modified().unwrap() > std::time::SystemTime::now() - std::time::Duration::from_secs(5));
        assert_eq!(std::fs::read_to_string(token).unwrap(), "account-owned-token");
        let env = environment_for_account(Some(dir.path())).resolve([
            ("SSH_CONNECTION".into(), "ambient SSH connection".into()), ("SSH_CLIENT".into(), "ambient client".into()), ("SSH_TTY".into(), "/dev/tty".into()),
        ]);
        assert!(env.keys().all(|name| !name.starts_with("SSH_")));
        assert_eq!(login_arguments(Some(dir.path())), ["--mode", "plan"]);
        std::fs::remove_file(token_path(dir.path())).unwrap();
        let login = login_arguments(Some(dir.path()));
        assert_eq!(login.first().map(String::as_str), Some("acp"));
        assert_eq!(login.last().map(String::as_str), Some("--login"));
        assert_eq!(PathBuf::from(&login[2]), dir.path().join(".gemini"));
        assert_eq!(login[4], "antigravity-cli", "the language server requires this subdirectory to be relative");
        assert!(login_arguments(None).is_empty());
    }

    #[test]
    fn file_auth_refuses_a_shared_cache_directory() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared-cache");
        let profile = dir.path().join("profile");
        std::fs::create_dir_all(profile.join(".gemini")).unwrap();
        std::fs::create_dir(&shared).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&shared, profile.join(".gemini/cache")).unwrap();
        #[cfg(windows)]
        junction::create(&shared, profile.join(".gemini/cache")).unwrap();
        assert!(prepare_file_auth(&profile).is_err());
        assert_eq!(std::fs::read_dir(shared).unwrap().count(), 0);
    }

    #[test]
    fn an_isolated_login_requires_a_real_nonempty_token_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(require_isolated_token(dir.path()).is_err());
        std::fs::create_dir(dir.path().join(".gemini")).unwrap();
        let token = token_path(dir.path());
        std::fs::create_dir_all(token.parent().unwrap()).unwrap();
        std::fs::write(&token, "").unwrap();
        assert!(require_isolated_token(dir.path()).is_err());
        std::fs::write(&token, "provider-owned-token").unwrap();
        assert!(require_isolated_token(dir.path()).is_ok());
        std::fs::write(legacy_token_path(dir.path()), "legacy-account").unwrap();
        assert!(require_isolated_token(dir.path()).is_err());
        assert!(prepare_file_auth(dir.path()).is_err());
        std::fs::remove_file(legacy_token_path(dir.path())).unwrap();
        #[cfg(unix)]
        {
            std::fs::remove_file(&token).unwrap();
            let other = dir.path().join("shared-token");
            std::fs::write(&other, "main-account").unwrap();
            std::os::unix::fs::symlink(&other, &token).unwrap();
            assert!(require_isolated_token(dir.path()).is_err());
        }
    }

    #[test]
    fn extra_accounts_use_their_own_home_and_bypass_the_shared_keyring() {
        let default = environment_for_account(None).resolve([(String::from("HOME"), String::from("/main"))]);
        assert_eq!(default.get("HOME").map(String::as_str), Some("/main"));
        assert!(!default.contains_key("SSH_CONNECTION"));
        let extra = environment_for_account(Some(Path::new("/accounts/work")))
            .resolve([(String::from("HOME"), String::from("/main")), (String::from("AGY_ADC_AUTH"), String::from("true"))]);
        assert_eq!(extra["HOME"], "/accounts/work");
        assert_eq!(extra["USERPROFILE"], "/accounts/work");
        assert_eq!(PathBuf::from(&extra["APPDATA"]), Path::new("/accounts/work").join("AppData/Roaming"));
        assert_eq!(PathBuf::from(&extra["LOCALAPPDATA"]), Path::new("/accounts/work").join("AppData/Local"));
        for name in ["RUSTUP_HOME", "CARGO_HOME", "GIT_CONFIG_GLOBAL"] {
            assert!(extra.contains_key(name));
            assert!(!Path::new(&extra[name]).starts_with("/accounts/work"));
        }
        for name in ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"] {
            assert!(!extra.contains_key(name), "account isolation must not impersonate an SSH session");
        }
        assert!(!extra.contains_key("AGY_ADC_AUTH"), "ambient API/enterprise credentials must not replace the chosen account");
        let mut options = GeminiLaunchOptions::new("/w", "/agy");
        options.account_home = Some(PathBuf::from("/accounts/work"));
        assert_eq!(crate::agents::AgentLaunch::Gemini(options).environment(), account_environment(Some(Path::new("/accounts/work"))));
    }

    #[test]
    fn a_trusted_workspace_edits_and_runs_commands_and_an_inspected_one_plans() {
        let trusted = arguments(Some("gemini-3.1-pro-high"), Some("high"), PermissionStance::AcceptEdits, None);
        assert_eq!(&trusted[..5], ["-p=", "--input-format", "stream-json", "--output-format", "stream-json"]);
        assert!(trusted.windows(2).any(|pair| pair == ["--model", "gemini-3.1-pro-high"]));
        assert!(trusted.windows(2).any(|pair| pair == ["--effort", "high"]));
        assert!(trusted.windows(2).any(|pair| pair == ["--mode", "accept-edits"]));
        assert!(trusted.contains(&"--dangerously-skip-permissions".to_string()));

        let inspected = arguments(None, Some("default"), PermissionStance::ReadOnly, Some("16b66afc"));
        assert!(inspected.windows(2).any(|pair| pair == ["--mode", "plan"]));
        assert!(!inspected.contains(&"--dangerously-skip-permissions".to_string()), "read-only never skips a prompt");
        assert!(!inspected.contains(&"--effort".to_string()), "the default effort is no flag");
        assert!(inspected.ends_with(&["--conversation".to_string(), "16b66afc".to_string()]));
        assert_eq!(GeminiLaunchOptions::new("/w", "/agy").permission_mode, PermissionStance::ReadOnly);
        assert_eq!(conversation_path(Path::new("/Users/me"), "c1"), PathBuf::from("/Users/me/.gemini/antigravity-cli/conversations/c1.db"));
    }
}
