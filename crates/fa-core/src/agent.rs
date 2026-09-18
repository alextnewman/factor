//! The agent loop: prompt -> tool calls -> approval -> results.
//!
//! Turn cap 25, full-history context, no compaction (prototype). The error
//! mode dial decides what a tool failure means for the turn.

use std::future::Future;
use std::sync::Arc;

use serde_json::json;

use crate::backend::LlmBackend;
use crate::executor::{ErrorMode, ExecEvent, Executor, ToolResult};
use crate::prompt::{build_messages, render_tool_result, ChatMessage, SessionFacts, SUFFIX_NUDGE};
use crate::protocol::{parse_tool_calls, ToolCall};
use crate::session::SessionDb;
use crate::{FaError, Result};

pub const TURN_CAP: usize = 25;

pub enum LoopEvent {
    AgentText(String),
    ToolCalls(Vec<ToolCall>),
    ToolResult(ToolResult),
    Warning(String),
    ModelUsage {
        prompt_tokens: u64,
        completion_tokens: u64,
        cached_tokens: Option<u64>,
        latency_ms: u64,
        server_prompt_ms: Option<u64>,
    },
}

#[derive(Debug, Clone)]
pub struct LoopOutcome {
    pub final_text: String,
    pub turns_used: usize,
}

pub struct AgentLoop {
    backend: Arc<dyn LlmBackend>,
    model: String,
    executor: Executor,
    db: Arc<SessionDb>,
    session_id: String,
    block_a: String,
    facts: SessionFacts,
    history: Vec<ChatMessage>,
    turn_cap: usize,
}

