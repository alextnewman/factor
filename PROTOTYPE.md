# FactorAgent / WinAgent32 — Prototype Plan

Drafted 2026-09-16. Design-only; no implementation authorized yet.
Source of truth for the design: `DESIGN.md` (this doc plans the build, it doesn't redesign it).

## 1. Goal

Validate the core architectural bets with the smallest build that exercises the
**full loop**: user → `fa32` console client → ACP → `factoragent` Rust core →
PowerShell host → `FA` cmdlets → back.

The prototype exists to answer three questions, in order:

1. Can Rust drive a persistent PowerShell host reliably and fast enough?
2. Can the agent loop (prompt → tool calls → approval → results) run end to end?
3. Can a small local model actually drive the tool vocabulary, or does
   local-first need rethinking before it infects the design?

Everything else is deferred (§6).

## 2. Architecture sketch (prototype)

```
┌──────────┐   ACP (subset)    ┌──────────────┐  JSON-RPC     ┌─────────────────┐
│   fa32   │ ◄──────────────► │  factoragent │ ◄────────────► │ pwsh host proc  │
│ console  │  named pipe       │  Rust core   │  named pipe   │ FactorAgent mod │
│  client  │  \\.\pipe\...     │  session lock│  per-proc     │  FA cmdlets     │
└──────────┘                 │  SQLite db   │               └─────────────────┘
                             │  llama.cpp   │  JSON-RPC           ┌───────────┐
                             │  HTTP client │ ◄─────────────────► │ terminal  │
                             └──────────────┘  per-terminal       │ pwsh procs│
                                        │                        └───────────┘
                                        ▼
                                  llama.cpp server
                                  (or mock backend)
```

- **Session**: one `factoragent` process per session. Owns the named mutex
  (`Local\WinAgent32\session\<session-id>`), serves the named pipe
  (`\\.\pipe\WinAgent32\<session-id>`), owns the per-session SQLite DB.
- **Engine host**: one long-lived `pwsh` child process importing the
  `FactorAgent` module. Managed `FA` cmdlets run here, each in a fresh child
  scope (§4.4). Rust↔host wire: **JSON-RPC over a named pipe** (spike M0
  settles the exact framing).
- **Terminals**: one `pwsh` child process per terminal (§4.12), same JSON-RPC
  wire, lazy default terminal on first raw command.
- **Isolation**: host backend only. The backend is a trait from day one
  (§7.1); the prototype ships one implementation.
- **Model**: `factoragent` talks to a **llama.cpp server over HTTP**.
  A **mock backend** (scripted tool-call sequences) ships alongside so the loop
  is testable with no model running.
- **Tool-call protocol**: text-first. The model emits calls in fenced blocks;
  the exact grammar is settled by spike M2 (candidate: ```` ```fa ```` blocks
  with `call <Cmdlet> {json args}`). Structured function calling is a later
  upgrade, not a prototype requirement.

## 3. Scope — in

**Binaries**
- `factoragent`: session lock, pipe server, ACP subset server, process
  supervision, SQLite session DB, agent loop, llama.cpp + mock backends.
- `fa32`: console chat loop, renders agent output, chain-approval prompts
  (approve / deny / edit), WhatIf expansion display.

**FactorAgent module** — 10 cmdlets:
- `Get-FASession`
- `Read-FAFile`, `Write-FAFile`, `Edit-FAFile`
- `Find-FAFile`, `Find-FAText`
- `New-FATerminal`, `Get-FATerminal`, `Invoke-FACommand`, `Remove-FATerminal`

**Loop policy**: turn cap (25), full-history context, no compaction,
`$FAErrorAction` honored, chain approval as structured data (console-rendered).

**Scopes**: session scope only (focus, working notes). User/project scope
stubbed — read paths exist, project.toml discovery deferred.

**DB**: SQLite via rusqlite, WAL mode. Minimal schema: `sessions`, `events`
(append-only audit), `terminals`, `scope_kv`.

## 4. Milestones

### M0 — Bridge spike (first, blocks everything)
Rust spawns `pwsh`, imports a stub module, JSON-RPC round-trips one cmdlet
(`Read-FAFile`), streams output, kills a hung command cleanly.
- **Settles**: wire framing, message shapes, cancellation semantics.
- **Records**: round-trip latency distribution on real Windows hardware;
  `pwsh` cold-start cost (the measurement that justifies persistent terminals).
- **Done when**: p50 trivial-call round trip measured and recorded; kill is
  reliable; no stdout/stderr interleaving corruption across 1,000 calls.

### M1 — Engine
The 10-cmdlet `FactorAgent` module (help-complete per §4.9 conventions),
host-process lifecycle, terminal lifecycle, SQLite session DB + event log.
- **Done when**: every cmdlet has Pester tests; a Rust integration test drives
  create-file → find-text → terminal-command through the real bridge.

### M2 — Loop
Agent loop against the **mock backend**: prompt construction per §8
(static Block A with the 10 cmdlet schemas, Block B session facts),
text-protocol parsing, turn cap, `$FAErrorAction`, approval-request data model.
- **Settles**: the text tool-call grammar (fuzz it with malformed model output).
- **Done when**: scripted mock scenarios complete without human intervention
  except approvals.

### M3 — Client + model
**Owner: the agent, in this Linux environment** (2026-09-16: the user
authorized building all the way through the live llama.cpp test here,
iterating independently). The Windows end-to-end demo still runs on the
user's hardware afterward.
`fa32` console client: chat rendering, approval UX, WhatIf display.
llama.cpp HTTP backend wired in.
- **Done when**: end-to-end demo scenario runs against a real local model:
  user asks for a file created, text found, and a command run in a
  persistent terminal — completed within the turn cap with at most one
  approval chain. Model choice is hardware-driven: this environment has
  2 CPUs and ~7 GiB RAM, so the provisional model is **Qwen3-4B-Q4_K_M**
  (2.5 GB, official `Qwen/Qwen3-4B-GGUF` repo) rather than 8B-class; the
  8B question belongs to M4's battery on real hardware.

### M4 — Spike: can a small model drive it?
The actual risk. A fixed task battery (file ops, search, multi-step terminal
work, error recovery) run against 2–3 small local models, scored on
tool-call validity and task completion.
- **Done when**: results recorded with a recommendation — proceed local-first,
  or revise the model story before v1 design continues.

### M5 — Windows transport (unblocks the native battery)
The fa32↔factoragent session channel was Unix-socket-only. M5 ports it:
named pipe `\\.\pipe\WinAgent32\<session-id>` on Windows, identical
newline-delimited JSON-RPC framing, type-erased stream halves in `fa-core`.
Session lock becomes an exclusive-share file handle (flock on Unix);
the pwsh host gets its own process group via `CREATE_NEW_PROCESS_GROUP`
and is reaped with `taskkill /T` (Job Object is the v1 hardening).
`cargo check --target x86_64-pc-windows-gnu` is green; Linux tests still pass.
- **Done when**: the M4 battery runs end-to-end on a real Windows machine
  (GPU), via `scripts/m4-battery/Run-M4Battery.ps1` — see `WINDOWS.md`.

## 5. Definition of done (prototype)

1. M0–M3 complete; the demo scenario runs on Windows with a local model.
2. All three spike measurements recorded in this doc's appendix.
3. M4 recommendation written: local-first stands or the design changes.
4. No prototype code depends on anything in §6 (deferred list).

## 6. Explicitly deferred

WinUI3 client · MSIX packaging · MXC sandbox (host backend only) · toolshop
mode (`New-FATool`, `Submit-FATool`) · MCP server · toolchain environments
(`-Toolchain` accepted nowhere yet) · `Invoke-FABuild` / structured
diagnostics · user/project scope (`project.toml` discovery) · prompt
compaction · multi-backend.

## 7. Risks

| Risk | Mitigation |
|---|---|
| Small local models can't reliably emit the tool protocol | M4 measures it early; text protocol is the forgiving choice |
| pwsh IPC proves flaky under load (encoding, hangs, orphaned procs) | M0 stress criteria; kill semantics specified before M1 |
| ACP subset drifts from the real spec | M0 includes reading the actual ACP spec; envelope-first, methods-minimal |
| Persistent terminals leak state across tasks in confusing ways | Terminal identity in every approval (§4.12); `Remove-FATerminal` is cheap |

## Appendix A — Spike results

*(Record M0/M4 measurements here as they land.)*

- M0 round-trip latency (p50/p99, Windows hardware):
- M0 `pwsh` cold-start cost:
- M0 kill reliability (kills / attempts):
- M4 model battery (model, tasks passed, tool-call validity %): qwen3-4b 6/6 PASS, 100% first-try validity
  (full matrix, incl. t5 error-recovery via heal-and-continue); qwen3-1.7b 3/6 — protocol nearly clean
  (96% first-try validity) but task-strategy failures; smollm2-1.7b NO DATA ×6 — the re-run died on a NEW
  infrastructure failure (hermetic session-dir paths exceed Unix SUN_LEN, session socket never spawns);
  verdict PARTIAL — details below.
- M4 recommendation: PARTIAL — 4B class clears the full 6-task harness bar; local-first stands as the
  working hypothesis for 4B. 1.7B class (qwen3): clears the protocol (96% first-try validity), fails at task
  strategy (3/6). No claim on smollm2-1.7b until its 6 cells run without the SUN_LEN harness bug (see below).

### M4 results — 2026-09-17 (Linux, llama.cpp b11011 CPU) — verdict PARTIAL

Battery: 3 models × 6 tasks via `scripts/m4-battery/run.sh`
(`--auto-approve --error-mode heal --turn-cap 6`); completion verified against
ground truth on disk (`check.py`), protocol validity from the session DB
(`m4score`). The orchestrator ran the full chain 08:22–08:54 PDT; the deadline
(11:15 PDT) passed with no live processes, so the battery is declared done/dead.

| model | task | completed | 1st-try valid | turns | approvals | tool errs |
|---|---|---|---|---|---|---|
| qwen3-4b | t1 (file create) | PASS | 100% (1/1) | 2 | 1 | 0 |
| qwen3-4b | t2 (search) | PASS | 100% (1/1) | 2 | 0 | 0 |
| qwen3-4b | t3 (edit line) | PASS | 100% (1/1) | 2 | 1 | 0 |
| qwen3-4b | t4 (terminal persist) | PASS | 100% (1/1) | 2 | 1 | 0 |
| qwen3-4b | t5 (error recovery) | NO DATA | infra: LLM connection refused | — | — | — |
| qwen3-4b | t6 (3-step chain) | NO DATA | infra: LLM connection refused | — | — | — |
| smollm2-1.7b | t1–t6 | NO DATA ×6 | infra: session-socket spawn failure | — | — | — |
| qwen3-1.7b | t1–t6 | NO DATA ×6 | infra: session-socket spawn failure | — | — | — |

qwen3-4b: 4/4 measured tasks PASS | first-try validity 100% (4/4) |
post-warning validity n/a (no warnings issued) | warning recovery n/a
smollm2-1.7b: 0/6 — no model data
qwen3-1.7b: 0/6 — no model data

**Missing data (14 cells, explicitly):** qwen3-4b t5 + t6 (each run failed with
`Error: io: Connection refused` — the `fa32 run` LLM backend never connected to
llama-server; the model produced zero output), and all 6 tasks each for
smollm2-1.7b and qwen3-1.7b (each failed with `Error: io: No such file or
directory` + `timed out waiting for session socket` — the session subprocess
never spawned; zero model turns). These cells hold zero-stub score files
(`turns_used: 0, total_calls: 0`) — they are infrastructure failures, **not**
model failures. The failures started after qwen3-4b t4 passed at ~08:27 PDT.

**M4 recommendation (PARTIAL, calibrated per the user's steer):** judged as a
harness+model system, the 4B class clears the bar on what was measured — fenced
`Call as` JSON on the first try in all 4 cells, zero parser warnings, approvals
engaged where Write-FAFile demanded them, ground-truth-verified task
completion. Local-first stands as the working hypothesis **for the 4B class**.
The floor the user asked to name: 1.7B-class models have produced **no data at
all** — the battery says nothing about whether they clear protocol validity
even with parser warnings and heal-and-continue. Before v1 design continues on
the model story: (1) fix the session-socket spawn / LLM-connect flakiness in
`fa32 run` (the chain went 4/4 green then died every cell for the rest of the
morning — nothing in the battery design is worth measuring until the harness
survives its own orchestration), (2) re-run the 14 missing cells, (3) then
confirm t5 (error recovery) and t6 (multi-step chain) for 4B and the full
matrix for the two 1.7B models. Do not claim sub-4B support in v1 until the
re-run lands.

### M4 re-run — 2026-09-17 ~11:45–12:05 PDT (Linux, llama.cpp CPU) — verdict PARTIAL

The 14 missing cells were re-launched after the harness repair (commit
238ec10: pwsh self-heal preflight, hermetic session dirs). 12/18 cells now
hold real score files. At 15:00 PDT no llama-server or runner processes were
alive and no log had fresh activity since 12:05 PDT — the battery is declared
dead; the re-run did not complete.

| model | task | completed | 1st-try valid | turns | approvals | tool errs |
|---|---|---|---|---|---|---|
| qwen3-4b | t1 (file create) | PASS | 100% (1/1) | 2 | 1 | 0 |
| qwen3-4b | t2 (search) | PASS | 100% (1/1) | 2 | 0 | 0 |
| qwen3-4b | t3 (edit line) | PASS | 100% (1/1) | 2 | 1 | 0 |
| qwen3-4b | t4 (terminal persist) | PASS | 100% (1/1) | 2 | 1 | 0 |
| qwen3-4b | t5 (error recovery) | PASS | 100% (1/1) | 2 | 0 | 1 |
| qwen3-4b | t6 (3-step chain) | PASS | 100% (1/1) | 2 | 1 | 0 |
| smollm2-1.7b | t1–t6 | NO DATA ×6 | infra: session.sock path > SUN_LEN | — | — | — |
| qwen3-1.7b | t1 (file create) | PASS | 100% (6/6) | 6 | 1 | 0 |
| qwen3-1.7b | t2 (search) | FAIL | 100% (6/6) | 6 | 0 | 3 |
| qwen3-1.7b | t3 (edit line) | PASS | 100% (1/1) | 2 | 1 | 0 |
| qwen3-1.7b | t4 (terminal persist) | PASS | 100% (6/6) | 6 | 6 | 14 |
| qwen3-1.7b | t5 (error recovery) | FAIL | 100% (6/6) | 6 | 0 | 1 |
| qwen3-1.7b | t6 (3-step chain) | FAIL | 67% (2/3) | 3 | 2 | 0 |

qwen3-4b: 6/6 PASS | first-try validity 100% (6/6) | post-warning validity n/a
(no warnings issued) | warning recovery n/a
smollm2-1.7b: 0/6 — no model data (infrastructure)
qwen3-1.7b: 3/6 | first-try validity 96% (27/28) | post-warning validity n/a
(0/0) | warning recovery 0% (0/1)

Notes on the qwen3-4b t5 cell: the single tool error was absorbed and the
cell still passed — heal-and-continue working as designed. The 1.7B-class
result is real model signal: protocol validity is nearly clean (96%
first-try, exactly one parser warning across 18 tasks — and that warning
came on the final line of t6, with 0 recoveries), but completion is 3/6.
The failures are task-strategy failures, not protocol failures: t2 found the
search target but named the wrong file in its final text; t5 ran a bogus
Find-FAText instead of verifying the ghost file it was asked about; t6 did
the on-disk work (`t6.txt` correct) but never reported the steps. The floor
the user asked to name: **a 1.7B-class model clears the tool protocol with
imitation lines + parser warnings, but cannot reliably complete tasks —
3/6 on ground-truth completion, with one non-recovered protocol warning.**
The bar for the local-first model story is the 4B class.

**Missing data (6 cells, explicitly):** smollm2-1.7b t1–t6. All six died
with `Error: io: path must be shorter than SUN_LEN` followed by `timed out
waiting for session socket .../state/smollm2-1.7b/sessions/
m4-smollm2-1.7b-t6/session.sock` — the hermetic session dirs from the
238ec10 repair lengthened the socket path past the 108-byte Unix limit (the
full path is ~110 chars), so the harness never spawned; zero model turns.
New harness bug, **not** a model failure. Before a third attempt: shorten or
hash session dir names (or anchor them under a short TMPDIR) so the socket
path stays well under SUN_LEN.

**M4 recommendation (PARTIAL, calibrated per the user's steer):** judged as
a harness+model system, qwen3-4b clears the full 6-task bar — fenced `Call
as` JSON on the first try in every cell, zero parser warnings, approvals
engaged where Write-FAFile demanded them, t5's tool error healed and the
cell still passed. Local-first stands as the working hypothesis **for the
4B class**. qwen3-1.7b shows protocol compliance is not enough — it emits
valid calls and still fails half the tasks. v1's model story: 4B-class and
up; do not claim 1.7B support. And before any third run: fix the SUN_LEN
session-dir bug, then run smollm2-1.7b's 6 cells clean so the 1.7B-class
claim rests on two models, not one.

### M0 results — 2026-09-16 (Linux, pwsh 7.6.6, stdio framing)

Rust drives the pwsh JSON-RPC bridge (`psmodule/FactorAgent/bridge.ps1` +
`crates/fa-bridge/examples/m0.rs`). All green:

- **Cold start** (spawn -> first response, incl. module import): **512 ms**
- **Round-trip** over 1000 `Read-FAFile` calls: **mean 889 us, p50 782 us,
  p99 3.0 ms, max 16.1 ms** — no interleaving corruption across all 1000 calls
- **Error propagation**: missing file -> JSON-RPC error object with message
- **Method allowlist**: non-`FA` method refused by the bridge
- **Clean shutdown**: bridge exits on stdin EOF
- **Kill reliability**: hung `pwsh` reaped **6.3 ms** after kill signal, 1/1

Notes: measured on Linux; Windows cold-start is expected to be slower (the
persistent-terminal decision exists precisely because of it — to be measured
on real Windows hardware). Framing validated over stdio; Windows will use a
named pipe with identical newline-delimited JSON-RPC framing. Per-call
overhead (~0.8 ms) is negligible next to model latency.

### M1 results — 2026-09-17 (Linux, pwsh 7.6.6, real bridge)

Ten cmdlets, all real, all Pester-covered (`psmodule/FactorAgent`, 39/39
Pester green), driven end-to-end from Rust (`crates/factoragent/tests/
m1_bridge.rs`, 1/1):

- **Host cold start** (spawn -> first response, module import incl. all 10
  cmdlets): **644 ms** (M0 was 512 ms; the module grew up)
- **Manifest reflection**: bridge reflects exactly **11 agent tools** from
  comment-based help (synopsis, typed parameters, examples)
- **File round-trip**: Write-FAFile -> Find-FAText (1 hit) -> Read-FAFile
  numbered lines — all shapes asserted on the Rust side
- **Terminal round-trip**: New-FATerminal -> Invoke-FACommand ->
  cross-command state persistence (`$M1Probe` survives) -> Remove-FATerminal;
  lazy default terminal; unknown-terminal and duplicate-name errors are loud
- **WhatIf**: `_WhatIf` previews without touching disk/processes; result
  shape carries `Preview`
- **Error propagation**: tool errors surface as JSON-RPC errors;
  non-`FA` methods refused

Two protocol bugs found and fixed by real-pwsh testing (see DESIGN.md
decision log 2026-09-17):

- **Scalar unwrap**: single-hit Find-FAText / one-line Read-FAFile returned a
  bare object instead of an array. Collection cmdlets now emit via
  `Write-Output -NoEnumerate` — wire shapes are count-independent.
- **WhatIf framing corruption**: the engine's `What if:` line is written by
  the console host straight to stdout, bypassing every stream. `_WhatIf`
  calls now set a module-scope `$script:FAForceWhatIf` flag and invoke
  *without* `-WhatIf`; cmdlets short-circuit to their Preview object.
  Human `-WhatIf`/`-Confirm` behavior unchanged.

### M2 results — 2026-09-17 (Linux, pwsh 7.6.6, mock backend)

Agent loop against the mock backend (`crates/factoragent/tests/m2_loop.rs`,
5/5) plus unit tests (`cargo test -p fa-core`, 7/7) and an RPC regression
test (1/1):

- **Happy path**: 3-turn mock script (write file -> find text -> report)
  completes; append-only event log records `turn.started`/`tool.result`
- **Approval deny**: `Err(FaError::Denied)`; denied write never touches disk
- **Approval edit**: operator-rewritten args executed; file contains the
  edited content
- **StopAndReport**: tool failure aborts with `Err(FaError::ToolFailed)`
  carrying the real tool name + message (per §4.6 "real error surfaces")
- **HealAndContinue**: failure fed back as a user message; mock self-corrects
  and the loop completes in 2 turns; failed result logged with `ok=false`
- **Parser** (unit): fenced `fa` blocks parsed; prose outside fences ignored;
  malformed lines become warnings, never panics (fuzz corpus)
- **Prompt blocks** (unit): Block A/B order stable and deterministic
- **Session store** (unit): append-only events, `scope_kv` precedence

**RPC approval deadlock — found and fixed.** The server read loop awaited
`on_request` inline; `session.prompt` blocks on the client's
`event.approval_resolved` notification, which the stuck read loop could
never read: deadlock on any approval over the socket. The read loop now
dispatches every request/notification as an independent task; response
writes stay serialized via the writer mutex. Regression test
`rpc_request_survives_notification_roundtrip` fails (10 s timeout) on the
old code and passes on the new. Approve/deny/edit over the socket get
their full end-to-end workout in M3 with the live model.

### M3 results — 2026-09-17 (Linux, pwsh 7.6.6, live llama.cpp + Qwen3-4B)

End-to-end through `fa32 run` -> Unix socket -> `factoragent serve` ->
llama.cpp (`b11011`, `0.4.1-dev`, commit `aa39d7a3e`) serving
`Qwen/Qwen3-4B-GGUF/Qwen3-4B-Q4_K_M.gguf` (2,497,280,256 bytes, GGUF magic
verified), thinking disabled, `max_tokens: 1024`, temperature `0.2`,
`cache_prompt: true`. One scenario, one approval chain, piped `a`:

> write `live-demo.txt` (two lines, `marker-4271` on line 2) -> Find-FAText
> the marker -> persistent terminal: set state, read it back -> prose report.

- **Turn 1**: 2396 prompt + 88 completion tokens (**249 cached**), 185.8 s
  wall, 163.5 s server prompt-eval. The model emitted a 4-call fenced chain
  in valid JSON on the first try (after the prompt fixes below).
- **Approval**: exactly **1** chain; previews rendered
  (`New-FATerminal`, `Write-FAFile`, `Invoke-FACommand`; the read-only
  `Find-FAText` needed none); operator approved.
- **Execution**: `New-FATerminal` 532 ms, `Write-FAFile` 18 ms,
  `Find-FAText` 42 ms, `Invoke-FACommand` 159 ms — all `ok: true`.
- **Turn 2**: 2876 prompt + 102 completion (**2356 cached**), 75.7 s wall,
  46.3 s server prompt-eval — KV-cache reuse cut prompt-eval ~3.5x.
- **Session record**: 2 turns, 4 tool.call/tool.result pairs, append-only
  `events` table; terminal `default` persisted in `terminals`.
- **File on disk**: `live-demo.txt` = `FIRST LINE\nmarker-4271`, exactly as
  the model wrote it; Find-FAText found the marker.

**Prompt engineering, earned the hard way** (see DESIGN.md decision log
2026-09-17):

- *Attempt 1–2*: the model wrote PowerShell syntax (`-Path foo`) instead of
  JSON args, twice, and did not self-correct from the "invalid JSON"
  warning. Root cause: Block A was *full* of PowerShell (cmdlet names,
  `-Param` help, PS `.EXAMPLE` snippets) and showed JSON only twice.
- *Fix*: every tool now renders a synthesized `Call as: call <Name>
  {"Req": "..."}` line (required params only) directly under its heading —
  10 JSON shapes to imitate. Plus an explicit anti-example in the preamble
  (`-Path notes.txt` is WRONG) and a targeted parser warning when args look
  like `-Param value` syntax.
- *Attempt 3*: valid JSON on the first try — but the model filled an
  invented `ProgressAction` string. Root cause: the manifest reflected the
  PS **common parameter** `ProgressAction` (missing from the `$skip` list),
  typed as free string. Fixed by adding it to `$skip`; `ActionPreference`
  is harness territory, not agent vocabulary.
- *Attempt 4 (this run)*: clean 4-call chain, first try.

**Model fidelity notes** (M4-relevant, not harness bugs): the model merged
the two terminal commands into one (`$env:demo='terminal-alive'; $env:demo`
— so cross-*command* persistence wasn't strictly re-proven live, though M1
Pester already proves it) and dropped "second line" from the file content.
A 4B model drives the protocol reliably *once the prompt teaches JSON*,
but its instruction fidelity is loose — the M4 battery should measure this.

Gates at commit: Pester 39/39, `cargo test --workspace` all green,
`cargo fmt --check` clean, `cargo clippy --workspace --all-targets`
`-D warnings` clean.
