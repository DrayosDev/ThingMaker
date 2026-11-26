//! Native Gemini ACP with real session-local MCP relay and mock providers.
use std::{path::{Path, PathBuf}, sync::Arc, time::Duration};
use serde_json::{Value, json};
use thingmaker_supervisor::{
    DesktopError, acp::LaunchTarget,
    agents::{Provider, PermissionStance, gemini::GeminiLaunchOptions},
    delegation::{Combo, Delegation, JobStatus, WorkerLauncher, WorkerSpec, jobs::BoxFuture, socket::{DelegationSocket, server_entry, socket_path}},
    supervisor::{AgentLaunch, SessionActor, SessionActorConfig, SessionEvent, TurnEffect, TurnPhase},
};

fn node() -> PathBuf {
    let path = std::env::var_os("THINGMAKER_TEST_NODE").map(PathBuf::from).unwrap_or_else(|| "node".into());
    assert!(std::process::Command::new(&path).arg("--version").output().is_ok_and(|o| o.status.success()), "Node is required; put cached Node on PATH or set THINGMAKER_TEST_NODE");
    path
}

fn config(root: &Path, capture: &Path, flags: &[&str], resume: Option<&str>) -> SessionActorConfig {
    let program = node();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/test-fixtures/acp/mock-agy-acp.cjs").canonicalize().unwrap();
    let mut prefix_args = vec![fixture.to_string_lossy().into_owned(), format!("--capture={}", capture.display())];
    prefix_args.extend(flags.iter().map(|s| s.to_string()));
    let mut options = GeminiLaunchOptions::new(root, &program);
    options.permission_mode = PermissionStance::AcceptEdits;
    options.model = Some("gemini-test".into());
    options.effort = Some("high".into());
    options.resume = resume.map(str::to_string);
    options.initial_prompt_prefix = Some("You lead this session's team. Use delegate and await_jobs.".into());
    let mut config = SessionActorConfig::new(LaunchTarget { executable: program, prefix_args }, AgentLaunch::Gemini(options));
    config.timeouts.control = Duration::from_secs(10);
    config.timeouts.turn = Duration::from_secs(20);
    config
}

fn records(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

async fn turn(actor: &SessionActor, request: &str) -> Vec<SessionEvent> {
    let mut subscription = actor.subscribe();
    actor.submit_text(request, "Do the task").await.unwrap();
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(25), async {
        while let Some(envelope) = subscription.recv().await {
            let settled = matches!(envelope.payload, SessionEvent::Turn(TurnEffect::Settled { .. }));
            events.push(envelope.payload.clone());
            if settled { break; }
        }
    }).await.unwrap();
    assert!(events.iter().any(|event| matches!(event, SessionEvent::Turn(TurnEffect::Settled { phase: TurnPhase::Succeeded, .. }))), "{events:#?}");
    events
}

struct MockWorkers;
impl WorkerLauncher for MockWorkers {
    fn launch(&self, spec: WorkerSpec) -> BoxFuture<Result<SessionActor, DesktopError>> {
        Box::pin(async move {
            let mut config = config(&spec.root, &spec.root.join("worker.jsonl"), &[], None);
            if let AgentLaunch::Gemini(options) = &mut config.launch { options.native_acp = true; }
            SessionActor::open(config).await
        })
    }
    fn released(&self, _: &str) {}
}

fn combo() -> Combo {
    serde_json::from_value(json!({"workers":[{"name":"helper","provider":"gemini"}]})).unwrap()
}

