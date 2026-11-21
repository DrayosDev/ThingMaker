use thingmaker_supervisor::{agents::Provider, continuity::{Identity, Store, MAX_BYTES, MAX_CHARS}};
use tempfile::TempDir;

fn identity(root: &TempDir, session: &str) -> Identity { Identity::new(root.path(), Provider::Codex, "default".into(), session.into()).unwrap() }

#[test]
fn immutable_full_chain_and_restart() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let a = identity(&root, "a"); let b = identity(&root, "b"); let c = identity(&root, "c");
    let store = Store::open(data.path()).unwrap();
    let first = store.save(&a, "Full A history").unwrap();
    assert!(store.bound(&a).unwrap().is_none());
    let retry = store.save(&a, "Full A history").unwrap();
    assert_ne!(first.id, retry.id);
    assert_eq!(std::fs::read_to_string(&first.path).unwrap(), "Full A history");
    store.attach(&a, &b, &first.id).unwrap(); drop(store);
    let store = Store::open(data.path()).unwrap();
    assert_eq!(store.bound(&b).unwrap().unwrap().id, first.id);
    let second = store.save(&b, "Full B history").unwrap();
    store.attach(&b, &c, &second.id).unwrap(); drop(store);
    let store = Store::open(data.path()).unwrap();
    let third = store.save(&c, "Full C history").unwrap();
    let page = store.read(&c, &third.id, None, None).unwrap();
    assert_eq!(page.text, "Full A history\n\n---\n\nFull B history\n\n---\n\nFull C history");
    assert_eq!(std::fs::read_to_string(first.path).unwrap(), "Full A history");
}

#[test]
fn unicode_pages_and_bounds() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap(); let a = identity(&root,"a");
    let store = Store::open(data.path()).unwrap(); let archive = store.save(&a, "A😀é中Z").unwrap();
    let p = store.read(&a, &archive.id, Some(1), Some(2)).unwrap();
    assert_eq!(p.text,"😀é"); assert_eq!(p.total_chars,5); assert_eq!(p.next_offset,Some(3));
    assert_eq!(store.read(&a,&archive.id,Some(5),None).unwrap().next_offset,None);
    for (offset,limit) in [(6,1),(0,0),(0,MAX_CHARS+1)] { assert!(store.read(&a,&archive.id,Some(offset),Some(limit)).is_err()); }
}

#[test]
fn scoped_access_workers_and_unbound_owner_denied_by_tool() {
    let data=TempDir::new().unwrap(); let root=TempDir::new().unwrap(); let other=TempDir::new().unwrap();
    let a=identity(&root,"a"); let b=identity(&root,"b"); let stranger=identity(&root,"stranger"); let foreign=identity(&other,"b");
    let store=Store::open(data.path()).unwrap(); let archive=store.save(&a,"private").unwrap();
    assert!(store.read(&stranger,&archive.id,None,None).is_err());
    assert!(store.attach(&a,&foreign,&archive.id).is_err());
    assert!(store.attach(&stranger,&b,&archive.id).is_err());
    assert!(store.tool_read(&a,true,Some(&archive.id),None,None).is_err());
    store.attach(&a,&b,&archive.id).unwrap();
    assert!(store.tool_read(&b,true,Some(&archive.id),None,None).is_ok());
    assert!(store.tool_read(&b,false,Some(&archive.id),None,None).is_err());
    assert!(store.tool_read(&b,false,None,None,None).is_err());
    assert!(store.tool_read(&foreign,true,Some(&archive.id),None,None).is_err());
    assert!(store.tool_read(&stranger,true,None,None,None).unwrap()["archive"].is_null());
    let other_provider=Identity::new(root.path(),Provider::Claude,"default".into(),"b".into()).unwrap();
    assert!(store.read(&other_provider,&archive.id,None,None).is_err());
}

