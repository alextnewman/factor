//! M2: agent-loop tests against the real bridge with a scripted mock backend.
//! Happy-path loop, approval deny/edit, and both error modes.
//! Skips gracefully when pwsh is not on PATH.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use fa_bridge::{HostBridge, SessionEnv};
use fa_core::agent::{AgentLoop, LoopEvent};
use fa_core::approver::{ApprovalDecision, ApprovalRequest, Approver, ApproverRef, AutoApprover};
use fa_core::backend::MockBackend;
use fa_core::executor::{ErrorMode, Executor};
use fa_core::manifest::load_manifest;
use fa_core::prompt::{build_block_a, SessionFacts};
use fa_core::session::SessionDb;
use serde_json::{json, Map, Value};

fn pwsh_present() -> bool {
    std::process::Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", "$true"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

struct Harness {
    work: std::path::PathBuf,
    session_id: String,
    db: Arc<SessionDb>,
    block_a: String,
}

impl Harness {
    async fn new(tag: &str) -> Option<(Self, HostBridge)> {
        if !pwsh_present() {
            eprintln!("SKIP m2 ({tag}): pwsh not on PATH");
            return None;
        }
        let bridge_ps1 = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../psmodule/FactorAgent/bridge.ps1"
        );
        let work = std::env::temp_dir().join(format!("fa-m2-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let session_id = format!("m2-{tag}");
        let db = Arc::new(SessionDb::open(&work.join("session.db")).expect("open test db"));
        db.create_session(
            &session_id,
            "stop",
            "host",
            "0.1.0",
            &work.to_string_lossy(),
        )
        .unwrap();

        let (mut bridge, _) = HostBridge::spawn(
            bridge_ps1,
            &SessionEnv {
                session_json: format!(
                    r#"{{"id":"{session_id}","mode":"managed","backend":"host"}}"#
                ),
                error_action: "Stop".into(),
                max_terminals: 8,
                cwd: work.to_string_lossy().into(),
            },
        )
        .await
        .expect("spawn host");

        let m = bridge
            .call("Get-FAToolManifest", &args(json!({})))
            .await
            .unwrap();
        let schemas = load_manifest(m.as_str().unwrap()).unwrap();
        assert_eq!(schemas.len(), 10);
        let block_a = build_block_a(&schemas);
        let h = Self {
            work,
            session_id,
            db,
            block_a,
        };
        Some((h, bridge))
    }

    fn facts(&self) -> SessionFacts {
        SessionFacts {
            session_id: self.session_id.clone(),
            cwd: self.work.to_string_lossy().into(),
            error_mode: "stop".into(),
            backend: "host".into(),
            manifest_version: "0.1.0".into(),
            scope_notes: vec![],
            terminals: vec![],
        }
    }

    fn agent(
        &self,
        backend: MockBackend,
        bridge: HostBridge,
        approver: ApproverRef,
        auto_approve: bool,
        error_mode: ErrorMode,
    ) -> AgentLoop {
        let executor = Executor::new(
            bridge,
            self.db.clone(),
            self.session_id.clone(),
            approver,
            auto_approve,
            error_mode,
        );
        AgentLoop::new(
            Arc::new(backend),
            "mock".into(),
            executor,
            self.db.clone(),
            self.session_id.clone(),
            self.block_a.clone(),
            self.facts(),
        )
    }

    fn finish(self) {
        let _ = std::fs::remove_dir_all(&self.work);
    }
}

/// Scripted approver: pops decisions in order, defaults to Approve.
struct ScriptApprover {
    decisions: Mutex<VecDeque<ApprovalDecision>>,
}

impl ScriptApprover {
    fn new(decisions: Vec<ApprovalDecision>) -> Self {
        Self {
            decisions: Mutex::new(decisions.into()),
        }
    }
}

impl Approver for ScriptApprover {
    fn decide<'a>(
        &'a self,
        req: &'a ApprovalRequest,
    ) -> Pin<Box<dyn Future<Output = fa_core::Result<ApprovalDecision>> + Send + 'a>> {
        assert!(!req.chain.is_empty(), "approver called with an empty chain");
        assert!(!req.previews.is_empty(), "approval without WhatIf previews");
        let d = self
            .decisions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ApprovalDecision::Approve);
        Box::pin(async move { Ok(d) })
    }
}

async fn silent(_: LoopEvent) {}

