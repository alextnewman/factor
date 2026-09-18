//! M1: real-bridge integration — create file → find text → terminal command,
//! plus manifest reflection, WhatIf previews, and terminal persistence.
//! Skips gracefully when pwsh is not on PATH.

use fa_bridge::{HostBridge, SessionEnv};
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

#[tokio::test]
async fn m1_create_find_terminal() {
    if !pwsh_present() {
        eprintln!("SKIP m1_create_find_terminal: pwsh not on PATH");
        return;
    }
    let bridge_ps1 = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../psmodule/FactorAgent/bridge.ps1"
    );
    assert!(
        std::path::Path::new(bridge_ps1).exists(),
        "bridge.ps1 missing at {bridge_ps1}"
    );
    let work = std::env::temp_dir().join(format!("fa-m1-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    // Clean slate: remove leftovers from a previous crashed run.
    let _ = std::fs::remove_file(work.join("notes.txt"));

    let (mut bridge, cold) = HostBridge::spawn(
        bridge_ps1,
        &SessionEnv {
            session_json: r#"{"id":"m1-test","mode":"managed","backend":"host"}"#.into(),
            error_action: "Stop".into(),
            max_terminals: 8,
            cwd: work.to_string_lossy().into(),
        },
    )
    .await
    .expect("spawn host");
    eprintln!("M1 host cold start: {cold:?}");

    // 1. The manifest IS the module: 11 agent tools reflected.
    let m = bridge
        .call("Get-FAToolManifest", &args(json!({})))
        .await
        .unwrap();
    let tools: Vec<Value> = serde_json::from_str(m.as_str().unwrap()).unwrap();
    assert_eq!(tools.len(), 11, "expected 11 agent tools, got {tools:?}");

    // 2. Create a file.
    let f = work.join("notes.txt");
    let content = "alpha\nTODO: fix the thing\nbeta\n";
    let r = bridge
        .call(
            "Write-FAFile",
            &args(json!({"Path": f.to_string_lossy(), "Content": content})),
        )
        .await
        .unwrap();
    assert_eq!(r["BytesWritten"], content.len() as u64);
    assert_eq!(r["Created"], true);

    // 3. Find text in it.
    let r = bridge
        .call(
            "Find-FAText",
            &args(json!({"Pattern": "TODO", "Path": work.to_string_lossy()})),
        )
        .await
        .unwrap();
    let hits = r.as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["LineNumber"], 2);
    assert!(hits[0]["Line"].as_str().unwrap().contains("TODO"));

    // 4. Read it back through the numbered-lines tool.
    let r = bridge
        .call("Read-FAFile", &args(json!({"Path": f.to_string_lossy()})))
        .await
        .unwrap();
    let lines = r.as_array().unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1].as_str().unwrap(), "2: TODO: fix the thing");

    // 5. WhatIf preview: no disk touch, predictable object shape.
    let ghost = work.join("ghost.txt");
    let r = bridge
        .call(
            "Write-FAFile",
            &args(json!({"Path": ghost.to_string_lossy(), "Content": "x", "_WhatIf": true})),
        )
        .await
        .unwrap();
    assert!(r["Preview"].as_str().unwrap().contains("Write 1 bytes"));
    assert!(!ghost.exists(), "_WhatIf must not touch the disk");

    // 6. Terminal: new → command → state persists → remove.
    let r = bridge
        .call("New-FATerminal", &args(json!({"Name": "m1t"})))
        .await
        .unwrap();
    assert_eq!(r["State"], "running");
    let r = bridge
        .call(
            "Invoke-FACommand",
            &args(json!({"Terminal": "m1t", "Command": "$M1Probe = 40 + 2"})),
        )
        .await
        .unwrap();
    assert_eq!(r["ExitCode"], 0);
    // The dot-sourced terminal must remember $M1Probe across commands.
    let r = bridge
        .call(
            "Invoke-FACommand",
            &args(json!({"Terminal": "m1t", "Command": "$M1Probe"})),
        )
        .await
        .unwrap();
    assert_eq!(r["ExitCode"], 0);
    assert!(
        r["Output"].as_str().unwrap().contains("42"),
        "terminal state did not persist: {}",
        r["Output"]
    );
    let r = bridge
        .call("Remove-FATerminal", &args(json!({"Name": "m1t"})))
        .await
        .unwrap();
    assert_eq!(r["Removed"], true);

    // 7. Errors propagate as tool errors, not transport failures.
    let err = bridge
        .call(
            "Read-FAFile",
            &args(json!({"Path": work.join("nope.txt").to_string_lossy()})),
        )
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Read-FAFile"), "unexpected error shape: {msg}");

    // 8. Non-FA methods are refused at the bridge.
    let err = bridge
        .call("Get-Process", &args(json!({})))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("refusing"), "unexpected: {err}");

    bridge.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&work);
    eprintln!("M1 integration: PASS");
}