#[test]
fn size_and_invalid_ids_fail_without_changing_bindings() {
    let data=TempDir::new().unwrap(); let root=TempDir::new().unwrap(); let a=identity(&root,"a"); let b=identity(&root,"b");
    let store=Store::open(data.path()).unwrap();
    assert!(store.save(&a,&"x".repeat(MAX_BYTES+1)).is_err());
    let archive=store.save(&a,&"x".repeat(MAX_BYTES)).unwrap(); store.attach(&a,&b,&archive.id).unwrap();
    assert!(store.save(&b,"new").is_err()); assert_eq!(store.bound(&b).unwrap().unwrap().id,archive.id);
    for id in ["", "../index.sqlite3","../../secret","not-a-uuid", "C:\\secret.md", "00000000-0000-0000-0000-000000000000"] {
        assert!(store.read(&a,id,None,None).is_err()); assert!(store.attach(&a,&b,id).is_err());
        assert!(store.tool_read(&b,true,Some(id),None,None).is_err());
    }
}

#[test]
fn accounts_are_isolated_and_explicit_transfer_requires_the_owner() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let source = identity(&root, "same-provider-session-id");
    let target = Identity::new(root.path(), Provider::Codex, "second-account".into(), "same-provider-session-id".into()).unwrap();
    let stranger = Identity::new(root.path(), Provider::Codex, "third-account".into(), "same-provider-session-id".into()).unwrap();
    let store = Store::open(data.path()).unwrap();
    let archive = store.save(&source, "only source owns this").unwrap();
    assert!(store.read(&target, &archive.id, None, None).is_err());
    assert!(store.tool_read(&target, true, Some(&archive.id), None, None).is_err());
    assert!(store.attach(&target, &stranger, &archive.id).is_err());
    store.attach(&source, &target, &archive.id).unwrap();
    assert!(store.read(&target, &archive.id, None, None).is_ok());
    assert!(store.tool_read(&target, true, Some(&archive.id), None, None).is_ok());
    assert!(store.read(&stranger, &archive.id, None, None).is_err());
    assert!(store.attach(&target, &stranger, &archive.id).is_err());
    drop(store);
    let store = Store::open(data.path()).unwrap();
    assert_eq!(store.bound(&target).unwrap().unwrap().id, archive.id);
    assert!(store.bound(&stranger).unwrap().is_none());
    let normalized = Identity::new(root.path(), Provider::Codex, String::new(), "same-provider-session-id".into()).unwrap();
    assert_eq!(normalized, source);
    assert!(store.read(&normalized, &archive.id, None, None).is_ok());
    assert!(Identity::new(root.path(), Provider::Codex, "default".into(), String::new()).is_err());
}

#[test]
fn bindings_are_idempotent_but_cannot_be_reassigned_or_self_attached() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let a = identity(&root, "a"); let b = identity(&root, "b");
    let store = Store::open(data.path()).unwrap();
    let first = store.save(&a, "first").unwrap();
    let second = store.save(&a, "second").unwrap();
    assert!(store.attach(&a, &a, &first.id).is_err());
    assert!(store.bound(&a).unwrap().is_none());
    store.attach(&a, &b, &first.id).unwrap();
    store.attach(&a, &b, &first.id).unwrap();
    assert!(store.attach(&a, &b, &second.id).is_err());
    assert_eq!(store.bound(&b).unwrap().unwrap().id, first.id);
    assert!(store.tool_read(&b, true, Some(&second.id), None, None).is_err());
    drop(store);
    let store = Store::open(data.path()).unwrap();
    assert!(store.attach(&a, &b, &second.id).is_err());
    assert_eq!(store.read(&b, &first.id, None, None).unwrap().text, "first");
}

