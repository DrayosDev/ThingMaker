//! Shared per-worker catalog. Public entries contain no MCP configuration
//! or credentials; resolution happens again in the host immediately before launch.
use std::{collections::{BTreeMap, HashMap}, path::{Path, PathBuf}};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{DesktopError, storage::content_hash};

#[cfg(windows)]
mod windows_stdio;
mod bundled;
mod imported;
mod plugins;
mod skill_dependencies;
mod skill_links;
pub use bundled::install as install_bundled_skills;
pub use imported::{ImportStatus, PROVIDERS as IMPORT_PROVIDERS, root as imports_root, setup as setup_import};
pub use skill_links::expand_skills;
pub use skill_dependencies::McpDependency;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
// No deny_unknown_fields: a field added later must not make an older build
// reject the whole team that carries it.
#[serde(rename_all = "camelCase")]
pub struct ResourceSelection {
    pub skills: Vec<String>,
    pub mcps: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceEntry {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub group: String,
    pub available: bool,
    pub problem: Option<String>,
    pub source: String,
    /// What an MCP server runs, or the host it connects to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub mcp_dependencies: Vec<McpDependency>,
    pub dependency_notes: Vec<String>,
    /// Ids of the skills this skill refers to (direct only; see `skill_links`).
    pub skill_dependencies: Vec<String>,
    /// Whose skill or server this is: claude, codex, gemini or thingmaker.
    pub provider: Option<String>,
    /// The ThingMaker skill this one is a word-for-word copy of. Both stay
    /// selectable; a member given both loads it once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
    #[serde(skip)]
    runtime_name: String,
    #[serde(skip)]
    config: Value,
    #[serde(skip)]
    revision: String,
    #[serde(skip)]
    skill_path: Option<PathBuf>,
    /// Hash of the SKILL.md this entry was read from.
    #[serde(skip)]
    skill_hash: String,
    #[serde(skip)]
    bridge_skill: bool,
    /// Listed from a plugin Claude Code did not install itself: it never
    /// takes a name from a definition listed before it.
    #[serde(skip)]
    yields: bool,
    /// The plugin a skill came from, by its short name.
    #[serde(skip)]
    plugin: Option<String>,
    /// What an import copies to keep a skill working: its plugin's root, or
    /// the skill's own folder.
    #[serde(skip)]
    unit_root: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceCatalog {
    pub entries: Vec<ResourceEntry>,
    pub notes: Vec<String>,
    /// Which providers' skills are imported copies.
    pub imports: Vec<ImportStatus>,
    #[serde(skip)]
    local_only: bool,
}

fn local_endpoint(config: &Value) -> bool {
    match config.get("url") {
        None => config["command"].is_string(),
        Some(endpoint) => endpoint.as_str().and_then(|url| reqwest::Url::parse(url).ok()).is_some_and(|url|
            matches!(url.scheme(), "http" | "https") && url.host_str().is_some_and(|host|
                host == "localhost" || host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()))),
    }
}

impl ResourceCatalog {
    pub fn local_only(mut self) -> Self {
        self.local_only = true;
        // The built-in Web connection has no definition here: its loopback,
        // token-protected endpoint is created per session by `web::prepare`.
        for entry in self.entries.iter_mut().filter(|entry| entry.available && entry.kind == "mcp" && entry.id != BUILTIN_WEB) {
            if !local_endpoint(&entry.config) {
                entry.available = false;
                entry.problem = Some("Local agents can only reach MCP servers on this computer".into());
            }
        }
        self
    }

    /// Candidate IDs remain visible, including incompatible definitions, so
    /// the selector can explain why a dependency cannot be activated here.
    pub fn for_provider(mut self, provider: crate::agents::Provider) -> Self {
        if provider == crate::agents::Provider::Codex {
            for entry in self.entries.iter_mut().filter(|entry| entry.kind == "mcp" && entry.config["type"] == "sse") {
                entry.available = false;
                entry.problem = Some("SSE MCP transport is not supported by Codex".into());
            }
        }
        self
    }
}

/// One ordinary folder name. Joined to a folder it stays inside it: no
/// separators, no `..`, and no `C:name`, which Windows reads as another drive.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':'])
}

fn read(path: &Path) -> Result<Value, DesktopError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| DesktopError::io(format!("Invalid JSON in {}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(_) => Err(DesktopError::io(format!("Cannot read {}", path.display()))),
    }
}

fn description(text: &str, parsed: Option<String>) -> String {
    let parsed = parsed.unwrap_or_default();
    if ![">", ">-", "|", "|-"].contains(&parsed.as_str()) { return parsed }
    let lines: Vec<&str> = text.lines().skip_while(|line| !line.starts_with("description:")).skip(1)
        .take_while(|line| line.starts_with(' ') || line.starts_with('\t') || line.is_empty()).map(str::trim).collect();
    lines.join(if parsed.starts_with('|') { "\n" } else { " " }).trim().to_string()
}

fn add_skills(out: &mut Vec<ResourceEntry>, directory: &Path, group: &str, prefix: &str, plugin: Option<&str>, enabled: bool) {
    let directories: Vec<PathBuf> = if directory.join("SKILL.md").is_file() { vec![directory.to_path_buf()] }
        else { std::fs::read_dir(directory).into_iter().flatten().flatten().map(|entry| entry.path()).collect() };
    for directory in directories {
        let file = directory.join("SKILL.md");
        let Ok(bytes) = std::fs::read(&file) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let front = super::skills::parse_frontmatter(&text);
        let Some(name) = directory.file_name().map(|name| name.to_string_lossy().into_owned()) else { continue };
        let runtime_name = plugin.map(|plugin| {
            let skill = front.name.as_deref().unwrap_or(&name);
            if skill.starts_with(&format!("{plugin}:")) { skill.to_string() } else { format!("{plugin}:{skill}") }
        }).unwrap_or_else(|| name.clone());
        let disabled = front.other.get("disable-model-invocation").is_some_and(|value| value == "true") || front.other.get("context").is_some_and(|value| value == "fork");
        let revision = content_hash(&bytes);
        out.push(ResourceEntry { id: format!("skill:{prefix}:{name}"), kind: "skill".into(), name: runtime_name.clone(),
            description: description(&text, front.description), group: group.into(), available: enabled && !disabled,
            problem: if !enabled { Some("Plugin disabled for this project".into()) } else { disabled.then(|| "Manual-only or forked skill; not supported by the member pilot".into()) }, runtime_name, config: Value::Null,
            revision: format!("{}:{revision}", file.display()), skill_hash: revision, source: if plugin.is_some() { "plugin".into() } else { ".claude/skills".into() },
            plugin: plugin.map(str::to_string), unit_root: Some(directory.clone()), skill_path: Some(file), ..Default::default() });
    }
}