fn server(socket: &DelegationSocket, token: &str) -> Value {
    server_entry(Path::new(env!("CARGO_BIN_EXE_thingmaker-mcp")), &[], socket.path(), token)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_new_and_load_forward_only_their_own_session_servers_and_config() {
    let dir = tempfile::tempdir().unwrap();
    let delegation = Delegation::new(Arc::new(MockWorkers), |_| {});
    let socket = DelegationSocket::bind(delegation.clone(), socket_path(dir.path())).unwrap();
    let shared = dir.path().join(".gemini/config");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("mcp_config.json"), "user-config-unchanged").unwrap();
    let mut actors = Vec::new();
    for (name, resume) in [("first", None), ("second", Some("saved-conversation"))] {
        let capture = dir.path().join(format!("{name}.jsonl"));
        let mut config = config(dir.path(), &capture, &[], resume);
        config.account_id = Some(format!("account-{name}"));
        let key = config.launch.session_id();
        let mut own_team = combo();
        own_team.workers[0].name = format!("helper-{name}");
        let token = delegation.register(&key, Provider::Gemini, dir.path().into(), own_team);
        let entry = server(&socket, &token);
        config.mcp_servers = vec![entry.clone()];
        let actor = SessionActor::open(config).await.unwrap();
        let snapshot = actor.snapshot().await.unwrap();
        assert_eq!(snapshot.account_id.as_deref(), Some(format!("account-{name}").as_str()));
        assert_eq!(serde_json::to_value(snapshot).unwrap()["accountId"], format!("account-{name}"));
        let log = records(&capture);
        let open = log.iter().find(|v| v["method"] == if resume.is_some() { "session/load" } else { "session/new" }).unwrap();
        assert_eq!(open["params"]["mcpServers"], json!([entry]));
        if let Some(id) = resume { assert_eq!(open["params"]["sessionId"], id); }
        for (id, value) in [("model", "gemini-test"), ("thought_level", "high"), ("mode", "accept-edits")] {
            assert!(log.iter().any(|v| v["method"] == "session/set_config_option" && v["params"]["configId"] == id && v["params"]["value"] == value));
        }
        actors.push((actor, key, token, name, capture));
    }
    assert_ne!(actors[0].2, actors[1].2);
    for (actor, key, token, name, capture) in actors {
        actor.set_config_option("thought_level", json!("low")).await.unwrap();
        turn(&actor, "first-request").await;
        actor.set_config_option("model", json!("gemini-next")).await.unwrap();
        turn(&actor, "second-request").await;
        assert_eq!(actor.snapshot().await.unwrap().account_id, Some(format!("account-{name}")));
        let log = records(&capture);
        let prompts: Vec<_> = log.iter().filter(|v| v["method"] == "session/prompt").collect();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0]["params"]["prompt"].as_array().unwrap().len(), 2);
        assert_eq!(prompts[1]["params"]["prompt"], json!([{"type":"text", "text":"Do the task"}]));
        assert_eq!(log.iter().filter(|v| v["method"] == "initialize").count(), 1, "native configuration must not replace the attachment or its MCP servers");
        assert!(log.iter().any(|v| v["method"] == "session/set_config_option" && v["params"]["value"] == "low"));
        let listed = log.iter().find(|v| v.get("workers").is_some()).unwrap().to_string();
        assert!(listed.contains(&format!("helper-{name}")), "{listed}");
        let other = if name == "first" { "second" } else { "first" };
        assert!(!listed.contains(&format!("helper-{other}")), "another session's team leaked: {listed}");
        actor.stop().await.unwrap();
        delegation.release(&key).await;
        assert!(delegation.caller_for_token(&token).is_none());
    }
    assert_eq!(std::fs::read_to_string(shared.join("mcp_config.json")).unwrap(), "user-config-unchanged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_leader_calls_team_relay_delegates_and_receives_worker_result() {
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("leader.jsonl");
    let delegation = Delegation::new(Arc::new(MockWorkers), |_| {});
    let socket = DelegationSocket::bind(delegation.clone(), socket_path(dir.path())).unwrap();
    let mut config = config(dir.path(), &capture, &["--delegate"], None);
    let key = config.launch.session_id();
    let token = delegation.register(&key, Provider::Gemini, dir.path().into(), combo());
    config.mcp_servers = vec![server(&socket, &token)];
    let actor = SessionActor::open(config).await.unwrap();
    let events = turn(&actor, "test-request").await;
    assert!(events.iter().any(|e| matches!(e, SessionEvent::PermissionRequest { .. })));
    let jobs = delegation.jobs(&key);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].status, JobStatus::Succeeded);
    assert_eq!(jobs[0].result.as_deref(), Some("worker-result"));
    let log = records(&capture);
    let prompt = log.iter().find(|v| v["method"] == "session/prompt").unwrap();
    assert!(prompt["params"]["prompt"][0]["text"].as_str().unwrap().contains("delegate"));
    assert_eq!(prompt["params"]["prompt"][1]["text"], "Do the task");
    assert!(log.iter().any(|v| v.get("delegationResult").is_some()));
    actor.stop().await.unwrap();
    delegation.release(&key).await;
    assert!(delegation.caller_for_token(&token).is_none());
}

#[tokio::test]
async fn native_load_failure_is_reported_instead_of_opening_a_plain_session() {
    let dir = tempfile::tempdir().unwrap();
    for flag in ["--fail-load", "--wrong-load", "--fail-init"] {
        let mut config = config(dir.path(), &dir.path().join("failed.jsonl"), &[flag], Some("missing"));
        config.mcp_servers = vec![json!({"name":"team","command":"unused","args":[],"env":[]})];
        assert!(SessionActor::open(config).await.is_err());
    }
}

