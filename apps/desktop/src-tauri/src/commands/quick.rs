//! Quick chats: conversations that belong to no project.
//!
//! Each one is a workspace like any other, so every session feature works
//! unchanged, but its folder is an empty one ThingMaker makes for it under
//! `<data>/quick-chats/<id>` and trusts at once (there is nothing in it to
//! review). The list of quick chats, and which are temporary, is kept in one
//! setting.
//!
//! Deleting a quick chat stops its sessions, removes their transcripts from
//! the providers' folders (the same scoped removal as Wipe All, plus the
//! Local Models folders), forgets the workspace and removes its folder. A
//! temporary chat is deleted when ThingMaker quits, and any left by a crash
//! are deleted at the next start.
//!
//! What a provider keeps on its own side is not touched: there is no request
//! for that, so the interface says so beside the Temporary switch.

use std::{path::{Path, PathBuf}, sync::{LazyLock, Mutex}};

use serde::{Deserialize, Serialize};
use tauri::State;
use thingmaker_supervisor::{
    DesktopError,
    agents::{Provider, local::local_home},
    storage::{settings::GLOBAL_SCOPE, workspaces::{TrustState, WorkspaceRecord}},
};

use super::{CommandResult, storage_error};
use crate::state::AppState;

const QUICK_CHATS_KEY: &str = "quickChats";

/// One quick chat, as the sidebar lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickChat {
    pub workspace_id: String,
    pub temporary: bool,
    pub created_at: u64,
    /// When it became temporary. A temporary chat from before this start is
    /// waiting to be deleted, and is listed only so it can be hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporary_since: Option<u64>,
    /// Being deleted: the sidebar leaves it out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleting: bool,
}

/// When this run of ThingMaker started.
static STARTED_AT: LazyLock<u64> = LazyLock::new(now_ms);

/// Records the start, before anything can make a chat temporary.
pub fn mark_start() {
    LazyLock::force(&STARTED_AT);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Where quick chats' folders live.
pub fn quick_chats_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("quick-chats")
}

/// Held while the list is read, changed and written back, so a chat made or
/// changed while the start-up clean-up runs is not lost.
static LIST_LOCK: Mutex<()> = Mutex::new(());

fn read_list(state: &AppState) -> CommandResult<Vec<QuickChat>> {
    Ok(state.with_storage(|s| s.setting_get::<Vec<QuickChat>>(QUICK_CHATS_KEY, GLOBAL_SCOPE)).map_err(storage_error)?.unwrap_or_default())
}

fn write_list(state: &AppState, list: &[QuickChat]) -> CommandResult<()> {
    state.with_storage(|s| s.setting_set(QUICK_CHATS_KEY, GLOBAL_SCOPE, &list)).map_err(storage_error)
}

