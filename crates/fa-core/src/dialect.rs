//! Script dialects: the vernacular the agent writes command text in.
//!
//! A dialect is a true PowerShell dialect, not a rendering trick. The agent
//! writes real scripts with the dialect's aliases (`ls -Recurse`); the exact
//! bytes it writes are what the operator vets at the approval gate and what
//! the host executes. There is no translation layer anywhere in the pipeline
//! — unlike print forms (§4.14), which deliberately render an action view
//! distinct from the incantation, a dialect has one representation.
//!
//! The firewall: dialects govern freeform script text only — the `-Command`
//! of `Invoke-FACommand` and raw terminal input. Tool names are never
//! aliased (`call Read-FAFile {...}` in every dialect); the tool vocabulary
//! stays canonical. Files written with `Write-FAFile` keep full cmdlet
//! names: saved scripts must not depend on aliases.
//!
//! Alias tables are authoritative for the Windows host (the product
//! platform). PowerShell on Unix strips the aliases that collide with native
//! commands (`ls`, `cat`, `rm`, …), so the posix table below only fully
//! holds where the agent host is Windows.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

/// The vernacular the agent uses for PowerShell command text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScriptDialect {
    /// Full cmdlet names. The default: maximum clarity for the model.
    #[default]
    Full,
    /// PowerShell's own short aliases (`gci`, `gc`, `ri`, …).
    Brief,
    /// Posix-style aliases (`ls`, `cat`, `rm`, …). Still PowerShell.
    Posix,
    /// Cmd-style aliases (`dir`, `type`, `del`, …). Still PowerShell.
    Windows,
}

impl ScriptDialect {
    /// Canonical config/CLI spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Brief => "brief",
            Self::Posix => "posix",
            Self::Windows => "windows",
        }
    }

    /// Human label for prompt headers and help text.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Full => "full — PowerShell, full cmdlet names",
            Self::Brief => "brief — PowerShell short aliases",
            Self::Posix => "posix — posix-style aliases, still PowerShell",
            Self::Windows => "windows — cmd-style aliases, still PowerShell",
        }
    }

    /// The closed alias table: (alias, canonical cmdlet). Anything not in
    /// this table keeps its full cmdlet name; the model must never invent
    /// aliases.
    pub fn aliases(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Full => &[],
            Self::Brief => &[
                ("gci", "Get-ChildItem"),
                ("gc", "Get-Content"),
                ("ri", "Remove-Item"),
                ("cp", "Copy-Item"),
                ("mv", "Move-Item"),
                ("gl", "Get-Location"),
                ("gp", "Get-ItemProperty"),
                ("gi", "Get-Item"),
                ("ni", "New-Item"),
                ("select", "Select-Object"),
                ("where", "Where-Object"),
                ("foreach", "ForEach-Object"),
            ],
            Self::Posix => &[
                ("ls", "Get-ChildItem"),
                ("cat", "Get-Content"),
                ("rm", "Remove-Item"),
                ("cp", "Copy-Item"),
                ("mv", "Move-Item"),
                ("pwd", "Get-Location"),
                ("echo", "Write-Output"),
                ("man", "Get-Help"),
                ("clear", "Clear-Host"),
                ("kill", "Stop-Process"),
                ("sleep", "Start-Sleep"),
                ("ps", "Get-Process"),
            ],
            Self::Windows => &[
                ("dir", "Get-ChildItem"),
                ("type", "Get-Content"),
                ("del", "Remove-Item"),
                ("erase", "Remove-Item"),
                ("copy", "Copy-Item"),
                ("move", "Move-Item"),
                ("cd", "Set-Location"),
                ("cls", "Clear-Host"),
                ("md", "New-Item"),
                ("rd", "Remove-Item"),
                ("ren", "Rename-Item"),
            ],
        }
    }

    /// The Block B prompt section for this dialect: the closed table plus
    /// the rules that keep the dialect honest.
    ///
    /// This section is placed at the TOP of Block A — immediately after the
    /// tool protocol, before the manifest — so it frames how the model reads
    /// everything after it. A rule at the end of Block B loses to four
    /// thousand tokens of full-fat manifest examples; the model imitates
    /// what it sees most, so the dialect must speak first and show its
    /// shape, not just state its rules.
    pub fn prompt_section(&self) -> String {
        let mut out = format!("## Script dialect: {}\n\n", self.label());
        match self {
            Self::Full => {
                out.push_str(
                    "You write ALL PowerShell command text with full cmdlet names: \
                     `Get-ChildItem`, not `gci` or `ls`. Parameters stay spelled out.\n",
                );
            }
            _ => {
                // The table's first alias drives the few-shot example, so the
                // example always shows this dialect's own vernacular.
                let (eg_alias, _) = self.aliases()[0];
                out.push_str(
                    "You write ALL PowerShell command text (the `-Command` of Invoke-FACommand, \
                     terminal input) in this dialect. The tool reference below uses full cmdlet \
                     names — that is the reference register, not your writing register.\n\
                     \nAliases:\n",
                );
                for chunk in self.aliases().chunks(3) {
                    let row: Vec<String> =
                        chunk.iter().map(|(a, c)| format!("{a} -> {c}")).collect();
                    out.push_str(&format!("- {}\n", row.join("; ")));
                }
                out.push_str(&format!(
                    "\nYou write:\n\
                     call Invoke-FACommand {{\"Command\": \"{eg_alias} -File | Sort-Object Length -Descending\"}}\n\
                     Never:\n\
                     call Invoke-FACommand {{\"Command\": \"Get-ChildItem -File | Sort-Object -Property Length -Descending\"}}\n\
                     \nRules:\n"
                ));
                match self {
                    Self::Brief => out.push_str(
                        "- Aliases rename cmdlets only. Parameters stay spelled out: `gci -Recurse` \
                         is right; `gci -r` is WRONG (prefix abbreviations can be ambiguous).\n",
                    ),
                    Self::Posix => out.push_str(
                        "- This is still PowerShell: aliases rename cmdlets only. Parameters stay \
                         PowerShell and spelled out: `ls -Recurse` is right; `ls -la` is WRONG \
                         (posix flags do not exist here).\n",
                    ),
                    Self::Windows => out.push_str(
                        "- This is still PowerShell: aliases rename cmdlets only. Parameters stay \
                         PowerShell and spelled out: `dir -Recurse` is right.\n",
                    ),
                    Self::Full => unreachable!(),
                }
                out.push_str(
                    "- Only the aliases listed above. Anything not listed keeps its full cmdlet \
                     name; never invent aliases. (Note `Sort-Object` in the example: not in the \
                     table, so it stays full — aliases and full names compose freely.)\n\
                     - Tool calls are never aliased: `call Read-FAFile {...}`, always.\n\
                     - Files written with Write-FAFile keep full cmdlet names: saved scripts must \
                     not depend on aliases.\n",
                );
            }
        }
        out
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("unknown dialect {0:?}; expected one of: full, brief, posix, windows")]
pub struct DialectParseError(String);