fn add_mcps(out: &mut Vec<ResourceEntry>, value: &Value, group: &str, prefix: &str, plugin: Option<&str>, enabled: bool, plugin_root: Option<&Path>) {
    let servers = value.get("mcpServers").unwrap_or(value);
    let Some(servers) = servers.as_object() else { return };
    for (name, config) in servers {
        let valid = config.get("command").is_some_and(Value::is_string) || config.get("url").is_some_and(Value::is_string);
        let available = enabled && valid && config.get("disabled").and_then(Value::as_bool) != Some(true);
        let runtime_name = plugin.map(|plugin| format!("plugin:{plugin}:{name}")).unwrap_or_else(|| name.clone());
        let mut config = config.clone();
        if let Some(root) = plugin_root {
            replace_plugin_root(&mut config, &root.to_string_lossy());
        }
        out.push(ResourceEntry { id: format!("mcp:{prefix}:{name}"), kind: "mcp".into(), name: runtime_name.clone(),
            // Never put full URLs, arguments, environment or headers in the
            // public catalog: only the program or the host, in `target`.
            description: "MCP server".into(), group: group.into(), available, target: server_target(&config),
            problem: (!available).then(|| "Server disabled or unsupported transport".into()), runtime_name,
            revision: content_hash(serde_json::to_string(&config).unwrap_or_default().as_bytes()), config, source: if plugin.is_some() { "plugin".into() } else { ".claude".into() }, ..Default::default() });
    }
}

/// What a server runs or where it connects, so the person can see what they
/// are choosing: the program, or the URL's host. Never arguments, headers,
/// environment, user info or query, which can carry credentials.
fn server_target(config: &Value) -> Option<String> {
    if let Some(command) = config.get("command").and_then(Value::as_str) {
        return Some(command.to_string()).filter(|command| !command.is_empty());
    }
    let url = config.get("url").and_then(Value::as_str)?;
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    (!host.is_empty()).then(|| host.to_string())
}

/// Claude Code's own approval of a project's shared `.mcp.json` servers: a
/// server it would not start is never offered. Approvals count only from the
/// person's own files (user settings, `settings.local.json`, the project's
/// entry in `.claude.json`), never from the committed `.claude/settings.json`,
/// which the project's authors control; refusals count from all of them.
#[derive(Default)]
struct ProjectServerApproval {
    all: bool,
    enabled: Vec<String>,
    disabled: Vec<String>,
}

impl ProjectServerApproval {
    fn read(&mut self, value: &Value, trusted: bool) {
        let names = |key: &str| value.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
        if trusted {
            self.all |= value.get("enableAllProjectMcpServers").and_then(Value::as_bool) == Some(true);
            self.enabled.extend(names("enabledMcpjsonServers"));
        }
        self.disabled.extend(names("disabledMcpjsonServers"));
    }

    fn problem(&self, name: &str) -> Option<&'static str> {
        if self.disabled.iter().any(|disabled| disabled == name) {
            Some("Turned off for this project in Claude Code")
        } else if !self.all && !self.enabled.iter().any(|enabled| enabled == name) {
            Some("Project server not approved in Claude Code. Approve it there, then refresh.")
        } else {
            None
        }
    }
}

fn replace_plugin_root(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace("${CLAUDE_PLUGIN_ROOT}", root),
        Value::Array(items) => items.iter_mut().for_each(|item| replace_plugin_root(item, root)),
        Value::Object(items) => items.values_mut().for_each(|item| replace_plugin_root(item, root)),
        _ => {}
    }
}

fn add_codex_mcps(out: &mut Vec<ResourceEntry>, file: &Path, group: &str, prefix: &str) -> Result<(), DesktopError> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(DesktopError::io("Cannot read Codex MCP configuration")),
    };
    let config: toml::Table = text.parse().map_err(|_| DesktopError::io("Invalid Codex MCP configuration"))?;
    let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table) else { return Ok(()) };
    let mut normalized = Map::new();
    for (name, raw) in servers {
        let mut server = serde_json::to_value(raw).map_err(|_| DesktopError::io("Invalid Codex MCP server"))?;
        let Some(server) = server.as_object_mut() else { continue };
        if server.get("enabled") == Some(&json!(false)) { server.insert("disabled".into(), json!(true)); }
        if server.contains_key("url") {
            server.insert("type".into(), json!("http"));
            let mut headers = server.remove("http_headers").and_then(|value| value.as_object().cloned()).unwrap_or_default();
            if let Some(token) = server.get("bearer_token_env_var").and_then(Value::as_str) { headers.insert("Authorization".into(), json!(format!("Bearer ${{{token}}}"))); }
            server.insert("headers".into(), Value::Object(headers));
        }
        if let Some(names) = server.remove("env_vars").and_then(|value| value.as_array().cloned()) {
            let env = server.entry("env").or_insert_with(|| json!({})).as_object_mut().ok_or_else(|| DesktopError::io("Invalid Codex MCP environment"))?;
            for name in names.iter().filter_map(Value::as_str) { env.entry(name).or_insert_with(|| json!(format!("${{{name}}}"))); }
        }
        normalized.insert(name.clone(), Value::Object(server.clone()));
    }
    let start = out.len();
    add_mcps(out, &Value::Object(normalized), group, prefix, None, true, None);
    for entry in &mut out[start..] { entry.source = ".codex/config.toml".into(); }
    Ok(())
}

/// Account config directory, plus the project the member will work in. Project
/// identifiers deliberately omit absolute paths so exported templates can move.
/// One plugin's skills and MCP servers: its default `skills/` folder and
/// `.mcp.json`, plus whatever its manifest lists.
fn add_plugin(out: &mut Vec<ResourceEntry>, path: &Path, manifest: &Value, id: &str, group: &str, name: &str, enabled: bool) -> Result<(), DesktopError> {
    let start = out.len();
    if path.join("SKILL.md").is_file() { add_skills(out, path, group, id, Some(name), enabled); }
    add_skills(out, &path.join("skills"), group, id, Some(name), enabled);
    if let Some(paths) = manifest.get("skills") {
        for custom in paths.as_array().cloned().unwrap_or_else(|| vec![paths.clone()]) {
            if let Some(custom) = custom.as_str() { add_skills(out, &path.join(custom), group, id, Some(name), enabled); }
        }
    }
    add_mcps(out, &read(&path.join(".mcp.json"))?, group, id, Some(name), enabled, Some(path));
    if let Some(custom) = manifest.get("mcpServers") {
        if custom.is_object() { add_mcps(out, custom, group, id, Some(name), enabled, Some(path)); }
        else {
            for custom in custom.as_array().cloned().unwrap_or_else(|| vec![custom.clone()]) {
                if let Some(custom) = custom.as_str() { add_mcps(out, &read(&path.join(custom))?, group, id, Some(name), enabled, Some(path)); }
            }
        }
    }
    // Plugin skills reach shared files through the plugin's root.
    for entry in out[start..].iter_mut().filter(|entry| entry.kind == "skill") { entry.unit_root = Some(path.to_path_buf()); }
    Ok(())
}

pub fn catalog(home: &Path, config_dir: &Path, root: Option<&Path>) -> Result<ResourceCatalog, DesktopError> {
    catalog_with_homes(home, config_dir, &home.join(".codex"), root)
}

