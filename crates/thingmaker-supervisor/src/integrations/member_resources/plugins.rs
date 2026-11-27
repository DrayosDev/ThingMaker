//! Plugins installed outside Claude Code's own registry
//! (`plugins/installed_plugins.json`): the ones claude.ai syncs for an
//! organization, and the ones Codex installs. Neither is loaded natively by
//! the other harness, so their skills are bridged like external skill folders.
//! A plugin that cannot be read is skipped; it never hides the rest of the catalog.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{ResourceEntry, add_plugin, bridge_skills, plain_name, read};

/// Adds one plugin, grouped as `plugin:<id>`, then bridges its skills. A
/// disabled plugin stays listed with its reason. On failure, drops what it added.
fn add_bridged(out: &mut Vec<ResourceEntry>, path: &Path, manifest: &Value, id: &str, name: &str, source: &str, disabled: Option<&str>) {
    let start = out.len();
    if add_plugin(out, path, manifest, id, &format!("plugin:{id}"), name, disabled.is_none()).is_err() {
        out.truncate(start);
        return;
    }
    bridge_skills(&mut out[start..], source);
    for entry in &mut out[start..] {
        entry.source = source.into();
        entry.yields = true;
        if let Some(problem) = disabled { entry.problem = Some(problem.into()); }
    }
}

/// The folders claude.ai syncs for the signed-in account. Each is named
/// `<organization>_<account>`, and one computer can hold several: other
/// organizations of the same person, and other people's. Only the signed-in
/// organization's are the account's own; when the sign-in does not say which
/// that is, every folder is kept and told apart by its name.
fn synced_folders(synced: &Path, account: &Value) -> Vec<PathBuf> {
    let Ok(folders) = std::fs::read_dir(synced) else { return Vec::new() };
    let mut folders: Vec<PathBuf> = folders.flatten().map(|entry| entry.path()).filter(|path| path.join("manifest.json").is_file()).collect();
    folders.sort();
    let part = |path: &Path, wanted: &str| path.file_name().is_some_and(|name| name.to_string_lossy().split('_').any(|part| part.eq_ignore_ascii_case(wanted)));
    if let Some(organization) = account["organizationUuid"].as_str().filter(|id| !id.is_empty()) {
        folders.retain(|path| part(path, organization));
        if let Some(person) = account["accountUuid"].as_str().filter(|id| folders.iter().any(|path| part(path, id))) {
            folders.retain(|path| part(path, person));
        }
    }
    folders
}

/// claude.ai organization plugins: `synced/<organization>_<account>/manifest.json`
/// lists them, and each lives in `<name>` or, from its second generation,
/// `<name>~g<n>`. `account` is the signed-in `oauthAccount` of `.claude.json`.
pub(super) fn add_synced_claude_plugins(out: &mut Vec<ResourceEntry>, synced: &Path, account: &Value) {
    let folders = synced_folders(synced, account);
    for folder in &folders {
        let Ok(manifest) = read(&folder.join("manifest.json")) else { continue };
        // Two folders can sync a plugin of the same name: its folder then
        // belongs in the id, so neither replaces the other.
        let origin = if folders.len() == 1 { "claude.ai".to_string() } else { format!("claude.ai~{}", folder.file_name().unwrap_or_default().to_string_lossy()) };
        for plugin in manifest["plugins"].as_array().into_iter().flatten() {
            let Some(name) = plugin["name"].as_str().filter(|name| plain_name(name)) else { continue };
            let directory = match plugin["generation"].as_u64() { Some(generation) if generation > 1 => format!("{name}~g{generation}"), _ => name.to_string() };
            let path = folder.join(directory);
            if !path.is_dir() { continue }
            let plugin_manifest = read(&path.join(".claude-plugin/plugin.json")).unwrap_or_else(|_| json!({}));
            add_bridged(out, &path, &plugin_manifest, &format!("{name}@{origin}"), name, ".claude/plugins/synced", None);
        }
    }
}

/// The newest installed version of a cached Codex plugin: its `latest` link
/// when present, otherwise the most recently written version folder.
fn newest_version(plugin: &Path) -> Option<PathBuf> {
    let latest = plugin.join("latest");
    if latest.is_dir() { return latest.canonicalize().ok() }
    std::fs::read_dir(plugin).ok()?.flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.') && entry.path().is_dir())
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max().map(|(_, path)| path)
}

fn codex_manifest(path: &Path) -> Value {
    [".codex-plugin/plugin.json", ".claude-plugin/plugin.json"].iter()
        .map(|file| read(&path.join(file)).unwrap_or_else(|_| json!({})))
        .find(|manifest| manifest.as_object().is_some_and(|fields| !fields.is_empty()))
        .unwrap_or_else(|| json!({}))
}