impl FromStr for ScriptDialect {
    type Err = DialectParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "full" | "powershell-full" | "ps-full" => Ok(Self::Full),
            "brief" | "powershell-brief" | "ps-brief" => Ok(Self::Brief),
            "posix" => Ok(Self::Posix),
            "windows" | "win" | "cmd" => Ok(Self::Windows),
            other => Err(DialectParseError(other.to_string())),
        }
    }
}

impl fmt::Display for ScriptDialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Flat shape of `<state_dir>/config/preferences.toml` (§4.10). Only the
/// keys the engine reads are modeled; anything else in the file is ignored.
#[derive(Debug, Default, serde::Deserialize)]
struct UserPreferences {
    #[serde(default)]
    dialect: Option<String>,
}

/// Read the user's persisted dialect selection from
/// `<state_dir>/config/preferences.toml` (`dialect = "posix"`).
/// Missing file, missing key, or unparsable value → None (warn, don't fail:
/// a cosmetic preference must never brick session start).
pub fn read_user_dialect(state_dir: &Path) -> Option<ScriptDialect> {
    let path = state_dir.join("config").join("preferences.toml");
    let text = std::fs::read_to_string(&path).ok()?;
    let prefs: UserPreferences = match toml::from_str(&text) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("ignoring unparsable {}: {e}", path.display());
            return None;
        }
    };
    let raw = prefs.dialect?;
    match raw.parse::<ScriptDialect>() {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::warn!("ignoring invalid dialect in {}: {e}", path.display());
            None
        }
    }
}

