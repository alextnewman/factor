//! Generic room derivation: the directory-scope of the workspace a tool
//! call acts in.
//!
//! No tool-specific knowledge lives here. Every string argument is treated
//! the same: a value that resolves (against `cwd` when relative) to an
//! existing file contributes its parent directory, an existing directory
//! contributes itself, and a nonexistent but path-shaped value contributes
//! its nearest existing ancestor directory. Values that are not path-shaped
//! (command text, prose, glob patterns) never resolve, so they never move
//! the room. Candidates outside `workspace_root` are ignored; the deepest
//! candidate wins.
//!
//! The engine puts the result on the wire as `room` (canonical
//! repo-relative, `/`-separated, `"."` for the root) so every camera can
//! render the trail without inferring location from argument strings.

use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

/// Whether a nonexistent value is shaped like a path at all (as opposed
/// to command text, prose, or a glob pattern). Existence is the primary
/// test; this only gates the nearest-ancestor fallback so that
/// `"ls -File | ..."` never resolves to the cwd and spuriously resets the
/// trail to the root room.
fn path_shaped(s: &str) -> bool {
    if s.contains('/') || s.contains('\\') {
        return true;
    }
    !s.chars()
        .any(|c| c.is_whitespace() || "|<>$`\"';*?".contains(c))
}

/// Lexically normalize `.`/`..` without touching the filesystem.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// The room a tool call acts in, derived generically from its arguments.
/// See the module docs for the rule. Returns `None` when no argument names
/// a location inside the workspace.
pub fn room_for_args(
    args: &Map<String, Value>,
    workspace_root: &Path,
    cwd: &Path,
) -> Option<String> {
    let mut best: Option<(usize, PathBuf)> = None;
    for v in args.values() {
        let s = match v.as_str() {
            Some(s) => s.trim(),
            None => continue,
        };
        // Path-shaped values are short; skip prose/script blobs early so we
        // never stat a 10 KiB Write-FAFile body.
        if s.is_empty() || s.len() > 512 {
            continue;
        }
        let p = Path::new(s);
        let abs = normalize(&if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        });
        // Nearest existing ancestor: the path itself when it exists, else
        // walk up — but only for path-shaped values, so command text never
        // falls back to the cwd and spuriously resets the room to root.
        // A file contributes its parent directory.
        let mut cand: Option<PathBuf> = None;
        if abs.is_dir() {
            cand = Some(abs.clone());
        } else if abs.is_file() {
            cand = abs.parent().filter(|p| p.is_dir()).map(Path::to_path_buf);
        } else if path_shaped(s) {
            let mut cur: Option<&Path> = abs.parent();
            while let Some(c) = cur {
                if c.is_dir() {
                    cand = Some(c.to_path_buf());
                    break;
                }
                cur = c.parent();
            }
        }
        let rel =
            match cand.and_then(|c| c.strip_prefix(workspace_root).ok().map(Path::to_path_buf)) {
                Some(r) => r,
                None => continue,
            };
        let depth = rel.components().count();
        let deeper = best.as_ref().is_none_or(|(d, _)| depth > *d);
        if deeper {
            best = Some((depth, rel));
        }
    }
    best.map(|(_, rel)| {
        if rel.as_os_str().is_empty() {
            ".".to_string()
        } else {
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn tree() -> (TempGuard, PathBuf) {
        // Build: root/crates/fa-core/src/manifest.rs, root/crates/factoragent/tests/x.rs
        let base = std::env::temp_dir().join(format!(
            "fa-room-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let src = base.join("crates/fa-core/src");
        let tests = base.join("crates/factoragent/tests");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tests).unwrap();
        fs::write(src.join("manifest.rs"), "// hi").unwrap();
        fs::write(tests.join("x.rs"), "// hi").unwrap();
        (TempGuard(base.clone()), base)
    }

    struct TempGuard(PathBuf);
    impl Drop for TempGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn file_arg_yields_parent_dir() {
        let (_g, root) = tree();
        let a = args(json!({"Path": "crates/fa-core/src/manifest.rs"}));
        assert_eq!(
            room_for_args(&a, &root, &root),
            Some("crates/fa-core/src".to_string())
        );
    }

    #[test]
    fn dir_arg_yields_itself_and_deepest_wins() {
        let (_g, root) = tree();
        // -Pattern "*.rs" resolves nowhere; its nearest existing ancestor is
        // the cwd, depth 0 — the real -Path wins by depth.
        let a = args(json!({"Pattern": "*.rs", "Path": "crates/factoragent/tests"}));
        assert_eq!(
            room_for_args(&a, &root, &root),
            Some("crates/factoragent/tests".to_string())
        );
    }

    #[test]
    fn nonexistent_file_yields_nearest_existing_ancestor() {
        let (_g, root) = tree();
        let a = args(json!({"Path": "crates/newdir/draft.ps1"}));
        assert_eq!(room_for_args(&a, &root, &root), Some("crates".to_string()));
    }

    #[test]
    fn glob_pattern_never_moves_the_room() {
        let (_g, root) = tree();
        let a = args(json!({"Pattern": "*.rs", "Path": "."}));
        // "*.rs" is not path-shaped (glob metachar); "." is the root.
        assert_eq!(room_for_args(&a, &root, &root), Some(".".to_string()));
    }

    #[test]
    fn bare_new_filename_resolves_to_cwd() {
        let (_g, root) = tree();
        let a = args(json!({"Path": "draft.ps1"}));
        assert_eq!(room_for_args(&a, &root, &root), Some(".".to_string()));
    }
    #[test]
    fn workspace_root_itself_is_dot() {
        let (_g, root) = tree();
        let a = args(json!({"Path": "."}));
        assert_eq!(room_for_args(&a, &root, &root), Some(".".to_string()));
    }

    #[test]
    fn command_text_never_moves_the_room() {
        let (_g, root) = tree();
        let a = args(json!({"Command": "ls -File | Sort-Object Length -Descending"}));
        assert_eq!(room_for_args(&a, &root, &root), None);
    }

    #[test]
    fn outside_workspace_is_ignored() {
        let (_g, root) = tree();
        // temp_dir() is an ancestor of (or unrelated to) the tree — never
        // *under* the workspace root, so it contributes no room.
        let a = args(json!({"Path": std::env::temp_dir().to_string_lossy()}));
        assert_eq!(room_for_args(&a, &root, &root), None);
    }

    #[test]
    fn parent_dir_segments_normalize() {
        let (_g, root) = tree();
        let a = args(json!({"Path": "crates/fa-core/src/../src"}));
        assert_eq!(
            room_for_args(&a, &root, &root),
            Some("crates/fa-core/src".to_string())
        );
    }
}
