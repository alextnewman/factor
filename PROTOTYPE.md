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
**Owner: the user, on their own Windows hardware** (llama.cpp server + local
model live there; M0–M2 are built here).
`fa32` console client: chat rendering, approval UX, WhatIf display.
llama.cpp HTTP backend wired in.
- **Done when**: end-to-end demo scenario runs against a real local model
  (8B-class): user asks for a file created, text found, and a command run in
  a persistent terminal — completed within the turn cap with at most one
  approval chain.

### M4 — Spike: can a small model drive it?
The actual risk. A fixed task battery (file ops, search, multi-step terminal
work, error recovery) run against 2–3 small local models, scored on
tool-call validity and task completion.
- **Done when**: results recorded with a recommendation — proceed local-first,
  or revise the model story before v1 design continues.

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
- M4 model battery (model, tasks passed, tool-call validity %):
- M4 recommendation:

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