#[test]
fn same_size_tamper_is_rejected_for_read_attach_and_inheritance() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let a = identity(&root, "a"); let b = identity(&root, "b"); let c = identity(&root, "c");
    let store = Store::open(data.path()).unwrap();
    let archive = store.save(&a, "original").unwrap();
    store.attach(&a, &b, &archive.id).unwrap();
    std::fs::write(&archive.path, "modified").unwrap();
    assert_eq!(std::fs::metadata(&archive.path).unwrap().len(), archive.bytes);
    assert!(store.read(&a, &archive.id, None, None).is_err());
    assert!(store.tool_read(&b, true, Some(&archive.id), None, None).is_err());
    assert!(store.attach(&a, &c, &archive.id).is_err());
    assert!(store.save(&b, "next").is_err());
    assert!(store.bound(&c).unwrap().is_none());
    assert_eq!(store.bound(&b).unwrap().unwrap().id, archive.id);
    drop(store);
    assert!(Store::open(data.path()).unwrap().read(&a, &archive.id, None, None).is_err());
}

#[test]
fn legacy_database_opens_but_missing_integrity_is_not_silently_trusted() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let a = identity(&root, "a");
    let dir = data.path().join("continuity"); std::fs::create_dir(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE archives(id TEXT PRIMARY KEY,owner TEXT NOT NULL,root TEXT NOT NULL,bytes INTEGER NOT NULL,created INTEGER NOT NULL); CREATE TABLE bindings(target TEXT PRIMARY KEY,archive TEXT NOT NULL);").unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let canonical = root.path().canonicalize().unwrap().to_string_lossy().into_owned();
    let owner = serde_json::to_string(&(&canonical, "codex", "default", "a")).unwrap();
    db.execute("INSERT INTO archives VALUES(?1,?2,?3,6,0)", rusqlite::params![id, owner, canonical]).unwrap();
    std::fs::write(dir.join(format!("{id}.md")), "legacy").unwrap(); drop(db);
    let store = Store::open(data.path()).unwrap();
    assert!(store.read(&a, &id, None, None).unwrap_err().message.contains("no integrity hash"));
    let fresh = store.save(&a, "fresh").unwrap();
    assert_eq!(store.read(&a, &fresh.id, None, None).unwrap().text, "fresh");
    drop(store);
    assert!(Store::open(data.path()).unwrap().read(&a, &fresh.id, None, None).is_ok());
}

#[test]
fn tool_discovery_instructs_complete_codepoint_traversal() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let a = identity(&root, "a"); let b = identity(&root, "b");
    let store = Store::open(data.path()).unwrap(); let text = "\u{1f600}".repeat(12_001);
    let archive = store.save(&a, &text).unwrap(); store.attach(&a, &b, &archive.id).unwrap();
    let discovery = store.tool_read(&b, true, None, None, None).unwrap();
    let note = discovery["note"].as_str().unwrap();
    assert!(note.contains("offset 0") && note.contains("nextOffset is null") && note.contains("entire archive"));
    let mut offset = Some(0); let mut restored = String::new();
    while let Some(start) = offset {
        let page = store.tool_read(&b, true, Some(&archive.id), Some(start), None).unwrap();
        restored.push_str(page["text"].as_str().unwrap());
        offset = page["nextOffset"].as_u64().map(|n| n as usize);
    }
    assert_eq!(restored, text);
}

#[cfg(unix)]
#[test]
fn symlink_archive_is_rejected() {
    let data=TempDir::new().unwrap(); let root=TempDir::new().unwrap(); let a=identity(&root,"a");
    let store=Store::open(data.path()).unwrap(); let archive=store.save(&a,"secret").unwrap();
    std::fs::remove_file(&archive.path).unwrap();
    std::os::unix::fs::symlink(root.path().join("outside"),&archive.path).unwrap();
    assert!(store.read(&a,&archive.id,None,None).is_err());
}

#[cfg(windows)]
#[test]
fn junction_storage_escape_is_rejected() {
    let data = TempDir::new().unwrap(); let outside = TempDir::new().unwrap();
    junction::create(outside.path(), data.path().join("continuity")).unwrap();
    assert!(Store::open(data.path()).is_err());
    junction::delete(data.path().join("continuity")).unwrap();
}