/// Codex plugins: the `[plugins."name@marketplace"]` entries of `config.toml`,
/// installed under `plugins/cache/<marketplace>/<name>/<version>` or, for a
/// local marketplace, under its `plugins/<name>`; plus remote installs, which
/// carry a `.codex-remote-plugin-install.json` marker and no config entry.
pub(super) fn add_codex_plugins(out: &mut Vec<ResourceEntry>, codex_home: &Path) {
    let config = std::fs::read_to_string(codex_home.join("config.toml")).ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|table| serde_json::to_value(table).ok()).unwrap_or_else(|| json!({}));
    let cache = codex_home.join("plugins/cache");
    let mut plugins: Vec<(String, bool)> = config["plugins"].as_object().into_iter().flatten()
        .map(|(id, settings)| (id.clone(), settings["enabled"].as_bool().unwrap_or(true))).collect();
    for market in std::fs::read_dir(&cache).into_iter().flatten().flatten() {
        for plugin in std::fs::read_dir(market.path()).into_iter().flatten().flatten() {
            if !plugin.path().join(".codex-remote-plugin-install.json").is_file() { continue }
            let id = format!("{}@{}", plugin.file_name().to_string_lossy(), market.file_name().to_string_lossy());
            if !plugins.iter().any(|(known, _)| *known == id) { plugins.push((id, true)); }
        }
    }
    plugins.sort();
    for (id, enabled) in plugins {
        let Some((name, market)) = id.split_once('@') else { continue };
        if !plain_name(name) || !plain_name(market) { continue }
        let local = config["marketplaces"][market]["source"].as_str().filter(|_| config["marketplaces"][market]["source_type"] == "local")
            .map(|source| Path::new(source).join("plugins").join(name)).filter(|path| path.is_dir());
        let Some(path) = newest_version(&cache.join(market).join(name)).or(local) else { continue };
        let manifest = codex_manifest(&path);
        let display = manifest["name"].as_str().unwrap_or(name).to_string();
        add_bridged(out, &path, &manifest, &format!("codex:{id}"), &display, ".codex/plugins", (!enabled).then_some("Plugin disabled in Codex"));
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn synced_and_codex_plugins_are_listed_and_bridged() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        let skill = "---\nname: draw\ndescription: Draws\n---\n";
        // claude.ai: the manifest's generation picks the folder; an old one is ignored.
        let synced = config.join("plugins/synced/org");
        write(&synced.join("manifest.json"), &json!({"plugins":[{"name":"art","generation":2},{"name":"missing"}]}).to_string());
        write(&synced.join("art/skills/old/SKILL.md"), skill);
        write(&synced.join("art~g2/skills/draw/SKILL.md"), skill);
        // Codex: a cached install, a disabled one, a local marketplace and a remote install.
        let cache = codex.join("plugins/cache");
        let local = temp.path().join("runtime");
        write(&codex.join("config.toml"), &format!("[marketplaces.rt]\nsource_type = \"local\"\nsource = {:?}\n[plugins.\"paint@market\"]\nenabled = true\n[plugins.\"off@market\"]\nenabled = false\n[plugins.\"doc@rt\"]\nenabled = true\n", local.to_string_lossy()));
        write(&cache.join("market/paint/1.0.0/.codex-plugin/plugin.json"), r#"{"name":"paint","skills":"./skills/","mcpServers":"./.mcp.json"}"#);
        write(&cache.join("market/paint/1.0.0/skills/draw/SKILL.md"), skill);
        write(&cache.join("market/paint/1.0.0/.mcp.json"), r#"{"mcpServers":{"canvas":{"type":"http","url":"https://canvas.example/mcp"}}}"#);
        write(&cache.join("market/off/1.0.0/skills/draw/SKILL.md"), skill);
        write(&local.join("plugins/doc/skills/draw/SKILL.md"), skill);
        write(&cache.join("remote/notes/.codex-remote-plugin-install.json"), "{}");
        write(&cache.join("remote/notes/2.0.0/skills/draw/SKILL.md"), skill);

        let found = super::super::catalog_with_homes(home, &config, &codex, None).unwrap();
        let get = |id: &str| found.entries.iter().find(|entry| entry.id == id).unwrap_or_else(|| panic!("{id} missing"));
        let synced_skill = get("skill:art@claude.ai:draw");
        assert!(synced_skill.available && synced_skill.bridge_skill);
        assert_eq!((synced_skill.name.as_str(), synced_skill.group.as_str()), ("draw", "plugin:art@claude.ai"));
        assert!(synced_skill.runtime_name.starts_with("tm-") && synced_skill.runtime_name.ends_with(":draw"));
        assert!(found.entries.iter().all(|entry| entry.id != "skill:art@claude.ai:old"), "only the manifest's generation is read");
        assert!(get("skill:codex:paint@market:draw").available);
        assert_eq!(get("mcp:codex:paint@market:canvas").group, "plugin:codex:paint@market");
        let off = get("skill:codex:off@market:draw");
        assert!(!off.available);
        assert_eq!(off.problem.as_deref(), Some("Plugin disabled in Codex"));
        assert!(get("skill:codex:doc@rt:draw").available, "local marketplaces are read from their source");
        assert!(get("skill:codex:notes@remote:draw").available, "remote installs need no config entry");
    }

    #[test]
    fn a_plugin_from_elsewhere_never_takes_a_claude_plugins_server() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        let server = |command: &str| json!({"mcpServers":{"paint":{"command":command}}}).to_string();
        // The same plugin and server name, installed in Claude Code, in Codex twice, and synced.
        let native = temp.path().join("installed/art");
        write(&native.join(".claude-plugin/plugin.json"), r#"{"name":"art"}"#);
        write(&native.join(".mcp.json"), &server("claude-paint"));
        write(&config.join("plugins/installed_plugins.json"), &json!({"version":2,"plugins":{"art@market":[{"scope":"user","installPath":native}]}}).to_string());
        write(&config.join("settings.json"), r#"{"enabledPlugins":{"art@market":true}}"#);
        write(&config.join("plugins/synced/org/manifest.json"), r#"{"plugins":[{"name":"art"}]}"#);
        write(&config.join("plugins/synced/org/art/.mcp.json"), &server("synced-paint"));
        write(&codex.join("config.toml"), "[plugins.\"art@one\"]\nenabled = true\n[plugins.\"art@two\"]\nenabled = true\n");
        for market in ["one", "two"] { write(&codex.join("plugins/cache").join(market).join("art/1.0.0/.mcp.json"), &server("codex-paint")); }

        let found = super::super::catalog_with_homes(home, &config, &codex, None).unwrap();
        let get = |id: &str| found.entries.iter().find(|entry| entry.id == id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(get("mcp:art@market:paint").available, "the server a member already selected stays");
        for later in ["mcp:art@claude.ai:paint", "mcp:codex:art@one:paint", "mcp:codex:art@two:paint"] {
            assert_eq!(get(later).problem.as_deref(), Some("Replaced by another definition of the same name"), "{later}");
        }
        let selection = super::super::ResourceSelection { skills: vec![], mcps: vec!["mcp:art@market:paint".into()], ..Default::default() };
        let resolved = super::super::resolve(&found, &selection, &Default::default()).unwrap();
        assert_eq!(resolved.mcps["plugin:art:paint"]["command"], "claude-paint");

        // With Claude's own plugin turned off, the first of the others is offered.
        write(&config.join("settings.json"), r#"{"enabledPlugins":{"art@market":false}}"#);
        let found = super::super::catalog_with_homes(home, &config, &codex, None).unwrap();
        let available: Vec<&str> = found.entries.iter().filter(|entry| entry.kind == "mcp" && entry.available && entry.name == "plugin:art:paint").map(|entry| entry.id.as_str()).collect();
        assert_eq!(available, ["mcp:art@claude.ai:paint"]);
    }

    #[test]
    fn synced_plugins_are_the_signed_in_organizations_only() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        let skill = |text: &str| format!("---\nname: draw\ndescription: {text}\n---\n");
        // Two organizations of one person and another person of the first one, all syncing `art`.
        for folder in ["org-a_ana", "org-b_ana", "org-a_ben"] {
            let synced = config.join("plugins/synced").join(folder);
            write(&synced.join("manifest.json"), r#"{"plugins":[{"name":"art"},{"name":"C:art"},{"name":"../art"}]}"#);
            write(&synced.join("art/skills/draw/SKILL.md"), &skill(folder));
        }
        let ids = |found: &super::super::ResourceCatalog| found.entries.iter().filter(|entry| entry.kind == "skill").map(|entry| (entry.id.clone(), entry.description.clone())).collect::<Vec<_>>();

        write(&config.join(".claude.json"), r#"{"oauthAccount":{"organizationUuid":"org-a","accountUuid":"ana"}}"#);
        let found = super::super::catalog_with_homes(home, &config, &codex, None).unwrap();
        assert_eq!(ids(&found), [("skill:art@claude.ai:draw".to_string(), "org-a_ana".to_string())]);

        // The same person in the other organization sees that one's plugin under the same id.
        write(&config.join(".claude.json"), r#"{"oauthAccount":{"organizationUuid":"org-b","accountUuid":"ana"}}"#);
        let found = super::super::catalog_with_homes(home, &config, &codex, None).unwrap();
        assert_eq!(ids(&found), [("skill:art@claude.ai:draw".to_string(), "org-b_ana".to_string())]);

        // An organization with nothing synced lists none of the others'.
        write(&config.join(".claude.json"), r#"{"oauthAccount":{"organizationUuid":"org-c","accountUuid":"ana"}}"#);
        assert!(ids(&super::super::catalog_with_homes(home, &config, &codex, None).unwrap()).is_empty());

        // A sign-in that does not name its organization: every folder, each under its own id.
        write(&config.join(".claude.json"), "{}");
        let found = super::super::catalog_with_homes(home, &config, &codex, None).unwrap();
        assert_eq!(ids(&found).into_iter().map(|(id, _)| id).collect::<Vec<_>>(), ["skill:art@claude.ai~org-a_ana:draw", "skill:art@claude.ai~org-a_ben:draw", "skill:art@claude.ai~org-b_ana:draw"]);
    }
}