#[tokio::test]
async fn native_permission_callback_is_denied_in_read_only_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("denied.jsonl");
    let mut config = config(dir.path(), &capture, &[], None);
    if let AgentLaunch::Gemini(options) = &mut config.launch {
        options.native_acp = true;
        options.permission_mode = PermissionStance::ReadOnly;
    }
    let actor = SessionActor::open(config).await.unwrap();
    let mut subscription = actor.subscribe();
    actor.submit_text("read-only", "Do the task").await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = subscription.recv().await {
            if let SessionEvent::Turn(TurnEffect::Settled { phase, .. }) = event.payload {
                assert_eq!(phase, TurnPhase::Cancelled);
                break;
            }
        }
    }).await.unwrap();
    assert!(records(&capture).iter().any(|v| v["permission"]["outcome"]["outcome"] == "cancelled"));
    actor.stop().await.unwrap();
}

#[tokio::test]
async fn native_account_without_private_token_fails_before_starting_agent() {
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("never-started.jsonl");
    let mut config = config(dir.path(), &capture, &[], None);
    if let AgentLaunch::Gemini(options) = &mut config.launch { options.account_home = Some(dir.path().join("missing-account")); }
    config.mcp_servers = vec![json!({"name":"team"})];
    assert!(SessionActor::open(config).await.is_err());
    assert!(!capture.exists());
}