pub fn catalog_with_homes(home: &Path, config_dir: &Path, codex_home: &Path, root: Option<&Path>) -> Result<ResourceCatalog, DesktopError> {
    let mut entries = Vec::new();
    let mut settings = read(&config_dir.join("settings.json"))?;
    let mut project_dirs = Vec::new();
    if let Some(root) = root {
        // The project's folders: up to its repository's top, or the workspace
        // alone outside a repository. Never $HOME or above it: its .claude and
        // .codex are the person's own, already read as user resources.
        let real_home = home.canonicalize().ok();
        let below_home = |directory: &&Path| *directory != home && Some(*directory) != real_home.as_deref();
        let top = root.ancestors().take_while(below_home).find(|directory| directory.join(".git").exists()).unwrap_or(root);
        for directory in root.ancestors().take_while(below_home) {
            project_dirs.push(directory.to_path_buf());
            if directory == top { break }
        }
        project_dirs.reverse();
    }
    let mut enabled_plugins = BTreeMap::new();
    let mut disabled_mcps = Vec::new();
    let mut skill_overrides = BTreeMap::new();
    let mut approval = ProjectServerApproval::default();
    let mut merge = |value: &Value, trusted: bool| {
        approval.read(value, trusted);
        if let Some(plugins) = value.get("enabledPlugins").and_then(Value::as_object) {
            for (name, enabled) in plugins { enabled_plugins.insert(name.clone(), enabled.as_bool().unwrap_or(false)); }
        }
        if let Some(disabled) = value.get("disabledMcpServers").and_then(Value::as_array) {
            disabled_mcps.extend(disabled.iter().filter_map(Value::as_str).map(str::to_string));
        }
        if let Some(overrides) = value.get("skillOverrides").and_then(Value::as_object) {
            for (name, mode) in overrides { skill_overrides.insert(name.clone(), mode.as_str().unwrap_or("on").to_string()); }
        }
    };
    merge(&settings, true);
    for directory in &project_dirs {
        for (name, trusted) in [("settings.json", false), ("settings.local.json", true)] { merge(&read(&directory.join(".claude").join(name))?, trusted); }
    }
    // The default account stores .claude.json beside ~/.claude; added accounts
    // have their own merged copy inside CLAUDE_CONFIG_DIR.
    let account_file = config_dir.join(".claude.json");
    let default_file = home.join(".claude.json");
    settings = read(if account_file.exists() { &account_file } else { &default_file })?;
    add_skills(&mut entries, &config_dir.join("skills"), "user", "user", None, true);
    add_external_skills(&mut entries, &home.join(".agents/skills"), "user", "agents-user", ".agents/skills");
    add_external_skills(&mut entries, &codex_home.join("skills"), "user", "codex-user", ".codex/skills");
    add_external_skills(&mut entries, &codex_home.join("skills/.system"), "user", "codex-system", ".codex/skills/.system");
    add_external_skills(&mut entries, &home.join(".gemini/skills"), "user", "gemini-user", ".gemini/skills");
    add_external_skills(&mut entries, &thingmaker_skills_root(home), "thingmaker", "thingmaker", "ThingMaker");
    add_mcps(&mut entries, &settings["mcpServers"], "user", "user", None, true, None);
    add_codex_mcps(&mut entries, &codex_home.join("config.toml"), "user", "codex-user")?;
    for directory in &project_dirs {
        let relative = root.and_then(|root| root.strip_prefix(directory).ok()).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        let prefix = if relative.is_empty() { "project".into() } else { format!("project-parent:{relative}") };
        add_skills(&mut entries, &directory.join(".claude/skills"), "project", &prefix, None, true);
        add_external_skills(&mut entries, &directory.join(".agents/skills"), "project", &format!("agents-{prefix}"), ".agents/skills");
        add_external_skills(&mut entries, &directory.join(".codex/skills"), "project", &format!("codex-{prefix}"), ".codex/skills");
        add_codex_mcps(&mut entries, &directory.join(".codex/config.toml"), "project", &format!("codex-{prefix}"))?;
    }
    if let Some(root) = root {
        let local = &settings["projects"][root.to_string_lossy().as_ref()];
        approval.read(local, true);
        let start = entries.len();
        add_mcps(&mut entries, &read(&root.join(".mcp.json"))?, "project", "project", None, true, None);
        // Before precedence: an unapproved project server replaces nothing.
        for entry in entries[start..].iter_mut().filter(|entry| entry.available) {
            if let Some(problem) = approval.problem(&entry.runtime_name) {
                entry.available = false;
                entry.problem = Some(problem.into());
            }
        }
        add_mcps(&mut entries, &local["mcpServers"], "project", "local", None, true, None);
        if let Some(disabled) = local["disabledMcpServers"].as_array() {
            disabled_mcps.extend(disabled.iter().filter_map(Value::as_str).map(str::to_string));
        }
    }
    let registry = read(&config_dir.join("plugins/installed_plugins.json"))?;
    if registry.get("plugins").is_some() && registry["version"] != 2 {
        return Err(DesktopError::unsupported("Unsupported Claude plugin registry version"));
    }
    if let Some(plugins) = registry["plugins"].as_object() {
        for (id, installs) in plugins {
            let Some(installs) = installs.as_array() else { continue };
            let chosen = installs.iter().rev().find(|install| install["projectPath"].as_str().is_some_and(|path| root.is_some_and(|root| root == Path::new(path))))
                .or_else(|| installs.iter().rev().find(|install| install["scope"] == "user" || install["scope"] == "managed"));
            let Some(path) = chosen.and_then(|install| install["installPath"].as_str()).map(PathBuf::from) else { continue };
            let manifest = read(&path.join(".claude-plugin/plugin.json"))?;
            let name = manifest["name"].as_str().unwrap_or_else(|| id.split('@').next().unwrap_or(id));
            let enabled = enabled_plugins.get(id).copied().unwrap_or(false);
            add_plugin(&mut entries, &path, &manifest, id, &format!("plugin:{id}"), name, enabled)?;
        }
    }
    plugins::add_synced_claude_plugins(&mut entries, &config_dir.join("plugins/synced"), &settings["oauthAccount"]);
    plugins::add_codex_plugins(&mut entries, codex_home);
    for entry in &mut entries { entry.provider = provider_of(&entry.source).map(str::to_string); }
    let imports = imported::apply(&mut entries, &imported::root(home));
    // Later native definitions have precedence. A hidden duplicate cannot be
    // selected by origin when Claude itself resolves both by the same name.
    // The same manifest can explicitly list its default directories/files.
    // Keep its last occurrence before computing precedence.
    let mut unique = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() { unique.insert(entry.id.clone(), index); }
    entries = entries.into_iter().enumerate().filter(|(index, entry)| unique.get(&entry.id) == Some(index)).map(|(_, entry)| entry).collect();
    let mut winners = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.available {
            let key = (entry.kind.clone(), entry.runtime_name.clone(), entry.source.starts_with(".codex/config"));
            let previous: Option<usize> = winners.get(&key).copied();
            let user_skill_wins = entry.kind == "skill" && previous.is_some_and(|previous| entries[previous].group == "user");
            // A plugin synced from claude.ai or installed in Codex is new to
            // this list: a member that already selected a Claude plugin's
            // server of the same name keeps the server it selected.
            let yields = entry.yields && previous.is_some();
            if !user_skill_wins && !yields { winners.insert(key, index); }
        }
    }
    // ThingMaker's own skills are also installed where agents read them
    // natively (`big-thing` in ~/.agents/skills). The native copy is marked,
    // never turned off: a team saved with it keeps starting.
    let own: BTreeMap<String, String> = entries.iter().filter(|entry| entry.kind == "skill" && entry.group == "thingmaker").map(|entry| (entry.skill_hash.clone(), entry.id.clone())).collect();
    for (index, entry) in entries.iter_mut().enumerate() {
        if entry.kind == "skill" && entry.group != "thingmaker" { entry.duplicate_of = own.get(&entry.skill_hash).cloned(); }
        if entry.kind == "skill" && !entry.group.starts_with("plugin:") && matches!(entry.source.as_str(), ".claude/skills" | "imported-skills/claude")
            && skill_overrides.get(&entry.name).is_some_and(|mode| mode == "off" || mode == "user-invocable-only") {
            entry.available = false;
            entry.problem = Some("Skill disabled for model invocation in Claude settings".into());
        } else if entry.kind == "mcp" && !entry.source.starts_with(".codex/config") && disabled_mcps.contains(&entry.runtime_name) {
            entry.available = false;
            entry.problem = Some("Server disabled for this project".into());
        } else if entry.available && winners.get(&(entry.kind.clone(), entry.runtime_name.clone(), entry.source.starts_with(".codex/config"))) != Some(&index) {
            entry.available = false;
            entry.problem = Some("Replaced by another definition of the same name".into());
        }
    }
    if WEB_SUPPORTED {
        entries.push(ResourceEntry {
            id: BUILTIN_WEB.into(), name: "ThingMaker Web".into(), kind: "mcp".into(),
            description: "Allow this agent to search and read the web using ThingMaker.".into(),
            group: "builtin".into(), available: true, problem: None, source: "ThingMaker".into(),
            target: None, mcp_dependencies: vec![], dependency_notes: vec![],
            runtime_name: "thingmaker-web".into(), config: Value::Null, revision: "1".into(),
            provider: Some("thingmaker".into()), ..Default::default()
        });
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    skill_dependencies::annotate(&mut entries);
    skill_links::annotate(&mut entries);
    Ok(ResourceCatalog { entries, local_only: false, imports, notes: vec![
        "Shared skills: .claude, .agents, .codex and .gemini folders, ThingMaker's own skills, installed Claude plugins, plugins synced from claude.ai and enabled Codex plugins. Imported providers list their copies under ~/.thingmaker/imported-skills instead. Legacy commands, enterprise/synced skills and cloud connectors are not listed. Skills may depend on tools or syntax specific to their original harness.".into(),
        "Selection controls model-invoked skills and MCP servers. File access, slash commands, plugin hooks and managed policies are separate.".into(),
    ] })
}