/// The quick chats whose workspace still exists, oldest first.
#[tauri::command]
pub fn quick_chat_list(state: State<'_, AppState>) -> CommandResult<Vec<QuickChat>> {
    let list = read_list(&state)?;
    let workspaces = state.with_storage(|s| s.workspace_list()).map_err(storage_error)?;
    let started = *STARTED_AT;
    Ok(list
        .into_iter()
        .filter(|chat| workspaces.iter().any(|w| w.id == chat.workspace_id))
        .map(|mut chat| {
            chat.deleting = chat.temporary && chat.temporary_since.unwrap_or(chat.created_at) < started;
            chat
        })
        .collect())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickChatCreated {
    pub chat: QuickChat,
    pub workspace: WorkspaceRecord,
}

/// Makes a new quick chat: an empty folder, registered and trusted.
#[tauri::command]
pub fn quick_chat_create(temporary: Option<bool>, state: State<'_, AppState>) -> CommandResult<QuickChatCreated> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let folder = quick_chats_dir(&state.data_dir).join(&id[..12]);
    std::fs::create_dir_all(&folder).map_err(|e| DesktopError::io(format!("could not make the chat's folder: {e}")))?;
    let identity = thingmaker_supervisor::workspace::WorkspaceIdentity::resolve(&folder)?;
    let workspace = state
        .with_storage(|s| {
            let record = s.workspace_upsert(&identity.canonical_root.to_string_lossy(), &identity.display_path, &identity.workspace_hash)?;
            // An empty folder ThingMaker made itself has nothing to review.
            let (_, digest) = super::workspace::inspect_sources(&identity.canonical_root);
            s.workspace_set_trust(&record.id, TrustState::TrustedLocal, Some(&digest))
        })
        .map_err(storage_error)?;
    let temporary = temporary.unwrap_or(false);
    let created_at = now_ms();
    let chat = QuickChat { workspace_id: workspace.id.clone(), temporary, created_at, temporary_since: temporary.then_some(created_at), deleting: false };
    let _guard = LIST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut list = read_list(&state)?;
    list.push(chat.clone());
    write_list(&state, &list)?;
    tracing::info!(workspace = %workspace.id, temporary = chat.temporary, "quick chat created");
    Ok(QuickChatCreated { chat, workspace })
}

/// Marks a quick chat temporary (deleted when ThingMaker quits) or not.
#[tauri::command]
pub fn quick_chat_set_temporary(workspace_id: String, temporary: bool, state: State<'_, AppState>) -> CommandResult<QuickChat> {
    let _guard = LIST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut list = read_list(&state)?;
    let chat = list.iter_mut().find(|chat| chat.workspace_id == workspace_id).ok_or_else(|| DesktopError::not_ready("that quick chat is gone"))?;
    chat.temporary = temporary;
    chat.temporary_since = temporary.then(now_ms);
    let chat = chat.clone();
    write_list(&state, &list)?;
    Ok(chat)
}

/// Deletes a quick chat: its sessions, their transcripts, the workspace and
/// its folder.
#[tauri::command]
pub async fn quick_chat_delete(workspace_id: String, state: State<'_, AppState>) -> CommandResult<()> {
    delete(&state, &workspace_id).await
}

pub(crate) async fn delete(state: &AppState, workspace_id: &str) -> CommandResult<()> {
    let list = read_list(state)?;
    if !list.iter().any(|chat| chat.workspace_id == workspace_id) {
        return Err(DesktopError::unsupported("only quick chats can be deleted this way"));
    }
    if let Some(record) = state.with_storage(|s| s.workspace_get(workspace_id)).map_err(storage_error)? {
        let stored = PathBuf::from(&record.canonical_root);
        let base = quick_chats_dir(&state.data_dir);
        let base = dunce::canonicalize(&base).unwrap_or(base);
        let root = chat_folder(&stored, &base)?;
        // A project opened from inside the chat's folder would go with it.
        let workspaces = state.with_storage(|s| s.workspace_list()).map_err(storage_error)?;
        if workspaces.iter().any(|w| w.id != workspace_id && same_or_inside(Path::new(&w.canonical_root), &root)) {
            return Err(DesktopError::conflict("A project inside this chat's folder is open in ThingMaker. Remove that project first."));
        }

        for id in state.actors_under(&root) {
            stop(state, &id).await;
        }

        // Local Models keep their transcripts in ThingMaker's own folders,
        // which Wipe All leaves alone.
        let local_ids: Vec<String> = state
            .with_storage(|s| s.session_list(workspace_id))
            .map_err(storage_error)?
            .into_iter()
            .filter(|session| session.provider == Provider::Local)
            .map(|session| session.agent_session_id)
            .collect();
        if !local_ids.is_empty() {
            let home = local_home(&state.data_dir);
            super::wipe::wipe_files(&root, &[home.join("claude")], &[home.join("codex")], &local_ids, &local_ids);
        }
        // Gemini keeps one database per conversation; extra accounts link to
        // the same folder.
        if let Some(home) = state.home.as_deref() {
            let gemini_ids: Vec<String> = state
                .with_storage(|s| s.session_list(workspace_id))
                .map_err(storage_error)?
                .into_iter()
                .filter(|session| session.provider == Provider::Gemini && super::wipe::safe_session_id(&session.agent_session_id))
                .map(|session| session.agent_session_id)
                .collect();
            for id in gemini_ids {
                let path = thingmaker_supervisor::agents::gemini::launch::conversation_path(home, &id);
                if let Err(error) = std::fs::remove_file(&path)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(%error, "could not remove a quick chat's Gemini conversation");
                }
            }
        }
        let report = super::wipe::wipe_workspace(state, workspace_id).await?;
        if !report.problems.is_empty() {
            tracing::warn!(workspace = %workspace_id, problems = ?report.problems, "quick chat deleted with problems");
        }
        // Every continuity archive of the chat, whichever provider it was on.
        if let Err(error) = super::continuity::forget_workspace(state, &root) {
            tracing::warn!(error = %error.message, "a deleted quick chat's continuity archives were not removed");
        }
        // The folder first: if it can't go, the chat stays listed and the
        // next start tries again.
        if let Err(error) = std::fs::remove_dir_all(&root)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(DesktopError::io(format!("could not remove the chat's folder: {error}")));
        }
        state.with_storage(|s| s.workspace_delete(workspace_id)).map_err(storage_error)?;
    }
    let _guard = LIST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let list: Vec<QuickChat> = read_list(state)?.into_iter().filter(|chat| chat.workspace_id != workspace_id).collect();
    write_list(state, &list)?;
    tracing::info!(workspace = %workspace_id, "quick chat deleted");
    Ok(())
}