impl AgentLoop {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        backend: Arc<dyn LlmBackend>,
        model: String,
        executor: Executor,
        db: Arc<SessionDb>,
        session_id: String,
        block_a: String,
        facts: SessionFacts,
    ) -> Self {
        Self {
            backend,
            model,
            executor,
            db,
            session_id,
            block_a,
            facts,
            history: Vec::new(),
            turn_cap: TURN_CAP,
        }
    }

    pub fn history(&self) -> &[ChatMessage] {
        &self.history
    }

    /// Override the turn cap (prototype default is TURN_CAP).
    /// The M4 battery lowers this so a spiraling model can't burn an hour.
    pub fn set_turn_cap(&mut self, cap: usize) {
        self.turn_cap = cap.max(1);
    }

    fn log(&self, typ: &str, payload: &serde_json::Value) -> Result<()> {
        self.db.append_event(&self.session_id, typ, payload)?;
        Ok(())
    }

    /// Run one user prompt to completion (or the turn cap).
    pub async fn run_prompt<F, Fut>(&mut self, user_text: &str, emit: &F) -> Result<LoopOutcome>
    where
        F: Fn(LoopEvent) -> Fut + Sync,
        Fut: Future<Output = ()> + Send,
    {
        self.log("turn.started", &json!({"cap": self.turn_cap}))?;
        self.log("user.message", &json!({"text": user_text}))?;
        self.history.push(ChatMessage::user(user_text));

        let outcome = self.drive(emit).await;

        self.log(
            "turn.completed",
            &json!({
                "turns_used": outcome.as_ref().map(|o| o.turns_used).unwrap_or(0),
                "ok": outcome.is_ok(),
            }),
        )?;
        outcome
    }

    async fn drive<F, Fut>(&mut self, emit: &F) -> Result<LoopOutcome>
    where
        F: Fn(LoopEvent) -> Fut + Sync,
        Fut: Future<Output = ()> + Send,
    {
        for turn in 1..=self.turn_cap {
            let block_b = crate::prompt::build_block_b(&self.facts);
            let messages = build_messages(&self.block_a, &block_b, &self.history, SUFFIX_NUDGE);
            let prompt_hash = hash_messages(&messages);
            self.log(
                "prompt.built",
                &json!({"turn": turn, "messages": messages.len(), "hash": prompt_hash}),
            )?;

            let resp = self.backend.complete(&self.model, &messages).await?;
            self.log(
                "llm.response",
                &json!({
                    "turn": turn,
                    "prompt_tokens": resp.usage.prompt_tokens,
                    "completion_tokens": resp.usage.completion_tokens,
                    "cached_tokens": resp.usage.cached_tokens,
                    "latency_ms": resp.usage.latency_ms,
                }),
            )?;
            emit(LoopEvent::ModelUsage {
                prompt_tokens: resp.usage.prompt_tokens,
                completion_tokens: resp.usage.completion_tokens,
                cached_tokens: resp.usage.cached_tokens,
                latency_ms: resp.usage.latency_ms,
                server_prompt_ms: resp.usage.server_prompt_ms,
            })
            .await;
            emit(LoopEvent::AgentText(resp.content.clone())).await;
            self.log(
                "agent.message",
                &json!({"turn": turn, "text": resp.content}),
            )?;
            self.history
                .push(ChatMessage::assistant(resp.content.clone()));

            let (calls, warnings) = parse_tool_calls(&resp.content);
            for w in &warnings {
                emit(LoopEvent::Warning(w.clone())).await;
            }
            if !warnings.is_empty() {
                // Feed the diagnostics back so the model can self-correct.
                self.history.push(ChatMessage::user(format!(
                    "Tool protocol notes (fix these, then continue):\n{}",
                    warnings.join("\n")
                )));
            }
            if calls.is_empty() {
                return Ok(LoopOutcome {
                    final_text: resp.content,
                    turns_used: turn,
                });
            }

            emit(LoopEvent::ToolCalls(calls.clone())).await;
            let results = match self
                .executor
                .run_chain(&calls, &|e| async move {
                    match e {
                        ExecEvent::Started(c) => {
                            let _ = c;
                        }
                        ExecEvent::Finished(r) => {
                            emit(LoopEvent::ToolResult(r)).await;
                        }
                    }
                })
                .await
            {
                Ok(results) => results,
                Err(FaError::Denied) => {
                    // Design §4.6: the real outcome surfaces. A denial aborts
                    // the chain as an error; the frontend reports it and the
                    // operator decides what happens next. (The executor
                    // already logged approval.resolved=denied.)
                    return Err(FaError::Denied);
                }
                Err(e) => return Err(e),
            };

            let mut stop_turn: Option<String> = None;
            for r in &results {
                let rendered = render_tool_result(
                    &r.call.name,
                    &r.call.args,
                    r.ok,
                    &r.output,
                    r.error.as_deref(),
                    r.duration_ms,
                );
                self.history.push(ChatMessage::user(rendered));
                if !r.ok && self.executor.error_mode() == ErrorMode::StopAndReport {
                    stop_turn = Some(format!(
                        "Tool {} failed: {}. The turn ends here (error mode is StopAndReport); \
                         report the failure plainly.",
                        r.call.name,
                        r.error.as_deref().unwrap_or("unknown error")
                    ));
                    break;
                }
            }
            if let Some(note) = stop_turn {
                // Design §4.6 StopAndReport: first failure aborts the chain
                // and the REAL error surfaces — not a polite paraphrase.
                let failed = results.iter().find(|r| !r.ok);
                let (tool, message) = match failed {
                    Some(r) => (
                        r.call.name.clone(),
                        r.error.clone().unwrap_or_else(|| "unknown error".into()),
                    ),
                    None => ("unknown".into(), note.clone()),
                };
                self.history
                    .push(ChatMessage::user(format!("system: {note}")));
                return Err(FaError::ToolFailed { tool, message });
            }
        }
        Ok(LoopOutcome {
            final_text: format!(
                "Turn cap ({}) reached without a final answer.",
                self.turn_cap
            ),
            turns_used: self.turn_cap,
        })
    }
}

fn hash_messages(messages: &[ChatMessage]) -> String {
    // FNV-1a over roles+contents: cheap, stable, good enough for prompt identity.
    let mut h: u64 = 0xcbf29ce484222325;
    for m in messages {
        for b in m.role.bytes().chain(m.content.bytes()) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    format!("{h:016x}")
}