#[tokio::test]
async fn m2_mock_loop_happy_path() {
    let Some((h, bridge)) = Harness::new("happy").await else {
        return;
    };
    let target = h.work.join("m2.txt");
    let script = vec![
        format!(
            "I'll create the file first.\n```fa\ncall Write-FAFile {{\"Path\": \"{}\", \"Content\": \"hello m2\\n\"}}\n```",
            target.to_string_lossy()
        ),
        format!(
            "Now search it.\n```fa\ncall Find-FAText {{\"Pattern\": \"hello\", \"Path\": \"{}\"}}\n```",
            h.work.to_string_lossy()
        ),
        "Done — the file is written and the text is found.".to_string(),
    ];
    let mut agent = h.agent(
        MockBackend::new(script),
        bridge,
        Arc::new(AutoApprover),
        true,
        ErrorMode::StopAndReport,
    );
    let outcome = agent
        .run_prompt("do the m2 dance", &silent)
        .await
        .expect("loop should succeed");
    assert_eq!(outcome.turns_used, 3, "expected 3 turns");
    assert!(outcome.final_text.contains("Done"));
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "hello m2\n",
        "file content mismatch"
    );
    // The event log is append-only and records the loop.
    let events = h.db.events(&h.session_id).unwrap();
    let types: Vec<&str> = events.iter().map(|e| e.typ.as_str()).collect();
    assert!(
        types.contains(&"turn.started"),
        "missing turn.started: {types:?}"
    );
    assert!(
        types.contains(&"tool.result"),
        "missing tool.result: {types:?}"
    );
    h.finish();
    eprintln!("M2 happy-path: PASS");
}

#[tokio::test]
async fn m2_approval_deny_stops_chain() {
    let Some((h, bridge)) = Harness::new("deny").await else {
        return;
    };
    let target = h.work.join("denied.txt");
    let script = vec![format!(
        "```fa\ncall Write-FAFile {{\"Path\": \"{}\", \"Content\": \"nope\"}}\n```",
        target.to_string_lossy()
    )];
    let approver: ApproverRef = Arc::new(ScriptApprover::new(vec![ApprovalDecision::Deny]));
    let mut agent = h.agent(
        MockBackend::new(script),
        bridge,
        approver,
        false,
        ErrorMode::StopAndReport,
    );
    let err = agent
        .run_prompt("write it", &silent)
        .await
        .expect_err("denied chain must fail the loop");
    assert!(
        err.to_string().contains("denied") || format!("{err:?}").contains("Denied"),
        "unexpected error: {err:?}"
    );
    assert!(!target.exists(), "denied write must not touch the disk");
    h.finish();
    eprintln!("M2 approval-deny: PASS");
}

#[tokio::test]
async fn m2_approval_edit_rewrites_args() {
    let Some((h, bridge)) = Harness::new("edit").await else {
        return;
    };
    let target = h.work.join("edited.txt");
    let script = vec![format!(
        "```fa\ncall Write-FAFile {{\"Path\": \"{}\", \"Content\": \"original\"}}\n```",
        target.to_string_lossy()
    )];
    let mut edited_args = Map::new();
    edited_args.insert(
        "Path".into(),
        Value::String(target.to_string_lossy().into()),
    );
    edited_args.insert("Content".into(), Value::String("edited-by-operator".into()));
    let approver: ApproverRef = Arc::new(ScriptApprover::new(vec![ApprovalDecision::Edit(
        edited_args,
    )]));
    let mut agent = h.agent(
        MockBackend::new(script),
        bridge,
        approver,
        false,
        ErrorMode::StopAndReport,
    );
    agent
        .run_prompt("write it", &silent)
        .await
        .expect("edited chain should run");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "edited-by-operator"
    );
    h.finish();
    eprintln!("M2 approval-edit: PASS");
}

#[tokio::test]
async fn m2_stop_and_report_halts_on_tool_error() {
    let Some((h, bridge)) = Harness::new("stopmode").await else {
        return;
    };
    let missing = h.work.join("missing.txt");
    let script = vec![format!(
        "```fa\ncall Read-FAFile {{\"Path\": \"{}\"}}\n```",
        missing.to_string_lossy()
    )];
    let mut agent = h.agent(
        MockBackend::new(script),
        bridge,
        Arc::new(AutoApprover),
        true,
        ErrorMode::StopAndReport,
    );
    let err = agent
        .run_prompt("read it", &silent)
        .await
        .expect_err("StopAndReport must surface the tool failure");
    assert!(
        format!("{err:?}").contains("ToolFailed"),
        "unexpected error: {err:?}"
    );
    h.finish();
    eprintln!("M2 stop-and-report: PASS");
}

#[tokio::test]
async fn m2_heal_and_continue_feeds_error_back() {
    let Some((h, bridge)) = Harness::new("healmode").await else {
        return;
    };
    let missing = h.work.join("missing.txt");
    let script = vec![
        format!(
            "```fa\ncall Read-FAFile {{\"Path\": \"{}\"}}\n```",
            missing.to_string_lossy()
        ),
        "I see the file is missing; reporting that instead.".to_string(),
    ];
    let mut agent = h.agent(
        MockBackend::new(script),
        bridge,
        Arc::new(AutoApprover),
        true,
        ErrorMode::HealAndContinue,
    );
    let outcome = agent
        .run_prompt("read it", &silent)
        .await
        .expect("HealAndContinue must not fail the loop");
    assert_eq!(outcome.turns_used, 2);
    assert!(outcome.final_text.contains("missing"));
    // The failed tool result is in the event log with ok=false.
    let events = h.db.events(&h.session_id).unwrap();
    let failed = events
        .iter()
        .find(|e| e.typ == "tool.result" && e.payload.get("ok") == Some(&Value::Bool(false)));
    assert!(failed.is_some(), "expected a failed tool.result event");
    h.finish();
    eprintln!("M2 heal-and-continue: PASS");
}