/// The folder a quick chat may delete: a real folder (not a link) directly
/// inside ThingMaker's quick-chat folder. `base` is already canonical.
fn chat_folder(stored: &Path, base: &Path) -> CommandResult<PathBuf> {
    let refuse = || DesktopError::unsupported("that workspace is not a quick chat's folder");
    if stored.components().any(|part| matches!(part, std::path::Component::ParentDir | std::path::Component::CurDir)) {
        return Err(refuse());
    }
    let root = match std::fs::symlink_metadata(stored) {
        Ok(meta) if meta.file_type().is_symlink() => return Err(refuse()),
        Ok(_) => dunce::canonicalize(stored).map_err(|_| refuse())?,
        // Already gone: only its transcripts remain, found by the stored path.
        Err(_) => dunce::simplified(stored).to_path_buf(),
    };
    if root.parent() != Some(base) {
        return Err(refuse());
    }
    Ok(root)
}

/// Whether `path` is `root` or inside it, comparing Windows paths with and
/// without their `\\?\` prefix alike.
fn same_or_inside(path: &Path, root: &Path) -> bool {
    dunce::simplified(path).starts_with(dunce::simplified(root))
}

/// Stops one session the way the Stop button does.
async fn stop(state: &AppState, id: &str) {
    super::delegation::release(state, id).await;
    if let Some(engine) = state.engine.get() {
        engine.unlink_side(id);
        for side in engine.sides_of(id) {
            engine.unlink_side(&side);
        }
    }
    if let Some(actor) = state.remove_actor(id) {
        let _ = actor.stop().await;
    }
}

