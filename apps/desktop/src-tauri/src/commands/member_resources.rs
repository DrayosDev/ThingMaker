//! Shared Claude/Codex member-resource catalog. Server secrets stay in Rust.
use std::path::{Path, PathBuf};
use serde::Serialize;
use std::collections::HashMap;
use tauri::State;
use thingmaker_supervisor::{DesktopError, agents::{Provider, accounts}, delegation::WorkerSlot,
    integrations::member_resources::{self, ResourceCatalog, ResolvedResources}};
use crate::state::AppState;
use super::{CommandResult, storage_error};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogView {
    #[serde(flatten)]
    catalog: ResourceCatalog,
    problem: Option<String>,
}

/// Skills shipped inside ThingMaker, installed in its own skills folder so
/// every agent's selector offers them.
const BUNDLED_SKILLS: [(&str, &str); 1] = [("big-thing", super::odyssey::SKILL_MD)];

/// Once, when the application starts: listing resources never writes. A
/// skill the person edited or removed there is left as they left it.
pub fn install_bundled_skills(home: Option<&Path>) {
    let Some(home) = home else { return };
    if let Err(error) = member_resources::install_bundled_skills(&member_resources::thingmaker_skills_root(home), &BUNDLED_SKILLS) {
        tracing::warn!(%error, "ThingMaker's own skills could not be installed");
    }
}

fn catalog_for(state: &AppState, root: Option<&Path>) -> CommandResult<ResourceCatalog> {
    let home = state.home.as_deref().ok_or_else(|| DesktopError::not_ready("HOME is not set"))?;
    let folder = state.try_account_folder(Provider::Claude, None)?.or_else(|| accounts::main_folder(Provider::Claude, home))
        .ok_or_else(|| DesktopError::not_ready("Claude account folder is unavailable"))?;
    let codex_folder = state.try_account_folder(Provider::Codex, None)?.or_else(|| accounts::main_folder(Provider::Codex, home))
        .ok_or_else(|| DesktopError::not_ready("Codex account folder is unavailable"))?;
    member_resources::catalog_with_homes(home, &folder, &codex_folder, root)
}

fn check_harness(state: &AppState, provider: Provider) -> CommandResult<()> {
    let resolved = state.resolve_provider(provider)?;
    if provider == Provider::Codex { return Ok(()) }
    let script = resolved.prefix_args.first().ok_or_else(|| DesktopError::not_ready("Claude ACP adapter script is unavailable"))?;
    member_resources::check_harness(Path::new(script))
}

fn local_catalog(state: &AppState, catalog: ResourceCatalog, root: Option<&Path>, harness: Provider, environment: &HashMap<String, String>) -> ResourceCatalog {
    let catalog = catalog.local_only();
    #[cfg(windows)]
    {
        let mut roots: Vec<_> = root.map(Path::to_owned).into_iter().collect();
        roots.push(thingmaker_supervisor::agents::local::local_home(&state.data_dir));
        roots.extend(["ProgramFiles", "ProgramFiles(x86)"].iter().filter_map(std::env::var_os).map(PathBuf::from));
        if let Some(windows) = std::env::var_os("SystemRoot") { roots.push(PathBuf::from(windows).join("System32")); }
        if let Some(user) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(user).join("Packages").join(thingmaker_appcontainer::PROFILE_NAME).join("AC"));
        }
        let mut env = environment.clone();
        if let Ok(provider) = state.resolve_provider(harness) {
            if let Some(dir) = provider.program.parent() {
                roots.push(dir.to_owned());
                let path = std::iter::once(dir.to_owned()).chain(std::env::split_paths(env.get("PATH").map(String::as_str).unwrap_or_default()));
                if let Ok(path) = std::env::join_paths(path) { env.insert("PATH".into(), path.to_string_lossy().into_owned()); }
            }
            if harness == Provider::Claude && let Some(dir) = provider.prefix_args.first().and_then(|script| super::session::adapter_package_root(Path::new(script))) {
                roots.push(dir);
            }
        }
        catalog.windows_stdio(root, &roots, &env)
    }
    #[cfg(not(windows))]
    { let _ = (state, root, harness, environment); catalog }
}

#[tauri::command]
pub fn member_resources_catalog(workspace_id: Option<String>, provider: Provider, local: Option<bool>, state: State<'_, AppState>) -> CommandResult<CatalogView> {
    if !matches!(provider, Provider::Claude | Provider::Codex) { return Err(DesktopError::unsupported("Member resources require Claude Code or Codex")) }
    let root = workspace_id.map(|id| state.with_storage(|s| s.workspace_get(&id)).map_err(storage_error)?
        .map(|record| PathBuf::from(record.canonical_root)).ok_or_else(|| DesktopError::not_ready("Workspace not found"))).transpose()?;
    let catalog = catalog_for(&state, root.as_deref())?.for_provider(provider);
    let local = local.unwrap_or(false);
    let catalog = if local {
        let env = super::session::launch_environment(&state).resolve(std::env::vars()).into_iter().collect();
        local_catalog(&state, catalog, root.as_deref(), provider, &env)
    } else { catalog };
    Ok(CatalogView { catalog, problem: check_harness(&state, provider).err().map(|error| error.message) })
}

/// Starts importing a provider's skills into ~/.thingmaker/imported-skills and
/// runs the first sync; later catalog builds keep the copies current.
#[tauri::command]
pub fn imported_skills_setup(provider: String, state: State<'_, AppState>) -> CommandResult<member_resources::ImportStatus> {
    let home = state.home.as_deref().ok_or_else(|| DesktopError::not_ready("HOME is not set"))?;
    member_resources::setup_import(&member_resources::imports_root(home), &provider)?;
    catalog_for(&state, None)?.imports.into_iter().find(|status| status.provider == provider)
        .ok_or_else(|| DesktopError::unsupported("Skills cannot be imported from this provider"))
}

pub fn resolve(state: &AppState, root: &Path, slot: &WorkerSlot) -> CommandResult<Option<ResolvedResources>> {
    resolve_for(state, root, slot.provider, slot.model.as_deref(), slot.resources.as_ref())
}

pub fn resolve_for(state: &AppState, root: &Path, provider: Provider, model: Option<&str>, selection: Option<&member_resources::ResourceSelection>) -> CommandResult<Option<ResolvedResources>> {
    let Some(selection) = selection else { return Ok(None) };
    let harness = if provider == Provider::Local { model.and_then(thingmaker_supervisor::agents::local::agent_harness).unwrap_or(provider) } else { provider };
    if !matches!(harness, Provider::Claude | Provider::Codex) {
        return Err(DesktopError::unsupported("Custom member resources require Claude Code or Codex. Switch provider or restore inherited resources."));
    }
    if selection.skills.len() + selection.mcps.len() > 5000 {
        return Err(DesktopError::limit_exceeded("Too many selected resources"));
    }
    check_harness(state, harness)?;
    let catalog = catalog_for(state, Some(root))?.for_provider(harness);
    let environment = super::session::launch_environment(state).resolve(std::env::vars()).into_iter().collect();
    let catalog = if provider == Provider::Local { local_catalog(state, catalog, Some(root), harness, &environment) } else { catalog };
    let resolved = member_resources::resolve(&catalog, selection, &environment)?;
    if resolved.mcps.contains_key(thingmaker_supervisor::delegation::mcp::SERVER_NAME) {
        return Err(DesktopError::conflict("A selected server uses ThingMaker's reserved team-server name"));
    }
    Ok(Some(resolved))
}