/// Skills installed for ThingMaker itself, selectable by any agent.
pub fn thingmaker_skills_root(home: &Path) -> PathBuf {
    home.join(".thingmaker").join("skills")
}

/// Whose resource an origin is. `.agents/skills` is read natively by Codex.
fn provider_of(source: &str) -> Option<&'static str> {
    match source {
        ".claude/skills" | ".claude" | "plugin" | ".claude/plugins/synced" => Some("claude"),
        ".agents/skills" | ".codex/skills" | ".codex/skills/.system" | ".codex/config.toml" | ".codex/plugins" => Some("codex"),
        ".gemini/skills" => Some("gemini"),
        "ThingMaker" => Some("thingmaker"),
        _ => None,
    }
}

pub const BUILTIN_WEB: &str = "mcp:builtin:thingmaker-web";
/// The systems whose installer carries the web runtime (its own Node.js and Chromium).
pub const WEB_SUPPORTED: bool = cfg!(any(windows, target_os = "macos"));

pub struct ResolvedResources {
    pub builtin_web: bool,
    pub skills: Vec<String>,
    pub mcps: BTreeMap<String, Value>,
    pub revision: String,
    pub skill_sources: Vec<SelectedSkill>,
}

pub struct SelectedSkill {
    staged_revision: Option<blake3::Hash>,
    pub path: PathBuf,
    pub runtime_name: String,
    pub bridge: bool,
    pub name: String,
    pub description: String,
}

pub fn codex_instructions(resources: &ResolvedResources) -> String {
    let mut text = String::from("\n\n<skills_instructions>\n## Available skills\nThis member's skill catalog is fixed for this session. To use a listed skill, read its SKILL.md at the provided path. Resolve its supporting files relative to that skill directory. Skill files may contain instructions specific to their original harness; use only tools actually available here.\n");
    for skill in &resources.skill_sources {
        text.push_str(&format!("- {}\n", json!({"name":skill.name,"description":skill.description,"path":skill.path})));
    }
    text.push_str("</skills_instructions>\n");
    text
}

/// Applies one resolved snapshot to the provider launch, returning ownership of
/// any temporary bridges so the session actor can keep them alive.
pub fn apply_to_launch(mut resources: ResolvedResources, launch: &mut crate::agents::AgentLaunch) -> Result<Option<std::sync::Arc<tempfile::TempDir>>, DesktopError> {
    use crate::agents::AgentLaunch;
    let local_home = match &*launch {
        AgentLaunch::Claude(options) => options.local.as_ref().map(|local| local.home.as_path()),
        AgentLaunch::Codex(options) if options.local => options.codex_home.as_deref().and_then(Path::parent),
        _ => None,
    };
    let staged = if let Some(home) = local_home.filter(|_| !resources.skill_sources.is_empty()) {
        let temp = tempfile::Builder::new().prefix("member-skills-").tempdir_in(home).map_err(|error| DesktopError::io(error.to_string()))?;
        let mut plugins = Vec::new();
        let mut names = Vec::new();
        for (index, skill) in resources.skill_sources.iter_mut().enumerate() {
            let plugin_name = format!("tm-local-{index}");
            let plugin = temp.path().join(&plugin_name);
            let source = skill.path.parent().ok_or_else(|| DesktopError::io("Missing skill directory"))?;
            let directory = plugin.join("skills").join(source.file_name().ok_or_else(|| DesktopError::io("Invalid skill directory"))?);
            if temp.path().canonicalize().map_err(|e| DesktopError::io(e.to_string()))?.starts_with(source) {
                return Err(DesktopError::io("The skill directory contains the local session folder"));
            }
            copy_skill_files(source, &directory).map_err(|e| DesktopError::io(format!("Cannot stage {}: {e}", skill.name)))?;
            if let Some(expected) = skill.staged_revision {
                let staged = skill_directory_hash(&directory).map_err(|e| DesktopError::io(e.to_string()))?;
                if staged != expected {
                    return Err(DesktopError::conflict(format!("{} changed while its files were being staged. Retry the launch.", skill.name)));
                }
            }
            skill.path = directory.join("SKILL.md").canonicalize().map_err(|e| DesktopError::io(e.to_string()))?;
            let manifest = plugin.join(".claude-plugin");
            std::fs::create_dir(&manifest).and_then(|_| std::fs::write(manifest.join("plugin.json"), json!({"name":plugin_name,"version":"1.0.0"}).to_string())).map_err(|e| DesktopError::io(e.to_string()))?;
            let text = std::fs::read_to_string(&skill.path).map_err(|e| DesktopError::io(e.to_string()))?;
            let name = super::skills::parse_frontmatter(&text).name.unwrap_or_else(|| directory.file_name().unwrap_or_default().to_string_lossy().into_owned());
            names.push(format!("{plugin_name}:{name}"));
            plugins.push(plugin);
        }
        if let AgentLaunch::Claude(options) = launch {
            options.resource_plugins = plugins;
            resources.skills = names;
        }
        Some(std::sync::Arc::new(temp))
    } else { None };
    match launch {
        crate::agents::AgentLaunch::Claude(options) => {
            if staged.is_some() {
                options.resources = Some((resources.skills, resources.mcps));
                return Ok(staged);
            }
            let prepared = prepare_claude_skills(&resources)?;
            if let Some(prepared) = &prepared { options.resource_plugins = prepared.plugins.clone(); }
            options.resources = Some((resources.skills, resources.mcps));
            Ok(prepared.map(|prepared| prepared.files))
        }
        crate::agents::AgentLaunch::Codex(options) => {
            let instructions = codex_instructions(&resources);
            options.developer_instructions = Some(format!("{}{instructions}", options.developer_instructions.as_deref().unwrap_or_default()));
            options.resources = Some(crate::agents::codex::launch::CodexResources {
                skills: resources.skill_sources.into_iter().map(|skill| skill.path).collect(), mcps: crate::agents::codex::resources::server_names(resources.mcps),
            });
            Ok(staged)
        }
        _ => Err(DesktopError::unsupported("This harness does not support member resources")),
    }
}

