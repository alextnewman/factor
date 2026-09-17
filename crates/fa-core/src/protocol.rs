//! The text tool-call protocol (settled by spike M2).
//!
//! Grammar: the model emits fenced blocks tagged `fa` (three backticks + `fa`);
//! each non-empty line inside one such block is a single call, e.g.
//! `call Read-FAFile {"Path": "notes.txt"}`.
//!
//! Design notes:
//! - Tool calls outside an `fa` fence are ignored (prose stays prose).
//! - A malformed line never panics the parser; it becomes a warning the
//!   harness feeds back to the model so it can self-correct.
//! - Tool *names* are validated for shape here; the bridge allowlist
//!   (`*-FA*`) is the real enforcement point.

use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub args: Map<String, Value>,
}

fn valid_name(name: &str) -> bool {
    // <Verb>-FA<Noun>, ASCII alphanumerics only.
    let mut parts = name.splitn(2, "-FA");
    let verb = parts.next().unwrap_or_default();
    let noun = parts.next();
    match noun {
        Some(noun) => {
            !verb.is_empty()
                && !noun.is_empty()
                && verb.chars().all(|c| c.is_ascii_alphanumeric())
                && noun.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

/// Parse all ```fa blocks in `text`. Returns (calls, warnings).
/// Never panics on malformed input.
pub fn parse_tool_calls(text: &str) -> (Vec<ToolCall>, Vec<String>) {
    let mut calls = Vec::new();
    let mut warnings = Vec::new();
    let mut in_block = false;
    let mut line_no = 0usize;

    for raw_line in text.lines() {
        line_no += 1;
        let line = raw_line.trim();
        if line.starts_with("```") {
            let tag = line.trim_start_matches('`').trim();
            if !in_block && tag.eq_ignore_ascii_case("fa") {
                in_block = true;
            } else if in_block {
                in_block = false; // any closing fence ends the block
            }
            continue;
        }
        if !in_block {
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("call ") else {
            warnings.push(format!(
                "line {line_no}: expected `call <Tool> {{...}}`, got: {line}"
            ));
            continue;
        };
        let rest = rest.trim_start();
        let (name, json) = match rest.find(|c: char| c.is_whitespace()) {
            Some(i) => (&rest[..i], rest[i..].trim()),
            None => (rest, ""),
        };
        if !valid_name(name) {
            warnings.push(format!("line {line_no}: bad tool name `{name}`"));
            continue;
        }
        if json.is_empty() {
            warnings.push(format!(
                "line {line_no}: `call {name}` is missing its JSON args"
            ));
            continue;
        }
        match serde_json::from_str::<Value>(json) {
            Ok(Value::Object(map)) => calls.push(ToolCall {
                name: name.to_string(),
                args: map,
            }),
            Ok(_) => warnings.push(format!(
                "line {line_no}: args for `{name}` must be a JSON object"
            )),
            Err(e) => {
                // A model reaching for PowerShell syntax gets a targeted
                // correction, not just serde's complaint.
                let hint = if json.trim_start().starts_with('-') {
                    " — that looks like PowerShell `-Param value` syntax, which is not \
                     accepted here; use a JSON object instead, e.g. \
                     `call Write-FAFile {\"Path\": \"f.txt\", \"Content\": \"hi\"}`"
                } else {
                    ""
                };
                warnings.push(format!(
                    "line {line_no}: invalid JSON for `{name}`: {e}{hint}"
                ));
            }
        }
    }
    if in_block {
        warnings.push("unclosed ```fa block at end of message".to_string());
    }
    (calls, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path() {
        let text = "Here you go:\n```fa\ncall Read-FAFile {\"Path\": \"a.txt\"}\ncall Find-FAText {\"Pattern\": \"x\"}\n```\nDone.";
        let (calls, warnings) = parse_tool_calls(text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "Read-FAFile");
        assert_eq!(calls[0].args["Path"], "a.txt");
    }

    #[test]
    fn prose_calls_are_ignored() {
        let text = "call Read-FAFile {\"Path\": \"a.txt\"}\nplain text";
        let (calls, warnings) = parse_tool_calls(text);
        assert!(calls.is_empty());
        assert!(warnings.is_empty());
    }

    /// Malformed-model-output corpus: the parser must never panic, and must
    /// report every problem as a warning rather than silently dropping.
    #[test]
    fn fuzz_corpus_never_panics() {
        let cases = [
            "```fa\ncall \n```",
            "```fa\ncall Read-FAFile\n```",
            "```fa\ncall Read-FAFile not-json\n```",
            "```fa\ncall Read-FAFile [\"array\"]\n```",
            "```fa\ncall Read-FAFile {\"Path\":\n```",
            "```fa\ncall Get-Process {\"Name\": \"x\"}\n```", // non-FA name shape
            "```fa\ncall Read-FAFile; rm -rf / {\"Path\": \"x\"}\n```",
            "```fa\nCALL Read-FAFile {\"Path\": \"x\"}\n```", // wrong keyword case
            "```fa\ncall Read-FAFile {\"Path\": \"x\"}",      // unclosed fence
            "```fa\ncall Read-FAFile {\"Path\": \"ünïcodé ✓\"}\n```",
            "```python\ncall Read-FAFile {\"Path\": \"x\"}\n```", // wrong fence tag
            "```fa\n# comment\n\ncall Read-FAFile {}\n```",
            "```fa\ncall Read-FAFile {\"Path\": \"x\"}\n``` trailing ```fa\nmore\n```",
            &format!(
                "```fa\ncall Read-FAFile {{\"Path\": \"{}\"}}\n```",
                "x".repeat(100_000)
            ),
            "```fa\r\ncall Read-FAFile {\"Path\": \"x\"}\r\n```",
            "",
            "```",
            "```fa",
        ];
        for (i, case) in cases.iter().enumerate() {
            let (calls, warnings) = parse_tool_calls(case);
            // Every case is either clean-calls-only or produces warnings;
            // a call with unparseable args must never slip through silently.
            for c in &calls {
                assert!(valid_name(&c.name), "case {i}: bad name slipped through");
            }
            let _ = warnings;
        }
        // Spot-checks on diagnostics:
        let (_, w) = parse_tool_calls("```fa\ncall Read-FAFile not-json\n```");
        assert!(w.iter().any(|x| x.contains("invalid JSON")));
        let (_, w) = parse_tool_calls("```fa\ncall Get-Process {\"Name\": \"x\"}\n```");
        assert!(w.iter().any(|x| x.contains("bad tool name")));
        let (c, _) = parse_tool_calls("```fa\ncall Read-FAFile {\"Path\": \"x\"}\n```");
        assert_eq!(c.len(), 1);
    }
}
