//! m4score: score one M4 battery session from its session.db.
//!
//! Usage: m4score <session.db> <session_id>
//! Emits a single JSON object on stdout. Uses the REAL fa_core parser, so
//! scoring can never drift from what the harness accepted.
//!
//! Response buckets (per the M4 calibration):
//! - first_try: assistant messages NOT preceded by a parser-warning feedback.
//! - post_warning: assistant messages immediately following a warned message
//!   (the harness fed the warnings back as a user message).
//! Validity = parsed cleanly (>=1 call, zero warnings).

use fa_core::protocol::parse_tool_calls;
use fa_core::session::SessionDb;
use serde_json::{json, Value};
use std::path::Path;

struct Resp {
    #[allow(dead_code)]
    turn: u64,
    text: String,
    n_calls: usize,
    n_warnings: usize,
    warnings: Vec<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = std::env::args().nth(1).ok_or("usage: m4score <session.db> <session_id>")?;
    let session_id = std::env::args().nth(2).ok_or("usage: m4score <session.db> <session_id>")?;
    let db = SessionDb::open(Path::new(&db_path))?;
    let events = db.events(&session_id)?;

    let mut resps: Vec<Resp> = Vec::new();
    for e in &events {
        if e.typ == "agent.message" {
            let text = e.payload.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let turn = e.payload.get("turn").and_then(|t| t.as_u64()).unwrap_or(0);
            let (calls, warnings) = parse_tool_calls(&text);
            resps.push(Resp {
                turn,
                text,
                n_calls: calls.len(),
                n_warnings: warnings.len(),
                warnings,
            });
        }
    }

    // Bucket each attempted response.
    let mut ft_attempted = 0u64;
    let mut ft_clean = 0u64;
    let mut pw_attempted = 0u64;
    let mut pw_clean = 0u64;
    let mut total_calls = 0u64;
    let mut total_warnings = 0u64;
    let mut episodes = 0u64;
    let mut recoveries = 0u64;
    let mut detail: Vec<Value> = Vec::new();
    for (i, r) in resps.iter().enumerate() {
        let attempted = r.n_calls + r.n_warnings > 0;
        let clean = attempted && r.n_warnings == 0 && r.n_calls > 0;
        let post_warning = i > 0 && resps[i - 1].n_warnings > 0;
        total_calls += r.n_calls as u64;
        total_warnings += r.n_warnings as u64;
        if attempted {
            if post_warning {
                pw_attempted += 1;
                if clean {
                    pw_clean += 1;
                }
            } else {
                ft_attempted += 1;
                if clean {
                    ft_clean += 1;
                }
            }
        }
        if r.n_warnings > 0 {
            episodes += 1;
            let recovered = resps.get(i + 1).map(|n| n.n_warnings == 0 && n.n_calls > 0).unwrap_or(false);
            if recovered {
                recoveries += 1;
            }
        }
        detail.push(json!({
            "turn": r.turn,
            "attempted": attempted,
            "post_warning": post_warning,
            "clean": clean,
            "calls": r.n_calls,
            "warnings": r.warnings,
        }));
    }

    // Tool results, approvals, turns, tokens.
    let mut tool_ok = 0u64;
    let mut tool_err = 0u64;
    let mut tool_errors: Vec<Value> = Vec::new();
    let mut approvals = 0u64;
    let mut approval_decisions: Vec<String> = Vec::new();
    let mut prompt_tokens = 0u64;
    let mut completion_tokens = 0u64;
    let mut turns_used = 0u64;
    let mut turn_ok = false;
    for e in &events {
        match e.typ.as_str() {
            "tool.result" => {
                if e.payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    tool_ok += 1;
                } else {
                    tool_err += 1;
                    tool_errors.push(json!({
                        "tool": e.payload.get("tool"),
                        "error": e.payload.get("error"),
                    }));
                }
            }
            "approval.resolved" => {
                approvals += 1;
                if let Some(d) = e.payload.get("decision").and_then(|v| v.as_str()) {
                    approval_decisions.push(d.to_string());
                }
            }
            "llm.response" => {
                prompt_tokens += e.payload.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                completion_tokens += e.payload.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            }
            "turn.completed" => {
                turns_used = e.payload.get("turns_used").and_then(|v| v.as_u64()).unwrap_or(0);
                turn_ok = e.payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            }
            _ => {}
        }
    }

    let final_text = resps.last().map(|r| r.text.clone()).unwrap_or_default();
    // Truncate the final text for the JSON blob; full text stays in the DB.
    let final_excerpt: String = final_text.chars().take(1500).collect();

    println!(
        "{}",
        json!({
            "session_id": session_id,
            "turns_used": turns_used,
            "turn_ok": turn_ok,
            "responses": resps.len(),
            "first_try": {"attempted": ft_attempted, "clean": ft_clean},
            "post_warning": {"attempted": pw_attempted, "clean": pw_clean},
            "warning_episodes": episodes,
            "warning_recoveries": recoveries,
            "total_calls": total_calls,
            "total_warnings": total_warnings,
            "tool_ok": tool_ok,
            "tool_err": tool_err,
            "tool_errors": tool_errors,
            "approval_chains": approvals,
            "approval_decisions": approval_decisions,
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "final_text_excerpt": final_excerpt,
            "response_detail": detail,
        })
    );
    Ok(())
}