/// Hash the exact tree a local worker receives. Sorting and framing the entries
/// makes the digest independent of filesystem enumeration order.
fn skill_directory_hash(directory: &Path) -> std::io::Result<blake3::Hash> {
    let mut entries = std::fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut tree = blake3::Hasher::new();
    for entry in entries {
        let name = entry.file_name();
        let bytes = name.as_encoded_bytes();
        tree.update(&(bytes.len() as u64).to_le_bytes()).update(bytes);
        let kind = entry.file_type()?;
        let digest = if kind.is_dir() {
            tree.update(b"d");
            skill_directory_hash(&entry.path())?
        } else if kind.is_file() {
            tree.update(b"f");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                tree.update(&entry.metadata()?.permissions().mode().to_le_bytes());
            }
            let mut contents = blake3::Hasher::new();
            contents.update_reader(std::fs::File::open(entry.path())?)?;
            contents.finalize()
        } else {
            return Err(std::io::Error::other(format!("unsupported linked or special file: {}", entry.path().display())));
        };
        tree.update(digest.as_bytes());
    }
    Ok(tree.finalize())
}

fn copy_skill_files(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_skill_files(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        } else {
            return Err(std::io::Error::other(format!("unsupported linked or special file: {}", entry.path().display())));
        }
    }
    Ok(())
}

pub struct PreparedClaudeSkills {
    pub files: std::sync::Arc<tempfile::TempDir>,
    pub plugins: Vec<PathBuf>,
}

fn add_external_skills(out: &mut Vec<ResourceEntry>, directory: &Path, group: &str, prefix: &str, source: &str) {
    let start = out.len();
    add_skills(out, directory, group, prefix, None, true);
    bridge_skills(&mut out[start..], source);
}

/// Skills Claude Code does not load natively are linked into ephemeral
/// single-skill plugins at launch (see `prepare_claude_skills`); Codex reads
/// them by path either way.
fn bridge_skills(entries: &mut [ResourceEntry], source: &str) {
    for entry in entries.iter_mut().filter(|entry| entry.kind == "skill") {
        let folder = entry.skill_path.as_ref().and_then(|path| path.parent()).and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| entry.runtime_name.clone());
        // A plugin skill may name itself `plugin:skill`; its bridge supplies the prefix.
        let original = entry.skill_path.as_ref().and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| super::skills::parse_frontmatter(&text).name)
            .map(|name| name.rsplit(':').next().unwrap_or_default().to_string()).filter(|name| !name.is_empty()).unwrap_or(folder);
        let plugin = format!("tm-{}", &content_hash(entry.id.as_bytes())[..16]);
        entry.runtime_name = format!("{plugin}:{original}");
        entry.name = original;
        entry.source = source.into();
        entry.bridge_skill = true;
    }
}

/// Only selected external directories are linked into ephemeral, skill-only
/// plugins. Their scripts/resources stay together and no global files change.
pub fn prepare_claude_skills(resources: &ResolvedResources) -> Result<Option<PreparedClaudeSkills>, DesktopError> {
    let external: Vec<_> = resources.skill_sources.iter().filter(|skill| skill.bridge).collect();
    if external.is_empty() { return Ok(None) }
    let temp = std::sync::Arc::new(tempfile::Builder::new().prefix("thingmaker-member-skills-").tempdir().map_err(|_| DesktopError::io("Cannot prepare member skills"))?);
    let mut plugins = Vec::new();
    for skill in external {
        let (plugin_name, _) = skill.runtime_name.split_once(':').ok_or_else(|| DesktopError::io("Invalid bridged skill name"))?;
        let plugin = temp.path().join(plugin_name);
        let source = skill.path.parent().ok_or_else(|| DesktopError::io("Invalid skill directory"))?;
        let name = source.file_name().ok_or_else(|| DesktopError::io("Invalid skill directory name"))?;
        std::fs::create_dir_all(plugin.join(".claude-plugin")).and_then(|_| std::fs::create_dir_all(plugin.join("skills")))
            .and_then(|_| std::fs::write(plugin.join(".claude-plugin/plugin.json"), json!({"name":plugin_name,"version":"1.0.0"}).to_string()))
            .map_err(|_| DesktopError::io("Cannot create temporary member plugin"))?;
        let target = source.canonicalize().map_err(|_| DesktopError::not_ready("Selected skill directory is no longer available"))?;
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&target, plugin.join("skills").join(name));
        #[cfg(windows)]
        let linked = {
            let destination = plugin.join("skills").join(name);
            std::os::windows::fs::symlink_dir(&target, &destination).or_else(|error| {
                if error.raw_os_error() == Some(1314) { copy_skill_files(&target, &destination) } else { Err(error) }
            })
        };
        linked.map_err(|_| DesktopError::io("Cannot link the selected skill into its temporary plugin"))?;
        plugins.push(plugin);
    }
    Ok(Some(PreparedClaudeSkills { files: temp, plugins }))
}

pub fn resolve(catalog: &ResourceCatalog, selection: &ResourceSelection, environment: &HashMap<String, String>) -> Result<ResolvedResources, DesktopError> {
    let mut skills = Vec::new();
    let mut mcps = BTreeMap::new();
    let mut revisions = BTreeMap::new();
    let mut skill_sources = Vec::new();
    let mut selected_paths = std::collections::BTreeSet::new();
    for entry in catalog.entries.iter().filter(|entry| entry.kind == "skill" && selection.skills.contains(&entry.id)) {
        for dependency in entry.mcp_dependencies.iter().filter(|dependency| dependency.required) {
            if !dependency.mcp_ids.iter().any(|id| selection.mcps.contains(id)) {
                return Err(DesktopError::not_ready(format!("Skill {} requires MCP {}. Enable a matching connection in this member's selector.", entry.name, dependency.name)));
            }
        }
    }
    // Skills the selection refers to come along; only explicit ones are saved.
    let skill_ids = expand_skills(catalog, &selection.skills);
    for (kind, ids) in [("skill", &skill_ids), ("mcp", &selection.mcps)] {
        for id in ids {
            if !WEB_SUPPORTED && kind == "mcp" && id == BUILTIN_WEB { continue; }
            let entry = catalog.entries.iter().find(|entry| entry.id == *id && entry.kind == kind && entry.available)
                .ok_or_else(|| DesktopError::not_ready(format!("Selected resource {id} is missing or disabled. Edit this member's selection.")))?;
            revisions.insert(id, entry.revision.clone());
            // The same skill twice, once as ThingMaker's own: load ThingMaker's.
            if kind == "skill" && entry.duplicate_of.as_ref().is_some_and(|own| skill_ids.contains(own)) { continue }
            if kind == "skill" {
                let path = entry.skill_path.as_ref().and_then(|path| path.canonicalize().ok()).ok_or_else(|| DesktopError::not_ready("Selected skill file is no longer available"))?;
                // A symlinked installation can expose the same physical skill
                // through several origins; load it once, preferring the first selection.
                if selected_paths.insert(path.clone()) {
                    skills.push(entry.runtime_name.clone());
                    let staged_revision = if catalog.local_only {
                        let source = path.parent().ok_or_else(|| DesktopError::io("Missing skill directory"))?;
                        let hash = skill_directory_hash(source).map_err(|e| DesktopError::io(format!("Cannot read {}: {e}", entry.name)))?;
                        revisions.insert(id, format!("{}:{hash}", entry.revision));
                        Some(hash)
                    } else { None };
                    skill_sources.push(SelectedSkill { staged_revision, path, runtime_name: entry.runtime_name.clone(), bridge: entry.bridge_skill, name: entry.name.clone(), description: entry.description.clone() });
                }
            }
            else if entry.id != BUILTIN_WEB {
                let mut config = entry.config.clone();
                expand_environment(&mut config, environment)?;
                if catalog.local_only && !local_endpoint(&config) {
                    return Err(DesktopError::not_ready("Local agents can only reach MCP servers on this computer"));
                }
                if mcps.insert(entry.runtime_name.clone(), config).is_some() {
                    return Err(DesktopError::conflict("Two selected MCP definitions use the same server name. Select one origin for that server."));
                }
            }
        }
    }
    skills.sort();
    skills.dedup();
    let revision = content_hash(serde_json::to_vec(&json!([revisions, mcps])).unwrap_or_default().as_slice());
    let builtin_web = WEB_SUPPORTED && selection.mcps.iter().any(|id| id == BUILTIN_WEB);
    if builtin_web && mcps.contains_key("thingmaker-web") {
        return Err(DesktopError::conflict("Choose one definition for thingmaker-web"));
    }
    Ok(ResolvedResources { builtin_web, skills, mcps, revision, skill_sources })
}

