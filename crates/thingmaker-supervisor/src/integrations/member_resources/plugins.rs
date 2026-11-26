//! Plugins installed outside Claude Code's own registry
//! (`plugins/installed_plugins.json`): the ones claude.ai syncs for an
//! organization, and the ones Codex installs. Neither is loaded natively by
//! the other harness, so their skills are bridged like external skill folders.
//! A plugin that cannot be read is skipped; it never hides the rest of the catalog.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{ResourceEntry, add_plugin, bridge_skills, read};

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
        if let Some(problem) = disabled { entry.problem = Some(problem.into()); }
    }
}

/// claude.ai organization plugins: `synced/<organization>/manifest.json` lists
/// them, and each lives in `<name>` or, from its second generation, `<name>~g<n>`.
pub(super) fn add_synced_claude_plugins(out: &mut Vec<ResourceEntry>, synced: &Path) {
    let Ok(organizations) = std::fs::read_dir(synced) else { return };
    let mut organizations: Vec<PathBuf> = organizations.flatten().map(|entry| entry.path()).filter(|path| path.join("manifest.json").is_file()).collect();
    organizations.sort();
    for organization in organizations {
        let Ok(manifest) = read(&organization.join("manifest.json")) else { continue };
        for plugin in manifest["plugins"].as_array().into_iter().flatten() {
            let Some(name) = plugin["name"].as_str().filter(|name| !name.is_empty() && !name.contains(['/', '\\']) && *name != "..") else { continue };
            let folder = match plugin["generation"].as_u64() { Some(generation) if generation > 1 => format!("{name}~g{generation}"), _ => name.to_string() };
            let path = organization.join(folder);
            if !path.is_dir() { continue }
            let plugin_manifest = read(&path.join(".claude-plugin/plugin.json")).unwrap_or_else(|_| json!({}));
            let id = format!("{name}@claude.ai");
            add_bridged(out, &path, &plugin_manifest, &id, name, ".claude/plugins/synced", None);
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
        if [name, market].iter().any(|part| part.is_empty() || part.contains(['/', '\\']) || *part == "..") { continue }
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
}