#[test]
fn native_launch_anchors_profile_without_stream_flags_or_credentials_in_args() {
    let mut options = GeminiLaunchOptions::new("workspace", "agy");
    assert!(options.arguments().contains(&"-p=".into()));
    options.native_acp = true;
    options.account_home = Some("private-profile".into());
    options.resume = Some("saved".into());
    let args = options.arguments();
    assert_eq!(args[0], "acp");
    assert!(args.contains(&"--process-per-session".into()));
    assert!(args.windows(2).any(|pair| pair == ["--app-data-dir", "antigravity-cli"]));
    assert!(args.windows(2).any(|pair| pair[0] == "--gemini-dir" && Path::new(&pair[1]) == Path::new("private-profile/.gemini")));
    for flag in ["-p=", "--conversation", "--model", "--effort", "--dangerously-skip-permissions"] { assert!(!args.contains(&flag.into())); }
    assert!(Provider::Gemini.can_orchestrate());
    assert!(!Provider::Gemini.can_lead_goal());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_thinking_variants_select_the_native_model_and_level_separately() {
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("thinking.jsonl");
    let mut config = config(dir.path(), &capture, &[], None);
    if let AgentLaunch::Gemini(options) = &mut config.launch {
        options.native_acp = true;
        options.model = Some("gemini-test-high".into());
        options.effort = None;
    }
    let actor = SessionActor::open(config).await.unwrap();
    actor.set_config_option("thought_level", json!("low")).await.unwrap();
    actor.set_config_option("model", json!("gemini-test-high")).await.unwrap();
    let snapshot = actor.snapshot().await.unwrap();
    assert!(snapshot.config_options.as_array().unwrap().iter().any(|option| option["id"] == "thought_level" && option["currentValue"] == "high"));
    let log = records(&capture);
    assert!(log.iter().any(|entry| entry["method"] == "session/set_config_option" && entry["params"]["configId"] == "model" && entry["params"]["value"] == "gemini-test"));
    assert!(log.iter().any(|entry| entry["method"] == "session/set_config_option" && entry["params"]["configId"] == "thought_level" && entry["params"]["value"] == "high"));
    actor.stop().await.unwrap();
}

#[test]
fn an_exact_native_model_with_a_thinking_word_in_its_name_is_preserved() {
    use thingmaker_supervisor::agents::gemini::launch::native_model_selection;
    let offered = vec!["gpt-oss-120b-medium".into(), "gemini-4-neon".into()];
    assert_eq!(native_model_selection("gpt-oss-120b-medium", &offered), Some(("gpt-oss-120b-medium".into(), None)));
    assert_eq!(native_model_selection("gemini-4-neon-high", &offered), Some(("gemini-4-neon".into(), Some("high".into()))));
    assert_eq!(native_model_selection("gemini-4-neon-unknown", &offered), None);
}

/// One real turn of a native session, to its settling: the phase, what the
/// agent said, and the error if it failed.
async fn real_turn(actor: &SessionActor, request: &str, prompt: &str) -> (TurnPhase, String, Option<String>) {
    let mut events = actor.subscribe();
    actor.submit_text(request, prompt).await.unwrap();
    tokio::time::timeout(Duration::from_secs(180), async {
        let mut text = String::new();
        while let Some(event) = events.recv().await {
            match &event.payload {
                SessionEvent::Update(thingmaker_supervisor::acp::SessionUpdate::AgentMessage(message)) => {
                    for block in &message.content {
                        if let thingmaker_supervisor::acp::updates::ContentBlock::Text { text: chunk } = block { text.push_str(chunk); }
                    }
                }
                SessionEvent::Turn(TurnEffect::Settled { phase, error, .. }) => return (*phase, text, error.clone()),
                _ => {}
            }
        }
        panic!("the real provider stream closed before settling");
    }).await.expect("the real leader finished within the test budget")
}

/// A real Gemini session that leads a team, twice over: it opens on native
/// ACP with the session's own team server, delegates once and settles; then
/// it is stopped and the same conversation is reopened (`session/load`) with
/// a new team server, and still knows what its worker reported. Two real
/// leader turns; the delegated worker is the deterministic ACP fixture.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "uses one real Gemini conversation on the signed-in default account"]
async fn a_real_gemini_leader_delegates_reads_the_worker_report_and_resumes() {
    assert_eq!(std::env::var("THINGMAKER_REAL_GEMINI").as_deref(), Ok("1"), "explicitly enable the real provider test");
    let program = PathBuf::from(std::env::var_os("THINGMAKER_AGY").expect("set the installed agy executable"));
    let dir = tempfile::tempdir().unwrap();
    let finished = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reports = finished.clone();
    let delegation = Delegation::new(Arc::new(MockWorkers), move |job| {
        if job.status == JobStatus::Succeeded { reports.lock().unwrap().push(job.clone()); }
    });
    let socket = DelegationSocket::bind(delegation.clone(), socket_path(dir.path())).unwrap();
    let open = |resume: Option<String>| {
        let token = delegation.register("real-gemini-lead", Provider::Gemini, dir.path().to_owned(), combo());
        let mut options = GeminiLaunchOptions::new(dir.path(), &program);
        options.permission_mode = PermissionStance::AcceptEdits;
        options.effort = Some("low".into());
        options.resume = resume;
        options.initial_prompt_prefix = Some("Use the session's team MCP tools for this verification. Do not use native subagents, shell, file editing or any other tools.".into());
        let mut config = SessionActorConfig::new(LaunchTarget::executable(program.clone()), AgentLaunch::Gemini(options));
        config.account_id = Some("default".into());
        config.leads_team = true;
        config.mcp_servers.push(server(&socket, &token));
        config.timeouts.control = Duration::from_secs(45);
        config.timeouts.turn = Duration::from_secs(180);
        SessionActor::open(config)
    };

    let actor = open(None).await.expect("real native ACP session opened");
    let (phase, text, error) = real_turn(&actor, "real-team-check", "Verify only the connected team tools. Call list_workers, delegate exactly one task to the worker named helper asking it to reply with worker-result, then await_jobs for that task. Reply with the worker's result. Do not create or edit files or use any other tools.").await;
    let conversation = actor.snapshot().await.unwrap().agent_session_id.expect("the conversation is named");
    let _ = actor.stop().await;
    delegation.release("real-gemini-lead").await;
    eprintln!("real leader phase={phase:?}, reply={text:?}, error={error:?}, succeeded workers={}, conversation={conversation}", finished.lock().unwrap().len());
    assert_eq!(phase, TurnPhase::Succeeded);
    assert!(text.to_lowercase().contains("worker-result"));
    assert_eq!(finished.lock().unwrap().len(), 1, "one real MCP delegation completed");

    // The same conversation, reopened as the application reopens it.
    let actor = open(Some(conversation.clone())).await.expect("the real conversation was loaded again");
    let snapshot = actor.snapshot().await.unwrap();
    assert_eq!(snapshot.agent_session_id.as_deref(), Some(conversation.as_str()), "the conversation asked for, not a new one");
    assert!(snapshot.leads_team);
    let (phase, text, error) = real_turn(&actor, "real-resume-check", "Without calling any tool: what exactly did the worker named helper report earlier in this conversation? Reply with that text only.").await;
    let _ = actor.stop().await;
    delegation.release("real-gemini-lead").await;
    eprintln!("resumed leader phase={phase:?}, reply={text:?}, error={error:?}");
    assert_eq!(phase, TurnPhase::Succeeded);
    assert!(text.to_lowercase().contains("worker-result"), "the reopened conversation remembers its worker's report");
    assert_eq!(finished.lock().unwrap().len(), 1, "nothing was delegated again");
}
