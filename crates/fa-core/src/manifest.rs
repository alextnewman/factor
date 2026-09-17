//! Tool manifest: reflected from the PowerShell module, not hand-written.
//!
//! `Get-FAToolManifest` runs inside the host at session start; this module
//! parses its JSON and renders Block A. The module is the manifest —
//! docs cannot drift from tools because they *are* the tools.

use serde::{Deserialize, Serialize};

use crate::Result;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolParam {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub r#enum: Option<Vec<String>>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolSchema {
    pub name: String,
    pub synopsis: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Vec<ToolParam>,
    #[serde(default)]
    pub examples: Vec<String>,
    #[serde(default)]
    pub outputs: Option<String>,
    #[serde(default = "builtin_source")]
    pub source: String,
}

fn builtin_source() -> String {
    "builtin".to_string()
}

/// Parse the JSON emitted by `Get-FAToolManifest`.
pub fn load_manifest(json: &str) -> Result<Vec<ToolSchema>> {
    let schemas: Vec<ToolSchema> = serde_json::from_str(json)?;
    Ok(schemas)
}

/// Render the tool section of Block A: full schemas, deterministic order.
pub fn render_tools(schemas: &[ToolSchema]) -> String {
    let mut out = String::from("## Tools\n\n");
    let mut sorted: Vec<&ToolSchema> = schemas.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for t in sorted {
        out.push_str(&format!("### {}\n{}\n", t.name, t.synopsis));
        // One canonical JSON call shape per tool, placed before the
        // PowerShell examples: the model imitates what it sees most, and
        // without this it reaches for `-Param value` syntax.
        out.push_str(&format!(
            "Call as: call {} {}\n",
            t.name,
            json_call_shape(t)
        ));
        if !t.description.is_empty() {
            out.push_str(&format!("{}\n", t.description));
        }
        if !t.parameters.is_empty() {
            out.push_str("Parameters:\n");
            for p in &t.parameters {
                let req = if p.required { ", required" } else { "" };
                let en = match &p.r#enum {
                    Some(vals) => format!(", one of: {}", vals.join("|")),
                    None => String::new(),
                };
                let desc = if p.description.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", p.description)
                };
                out.push_str(&format!("- {} ({}{}{}){}\n", p.name, p.ty, req, en, desc));
            }
        }
        if !t.examples.is_empty() {
            out.push_str("Examples:\n");
            for e in &t.examples {
                out.push_str(&format!("  {e}\n"));
            }
        }
        if let Some(o) = &t.outputs {
            out.push_str(&format!("Returns: {o}\n"));
        }
        out.push('\n');
    }
    out
}

/// Synthesize a canonical `{"Param": placeholder, ...}` JSON object for a
/// tool from its reflected schema, so Block A teaches the call shape.
/// Required params only: the model fills optional slots from the parameter
/// list when it needs them, and a minimal shape leaves less room for
/// invented arguments.
fn json_call_shape(t: &ToolSchema) -> String {
    let mut pairs = Vec::new();
    for p in &t.parameters {
        if !p.required {
            continue;
        }
        let ph = if let Some(vals) = &p.r#enum {
            format!("\"{}\"", vals.first().map(String::as_str).unwrap_or("..."))
        } else {
            match p.ty.as_str() {
                "boolean" => "true".to_string(),
                "integer" => "0".to_string(),
                "array(string)" => "[\"...\"]".to_string(),
                _ => "\"...\"".to_string(),
            }
        };
        pairs.push(format!("\"{}\": {ph}", p.name));
    }
    format!("{{{}}}", pairs.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[{
        "name": "Read-FAFile",
        "synopsis": "Reads a text file and returns numbered lines.",
        "description": "Longer text.",
        "parameters": [
            {"name": "Path", "type": "string", "required": true, "description": "File to read."},
            {"name": "Lines", "type": "integer", "required": false, "description": ""}
        ],
        "examples": ["Read-FAFile -Path README.md"],
        "outputs": "String[]",
        "source": "builtin"
    }]"#;

    #[test]
    fn parses_and_renders() {
        let schemas = load_manifest(SAMPLE).unwrap();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].parameters.len(), 2);
        let rendered = render_tools(&schemas);
        assert!(rendered.contains("### Read-FAFile"));
        assert!(rendered.contains("- Path (string, required)"));
        assert!(rendered.contains("- Lines (integer)"));
    }

    #[test]
    fn call_shape_uses_required_params_only() {
        let schemas = load_manifest(SAMPLE).unwrap();
        let rendered = render_tools(&schemas);
        // Required-only: the optional Lines param must not appear in the
        // synthesized call line, so the model isn't tempted to fill it.
        assert!(rendered.contains(r#"Call as: call Read-FAFile {"Path": "..."}"#));
    }
}
