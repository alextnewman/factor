//! Prompt construction: static-first, append-only (§8.3).
//!
//! Order blocks by stability so prefix caching reprocesses as little as
//! possible:
//!   Block A — system prompt + tool manifest. Byte-identical per session.
//!   Block B — session facts + scope state. Rarely changes.
//!   Block C — conversation. Strictly append-only.
//!   Suffix  — ephemeral nudges at the END. Never prepend.

use crate::manifest::{render_tools, ToolSchema};

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String, // "system" | "user" | "assistant" | "tool"
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

pub const SYSTEM_PREAMBLE: &str = "\
You are FactorAgent, a PowerShell-native agent. You act through tools; \
you cannot run shell commands except via Invoke-FACommand.

TOOL PROTOCOL — follow exactly:
- To call tools, emit a fenced block tagged `fa`, one call per line:
```fa
call Read-FAFile {\"Path\": \"notes.txt\"}
call Find-FAText {\"Pattern\": \"TODO\", \"Path\": \".\"}
```
- Arguments are a single JSON object. Quote every string.
- Only call tools listed below. Never invent tool names or parameters.
- After tool results arrive, either call more tools or give your final \
answer as plain text (no fence).
- Prefer a few precise calls over many guesses. If a call fails, read the \
error and adjust — do not repeat the identical call.
";

pub const SUFFIX_NUDGE: &str = "\
Reply with tool calls in a ```fa block, or with your final answer in plain \
text. Do not narrate tool calls outside the block.";

#[derive(Debug, Clone)]
pub struct SessionFacts {
    pub session_id: String,
    pub cwd: String,
    pub error_mode: String,
    pub backend: String,
    pub manifest_version: String,
    pub scope_notes: Vec<(String, String)>,
    pub terminals: Vec<String>,
}

/// Block A: system prompt + full tool manifest. Frozen per session.
pub fn build_block_a(schemas: &[ToolSchema]) -> String {
    format!("{SYSTEM_PREAMBLE}\n{}", render_tools(schemas))
}

/// Block B: session facts + harness-composed scope state.
pub fn build_block_b(facts: &SessionFacts) -> String {
    let mut out = String::from("## Session facts\n\n");
    out.push_str(&format!("- session: {}\n", facts.session_id));
    out.push_str(&format!("- working directory: {}\n", facts.cwd));
    out.push_str(&format!("- error mode: {}\n", facts.error_mode));
    out.push_str(&format!("- execution backend: {}\n", facts.backend));
    out.push_str(&format!(
        "- tool manifest version: {}\n",
        facts.manifest_version
    ));
    if facts.terminals.is_empty() {
        out.push_str("- terminals: none yet (Invoke-FACommand lazily creates `default`)\n");
    } else {
        out.push_str(&format!("- terminals: {}\n", facts.terminals.join(", ")));
    }
    if !facts.scope_notes.is_empty() {
        out.push_str("- scope notes:\n");
        for (k, v) in &facts.scope_notes {
            out.push_str(&format!("  - {k}: {v}\n"));
        }
    }
    out
}

/// Assemble the full message list: system(Block A + Block B), history
/// (Block C, append-only), suffix nudge.
pub fn build_messages(
    block_a: &str,
    block_b: &str,
    history: &[ChatMessage],
    suffix: &str,
) -> Vec<ChatMessage> {
    let mut msgs = Vec::with_capacity(history.len() + 2);
    msgs.push(ChatMessage::system(format!("{block_a}\n{block_b}")));
    msgs.extend(history.iter().cloned());
    msgs.push(ChatMessage::system(suffix.to_string()));
    msgs
}

/// Render one tool result for the conversation history.
pub fn render_tool_result(
    tool: &str,
    args: &serde_json::Map<String, serde_json::Value>,
    ok: bool,
    output: &serde_json::Value,
    error: Option<&str>,
    duration_ms: u64,
) -> String {
    let args_json = serde_json::to_string(args).unwrap_or_default();
    if ok {
        let out = serde_json::to_string_pretty(output).unwrap_or_default();
        format!("tool result: {tool} {args_json} -> ok ({duration_ms}ms)\n{out}")
    } else {
        format!(
            "tool result: {tool} {args_json} -> ERROR: {}\n",
            error.unwrap_or("unknown error")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_order_and_stability() {
        let a = "BLOCK-A";
        let b = "BLOCK-B";
        let history = vec![ChatMessage::user("hi"), ChatMessage::assistant("hello")];
        let msgs = build_messages(a, b, &history, "SUFFIX");
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, "system");
        assert!(msgs[0].content.starts_with("BLOCK-A\nBLOCK-B"));
        assert_eq!(msgs[1].content, "hi");
        assert_eq!(msgs[3].content, "SUFFIX");
    }
}