/// Deletes temporary quick chats: all of them when ThingMaker quits, and at
/// start only those from before it (a crash left them), never one made
/// temporary since.
pub(crate) async fn delete_temporary(state: &AppState, left_by_last_run: bool) {
    let started = *STARTED_AT;
    let Ok(list) = read_list(state) else { return };
    for chat in list.into_iter().filter(|chat| chat.temporary && (!left_by_last_run || chat.temporary_since.unwrap_or(chat.created_at) < started)) {
        if let Err(error) = delete(state, &chat.workspace_id).await {
            tracing::warn!(workspace = %chat.workspace_id, error = %error.message, "could not delete a temporary chat");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_quick_chat_is_a_trusted_empty_folder_and_deleting_it_removes_both() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::initialize(temp.path(), None).unwrap();
        let created = {
            let id = uuid::Uuid::new_v4().simple().to_string();
            let folder = quick_chats_dir(&state.data_dir).join(&id[..12]);
            std::fs::create_dir_all(&folder).unwrap();
            let identity = thingmaker_supervisor::workspace::WorkspaceIdentity::resolve(&folder).unwrap();
            let workspace = state
                .with_storage(|s| {
                    let record = s.workspace_upsert(&identity.canonical_root.to_string_lossy(), &identity.display_path, &identity.workspace_hash)?;
                    s.workspace_set_trust(&record.id, TrustState::TrustedLocal, None)
                })
                .unwrap();
            write_list(&state, &[QuickChat { workspace_id: workspace.id.clone(), temporary: true, created_at: 1, temporary_since: Some(1), deleting: false }]).unwrap();
            workspace
        };
        assert_eq!(created.trust_state, TrustState::TrustedLocal);
        let root = PathBuf::from(&created.canonical_root);
        std::fs::write(root.join("notes.txt"), "draft").unwrap();

        delete_temporary(&state, false).await;
        assert!(!root.exists(), "the chat's folder is removed");
        assert!(state.with_storage(|s| s.workspace_get(&created.id)).unwrap().is_none());
        assert!(read_list(&state).unwrap().is_empty());
    }

    #[tokio::test]
    async fn only_quick_chats_can_be_deleted_and_never_outside_their_folder() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::initialize(&temp.path().join("data"), None).unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let record = state
            .with_storage(|s| s.workspace_upsert(&project.to_string_lossy(), &project.to_string_lossy(), "h"))
            .unwrap();
        // Not on the list: refused.
        assert!(delete(&state, &record.id).await.is_err());
        // On the list but pointing outside the quick-chat folder: refused, folder kept.
        write_list(&state, &[QuickChat { workspace_id: record.id.clone(), temporary: true, created_at: 1, temporary_since: Some(1), deleting: false }]).unwrap();
        assert!(delete(&state, &record.id).await.is_err());
        assert!(project.exists());
    }

    #[test]
    fn only_a_real_folder_directly_inside_the_quick_chat_folder_is_deletable() {
        let temp = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(temp.path()).unwrap().join("quick-chats");
        std::fs::create_dir_all(base.join("chat1/inner")).unwrap();
        let outside = base.parent().unwrap().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        assert_eq!(chat_folder(&base.join("chat1"), &base).unwrap(), base.join("chat1"));
        // The folder itself, a deeper folder, `..` tricks and anything outside are refused.
        for path in [base.clone(), base.join("chat1/inner"), base.join("chat1/../.."), base.join("chat1/.."), outside.clone()] {
            assert!(chat_folder(&path, &base).is_err(), "{}", path.display());
        }
        // A link in the chat folder's place is refused, and its target kept.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, base.join("linked")).unwrap();
            assert!(chat_folder(&base.join("linked"), &base).is_err());
            assert!(outside.exists());
        }
    }

    #[tokio::test]
    async fn a_project_open_inside_a_chat_folder_stops_the_chat_being_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::initialize(temp.path(), None).unwrap();
        let folder = quick_chats_dir(&state.data_dir).join("abc123");
        std::fs::create_dir_all(folder.join("cloned")).unwrap();
        let chat = state.with_storage(|s| s.workspace_upsert(&dunce::canonicalize(&folder).unwrap().to_string_lossy(), "chat", "h1")).unwrap();
        let inner = dunce::canonicalize(folder.join("cloned")).unwrap();
        state.with_storage(|s| s.workspace_upsert(&inner.to_string_lossy(), "cloned", "h2")).unwrap();
        write_list(&state, &[QuickChat { workspace_id: chat.id.clone(), temporary: true, created_at: 1, temporary_since: Some(1), deleting: false }]).unwrap();
        assert!(delete(&state, &chat.id).await.is_err());
        assert!(inner.exists());
        assert_eq!(read_list(&state).unwrap().len(), 1, "the chat stays listed");
    }

    #[test]
    fn session_ids_that_could_name_a_shared_folder_are_never_used() {
        for id in ["", ".", "..", "../x", "a/b", "a\\b", "id with space"] {
            assert!(!super::super::wipe::safe_session_id(id), "{id:?}");
        }
        for id in ["0433d00e-911d-4cfe-ba39-4529764b03e7", "019a2b3c_def"] {
            assert!(super::super::wipe::safe_session_id(id));
        }
    }
}