fn expand_environment(value: &mut Value, env: &HashMap<String, String>) -> Result<(), DesktopError> {
    match value {
        Value::String(text) => {
            let mut expanded = String::new();
            let mut remaining = text.as_str();
            while let Some(start) = remaining.find("${") {
                expanded.push_str(&remaining[..start]);
                let tail = &remaining[start + 2..];
                let Some(end) = tail.find('}') else { return Err(DesktopError::not_ready("Invalid MCP environment reference")) };
                let (name, default) = tail[..end].split_once(":-").map_or((&tail[..end], None), |(name, default)| (name, Some(default)));
                let replacement = env.get(name).map(String::as_str).or(default).ok_or_else(|| DesktopError::not_ready(format!("Selected MCP needs environment variable {name}")))?;
                expanded.push_str(replacement);
                remaining = &tail[end + 1..];
            }
            expanded.push_str(remaining);
            *text = expanded;
        }
        Value::Array(items) => for item in items { expand_environment(item, env)?; },
        Value::Object(items) => for item in items.values_mut() { expand_environment(item, env)?; },
        _ => {}
    }
    Ok(())
}

/// Fail closed on older installations: this pilot was verified with these
/// adapter/SDK versions, rather than assuming unknown versions honor options.
pub fn check_harness(adapter_script: &Path) -> Result<(), DesktopError> {
    let package = adapter_script.parent().and_then(Path::parent).ok_or_else(|| DesktopError::not_ready("Cannot locate Claude ACP package"))?;
    let (adapter, sdk) = ((0, 106, 0), (0, 3, 322));
    for (file, minimum) in [(package.join("package.json"), adapter), (package.join("node_modules/@anthropic-ai/claude-agent-sdk/package.json"), sdk)] {
        let value = read(&file)?;
        let version = value["version"].as_str().unwrap_or("");
        let parts: Vec<u64> = version.split('.').map(|part| part.parse().unwrap_or(0)).collect();
        if parts.len() != 3 || (parts[0], parts[1], parts[2]) < minimum {
            return Err(DesktopError::not_ready(format!("Custom member resources require Claude ACP {}.{}.{}+ and Agent SDK {}.{}.{}+. Update the adapter before starting this member.", adapter.0, adapter.1, adapter.2, sdk.0, sdk.1, sdk.2)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn origins_collisions_and_disabled_plugins_are_visible_without_server_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let config = home.join(".claude");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let skill = "---\nname: draw\ndescription: Draw pictures\n---\n";
        write(&config.join("skills/draw/SKILL.md"), skill);
        write(&root.join(".claude/skills/draw/SKILL.md"), skill);
        write(&home.join(".claude.json"), r#"{"mcpServers":{"files":{"command":"npx","env":{"TOKEN":"secret-value"}}}}"#);
        let plugin = temp.path().join("plugin");
        write(&plugin.join(".claude-plugin/plugin.json"), r#"{"name":"art","skills":["./skills"]}"#);
        write(&plugin.join("skills/draw/SKILL.md"), skill);
        write(&config.join("plugins/installed_plugins.json"), &json!({"version":2,"plugins":{"art@market":[{"scope":"user","installPath":plugin}]}}).to_string());
        let found = catalog(&home, &config, Some(&root)).unwrap();
        assert!(!serde_json::to_string(&found).unwrap().contains("secret-value"));
        assert!(found.entries.iter().find(|entry| entry.id == "skill:user:draw").unwrap().available);
        assert!(!found.entries.iter().find(|entry| entry.id == "skill:project:draw").unwrap().available);
        let plugin_skill = found.entries.iter().filter(|entry| entry.id == "skill:art@market:draw").collect::<Vec<_>>();
        assert_eq!(plugin_skill.len(), 1);
        assert!(!plugin_skill[0].available);
        write(&config.join("settings.json"), r#"{"enabledPlugins":{"art@market":true}}"#);
        let enabled = catalog(&home, &config, Some(&root)).unwrap();
        assert!(enabled.entries.iter().find(|entry| entry.id == "skill:art@market:draw").unwrap().available);
    }

    #[test]
    fn explicit_empty_missing_and_environment_resolution_never_widen_selection() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join(".claude");
        write(&temp.path().join(".claude.json"), r#"{"mcpServers":{"files":{"command":"npx","env":{"TOKEN":"${TOKEN}"}}}}"#);
        let found = catalog(temp.path(), &config, None).unwrap();
        let empty = resolve(&found, &ResourceSelection::default(), &HashMap::new()).unwrap();
        assert!(empty.skills.is_empty() && empty.mcps.is_empty());
        let selected = ResourceSelection { skills: vec![], mcps: vec!["mcp:user:files".into()] };
        assert!(resolve(&found, &selected, &HashMap::new()).is_err());
        let resolved = resolve(&found, &selected, &HashMap::from([("TOKEN".into(), "credential".into())])).unwrap();
        assert_eq!(resolved.mcps["files"]["env"]["TOKEN"], "credential");
        let changed = resolve(&found, &selected, &HashMap::from([("TOKEN".into(), "rotated".into())])).unwrap();
        assert_ne!(resolved.revision, changed.revision);
        let missing = ResourceSelection { skills: vec!["skill:project:missing".into()], mcps: vec![] };
        assert!(resolve(&found, &missing, &HashMap::new()).is_err());
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn a_web_scout_skill_offers_the_builtin_connection_but_only_selecting_it_enables_web() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        write(&claude.join("skills/web-scout/SKILL.md"), "---\nname: web-scout\ndescription: Research with web-scout MCP tools.\n---\nUse web_search then web_read.");
        let found = catalog(home.path(), &claude, None).unwrap().local_only();
        let skill = found.entries.iter().find(|e| e.id == "skill:user:web-scout").unwrap();
        assert!(skill.mcp_dependencies.iter().any(|dep| dep.mcp_ids.iter().any(|id| id == BUILTIN_WEB)));
        let mut chosen = ResourceSelection { skills: vec![skill.id.clone()], mcps: vec![] };
        let without = resolve(&found, &chosen, &HashMap::new()).unwrap();
        assert!(!without.builtin_web);
        chosen.mcps.push(BUILTIN_WEB.into());
        let with = resolve(&found, &chosen, &HashMap::new()).unwrap();
        assert!(with.builtin_web);
        assert!(with.mcps.is_empty(), "the authenticated connection is created per session");
        assert_ne!(with.revision, without.revision);
    }

    #[test]
    fn plugin_mcps_expand_their_root_and_local_project_servers_override_user_servers() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join(".claude");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        write(&temp.path().join(".claude.json"), &json!({"mcpServers":{"same":{"command":"user"}},"projects":{root.to_string_lossy().as_ref():{"mcpServers":{"same":{"command":"local"}}}}}).to_string());
        write(&root.join(".mcp.json"), r#"{"mcpServers":{"same":{"command":"project"}}}"#);
        let plugin = temp.path().join("plugin");
        write(&plugin.join(".claude-plugin/plugin.json"), r#"{"name":"art"}"#);
        write(&plugin.join(".mcp.json"), r#"{"paint":{"command":"node","args":["${CLAUDE_PLUGIN_ROOT}/server.js"]}}"#);
        write(&config.join("settings.json"), r#"{"enabledPlugins":{"art@market":true}}"#);
        write(&config.join("plugins/installed_plugins.json"), &json!({"version":2,"plugins":{"art@market":[{"scope":"user","installPath":plugin}]}}).to_string());
        let found = catalog(temp.path(), &config, Some(&root)).unwrap();
        let selected = ResourceSelection { skills: vec![], mcps: vec!["mcp:local:same".into(), "mcp:art@market:paint".into()] };
        let resolved = resolve(&found, &selected, &HashMap::new()).unwrap();
        assert_eq!(resolved.mcps["same"]["command"], "local");
        assert_eq!(Path::new(resolved.mcps["plugin:art:paint"]["args"][0].as_str().unwrap()), plugin.join("server.js"));
        assert!(!found.entries.iter().find(|entry| entry.id == "mcp:user:same").unwrap().available);
    }

    #[test]
    fn local_resources_are_staged_for_each_harness_and_remote_servers_are_excluded() {
        use crate::agents::{AgentLaunch, claude::launch::ClaudeLaunchOptions, codex::CodexLaunchOptions, local::{LocalKind, LocalTarget}};
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        write(&home.join(".claude/skills/probe/SKILL.md"), "---\nname: probe\ndescription: Probe\n---\n");
        write(&home.join(".claude/skills/probe/note.txt"), "fixture");
        write(&home.join(".claude.json"), r#"{"mcpServers":{"remote":{"type":"http","url":"https://example.test"},"loopback":{"type":"http","url":"http://localhost:8800"}}}"#);
        let found = catalog(home, &home.join(".claude"), None).unwrap().local_only();
        assert!(!found.entries.iter().find(|entry| entry.id == "mcp:user:remote").unwrap().available);
        let selection = ResourceSelection { skills: vec!["skill:user:probe".into()], mcps: vec!["mcp:user:loopback".into()] };
        let mut claude = ClaudeLaunchOptions::new(home, "node", vec![]);
        claude.local = Some(LocalTarget { kind: LocalKind::Agent, model: "m".into(), served: "m".into(), window: 16384, home: home.into() });
        let mut codex = CodexLaunchOptions::new(home, "codex");
        codex.local = true;
        codex.codex_home = Some(home.join("codex"));
        for mut launch in [AgentLaunch::Claude(claude), AgentLaunch::Codex(codex)] {
            let resources = resolve(&found, &selection, &HashMap::new()).unwrap();
            let files = apply_to_launch(resources, &mut launch).unwrap().unwrap();
            assert_eq!(std::fs::read_to_string(files.path().join("tm-local-0/skills/probe/note.txt")).unwrap(), "fixture");
            match launch {
                AgentLaunch::Claude(options) => assert_eq!(options.resources.unwrap().0, ["tm-local-0:probe"]),
                AgentLaunch::Codex(options) => assert_eq!(options.resources.unwrap().skills, [files.path().join("tm-local-0/skills/probe/SKILL.md").canonicalize().unwrap()]),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn local_worker_revision_tracks_auxiliary_skill_files() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let directory = home.join(".claude/skills/helper");
        write(&directory.join("SKILL.md"), "# Helper");
        let found = catalog(home, &home.join(".claude"), None).unwrap().local_only();
        let selection = ResourceSelection { skills: vec!["skill:user:helper".into()], mcps: vec![] };
        let snapshot = || resolve(&found, &selection, &HashMap::new()).unwrap();
        let original = snapshot().revision;
        let script = directory.join("scripts/helper.py");
        write(&script, "first");
        let first = snapshot();
        assert_ne!(original, first.revision);
        write(&script, "other");
        let changed = snapshot().revision;
        assert_ne!(first.revision, changed);
        let mut options = crate::agents::codex::CodexLaunchOptions::new(home, "codex");
        options.local = true;
        options.codex_home = Some(home.join("codex"));
        assert!(apply_to_launch(first, &mut crate::agents::AgentLaunch::Codex(options)).is_err());
        std::fs::rename(&script, directory.join("scripts/tool.py")).unwrap();
        assert_ne!(changed, snapshot().revision);
        std::fs::remove_dir_all(directory.join("scripts")).unwrap();
        assert_eq!(original, snapshot().revision);
    }

    #[test]
    fn local_endpoints_fail_closed_before_and_after_expansion() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join(".claude.json"), r#"{"mcpServers":{"var":{"type":"http","url":"${MCP_URL}"},"bad":{"type":"http","url":"invalid"},"loop":{"type":"http","url":"http://localhost/${PATH}"}}}"#);
        let mut catalog = catalog(temp.path(), &temp.path().join(".claude"), None).unwrap().local_only();
        for name in ["var","bad"] {
            assert!(!catalog.entries.iter().find(|e| e.id == format!("mcp:user:{name}")).unwrap().available);
        }
        // Resolution validates the final definition independently of the catalog preview.
        let entry = catalog.entries.iter_mut().find(|e| e.id == "mcp:user:loop").unwrap();
        entry.config["url"] = json!("${MCP_URL}");
        let selection = ResourceSelection { skills: vec![], mcps: vec![entry.id.clone()] };
        let vars = HashMap::from([("MCP_URL".into(), "https://example.com".into())]);
        assert!(resolve(&catalog, &selection, &vars).is_err());
    }

    #[test]
    fn missing_or_old_harness_is_rejected_before_launch() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("dist/index.js");
        assert!(check_harness(&script).is_err());
        write(&temp.path().join("package.json"), r#"{"version":"0.106.0"}"#);
        write(&temp.path().join("node_modules/@anthropic-ai/claude-agent-sdk/package.json"), r#"{"version":"0.3.322"}"#);
        assert!(check_harness(&script).is_ok());
    }

    #[test]
    fn folded_descriptions_and_plugin_frontmatter_names_match_the_catalog() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("directory/SKILL.md"), "---\nname: art:paint\ndescription: >-\n  Paint pictures\n  in color\n---\nBody\n");
        let mut entries = Vec::new();
        add_skills(&mut entries, temp.path(), "plugin:art@market", "art@market", Some("art"), true);
        assert_eq!(entries[0].name, "art:paint");
        assert_eq!(entries[0].description, "Paint pictures in color");
    }

    #[test]
    fn shared_user_project_and_system_skills_keep_their_sources_and_supporting_files() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let body = "---\nname: art\ndescription: Draw\n---\nRead palette.txt\n";
        for source in [home.join(".agents/skills/art"), home.join(".codex/skills/art"), root.join(".agents/skills/art"), root.join(".codex/skills/art"), home.join(".codex/skills/.system/art")] {
            write(&source.join("SKILL.md"), body);
            write(&source.join("palette.txt"), "palette-marker");
        }
        let found = catalog(&home, &home.join(".claude"), Some(&root)).unwrap();
        for id in ["skill:agents-user:art", "skill:codex-user:art", "skill:agents-project:art", "skill:codex-project:art", "skill:codex-system:art"] {
            assert!(found.entries.iter().any(|entry| entry.id == id && entry.available));
        }
        let chosen = ResourceSelection { skills: vec!["skill:agents-user:art".into(), "skill:codex-project:art".into()], mcps: vec![] };
        let resolved = resolve(&found, &chosen, &HashMap::new()).unwrap();
        assert_ne!(resolved.skills[0], resolved.skills[1], "same-named skills get separate bridge namespaces");
        let prepared = prepare_claude_skills(&resolved).unwrap().unwrap();
        let directory = prepared.files.path().to_path_buf();
        assert_eq!(prepared.plugins.len(), 2);
        for plugin in &prepared.plugins {
            assert_eq!(std::fs::read_to_string(plugin.join("skills/art/palette.txt")).unwrap(), "palette-marker");
        }
        drop(prepared);
        assert!(!directory.exists(), "temporary bridges are removed with their owner");
        assert!(home.join(".agents/skills/art/palette.txt").exists());
    }

    #[test]
    fn native_codex_mcp_definitions_are_portable_but_same_named_origins_require_a_choice() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join(".claude.json"), r#"{"mcpServers":{"paint":{"command":"claude-paint"}}}"#);
        write(&temp.path().join(".codex/config.toml"), "[mcp_servers.paint]\nurl = \"https://paint.test/mcp\"\nbearer_token_env_var = \"PAINT_TOKEN\"\n");
        let found = catalog(temp.path(), &temp.path().join(".claude"), None).unwrap();
        assert!(found.entries.iter().filter(|entry| entry.kind == "mcp").all(|entry| entry.available));
        let environment = HashMap::from([("PAINT_TOKEN".into(), "not-for-renderer".into())]);
        let resolved = resolve(&found, &ResourceSelection { skills: vec![], mcps: vec!["mcp:codex-user:paint".into()] }, &environment).unwrap();
        assert_eq!(resolved.mcps["paint"]["headers"]["Authorization"], "Bearer not-for-renderer");
        assert!(!serde_json::to_string(&found).unwrap().contains("not-for-renderer"));
        assert!(resolve(&found, &ResourceSelection { skills: vec![], mcps: vec!["mcp:user:paint".into(), "mcp:codex-user:paint".into()] }, &environment).is_err());
    }

    #[test]
    fn a_workspace_outside_a_repository_never_reads_the_home_folder_as_its_project() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = home.join("work/project");
        std::fs::create_dir_all(&root).unwrap();
        write(&home.join(".codex/config.toml"), "[mcp_servers.paint]\ncommand = \"paint\"\n");
        write(&home.join(".claude/settings.json"), r#"{"enabledPlugins":{}}"#);
        write(&home.join("work/.codex/config.toml"), "[mcp_servers.parent]\ncommand = \"parent\"\n");
        let found = catalog(&home, &home.join(".claude"), Some(&root)).unwrap();
        assert!(found.entries.iter().find(|entry| entry.id == "mcp:codex-user:paint").unwrap().available, "the user's own server keeps its place");
        assert!(found.entries.iter().all(|entry| !entry.id.contains("project-parent")), "no folder above a non-repository workspace is a project folder");
        let resolved = resolve(&found, &ResourceSelection { skills: vec![], mcps: vec!["mcp:codex-user:paint".into()] }, &HashMap::new()).unwrap();
        assert_eq!(resolved.mcps["paint"]["command"], "paint");
        // Nor is $HOME itself, when it is the workspace.
        write(&home.join(".codex/skills/art/SKILL.md"), "---\nname: art\ndescription: Draw\n---\n");
        let at_home = catalog(&home, &home.join(".claude"), Some(&home)).unwrap();
        assert!(at_home.entries.iter().all(|entry| entry.group != "project" || entry.source == ".claude"), "{:?}", at_home.entries.iter().map(|entry| &entry.id).collect::<Vec<_>>());
        assert!(at_home.entries.iter().find(|entry| entry.id == "mcp:codex-user:paint").unwrap().available);
    }

    #[test]
    fn project_servers_need_claude_approval_and_show_only_their_program_or_host() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let config = home.join(".claude");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        write(&root.join(".mcp.json"), &json!({"mcpServers":{
            "listed":{"command":"listed-cli","args":["--token=secret-arg"]},
            "local":{"type":"http","url":"https://user:secret-pass@mcp.example.test/path?key=secret-query","headers":{"Authorization":"secret-header"}},
            "refused":{"command":"refused-cli"},
            "committed":{"command":"committed-cli"},
            "same":{"command":"project-same"},
        }}).to_string());
        write(&home.join(".claude.json"), &json!({"mcpServers":{"same":{"command":"user-same"}},
            "projects":{root.to_string_lossy().as_ref():{"enabledMcpjsonServers":["listed"]}}}).to_string());
        write(&root.join(".claude/settings.local.json"), r#"{"enabledMcpjsonServers":["local","refused"],"disabledMcpjsonServers":["refused"]}"#);
        // The committed project settings cannot approve their own servers.
        write(&root.join(".claude/settings.json"), r#"{"enabledMcpjsonServers":["committed","same"],"enableAllProjectMcpServers":true}"#);
        let found = catalog(&home, &config, Some(&root)).unwrap();
        let entry = |id: &str| found.entries.iter().find(|entry| entry.id == id).unwrap();
        assert!(entry("mcp:project:listed").available);
        assert!(entry("mcp:project:local").available);
        assert!(!entry("mcp:project:refused").available);
        assert!(!entry("mcp:project:committed").available);
        assert!(!entry("mcp:project:same").available);
        assert!(entry("mcp:user:same").available, "an unapproved project server replaces nothing");
        assert_eq!(entry("mcp:project:listed").target.as_deref(), Some("listed-cli"));
        assert_eq!(entry("mcp:project:local").target.as_deref(), Some("mcp.example.test"));
        let public = serde_json::to_string(&found).unwrap();
        assert!(!public.contains("secret"), "{public}");
        assert!(resolve(&found, &ResourceSelection { skills: vec![], mcps: vec!["mcp:project:committed".into()] }, &HashMap::new()).is_err());
        write(&config.join("settings.json"), r#"{"enableAllProjectMcpServers":true}"#);
        let all = catalog(&home, &config, Some(&root)).unwrap();
        assert!(all.entries.iter().find(|entry| entry.id == "mcp:project:committed").unwrap().available);
        assert!(!all.entries.iter().find(|entry| entry.id == "mcp:project:refused").unwrap().available, "a refusal still wins");
    }
}