/// Resolve the session's dialect: explicit `--dialect` flag wins, then the
/// user's `preferences.toml`, then the default (`full`).
pub fn resolve_dialect(
    flag: Option<&str>,
    state_dir: &Path,
) -> Result<ScriptDialect, DialectParseError> {
    if let Some(raw) = flag {
        return raw.parse();
    }
    Ok(read_user_dialect(state_dir).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_and_forgiving_spellings() {
        assert_eq!("full".parse(), Ok(ScriptDialect::Full));
        assert_eq!("brief".parse(), Ok(ScriptDialect::Brief));
        assert_eq!("posix".parse(), Ok(ScriptDialect::Posix));
        assert_eq!("windows".parse(), Ok(ScriptDialect::Windows));
        assert_eq!("powershell-full".parse(), Ok(ScriptDialect::Full));
        assert_eq!("powershell-brief".parse(), Ok(ScriptDialect::Brief));
        assert_eq!("cmd".parse(), Ok(ScriptDialect::Windows));
        assert_eq!("POSIX".parse(), Ok(ScriptDialect::Posix));
        assert_eq!("  brief ".parse(), Ok(ScriptDialect::Brief));
        assert!("bogus".parse::<ScriptDialect>().is_err());
        assert!("".parse::<ScriptDialect>().is_err());
    }

    #[test]
    fn default_is_full() {
        assert_eq!(ScriptDialect::default(), ScriptDialect::Full);
        assert_eq!(ScriptDialect::Full.as_str(), "full");
    }

    #[test]
    fn alias_tables_are_closed_and_sane() {
        // Spot-check the mappings the prompt teaches; the full table was
        // verified against pwsh 7.6.6's own Get-Alias on 2026-09-18
        // (posix entries are Windows-host built-ins; PowerShell strips the
        // ones colliding with native commands on Unix).
        let brief: Vec<(&str, &str)> = ScriptDialect::Brief.aliases().to_vec();
        assert!(brief.contains(&("gci", "Get-ChildItem")));
        assert!(brief.contains(&("select", "Select-Object")));
        let posix: Vec<(&str, &str)> = ScriptDialect::Posix.aliases().to_vec();
        assert!(posix.contains(&("ls", "Get-ChildItem")));
        assert!(posix.contains(&("cat", "Get-Content")));
        let win: Vec<(&str, &str)> = ScriptDialect::Windows.aliases().to_vec();
        assert!(win.contains(&("dir", "Get-ChildItem")));
        assert!(win.contains(&("del", "Remove-Item")));
        assert!(ScriptDialect::Full.aliases().is_empty());
        // No alias may shadow a tool name or the call syntax.
        for d in [ScriptDialect::Brief, ScriptDialect::Posix, ScriptDialect::Windows] {
            for (alias, _) in d.aliases() {
                assert!(!alias.contains("FA"), "{alias} must not look like a tool name");
            }
        }
    }

    #[test]
    fn prompt_sections_carry_table_and_rules() {
        let posix = ScriptDialect::Posix.prompt_section();
        assert!(posix.contains("## Script dialect: posix"));
        assert!(posix.contains("ls -> Get-ChildItem"));
        assert!(posix.contains("`ls -la` is WRONG"));
        assert!(posix.contains("never aliased"));
        assert!(posix.contains("Write-FAFile keep full cmdlet names"));
        // Few-shot: the example shows this dialect's own vernacular, and the
        // reference register is explicitly not the writing register.
        assert!(posix.contains("\"ls -File | Sort-Object Length -Descending\""));
        assert!(posix.contains("reference register, not your writing register"));

        let brief = ScriptDialect::Brief.prompt_section();
        assert!(brief.contains("gci -> Get-ChildItem"));
        assert!(brief.contains("`gci -r` is WRONG"));
        assert!(brief.contains("\"gci -File | Sort-Object Length -Descending\""));

        let win = ScriptDialect::Windows.prompt_section();
        assert!(win.contains("dir -> Get-ChildItem"));
        assert!(win.contains("\"dir -File | Sort-Object Length -Descending\""));

        let full = ScriptDialect::Full.prompt_section();
        assert!(full.contains("## Script dialect: full"));
        assert!(full.contains("`Get-ChildItem`, not `gci` or `ls`"));
    }

    #[test]
    fn resolve_prefers_flag_then_file_then_default() {
        let dir = std::env::temp_dir().join(format!("fa-dialect-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();

        // No file -> default.
        assert_eq!(resolve_dialect(None, &dir).unwrap(), ScriptDialect::Full);
        // File -> file.
        std::fs::write(dir.join("config").join("preferences.toml"), "dialect = \"posix\"\n").unwrap();
        assert_eq!(resolve_dialect(None, &dir).unwrap(), ScriptDialect::Posix);
        assert_eq!(read_user_dialect(&dir), Some(ScriptDialect::Posix));
        // Flag beats file.
        assert_eq!(
            resolve_dialect(Some("brief"), &dir).unwrap(),
            ScriptDialect::Brief
        );
        // Bad flag is loud.
        assert!(resolve_dialect(Some("bogus"), &dir).is_err());
        // Bad file value warns and falls back to default (never bricks start).
        std::fs::write(dir.join("config").join("preferences.toml"), "dialect = \"bogus\"\n").unwrap();
        assert_eq!(resolve_dialect(None, &dir).unwrap(), ScriptDialect::Full);
        assert_eq!(read_user_dialect(&dir), None);
        // Unparsable TOML likewise.
        std::fs::write(dir.join("config").join("preferences.toml"), "[[[\n").unwrap();
        assert_eq!(resolve_dialect(None, &dir).unwrap(), ScriptDialect::Full);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
