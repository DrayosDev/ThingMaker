//! Local stdio runs in the harness AppContainer. Advertise commands and entry
//! files only when their paths resolve under a directory available at launch.
use std::{collections::HashMap, path::{Component, Path, PathBuf, Prefix}};
use serde_json::Value;
use super::{ResourceCatalog, expand_environment};

const PRIVATE: &str = "This MCP needs files outside the Windows local agent's accessible folders. Place the server and its dependencies in the workspace or install it under Program Files.";
const MISSING: &str = "The local MCP executable or entry file is missing. Check its path and preview workspace.";

/// Query only a drive letter. MCP definitions must not cause the desktop to
/// contact a network share while inspecting the local catalog.
fn local_drive(path: &Path) -> bool {
    use windows_sys::Win32::{Storage::FileSystem::GetDriveTypeW, System::WindowsProgramming::{DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE, DRIVE_CDROM}};
    let Some(Component::Prefix(prefix)) = path.components().next() else { return false };
    let (Prefix::Disk(drive) | Prefix::VerbatimDisk(drive)) = prefix.kind() else { return false };
    let root = [u16::from(drive), u16::from(b':'), u16::from(b'\\'), 0];
    // SAFETY: root is a NUL-terminated UTF-16 drive root.
    matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, DRIVE_FIXED | DRIVE_RAMDISK | DRIVE_REMOVABLE | DRIVE_CDROM)
}

impl ResourceCatalog {
    pub fn windows_stdio(mut self, workspace: Option<&Path>, roots: &[PathBuf], env: &HashMap<String, String>) -> Self {
        let roots: Vec<_> = roots.iter().filter(|path| local_drive(path)).filter_map(|path| path.canonicalize().ok()).collect();
        for entry in self.entries.iter_mut().filter(|e| e.available && e.config["command"].is_string()) {
            let mut config = entry.config.clone();
            let check = expand_environment(&mut config, env).map_err(|error| error.message)
                .and_then(|()| check_paths(&config, workspace, &roots, env).map_err(str::to_owned));
            if let Err(reason) = check {
                entry.available = false;
                entry.problem = Some(reason);
            }
        }
        self
    }
}

fn check_paths(config: &Value, workspace: Option<&Path>, roots: &[PathBuf], env: &HashMap<String, String>) -> Result<(), &'static str> {
    let check = |path: &Path| {
        if !local_drive(path) { return Err(PRIVATE); }
        let real = path.canonicalize().map_err(|_| MISSING)?;
        if roots.iter().any(|root| real.starts_with(root)) { Ok(()) } else { Err(PRIVATE) }
    };
    let resolve = |base: Option<&Path>, value: &Path| {
        if value.is_absolute() { Some(value.to_owned()) } else { base.map(|dir| dir.join(value)) }
    };
    let cwd = match config["cwd"].as_str() {
        Some(value) => Some(resolve(workspace, Path::new(value)).ok_or(MISSING)?),
        None => workspace.map(Path::to_owned),
    };
    if let Some(dir) = &cwd { check(dir)?; }
    let command = config["command"].as_str().ok_or(MISSING)?;
    let program = Path::new(command);
    if program.components().count() > 1 || program.is_absolute() {
        check(&resolve(cwd.as_deref(), program).ok_or(MISSING)?)?;
    } else {
        let path = config["env"].as_object().and_then(|vars| vars.iter().find(|(k, _)| k.eq_ignore_ascii_case("PATH")))
            .and_then(|(_, value)| value.as_str()).or_else(|| env.get("PATH").map(String::as_str)).unwrap_or_default();
        let extensions: &[&str] = if program.extension().is_some() { &[""] } else { &[".exe", ".com", ".cmd", ".bat"] };
        let mut found = None;
        for dir in std::env::split_paths(path).filter(|dir| dir.is_absolute()) {
            if !local_drive(&dir) { return Err(PRIVATE); }
            found = extensions.iter().map(|ext| dir.join(format!("{command}{ext}"))).find(|file| file.is_file());
            if found.is_some() { break; }
        }
        let file = found.ok_or(MISSING)?;
        check(&file)?;
    }
    // Existing path arguments include scripts, config files and directories.
    // Preserve ordinary tool arguments, flags and package names.
    for arg in config["args"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let arg = if arg.starts_with('-') { arg.split_once('=').map_or("", |(_, value)| value) } else { arg };
        if arg.is_empty() { continue; }
        let candidate = Path::new(arg);
        if let Some(path) = resolve(cwd.as_deref(), candidate) {
            if !local_drive(&path) { return Err(PRIVATE); }
            if path.exists() { check(&path)?; }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn network_and_device_names_are_rejected_before_file_access() {
        for path in [r"\\server\share\tool.py", r"\\?\UNC\server\share\tool.py", r"\\.\pipe\local-mcp"] {
            assert!(!local_drive(Path::new(path)));
        }
    }

    #[test]
    fn catalog_and_launch_reject_private_scripts_without_granting_their_parent() {
        let home = tempfile::tempdir().unwrap();
        let workspace = home.path().join("project");
        std::fs::create_dir(&workspace).unwrap();
        for script in [workspace.join("tool.js"), home.path().join("private.js"), workspace.join("node.exe")] {
            std::fs::write(script, "fixture").unwrap();
        }
        let config = json!({"mcpServers":{
            "private":{"command":"node","args":["${ENTRY}"]},
            "relative":{"command":"node","args":["../private.js"]},
            "project":{"command":"node","args":["tool.js"]}
        }});
        std::fs::write(home.path().join(".claude.json"), config.to_string()).unwrap();
        let env = HashMap::from([
            ("PATH".to_owned(), workspace.display().to_string()),
            ("ENTRY".to_owned(), home.path().join("private.js").display().to_string())
        ]);
        let catalog = super::super::catalog(home.path(), &home.path().join(".claude"), Some(&workspace)).unwrap()
            .local_only().windows_stdio(Some(&workspace), std::slice::from_ref(&workspace), &env);
        for name in ["private", "relative", "project"] {
            let id = format!("mcp:user:{name}");
            let entry = catalog.entries.iter().find(|entry| entry.id == id).unwrap();
            assert_eq!(entry.available, name == "project");
            let selection = super::super::ResourceSelection { skills: vec![], mcps: vec![id], ..Default::default() };
            assert_eq!(super::super::resolve(&catalog, &selection, &env).is_ok(), name == "project");
        }
    }
}
