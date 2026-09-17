//! Tool executor: chain approval, WhatIf expansion, error modes.
//!
//! One dumb pipe: model tool calls -> approval (mutating chains) ->
//! host bridge -> results. Terminal-lifecycle cmdlets ride the same pipe;
//! the PowerShell module owns terminal processes (§4.2: no tool logic in
//! Rust). Harness failures (closed pipe, protocol) abort; tool errors
//! become results governed by the error mode.

use std::sync::Arc;
use std::time::Instant;

use fa_bridge::{BridgeError, HostBridge};
use serde_json::{Map, Value};
use tracing::warn;

use crate::approver::{ApprovalDecision, ApprovalRequest, ApproverRef};
use crate::protocol::ToolCall;
use crate::session::SessionDb;
use crate::{FaError, Result};

/// Cmdlets that mutate the world: approval-gated.
pub const MUTATING: &[&str] = &[
    "Write-FAFile",
    "Edit-FAFile",
    "New-FATerminal",
    "Invoke-FACommand",
    "Remove-FATerminal",
];

/// Terminal-lifecycle cmdlets: mirrored into the session DB for audit.
const TERMINAL_TOOLS: &[&str] = &[
    "New-FATerminal",
    "Get-FATerminal",
    "Invoke-FACommand",
    "Remove-FATerminal",
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ErrorMode {
    /// First failure aborts the chain and ends the turn; the real error surfaces.
    StopAndReport,
    /// Failures become results; the loop continues and the model may self-heal.
    HealAndContinue,
}

impl ErrorMode {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "heal" | "healandcontinue" | "continue" => Self::HealAndContinue,
            _ => Self::StopAndReport,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StopAndReport => "StopAndReport",
            Self::HealAndContinue => "HealAndContinue",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub call: ToolCall,
    pub ok: bool,
    pub output: Value,
    pub error: Option<String>,
    pub duration_ms: u64,
}

pub enum ExecEvent {
    Started(ToolCall),
    Finished(ToolResult),
}

pub struct Executor {
    bridge: HostBridge,
    db: Arc<SessionDb>,
    session_id: String,
    approver: ApproverRef,
    auto_approve: bool,
    error_mode: ErrorMode,
}

impl Executor {
    pub fn new(
        bridge: HostBridge,
        db: Arc<SessionDb>,
        session_id: String,
        approver: ApproverRef,
        auto_approve: bool,
        error_mode: ErrorMode,
    ) -> Self {
        Self {
            bridge,
            db,
            session_id,
            approver,
            auto_approve,
            error_mode,
        }
    }

    pub fn error_mode(&self) -> ErrorMode {
        self.error_mode
    }

    fn needs_approval(name: &str) -> bool {
        MUTATING.contains(&name)
    }

    /// Run one chain (one model turn's tool calls). Returns per-call results.
    /// `Err(FaError::Denied)` when the operator denies the chain.
    pub async fn run_chain<F>(&mut self, calls: &[ToolCall], emit: &F) -> Result<Vec<ToolResult>>
    where
        F: Fn(ExecEvent) + Sync,
    {
        if calls.is_empty() {
            return Ok(Vec::new());
        }
        let mut calls: Vec<ToolCall> = calls.to_vec();
        let needs = calls.iter().any(|c| Self::needs_approval(&c.name));

        if needs && self.auto_approve {
            // Auto-approval is still an approval decision: log it so the
            // audit trail (and the M4 battery) can count chains hit.
            self.log(
                "approval.resolved",
                &serde_json::json!({"decision": "auto-approve", "calls": calls.len()}),
            )?;
        }

        if !self.auto_approve && needs {
            let previews = self.preview_chain(&calls).await;
            let req = ApprovalRequest {
                chain: calls.clone(),
                previews,
            };
            match self.approver.decide(&req).await? {
                ApprovalDecision::Approve => {
                    self.log(
                        "approval.resolved",
                        &serde_json::json!({"decision": "approve", "calls": calls.len()}),
                    )?;
                }
                ApprovalDecision::Deny => {
                    self.log(
                        "approval.resolved",
                        &serde_json::json!({"decision": "deny"}),
                    )?;
                    return Err(FaError::Denied);
                }
                ApprovalDecision::Edit(new_args) => {
                    if calls.len() == 1 {
                        calls[0].args = new_args;
                        self.log(
                            "approval.resolved",
                            &serde_json::json!({"decision": "edit"}),
                        )?;
                    } else {
                        warn!("edit decision on multi-call chain; treating as approve");
                        self.log(
                            "approval.resolved",
                            &serde_json::json!({"decision": "approve-after-edit-unsupported"}),
                        )?;
                    }
                }
            }
        }

        let mut results = Vec::with_capacity(calls.len());
        for call in &calls {
            emit(ExecEvent::Started(call.clone()));
            let result = self.run_one(call).await?;
            let failed = !result.ok;
            emit(ExecEvent::Finished(result.clone()));
            results.push(result);
            if failed && self.error_mode == ErrorMode::StopAndReport {
                break;
            }
        }
        Ok(results)
    }

    /// WhatIf expansion: run each mutating stage under `-WhatIf` and collect
    /// the `Preview` each cmdlet returns. No mutation happens here.
    async fn preview_chain(&mut self, calls: &[ToolCall]) -> Vec<String> {
        let mut previews = Vec::new();
        for call in calls {
            if !Self::needs_approval(&call.name) {
                continue;
            }
            let mut args = call.args.clone();
            args.insert("_WhatIf".to_string(), Value::Bool(true));
            match self.bridge.call(&call.name, &args).await {
                Ok(v) => {
                    let p = v
                        .get("Preview")
                        .and_then(|p| p.as_str())
                        .unwrap_or("(no preview)")
                        .to_string();
                    previews.push(format!("{}: {p}", call.name));
                }
                Err(e) => previews.push(format!("{}: preview failed: {e}", call.name)),
            }
        }
        previews
    }

    async fn run_one(&mut self, call: &ToolCall) -> Result<ToolResult> {
        self.log(
            "tool.call",
            &serde_json::json!({"tool": call.name, "args": call.args}),
        )?;
        let t0 = Instant::now();
        let (ok, output, error) = match self.bridge.call(&call.name, &call.args).await {
            Ok(v) => (true, v, None),
            Err(BridgeError::Tool { message, .. }) => (false, Value::Null, Some(message)),
            Err(e) => return Err(FaError::Bridge(e)),
        };
        let duration_ms = t0.elapsed().as_millis() as u64;
        let result = ToolResult {
            call: call.clone(),
            ok,
            output: output.clone(),
            error: error.clone(),
            duration_ms,
        };
        self.log(
            "tool.result",
            &serde_json::json!({
                "tool": call.name, "ok": ok,
                "error": error, "duration_ms": duration_ms,
            }),
        )?;
        self.mirror_terminal(call, &result)?;
        Ok(result)
    }

    /// Mirror terminal lifecycle into the session DB for audit.
    fn mirror_terminal(&self, call: &ToolCall, result: &ToolResult) -> Result<()> {
        if !TERMINAL_TOOLS.contains(&call.name.as_str()) || !result.ok {
            return Ok(());
        }
        match call.name.as_str() {
            "New-FATerminal" => {
                if let Some(name) = result.output.get("Name").and_then(|n| n.as_str()) {
                    let state = serde_json::to_string(&result.output)?;
                    self.db.upsert_terminal(&self.session_id, name, &state)?;
                }
            }
            "Remove-FATerminal" => {
                if let Some(name) = call.args.get("Name").and_then(|n| n.as_str()) {
                    self.db.remove_terminal(&self.session_id, name)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn log(&self, typ: &str, payload: &Value) -> Result<()> {
        self.db.append_event(&self.session_id, typ, payload)?;
        Ok(())
    }

    /// Clean shutdown of the host (EOF on stdin).
    pub async fn shutdown(self) -> Result<()> {
        self.bridge.shutdown().await?;
        Ok(())
    }

    /// Kill the whole process tree (host + terminals).
    pub async fn kill_tree(self) {
        self.bridge.kill_tree().await;
    }
}

/// Build a params map from JSON string (tests / harnesses).
pub fn args_of(json: &str) -> Map<String, Value> {
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}
