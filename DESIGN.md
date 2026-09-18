# WinAgent32 / FactorAgent — Design Doc

> Status: **core architecture locked 2026-09-16; naming decided 2026-09-16;**
> **toolset, terminals, toolchains, scopes, and trust model locked 2026-09-16** —
> decisions recorded as `[DECIDED]`, live threads as `[OPEN]` (none open).
> This doc is the source of truth for the architecture. Update it when a thread resolves.

## 0. One-line pitch

A Windows-native agentic coding harness: a portable PowerShell-speaking agent engine
(**FactorAgent**) inside an unrepentantly Windows-shaped distribution (**WinAgent32**).
A terminal that grew an agent — not an agent with a terminal bolted on.

## 1. Naming & split — [DECIDED]

- **FactorAgent** — the engine. Portable, PowerShell-native: the tool module, the session
  runner, the Rust service, ACP. Runs anywhere PowerShell 7 runs. *Factor* (Latin: doer,
  maker; English: mercantile agent) — the thing that does. Decided 2026-09-16 after a
  GitHub collision peek and bike-shedding: `HermesAgent` is hard-blocked by the active
  [NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent/releases/tag/v2026.8.18)
  project and its derivatives; `LegatusAgent` is blocked by
  [yevkavaliou/legatus-agent](https://github.com/yevkavaliou/legatus-agent) (Dockerized AI
  agent); `ActaAgent`'s root collides with the active
  [acta-dotnet/acta](https://github.com/acta-dotnet/acta) durable-jobs project (adjacent
  problem space, genuinely confusing); `NuntiusAgent` is clean but thinner; `FactorAgent`
  shows no shipped product under that exact name — only code-level class names in unrelated
  domains (belief-propagation simulators, a hackathon equity-factors repo) — and no
  PowerShell Gallery module. Pre-publish checklist: re-verify GitHub + PowerShell Gallery
  before first release, plus a real trademark screen.
- **WinAgent32** — the Windows distribution & experience layer: MSIX packaging, WinUI3 client,
  MXC sandboxing, VS toolchain awareness, tray supervisor, winget/Store delivery. Retained —
  descriptive beats pretentious for the platform layer — and it is also the WinUI3 client's name.
- The Windows-shaped-ness was never about being Windows-*only*. It is Windows-*first*:
  packaging, sandboxing, toolchain, and UI are natively Windows; portability falls out for
  free because the engine's mother tongue happens to be cross-platform.

**Derived identifiers** (everything hangs off the names above):

| What | Value |
|------|-------|
| Engine PowerShell module | `FactorAgent` |
| Built-in cmdlet noun prefix | `FA` (e.g. `Get-FASession`, `Test-FAToolScript`) — reserved; user tools may not use it |
| Session error-mode variable | `$FAErrorAction` |
| Engine service binary (Rust) | `factoragent` / `factoragent.exe` |
| Distribution CLI | `fa32` (renamed from `wa32` 2026-09-16) |
| Windows state dir | `%LOCALAPPDATA%\WinAgent32` (unchanged) |
| Session rendezvous pipe | `\\.\pipe\WinAgent32\<session-id>` (unchanged) |
| Session mutex | `Local\WinAgent32\session\<session-id>` |
| MSIX identity | `Name="WinAgent32"`, publisher CN set at signing (same cert as script signing, §4.9) |
| Portable engine state (non-Windows) | `$XDG_DATA_HOME/factoragent` (fallback `~/.local/share/factoragent`) |
| Portable engine config (non-Windows) | `$XDG_CONFIG_HOME/factoragent` (fallback `~/.config/factoragent`) |

## 2. Goals / non-goals

**Goals**
- An agent harness that feels poured into Windows, not scattered across it (classical
  application fashion: self-contained state and assets, clean install/uninstall).
- Small local models must succeed: tools with clear, humanistic, discoverable interfaces.
- Sessions a human operator can actually follow.
- Be kind to the LLM *server* (cache discipline), not just the LLM.
- Supervised by default; autonomy is opt-in, visible, and revocable.

**Non-goals (for v1)**
- The "claw" orchestrator — deferred; see §4.1.
- Enterprise identity planes (Entra/Intune/Agent 365) — MXC is scoped down; see §7.1.
- A private richer protocol for first-party frontends — ACP only; see §5.

## 3. Architecture overview

```
┌─────────────────────────────────────────────────────────────┐
│ WinAgent32 (distribution)                                    │
│  MSIX · winget/Store · tray supervisor · VS toolchain tools  │
│  MXC sandbox policy · %LOCALAPPDATA%\WinAgent32 state         │
│                                                              │
│  Frontends (all ACP clients, no private channels):            │
│    fa32 tui ──┐                                              │
│    WinUI3     ─┼──▶ FactorAgent engine (Rust service)         │
│    Zed        ─┘         │                                    │
│    VS Code (via ACP Client ext.)                             │
│                                                              │
│  FactorAgent engine:                                         │
│    ACP (stdio + named pipe) · per-session SQLite db        │
│    event bus (superset of ACP updates) · agent loop          │
│    tool module (PowerShell cmdlets) · pwsh session runners   │
│    MCP host · LLM backend trait (llama.cpp first-class)     │
└─────────────────────────────────────────────────────────────┘
```

## 4. The engine: FactorAgent

### 4.1 Session model — [DECIDED]

- The **session** is the first-class primitive: durable (SQLite), addressable,
  event-emitting, surviving process restarts.
- **1:1:1**: one agent process : one session : one DB file
  (`%LOCALAPPDATA%\WinAgent32\sessions\<id>.db`). The agent process is unaware of
  other sessions — no multiplexing inside it. Process trees are a fine workstation
  pattern; ten sessions under one process is a cloud habit, not a requirement.
- The agent process **is the mediator**: sole writer of its DB. Single ownership
  is enforced by a **named mutex** per session (`Local\WinAgent32\session\<session-id>`) —
  kernel-reaped on crash, so no stale locks, no PID-reuse hazards, no heartbeat timeouts.
  Many client processes
  may speak to the mediator (TUI, WinUI3, ACP adapters); exactly one writes.
- Rendezvous: per-session named pipe `\\.\pipe\WinAgent32\<session-id>`. First to
  bind + acquire the mutex owns the session; latecomers attach as clients.
  `fa32 --acp` (ACP over stdio for Zed/VS Code): wins the mutex → owner speaking
  ACP; loses → thin proxy over the owner's pipe. The tray supervisor keeps
  per-session processes alive; there is no multiplexed serve daemon.
- v1: TUI → one inner session, simple loop. The orchestrator ("claw") is **not**
  built now; later it is just a privileged session with `spawn_session` /
  `signal_session` tools — no new architecture. Orchestration = sessions managing
  sessions.
- Subagents are full 1:1:1 citizens: separate process, separate DB, which the
  parent may read but never owns or reaps. The parent log links the child session
  id. Data represents real work; deletion is the user's job, always.

### 4.2 Language split — [DECIDED]

- **Rust** for the core service + TUI (ratatui): the *supervisor*, not the implementer.
  Memory safety earns its keep at the trust boundary (ACP parsing, named pipe, session
  registry, sandbox enforcement) — single binary, tokio event bus. Rust does **not**
  implement tools: PowerShell wrappers in Rust are memory-safety theater. The threat
  model was never buffer overflows; it is agent misbehavior, mitigated by MXC +
  ConstrainedLanguage + approval — none of which a Rust wrapper improves.
  Principle: **small TCB**. Policy enforcement lives in the small Rust core;
  capabilities live in PowerShell.
- In short: a **rich PowerShell client**; a Rust harness that understands PowerShell
  well enough to *visualize* it (AST parsing for the TUI), and owns secrets,
  sandbox management, and secured API communication.
- **C# / WinUI3** for the later desktop client — Rust's WinUI bindings are poor;
  don't fight the platform.
- **PowerShell** for all tools (§4.3) and as the TUI's terminal core (pwsh is portable,
  so the TUI is portable *through* PowerShell rather than despite it).

### 4.3 Tools are PowerShell cmdlets — [DECIDED]

- Tools are **real cmdlets** in the `FactorAgent` module, designed for clarity
  (think `ls`/`dir`: simple surface, full depth underneath).
- The **module is the manifest**: function schemas for the model are generated by
  reflecting cmdlet metadata (names, parameters, help). Single source of truth —
  docs cannot drift from tools because they *are* the tools.
- The "simplified interface" of managed mode = the cmdlet's **default parameter set**.
  No separate alias layer; simplicity lives in cmdlet design.
- **Runtime discoverability**: a confused model runs `Get-Help <Cmdlet> -Examples`.
  Help text becomes descriptions, `-Examples` become few-shots. Discoverability is
  callable, not a docs page.
- **Real errors**: strict parameter binding fails loudly and helpfully — the best
  self-correction signal a small model can get. Beats a silently malformed patch diff.
- The Rust command runner is **dead simple**: it spawns a `pwsh` child and speaks a
  JSON envelope over stdio (`{id, command, mode, errorAction}` →
  `{id, streams, errors, whatIf}`); the runspace bootstrap (constrained language,
  module import, transcript) is itself PowerShell. No tool logic in Rust, no
  wrappers — the runner enforces *modes*, PowerShell provides *capabilities*.
- A significant share of this codebase will be PowerShell. That is the honest shape
  of a PowerShell-native agent, not a concession.

#### 4.3.1 Built-in tool inventory (v1) — [DECIDED]

**Core vocabulary** — full schemas inline in Block A (§8.3):
- Session & self: `Get-FASession` (id, mode, error mode, backend, manifest version,
  cwd — read-only; mode dials stay operator-owned).
- Files: `Read-FAFile` (numbered lines, `-Lines`/`-Tail`), `Find-FAFile` (glob),
  `Find-FAText` (content search with context), `Write-FAFile` (`-Append`;
  ShouldProcess), `Edit-FAFile` (exact `-OldText`/`-NewText` anchor, fails loudly
  on zero/ambiguous match; ShouldProcess), `New-FAItem` / `Remove-FAItem` /
  `Move-FAItem` / `Copy-FAItem` (ShouldProcess).
- Terminals: `New-FATerminal`, `Get-FATerminal`, `Invoke-FACommand`,
  `Remove-FATerminal` (§4.12).
- Toolchains & build: `Get-FAToolchain`, `New-FAToolchainEnv`, `Invoke-FABuild`
  (§4.11).
- Config: `Get-FAPreference`, `Get-FAProvider` (never secret values — Rust-only,
  §7.4).
- Catalog: `Find-FATool` (keyword search over the JIT index).
- Memory: `Get-FAEvent` (query the agent's own session log — sessions are
  self-describing from the inside).
- Toolshop (visible only in toolshop mode): `New-FATool`, `Test-FAToolScript`,
  `Submit-FATool` (draft → human approval; the agent drafts, never deploys).

**Long tail** — JIT index one-liners (§8.3): `Get-FARegistryValue`,
`Set-FARegistryValue`, `Get-FAService`, `Restart-FAService`, `Get-FAProcess`,
`Stop-FAProcess`, `Get-FAEnvironmentVariable`, `Set-FAEnvironmentVariable`,
`Get-FAScheduledTask`, `New-FAMsixPackage`, `Test-FALLMConnection`
(wraps `fa32 doctor --llm`), …

**Inventory principles:**
- The managed allowlist is `Microsoft.PowerShell.Core` only (composition with no
  side effects — pipelines, `Where-Object`, `Select-Object`); the `FactorAgent`
  module owns the full daily vocabulary, so manifest reflection, help-as-few-shots,
  and the FA trust brand cover what the agent actually uses.
- Built-ins dogfood the user-tool rules (§4.9): approved verbs, required help
  fields, lint-clean. If `Test-FAToolScript` would fail our own cmdlets, the format
  is theater.
- Index entries are one line each, always, carrying a `[builtin]`/`[user]` source
  tag so the model calibrates trust at a glance.
- There is deliberately no general-purpose `Invoke-FAExpression` in managed mode —
  the missing tool is what keeps the managed/raw line crisp. Raw terminals are the
  escape hatch.

**Agent's-eye view** (three zoom levels): index entry (cheap, always visible) →
full schema (core only, Block A: synopsis, reflected parameters, `.OUTPUTS` shape,
`.EXAMPLE` lines verbatim as few-shots) → `Get-Help <Cmdlet> -Examples` for the
deep dive. Chain approval (§4.5) renders per-stage WhatIf cards, with the terminal
named on raw stages.

### 4.4 Managed vs raw sessions — [DECIDED]

- **Managed session**: constrained runspace (Constrained Language Mode), only the
  `FactorAgent` module + small allowlist visible. Composition of blessed cmdlets.
  Every invocation flows through approval policy + audit transcript.
- **Raw ("shell") session**: full language mode, for terminal-like flows. Heavier
  approval, runs inside MXC.
- Crisp policy line: *composition of blessed cmdlets = managed; arbitrary
  scriptblocks / .NET interop / native commands = raw.*
- In raw sessions the agent may use the same cmdlets with full PowerShell syntax
  (pipes, `Get-Help`, `Trace-Command`) — including debugging its own tools.
- **Execution topology** — lifetimes match trust:
  - *Managed*: one persistent `pwsh` process, module loaded once; each invocation
    runs in a **fresh child scope**. Efficiency without state bleed — arena-style:
    the runspace lives long, scopes are freed per use. Constrained language bounds
    what state could accumulate anyway.
  - *Raw*: **persistent terminal environments** (§4.12), provisioned per session and
    managed by the agent like subagents. Warm, cultivated execution contexts — the
    Windows-native shape, not fork-per-command. Lifetimes still match trust: a
    terminal lives with the agent process (or until explicitly removed), and its
    containment is fixed by the session's isolation invariant (§7.1).
  - The *user's* interactive terminal pane is a separate thing: persistent,
    unsandboxed, their fingers, their trust. Not the agent's raw session.

### 4.5 Tool chaining & approval — [DECIDED]

- The pipeline `|` is the composition operator. One chained invocation replaces N
  model round-trips: token-efficient, and for small-context models it is a
  *capability* unlock (composition happens in the shell, not in working memory).
- **Approval is per chain (unit), not per stage.** A human thinks
  "find → patch → rebuild" as one intention; the policy sees what the human sees.
- Approval dialog shows **WhatIf expansion**: walk the AST, run each mutating
  cmdlet's `-WhatIf` preview, present the expanded chain with per-stage previews.
  (ShouldProcess is the platform's native approval idiom — `-WhatIf`/`-Confirm` come
  free with well-written cmdlets.)
- TUI parses the chain AST (`Parser::ParseInput`) into visual stages; failure
  highlights the failing stage via `$_.InvocationInfo`. Same renderer serves the
  audit log. Raw commands get the same collapsed-card → click-to-expand treatment,
  rewarding native shell usage.

### 4.6 Error modes — [DECIDED]

- Error handling is a **mode**, not a design constraint: a session-scoped
  `$FAErrorAction` dial, operator-chosen.
  - `StopAndReport` (default): first failure aborts the chain, real error surfaces.
  - `HealAndContinue` (opt-in): the agent may attempt self-healing.
- Principle: **supervised apprentice by default.** Defaulting to autonomy spooks
  users; spooked users don't trust the thing. The TUI shows the mode badge.
- All harness modes (error action, approval policy, sandbox backend, LLM backend)
  are **user preferences**, not code paths: config-driven, enforced by the Rust
  runner. Classical behavior — settings live in `%LOCALAPPDATA%\WinAgent32`, with a
  proper settings UI in WinUI3 later.

### 4.7 MCP — [DECIDED] (scoped down for v1)

- An MCP server is just **a subcommand launched by the agent session, or a URL**.
  Registered in the session (`session/new` already passes `mcpServers` through —
  hosting is native to the design). No notion of packages, provenance, or a
  registry for v1: too complex, deferred.
- Tool precedence: built-in `FactorAgent` cmdlets always win over MCP tools on
  name overlap (same anti-shadow rule as user tools, §4.9).
- [DECIDED] Whether to also ship the Windows tool layer as a standalone MCP server
  for other hosts (`fa32 mcp-serve`): **deferred to v2** (deferral confirmed 2026-09-16).
  Nothing architectural blocks it: the tools are scripts, manifest reflection is
  mechanical, MCP's stdio shape rhymes with 1:1:1. Not a v1 question; the subcommand
  name is reserved so v2 doesn't rename the CLI surface.

### 4.9 User-authored tools — [DECIDED]

- User tools are **just files**: one PowerShell script, one function, with
  comment-based help. That format choice is load-bearing — manifest reflection,
  `Get-Help` discoverability, and JIT indexing then work for user tools for free.
  (Format specifics — file/function naming, required help fields, lint rules —
  specified in §4.9.1–§4.9.4.)
- **Search path** (configuration, in this order): immutable built-in module path
  inside the install image → standard user data path
  (`%LOCALAPPDATA%\WinAgent32\tools\`) → repo/work dir (if plausible) →
  user-registered paths.
- **Built-ins always win.** A user script colliding with a built-in cmdlet name
  fails to load with a clear error — never silently. Shadowing a trusted tool
  with an untrusted file is exactly the attack this prevents; the precedence
  rule is a security property, not a convenience.
- **Native lint at load.** A PowerShell-based linter (written in PowerShell,
  e.g. a `Test-FAToolScript`-class cmdlet — dogfooding the module-is-manifest
  principle) runs at load time: format conformance, manifest extractability,
  basic safety hygiene. Lint failure = tool not registered, error stated plainly.
  Honest, never silent.
- **Provenance is code signing.** Built-in scripts are Authenticode-signed with
  our certificate at production time; the loader trusts our signature (verified
  via `Get-AuthenticodeSignature`). No package registry, no provenance
  infrastructure — this is PowerShell, and signing is the platform-native answer.
  User scripts are unsigned by definition and live under the always-lose
  precedence rule above.

#### 4.9.1 File & function naming — [DECIDED]

- One file, one tool: `<Verb>-<Noun>.ps1`. The file defines exactly one function at
  script scope, and the function name must equal the file's base name (case-insensitive).
  Helper functions must be nested inside the tool function, never at script scope.
- Verb: must be in PowerShell's approved-verbs list (`Get-Verb`).
- Noun: PascalCase. The `FA` noun prefix is **reserved** for built-in `FactorAgent`
  cmdlets; a user tool whose noun starts with `FA` fails to load. The noun must not
  collide with any built-in cmdlet, function, or alias name — collisions fail loudly at
  load (the built-ins-always-win rule, stated as a load error rather than a silent shadow).
- Example: `%LOCALAPPDATA%\WinAgent32\tools\Get-RepoStatus.ps1` defines
  `function Get-RepoStatus { [CmdletBinding()] param(...) ... }`.

#### 4.9.2 Required comment-based help — [DECIDED]

Every user tool carries comment-based help; the manifest generator reads it, so missing
fields are load errors, not style nits:
- `.SYNOPSIS` — one imperative line (becomes the tool description).
- `.DESCRIPTION` — what it does, why it exists, and its side effects stated plainly.
- `.PARAMETER <Name>` — one per parameter: valid values, defaults, what "empty" means.
- `.EXAMPLE` — at least one, copy-paste runnable (becomes the model's few-shot).
- `.OUTPUTS` — the object shape emitted (type names, key fields).
- `.NOTES` — optional (author, version, changelog pointer).
- Manifest mapping: parameters are reflected from the `param()` block — every parameter
  must be explicitly typed (`[string]`, `[int]`, `[switch]`, …); .NET type → JSON-schema
  type, `Mandatory` → required, `ValidateSet`/`ValidatePattern` → enum/pattern, defaults
  recorded. A tool edit starts a new manifest generation for the session (§8.3).

#### 4.9.3 Lint rules — [DECIDED]

`Test-FAToolScript` runs at load (harness-side, session budget — the agent never sees it
outside toolshop mode). **Errors** (tool not registered, stated plainly): file/function
name mismatch; more than one script-scope function; unapproved verb; `FA`-prefixed or
colliding noun; missing `[CmdletBinding()]`; untyped parameter; missing help field
(SYNOPSIS, DESCRIPTION, PARAMETER-per-parameter, ≥1 EXAMPLE, OUTPUTS);
`Invoke-Expression`/`iex`; `Add-Type`; `SupportsShouldProcess` declared but
`$PSCmdlet.ShouldProcess()` never called (AST check). **Warnings** (registered anyway,
surfaced in toolshop): mutating-command heuristic (`Set-`/`Remove-`/`New-`/`Move-`/`Copy-Item`,
registry/process/service writes, …) without `SupportsShouldProcess`; missing `.NOTES`;
`Write-Host` (prefer `Write-Verbose` / `Write-Output`); success output mixed with
progress text.

**Footgun (documented, not linted):** collection-returning tools must emit
`Write-Output -NoEnumerate ([T[]]$list)` so the JSON-RPC wire shape is stable
for 0/1/N results — but that same no-enumerate array, piped straight into
`Where-Object`/`ForEach-Object` in-process, arrives as *one object* (the
array), not N elements: `$_` is the whole array and `-like`/`-eq` act as
array filters in boolean context, so the filter silently passes everything
through. Assign first, then filter: `$all = Find-FAFile …; $hits = @($all |
Where-Object { … })`. Over the RPC wire this never bites (the bridge
serializes the return value); it only bites interactive/script composition.

#### 4.9.4 Scaffolder — [DECIDED]

Toolshop mode JIT-injects `New-FATool -Verb <Verb> -Noun <Noun>`, which generates the
file with `[CmdletBinding()] param()` stubs, help-field stubs, and a `ShouldProcess`
skeleton when the verb implies mutation — the format is produced, not memorized.

- **Toolshop mode** — [DECIDED]: authoring tools is a formal session mode, not a
  casual capability. An agent that writes its own tools is a self-modification
  path, and self-modification gets a supervised door.
  - Entry is **operator-only**: explicit, visible (TUI badge), revocable — same
    posture as `HealAndContinue`. The agent cannot elect itself into it.
  - Entering **JIT-injects the toolbox** (linter, scaffolder, format
    conventions) — the only context in which the agent spends budget on tooling.
    In normal mode the harness lints at load on the session's budget and the
    agent never sees any of it.
  - **Registration is gated**: drafts go to the user-tools path, but a tool
    becomes loadable only after lint passes *and* a human approves (the approval
    dialog shows the script). The agent can draft, never deploy.
  - Blast radius contained by this section's rules: user path always loses to
    built-ins; constrained language in managed sessions.

### 4.10 Configuration layout — [DECIDED]

- Principle: **the filesystem honestly represents the structure of config data.**
  Human-configurable state lives in discrete files, one per concern — no
  monolithic blobs humans must wade through and hand-edit (the `conf.d`
  pattern). Fully agent-controlled state (session DBs, internal caches) may be
  monolithic; no human needs to read it.
- Layout under `%LOCALAPPDATA%\WinAgent32\`:
  - `config/providers/<name>.toml` — one TOML file per LLM provider.
  - `config/preferences.toml` — scalar user preferences: default provider, error
    mode, approval policy, sandbox backend (§4.6), script dialect (§4.15).
    One small flat file is honest; a giant mixed blob is not.
  - `sessions/<id>.db`, `tools/`, caches — agent-controlled or user-data, not config.
- **Default backend: local-first.** `llama.cpp` at `http://127.0.0.1:8080` —
  zero-config, no key, private. Hosted providers (Meta Model API, …) are one
  `fa32 auth` away. Provider files reference Credential Manager credentials
  (`credential = "<name>"`), never plaintext secrets (§7.4) — discrete files must
  not become secret-plaintext files.
- The UI knows how to write these files: first-run setup is a wizard that probes
  (`fa32 doctor --llm`) and writes a provider file, and WinUI3 settings edits the
  same files the human could edit by hand. One format, two authors.

### 4.8 Session log data model — [DECIDED]

The session log is the durable heart: audit trail, resume state, and the model's
extended memory. SQLite, WAL mode, at `%LOCALAPPDATA%\WinAgent32\sessions.db`.

**Tables (v1):**
- `sessions(id TEXT PK, created_at, updated_at, title, frontend, mode, error_mode,
  backend, manifest_version, status, cwd, meta JSON)` — `manifest_version` pinned
  per session (§8.3).
- `events(seq INTEGER PK AUTOINCREMENT, session_id FK, ts, type TEXT,
  v INTEGER DEFAULT 1, payload JSON)` — append-only. `seq` is the ordering source
  of truth (clocks lie); `ts` is for display. Never UPDATE or DELETE; corrections
  are new events — the log is an audit artifact, not a mutable store.
- `blobs(hash TEXT PK, size, content BLOB)` — content-addressed (SHA-256),
  compressed. Prompts reference blobs by hash; Block A is byte-identical per
  session, so the frozen prefix dedupes to a single blob across every turn — the
  cache discipline reflected in storage.
- `migrations(version PK, applied_at, checksum)` — journal;
  `PRAGMA user_version` is the fast path.
- `transactions(txn_id TEXT PK, session_id FK, seq_started, writer_token,
  kind TEXT, state TEXT, updated_seq)` — the **mutable** projection over the
  immutable log. States: `started` → `observed-ok` | `observed-failed` | `unknown`.
  Every state transition is itself appended as an event; the table is the current
  truth, the log is the law.

**Write discipline — intent before action:**
- The **logical transaction** (a tool run with real side effects on real files and
  OS state) is the subject — not the DB transaction. The DB append is atomic;
  reality is not.
- `txn.started` (writer token + full intent) is durably appended **before** the
  tool process spawns or the script runs. A kill in the gap leaves
  intent-without-outcome — never outcome-without-intent. We are certain about our
  uncertainty; the worst case (the world changed with no record at all) is
  structurally excluded.
- Takeover: whoever holds the mutex is by construction the only writer, so a new
  owner marks the dead token's non-terminal transactions `unknown` — no
  timeout-based death detection, no inference. The agent or operator then verifies
  the world and asserts the formal states: `observed-ok` (agent said it worked) /
  `observed-failed` (agent said it failed), each assertion itself a log entry.
- The engine never auto-heals. It is stateful, not smart: it records attempts and
  observations honestly, and says "unknown" when it doesn't know.

**Event taxonomy (v1)** — `type` → payload:
- lifecycle: `session.started` / `resumed` / `archived`, `turn.started` / `completed`
- conversation: `user.message`, `agent.message` (Block C derives from these)
- model: `prompt.built` (blob refs + block versions + hash — prompts stay
  inspectable per §7.3), `llm.request` / `response` (backend, model, usage
  including `cached_tokens`, latency_ms — feeds the TUI cache display)
- tools: `chain.planned` (AST stage list — what the TUI visualizes),
  `tool.call` (cmdlet, structured args, mode, chain_id/stage_idx, approval ref),
  `tool.result` (streams, fully expanded error records, exit code, duration_ms,
  WhatIf payload when previewed)
- teaching: `jit.injected` (cmdlet, schema version) — the log records what the
  model was taught and when; sessions are self-describing
- transactions: `txn.started` (intent, ahead of action), `txn.observed` (ok/failed
  assertion by agent/operator), `txn.unknown` (takeover marking)
- policy: `approval.requested` / `resolved` (WhatIf expansion, decision, by whom),
  `sandbox.decision` (backend used, policy evaluation), `mode.changed`

**Versioning — two levels:**
1. *Schema*: `PRAGMA user_version` + ordered migration chain, applied
   transactionally at startup and journaled. Old binaries refuse
   forward-incompatible versions loudly — never silently misread.
2. *Payloads*: every event carries `v` per `type`. Readers dispatch on
   (type, v) with graceful fallback: unknown versions render as a generic card,
   never crash.
- Contract: **record richly, render defensively.** v1 records AST stage
  boundaries, expanded error records, and WhatIf payloads even where the v1 TUI
  renders them simply — the fields the v2 renderer will want are already in the
  log. Visual refinements later become renderer upgrades, not data migrations.

### 4.11 Toolchain environments — [DECIDED]

- A toolchain **is a ps1 env file, adopted as-is** (buy, don't build): it defines
  what the toolchain means to the tools themselves. The harness applies the file;
  it does not interpret variables.
- Definitions carry comment-based help headers; `Get-FAToolchain` reflects them —
  the module-is-manifest principle extends to environments.
- Toolchains attach to **terminals as decorators at creation**
  (`New-FATerminal -Toolchain vs2022`), applied once. No ambient mutable toolchain
  state, no enter/exit verbs.
- Vendors shipping only cmd/bat/sh setup scripts: `New-FAToolchainEnv -From <setup>`
  captures via a throwaway child (run the vendor script, diff the environment,
  emit bog-standard `$env:` assignments). Deterministic capture, not translation;
  the output is checked in as data.
- v1 ships `vs2022`, `rust-stable`, and the pwsh module toolchain. The mechanism is
  general (CMake, Intel, Python, …) — new toolchains are data, never engine changes.
- `Invoke-FABuild` is a convenience over terminals: the build system is
  auto-detected from directory files (`.sln` → MSBuild, `Cargo.toml` → cargo,
  `CMakeLists.txt` → cmake, `pyproject.toml` → python) and mapped to a toolchain,
  with explicit `-Toolchain` override. It ensures or reuses a terminal with that
  toolchain, sends the build, and returns **structured diagnostics** regardless of
  backend.

### 4.12 Raw terminals — [DECIDED]

Raw execution happens in **persistent terminal environments**, not fork-per-command
processes. Windows process creation + pwsh startup + env application make the Unix
fork model punitive; warm, cultivated execution contexts are the Windows-native shape.

- Lifecycle cmdlets: `New-FATerminal -Name <n> -Toolchain <t> -WorkingDirectory <d>`,
  `Get-FATerminal`, `Invoke-FACommand -Terminal <id> -Command <script>`,
  `Remove-FATerminal`. The agent manages terminals the way it manages subagents:
  provision, read the herd, cull.
- **No `-Backend` in the agent's vocabulary.** Isolation strategy is a
  framework-owned session invariant (§7.1, §7.5): set at session spawn from operator
  policy + machine capability, immutable for the session. `Get-FATerminal` shows the
  backend read-only. The agent chooses the *what* (toolchain); the framework chooses
  the *where*.
- **Default terminal:** the first raw command issued without explicit terminal
  management lazily provisions one; it lives with the agent process and is reused
  thereafter. Default toolchain comes from user scope; backend from the session
  invariant. Small models get warmth with zero lifecycle thinking.
- **Limit, no reaping:** max terminals per session is configured (preferences). No
  timeout-based reaping — at the limit, creation fails and tells the agent to remove
  one first. Lifecycle is explicit, consistent with the no-timeouts principle (§6).
- **Approval** stays per invocation (chain), with the terminal named in the dialog
  (`run … in terminal 'build' (vs2022)`). Approval is about the *action*;
  containment is already guaranteed by the invariant. Terminal *lifecycle*
  (New/Remove-FATerminal) is deliberately NOT approval-gated: a terminal is a
  managed, empty room until a command runs in it, spawning is bounded by
  FA_MAX_TERMINALS (default 8, session-configurable), and there is no security
  signal in a human approving "a third terminal" — gating it would be theater.
- **Tradeoff, recorded honestly:** the fresh-process structural guarantee against
  cross-command covert channels is traded away. Mitigation is visibility + audit:
  terminals are named, listed, transcripted, operator-visible — first-class like
  subagents. Managed sessions keep the cheap structural guarantee (fresh child scope
  per invocation, in-process).

### 4.13 Scopes — [DECIDED]

User scope, project scope, session scope — one mechanism that files memory and gives
decorators a home, so nothing is repeated memetically.

- **User scope:** durable, operator-owned. The existing
  `%LOCALAPPDATA%\WinAgent32\config\` (preferences, providers).
  "You don't like it when I do X."
- **Project scope:** repo-bound. `.factoragent/project.toml` at the repo root (found
  by walking up from cwd). Declares the project's toolchains, extra tool paths, and a
  `[conventions]` free-text section ("we always write code using Y pattern"). v1 keeps
  it to toolchains + conventions.
- **Session scope:** ephemeral, in the session DB. "Stop looking at Z, look at Q for
  now" — set by the operator mid-session (steering) or by the agent as working notes.
- **Precedence:** explicit per-call arguments > session > project > user. The harness
  composes one effective view; the agent never sees the seams.
- **Scopes feed Block B** (§8.3): scope-derived facts are harness-injected, versioned,
  cache-friendly. A scope change is an explicit Block B reprocessing event.
- **Trust:** scopes carry *context*; *policy* flows only from user scope and the
  framework. Project scope is repo-controlled: read and apply it freely, but **writes
  to user scope originating from project context require default approval** (on unless
  the operator relaxes it) — the anti-contamination guard (§7.5). Agent edits to the
  project-scope file itself go through normal chain approval like any file mutation.
- **Memory:** scopes are the filing system — user scope is durable memory, project
  scope is repo-convention memory, session scope is working memory.

### 4.14 Print forms — [DECIDED] (2026-09-18)

Two audiences, two renderings. The model gets the incantation: raw
PowerShell (`Read-FAFile -Path crates/factoragent/tests/m1_bridge.rs`),
precise and unambiguous — the golem reads exactly what is written. The
human gets a generic action view: `Read crates/factoragent/tests/m1_bridge.rs`.
Both derive from the
same invocation; neither is a paraphrase of the other.

- The print form lives with the tool: a `# .PRINTFORM:` comment line in the
  tool's own `.ps1`, right after the comment-based help block — built-in and
  user tools alike — extracted by the manifest generator like every other
  help field (§4.9.2). (It can't be a help keyword inside the `<# #>` block:
  `Get-Help` rejects blocks containing unknown keywords.) One file, one
  function, one print form; the §4.9.1 format rule stands.
- Template syntax is deliberately dumb: `{ParamName}` substitution from the
  bound parameters, nothing else. No conditionals, no expressions. (Client
  may truncate long substitutions; outcome glyphs ✓/✗ compose client-side.)
- The engine expands the template when emitting events: `event.tool_call`
  items and approval chain items carry both the incantation (`name`+`args`)
  and the action view (`print`). The model never sees print forms; clients
  render `print`, falling back to the raw invocation for tools without one.
  One expansion point means every client shows the same action view.
  Approval prompts, the Chronicle, and audit views render the print form;
  debug/diagnostic surfaces may show the raw invocation.
- Missing `.PRINTFORM`: lint warns, not a load error; the client falls back
  to the raw command string. Honest degradation, never a blank.
- **Terse, never chatty.** Print forms are not sentences — no "I will now…",
  no plain-English filler. The human's attention is scarce the way the
  model's tokens are scarce: dense, scannable, unambiguous. Same principle,
  two encodings.
- **Unambiguous.** Brevity comes from form, never from dropping identity.
  `{Path}` renders the full repo-relative path as passed
  (`crates/factoragent/tests/m1_bridge.rs`), never a bare basename that
  could collide with another room's file.
- **Width is the client's problem.** Terminals have character limits; the
  client may left-truncate with `…` for display (`…/tests/m1_bridge.rs`),
  but the full form is always one step away — and always complete in the
  Judgment Gate, where the veto must be informed.
- Examples: `Read-FAFile` → `Read {Path}` · `Write-FAFile` → `Write {Path}`
  · `Invoke-FACommand` → `Run {Command}` · `Find-FAFile` →
  `Find {Pattern} in {Path}` · `New-FATerminal` → `Open terminal {Name}`.

### 4.15 Script dialects — [DECIDED] (2026-09-18)

A true PowerShell dialect, not a rendering trick. The operator selects a
vernacular; it becomes a prompt section at the top of Block A — immediately
after the tool protocol, before the manifest — so it frames how the model
reads the manifest's full-fat examples rather than whispering after them
(§8.3). The agent writes real scripts in the dialect. The bytes the agent writes are the bytes the
operator vets at the approval gate and the bytes the host executes — one
representation, end to end. Contrast §4.14: print forms deliberately render
an action view distinct from the incantation (two audiences, two
renderings); a dialect has no translation layer anywhere, so the veto is
always WYSIWYG.

Four dialects:

| Dialect | Register | Example |
|---|---|---|
| `full` (default) | PowerShell, full cmdlet names | `Get-ChildItem` |
| `brief` | PowerShell short aliases | `gci` |
| `posix` | posix-style aliases, still PowerShell | `ls` |
| `windows` | cmd-style aliases, still PowerShell | `dir` |

Rules, enforced by the prompt section's closed alias table (the model never
invents aliases; unknown aliases fail loudly at the host and the model
self-corrects):

- **Tools are never aliased.** The dialect governs freeform script text only
  (`Invoke-FACommand -Command`, terminal input). `call Read-FAFile {...}`
  in every dialect; the tool vocabulary stays canonical.
- **Aliases rename cmdlets, never parameters.** `ls -Recurse` is right;
  `ls -la` is WRONG — posix flags do not exist in PowerShell, and the
  prompt section names this footgun explicitly.
- **Ephemeral vs. durable.** The dialect is for commands that *run*, not
  files that *persist*. `Write-FAFile` content keeps full cmdlet names:
  saved scripts must not depend on aliases (PSScriptAnalyzer flags them
  for the same reason).
- **Default `full`.** Full-fat is clearest for the model, and our validity
  data (M4: 100% first-try for the 4B class) is all full-fat. Dialects are
  opt-in until measured. Token savings are real but secondary — scripts are
  a thin slice of context; the prize is the operator vetting commands in
  the dialect they already think in.

- Alias tables are authoritative for the **Windows host** (the product
  platform). PowerShell on Unix strips the aliases that collide with native
  commands (`ls`, `cat`, `rm`, … resolve to the native binaries there), so
  the posix table only fully holds where the agent host is Windows.
- Selection: `--dialect` flag (`factoragent serve`, passed through by
  `fa32 run`) > `dialect` key in `config/preferences.toml` (§4.10) >
  `full`. A bad flag value fails fast; a bad file value warns and falls
  back — a cosmetic preference never bricks session start. Writes to
  `preferences.toml` are hand-edit (or wizard-owned, §4.10) for now.
- The dialect is baked into the session's Block B at spawn; joining a live
  session with `--dialect` warns and is ignored.

## 5. Wire protocol: ACP everywhere — [DECIDED]

- **ACP is the only wire protocol.** The TUI gets no private richer channel —
  that is what keeps the WinUI3 client (and Zed, and VS Code) first-class.
- Internal event bus (tokio::broadcast) is a **superset** of ACP session updates:
  richer Windows-native events for our frontends, translated down to ACP for
  external hosts.
- Zed: native external-agent support. VS Code: via the community ACP Client
  extension — zero VS Code extension code of our own for v1.

## 6. Event model — [DECIDED]

- The mediator accepts inputs from client processes, serializes them by arrival
  (**first writer wins** — no consensus, no vector clocks; simultaneous user
  injections are the user's business), appends to the log, and broadcasts state.
  Data is Law (immutable ordered log); State is Truth (the broadcast projection).
- **Bus vs. buried**: the bus carries *state transitions*, not bulk data and not
  executor internals. Bulk payloads travel by reference (blob hash); the bus
  carries pointers + lifecycle. Each event type declares its durability — the
  session-log writer is a filtered consumer. Ephemeral shimmer (streaming chunks)
  rides the bus but never touches the log.
- Every event carries `session_id` and a `caused_by` correlation link — under
  interleaving, timestamps can't pair requests with resolutions.
- **Approval**: requests broadcast to all attached frontends; the mediator takes
  the first resolution and broadcasts it. No election, no attending-frontend —
  first writer wins, like everything else.
- **Blocking over deadlines**: no timeouts on the log, the work queue, IPC, or
  approval waits. A UAC prompt open for hours leaves the txn `pending`, honestly.
  Liveness comes from kernel object lifetime (mutex), never from timers.
- Consumers: TUI renderer, audit renderer, ACP projection (explicit per-type
  project/summarize/drop table — also a redaction boundary), log writer, metrics,
  `fa32 events --follow` debugging. Slow consumers resync by `seq` cursor from
  SQLite — the log is the backstop that makes a lossy broadcast bus viable.
- Future orchestrator: privileged sessions with read subscriptions to other
  sessions' buses (parent→child read is already the subagent pattern). ACLs for
  cross-session subscription ride with that work, not now.

### 6.1 The world-map mental model — [DECIDED] (2026-09-18)

Philosophical grounding: **System Realism**. Most agent harnesses are
text-realist: the transcript is the reality, and tool calls are side effects
of text generation — the world exists only insofar as it is described in the
chat. FactorAgent is system-realist: the world (repo, processes, session
state) is the reality, and the model and operator are subjects acting within
it. Text — messages, the Chronicle — is the record, not the territory. The
event log is the ground truth of what happened *in the world*; every
renderer is a projection of it. The inversion, stated plainly: in text
realism the world is a side effect of the conversation; in system realism
the conversation is a side effect of the world.

The session is a game world — not a terminal session, and not a chat. The
model is the player: it reads, writes, executes, and navigates. The operator
is not the terminal's typist; the operator watches, judges (the Judgment
Gate), and intervenes. The event stream is the game's event log; clients are
cameras on the world, not shells.

This reframes the renderer work: the text and Fluent views are two cameras on
one simulation, and the glyph/sigil work was tileset design. A future
world-view client renders the codebase as the world and tool calls as marks
upon it — every action happens *somewhere*, and the world remembers.

Engine obligations (some already true, some future work):

- Every tool-call event carries an explicit `room`: the directory scope the
  call acted in, derived from its paths. Renderers must not infer location
  from argument strings. (Future work — the wire carries raw paths today.)
- Rooms have stable identity (canonical repo-relative paths) so clients can
  accumulate per-room history: reads, writes, executions, failures as marks
  on the room.
- Events already carry actor (`caused_by`) and outcome; the log's total order
  is the shared source of truth every camera reads.
- Product-language rule stands: game/world/maze/dungeon vocabulary lives in
  design docs and internal metaphor, not in shipped client text, where the
  operator sees the catalogue (drawers, INDEX cards, narrowing calls).

Open: the operator's role in game terms (spectator? DM?) — the true client's
central design question.

**Roles.** The world is real and stateful. Within it, the operator is the
mage and the model is the summoned golem. The mage holds intent and
authority but acts indirectly — through incantations (prompts), names
(precise specifications), and the binding circle (the sandbox: it holds or
it doesn't). Incantations work by exact wording, spoken precisely; a sloppy
incantation is how golems go wrong in the stories — which is why prompt
craft here is binding-craft. The golem is tireless, powerful, and perfectly literal; it does
exactly what it is told, which is its power and its characteristic failure
mode. So the harness is written for a literalist: protocols a golem can't
misread, and the Judgment Gate set where the mage's veto belongs — before
effects, not after. (In the old story the golem is animated by *emet*,
truth, set upon its forehead. Ours too.)

**Framing/code firewall.** The game/world/mage/golem/incantation vocabulary
is a design model for the humans building this system. It does not enter
code identifiers, cmdlet verbs/nouns, wire field names, event types, or
shipped product text. The code stays boring; the myth stays in this
section.

**Context contract** — what "enough context" means, per event, so the deeper
visualization stays possible without a wire change later:

- actor — have (`caused_by`)
- action — have (tool name + arguments)
- room — NEED: explicit directory scope the call acted in (future work)
- target — have (the item addressed, identified rather than buried in args)
- outcome — have (tool result: success/failure + compact summary)
- order — have (`seq`; the log's total order is the shared timeline)

The world graph itself is reconstructed from survey/INDEX events already in
the log — no new event type yet. A normal UI remains fine; this contract
only guarantees the deeper view is never priced out of the wire.

**Views.** The world is one; cameras differ. The Game View renders the
realized map — rooms, glyphs, the session's history written onto the world.
The textual view renders the collapsed form: the traversal as a single
trail-line with the actor's mark traveling it — expressive and dynamic
(the motion through the files is visible), but neither arcane (no concepts
the operator doesn't already have) nor expansive (no cumulative object
dumps). Each camera shows what its medium can carry; neither is the
territory.

### 6.2 The Textual Realist camera — [LOCKED] (2026-09-18)

fa32 is the textual camera: a rich line-oriented renderer over the event
stream, written in Rust. The engine stays presentation-free. Realized first
as `tui-flair/textual-realist.html` (web-first proof); the fa32 renderer
follows it.

- **Chronicle scroll.** Every tool call/result renders as its print form
  (§4.14): `⚙ Tree crates`, `✓ Tree crates (215ms)`. Token metadata whispers
  underneath in dim.
- **Detail rule.** Below the print form, a detail line carries the
  verifiable remainder — exact tool name, full canonical paths, execution
  context (`Write-FAFile · 252 bytes → V:\Sources\factor\list-by-size.ps1`).
  It must add information, never pure-restate. For `Run {Command}` the
  print form already embeds the command, so the detail carries context
  (`terminal 'default'`) and the gate's raw line carries the verbatim bytes.
  This is the WYSIWYG veto: the operator always sees exactly what executes.
- **The gate is the one box that earns its border.** The approval
  interruption renders as a box-drawn frame with the title on the border —
  the frame marks the veto boundary, which is its meaning. Action prominent;
  raw incantation as verifiable detail. Box-drawing appears nowhere else
  decoratively.
- **No branding in the real TUI.** Sections are structured by print forms,
  not labels. (Runic kickers were tried in the prototype and cut for
  clarity.)
- **Color schemes, true-color.** ember (default), frost, parchment, ghost.
  fa32 emits 24-bit ANSI (`38;2;r;g;b`); Windows Terminal and Ghostty carry
  it 1:1. ghost paints no background — translucent panels float on the
  operator's own terminal theme (keeps acrylic). Honest degradation:
  `NO_COLOR`, non-TTY (plain greppable lines), reduced-motion (static final
  frames, no in-place animation).
- **The trail, live.** The collapsed trail line with the traveling mark
  (`● winagent32 › crates › factoragent › tests`) renders from generic
  traversal-progress events — not per-tool renderer lore. In-place mark
  movement when interactive; one settled line when piped. Progress-event
  design is the remaining engine slice; the renderer contract is: a moving
  mark over stable room identity, no visited-object dumps. (Status
  2026-09-18: the wire carries explicit `room` + `root` on live
  `event.tool_call`/`event.tool_result`; fa-core derives the room from
  generic path-shape rules — existing file → parent, directory → itself,
  nonexistent → nearest ancestor, command text and globs ignored.)
- **Operator binding.** `fa32 run --scheme ember|frost|parchment|ghost`
  `--color auto|always|never`. A misspelled scheme fails fast, before any
  spawn. `NO_COLOR`, `--color never`, non-TTY, or `TERM=dumb` degrade to
  plain greppable lines — the ASCII gate keeps the veto readable without
  box art. `--color always` forces ANSI for capture. fa32 never paints a
  background in any scheme.

#### 6.2.1 Full terminal takeover — [DECIDED] (2026-09-18)

Styled interactive sessions own the whole canvas; the takeover is
transactional and the fallback is honest line mode.

- **Alternate-screen ownership.** `Screen::enter()` sets raw/console modes
  *first*, then takes the alternate canvas. If mode setup fails the
  terminal is untouched; if canvas takeover fails the mode guard is
  dropped explicitly, restoring everything. The operator is never stranded
  in the alternate screen with a hidden cursor. `Drop` restores console
  modes, cursor, and the main screen even on panic — the operator's
  scrollback is preserved.
- **Fixed regions.** Row 0: top bar (session left, model/scheme right —
  the session side truncates first, the bar is hard-clipped to the width
  and never wraps). Rows 1..n-3: the Chronicle viewport (semantic runs,
  width-sensitive wrap cache, Page Up/Down scroll, 5,000-line bound).
  Row n-2: status — collapsed trail, working spinner, token readout —
  hard-clipped to one row. Row n-1: the managed input row.
- **Input horizontal viewport.** The input row never wraps into its
  neighbors: a cursor-following window shows the slice around the cursor,
  so massive pastes stay on one row and remain inspectable via cursor
  movement. History (Up/Down), Home/End, word jumps, and Ctrl+C
  (clear-then-quit) / Ctrl+D (quit) behave.
- **Line-mode fallback.** `--message`, piped stdin, `NO_COLOR`,
  `TERM=dumb`, or `--color never` stay in line mode: plain greppable
  output, no takeover. EOF on piped stdin exits the REPL cleanly.
- **The gate as modal interruption.** In takeover mode the approval gate
  is a modal over the Chronicle: chain print forms prominent, previews as
  verifiable detail, `[a]pprove · [d]eny · [e]dit args` footer. Unfinished
  input is stashed while the modal is up and restored after the verdict.
  The height-capped rows (and any spill file) are prepared once per
  terminal size when the modal opens — not per keystroke — so an
  overflowing gate writes exactly one spill file however long the
  deliberation.
- **Scale policy.** Wrapping handles horizontal scale; the viewport
  handles vertical scale; spilling preserves completeness. No pager: a
  pager would seize terminal ownership, complicate piped and approval
  input, and add cross-platform lifecycle risk. The gate always shows the
  complete executable text — width wraps it, height spills it to a file,
  nothing is silently truncated. Newline/tab/CR hardening applies to every
  rendering (styled modal, plain gate, spill) — one hardened path.
- **Resize.** Width/height are re-read on every render; reflow happens on
  the next key or server event (no explicit resize event).

## 7. Security model

### 7.1 MXC sandboxing — [DECIDED]

- Use **MXC (Microsoft Execution Containers)** scoped down: hypervisor-isolated or
  Windows-native containers + a JSON policy. No enterprise bells (Agent 365,
  Entra/Intune) for v1.
- Shape: the agent's *hands* live in MXC, the *brain* on the host. Read-only recon
  on host; mutations in sandbox; destructive ops need approval + policy exception.
- **Graceful degradation**: MXC/WSB needs Pro/Enterprise + virtualization. On Home,
  fall back to approval-gated host execution. Only the tool executor knows the
  difference.
- PowerShell's Constrained Language Mode is the inner sandbox primitive for managed
  sessions (belt and suspenders with MXC outside it).
- **Isolation ladder** — syntactic vs. real boundaries, each rung crisp:
  1. *Child scope* (PowerShell): syntactic isolation, zero trust. Fresh scope per
     managed invocation.
  2. *Terminal process* (OS): the first real trust boundary. Persistent per-terminal
     processes (§4.12) — not per-command; the Rust supervisor spawns, constrains,
     and reaps. The agent cultivates terminals but never chooses their containment:
     isolation strategy is a framework-owned session invariant (below).
  3. *MXC container*: hypervisor/Windows-native isolation around raw execution.
  4. *VM* (future): just another rung — not a redesign.
- Design consequence: the Rust runner's execution backend is a **trait from day
  one** (`HostProcess`, `MxcContainer`, later `Vm`). New isolation rungs arrive as
  new backend implementations; the orchestrator never restructures. "Only the tool
  executor knows the difference" already covers graceful degradation — it will
  cover VMs the same way.
- **Boundary placement:** the containment boundary sits at **FactorAgent, not
  WinAgent32**. The captive is always the PowerShell subprocess, so the engine draws
  the boundary around its children. WinAgent32 supplies the package identity (which
  anchors MXC policy) and ships the policy JSON; enforcement lives in the engine's
  execution-backend trait. The same terminal API therefore works on non-Windows
  FactorAgent with a host backend — portability falls out.

### 7.2 Approval UX — [DECIDED]

- Mutating cmdlets implement `SupportsShouldProcess` → `-WhatIf` previews and
  `-Confirm` gating are native idioms the model already knows.
- Chain approval as a unit with WhatIf expansion (§4.5). ACP's `request_permission`
  callback is the wire mechanism; it is resumable.

### 7.3 Supervised-by-default — [DECIDED]

- Product posture: supervised apprentice, not autonomous agent. Autonomy
  (heal mode, yolo flags) is explicit, visible, per-session opt-in.
- Prompts are inspectable audit artifacts, viewable in the TUI. No black boxes.

### 7.4 Secrets handling — [DECIDED]

- API keys live in **Windows Credential Manager** (DPAPI-backed OS facility), never
  as plaintext in config files, state dirs, or logs. Config holds *references*
  (`credential: <name>`), not values — named credentials, so hosted endpoints
  (Meta Model API, etc.) and local backends (llama.cpp, usually keyless) coexist.
  No keys sitting next to the "school papers" folder.
- The Rust supervisor is the **only** component that touches secrets: it reads from
  Credential Manager and attaches auth headers to LLM API calls itself.
- The pwsh JSON envelope carries **no secret fields** — PowerShell runspaces never
  receive raw secrets. This closes prompt-injection-to-exfiltration *by
  construction*: the model can talk the shell into anything, but there is simply no
  key there to steal.
- Audit transcripts scrub secret-shaped values before persistence.
- `fa32 auth set-key|remove|status` — secure masked prompt; `status` never reveals values.

### 7.5 Trust model: one boundary, one guard — [DECIDED]

- The **isolation invariant is the only hard security boundary** (framework-owned,
  Rust-enforced, §7.1). It either holds or it doesn't.
- Inside the boundary, environment, preferences, toolchains, and conventions are
  **semantic state, not security perimeters**. The model already generates and runs
  code; treating env vars as a security boundary at that point is theater — a
  poisoned `$env:PATH` is no worse than a poisoned code suggestion.
- The **directional guard**: writes to user scope originating from project context
  require **default approval** (on unless the operator relaxes it, per
  supervised-by-default, §7.3). This protects persistence-of-self against
  cross-project contamination — not code safety.
- No approval theater for applying environments, switching session focus, or reading
  project scope.
- Multi-user confidentiality is out of scope for v1 (single-user workstation); it
  would arrive as a new isolation rung (§7.1), not as env hygiene.

## 8. LLM backends & cache discipline

### 8.1 Backend trait — [DECIDED]

- Provider-agnostic, OpenAI-protocol. Clean `messages[]` with proper roles; let the
  server apply the chat template. No hand-rolled special tokens, no proprietary
  features.

### 8.2 llama.cpp as first-class — [DECIDED]

- Talk to `llama-server`'s `/v1/chat/completions`; set `cache_prompt: true`.
- `response_format: json_schema` (compiled to GBNF, decode-time enforcement) with
  schema validation + plain-text fallback — grammar failures can fail *open*.
- Respect `--np` slot count via a concurrency setting; reuse connections.
- Surface `usage.prompt_tokens_details.cached_tokens` in the TUI — make the cache
  hit rate visible.
- Ship `fa32 doctor --llm` (health, ctx size, cache support) + a doc page of
  recommended flags (host-memory cache, `--kv-unified`, `--slot-prompt-similarity`).

### 8.3 Prompt construction — [DECIDED]

- **Static-first, append-only.** Order blocks by stability; a change at position N
  only reprocesses from N onward under prefix caching:
  1. Block A — system prompt + script dialect (§4.15) + tool manifest.
     Byte-identical per session. Frozen. The dialect section sits between the
     tool protocol and the manifest: the model imitates what it sees most, so
     the dialect must speak first — a rule at the end of Block B loses to four
     thousand tokens of full-fat manifest examples.
  2. Block B — session facts (cwd, mode) **plus harness-composed scope state**
     (user/project/session, §4.13). Rarely changes; a scope change is an explicit
     Block B reprocessing event.
  3. Block C — conversation. Strictly append-only. Never edit the middle.
  4. Suffix — ephemeral nudges at the END. **Never prepend** (the Claude Code sin).
- **Determinism rules**: sorted cmdlets/params/JSON keys; no timestamps, UUIDs, or
  PIDs in the prefix region (run IDs go in suffix/tool results); manifest version
  pinned at session creation — mid-session module updates start a new cache
  generation explicitly.
- [DECIDED] **Manifest dial: hybrid + JIT.** Core cmdlets inline (full schemas);
  the long tail indexed (tiny stable index). **JIT schema injection**: the harness
  appends a cmdlet's full schema as a suffix note on first use or on
  parameter-binding failure — append-only, cache-friendly. Rationale (economic): a
  static per-tool context cost restricts vocabulary by economics and strangles
  growth; JIT makes the marginal tool ~free, which is what makes user-authored
  tools viable — a new cmdlet lands in the index and the model learns it on first
  touch, zero prompt surgery.

### 8.4 Compaction — [DECIDED]

- Compact rarely and explicitly; summary goes in a fixed slot right after Block A.
  One bounded reprocessing event; Block A's KV stays hot across it. No silent
  middle-truncation.

### 8.5 Platform TLS — [DECIDED]

- `reqwest` with `default-features = false`, features `json` + `native-tls`.
  **Never rustls.** On Windows this is SChannel against the platform trust store,
  so corporate proxies and enterprise CAs just work; let Microsoft replace
  SChannel at its own pace. Locked in `Cargo.toml`.

## 9. Windows distribution: WinAgent32

### 9.1 Packaging — [DECIDED]

- **MSIX** (winget now, Store later). Self-contained: state + assets, clean
  install/uninstall. Package identity doubles as the anchor for MXC policy and
  named-pipe ACLs.
- The install image is immutable; user tools and state live in runtime user-data
  paths (§4.9). No conflict between the two.
- [DECIDED] MSIX capability declarations and named-pipe ACL specifics are **deferred**
  (deferral confirmed 2026-09-16 — no conflicts foreseen: the immutable image + runtime
  user-data search paths mean nothing structural depends on them). Intended shape when
  declared: `runFullTrust` (the engine spawns `pwsh` and MXC containers — a full-trust
  profile), pipe DACL granting the package SID + current user on
  `\\.\pipe\WinAgent32\*`, session mutexes in the `Local\` namespace so they need no
  ACL work at all.
- No true Windows Service under MSIX → **tray supervisor + StartupTask**.
  (Honest framing: it is a *user* agent, not a system daemon.)
- `fa32` on PATH. State in `%LOCALAPPDATA%\WinAgent32`. No dotfiles scattered
  like a Unix app.

### 9.2 Frontends — [DECIDED]

- TUI: ACP client, ratatui, portable by construction; terminal core is PowerShell.
- WinUI3 (C#): later; talks to the same named pipe. The v1 proof point (§10) is
  built *by* the agent.
- The TUI renders a **shell session**, not tool-call JSON cards: collapsed command
  cards → expandable stages/output. Audit log = same renderer.

### 9.3 VS toolchain awareness — [DECIDED]

- Delivered through the general toolchain-environment mechanism (§4.11), not
  VS-specific cmdlets: `vswhere` discovery produces a `vs2022` toolchain ps1;
  builds run in terminals decorated with it (§4.12) and return **structured
  diagnostics**. `devenv` and Windows SDK awareness ride the same mechanism.
  The agent natively knows how to build Windows software. This is the moat no
  Ubuntu-born CLI will ever dig.

## 10. Proof point / v1 definition of done — [DECIDED]

**v1 is done when FactorAgent builds its own WinUI3 client using its own
toolchain environments and build tools.** Dogfood as definition of done.

## 11. Open questions

None. All eight threads from the 2026-09-16 design session are resolved:

1. Naming → §1 ([DECIDED]: **FactorAgent** engine, **WinAgent32** distribution + WinUI client).
2. Standalone MCP server → §4.7 ([DECIDED]: deferred to v2, `fa32 mcp-serve` reserved).
3. User tool file format → §4.9.1–§4.9.4 ([DECIDED]).
4. MSIX capabilities / pipe ACLs → §9.1 ([DECIDED]: deferred, intended shape recorded).
5. Built-in tool inventory → §4.3.1 ([DECIDED]: core vocabulary + JIT tail, Core-only
   allowlist, built-ins dogfood user-tool rules, source-tagged index, no
   `Invoke-FAExpression` in managed).
6. Toolchain environments → §4.11 ([DECIDED]: buy-the-ps1, terminal decorators,
   `New-FAToolchainEnv` capture skill for bat/sh vendors, v1 ships
   vs2022/rust-stable/pwsh).
7. Raw terminal lifecycle → §4.12 ([DECIDED]: persistent LLM-managed terminals
   replace fork-per-command, lazy default terminal, per-session limit, no silent
   reaping, approval names the terminal).
8. Scopes, isolation invariant, trust model → §4.13, §7.1, §7.5 ([DECIDED]:
   user/project/session scopes feeding Block B; isolation strategy is a
   framework-owned session invariant (`-Backend` not in agent vocabulary); one hard
   boundary + project→user default-approval guard; env/prefs are semantic state).

## 12. Decision log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-09-16 | Split FactorAgent / WinAgent32 | Engine is portable via PowerShell; Windows-ness lives in distribution |
| 2026-09-16 | Rust core + TUI, C# for WinUI3 | Memory safety where privileged; first-class WinUI bindings |
| 2026-09-16 | ACP as only wire protocol | One protocol; every frontend (TUI, WinUI, Zed, VS Code) equal |
| 2026-09-16 | Sessions as the primitive; claw deferred | Orchestration falls out of sessions managing sessions |
| 2026-09-16 | Tools as real PowerShell cmdlets; module = manifest | Discoverability, real errors, no docs drift; small-model ergonomics |
| 2026-09-16 | Managed vs raw session split | Constrained runspace vs full language in MXC; enforceable policy line |
| 2026-09-16 | Chain approval as unit + WhatIf expansion | Humans think in chains; ShouldProcess is the native idiom |
| 2026-09-16 | Error modes; supervised default | `$FAErrorAction` dial; autonomy opt-in, never default |
| 2026-09-16 | MSIX over MSI | Store/winget; package identity as policy/ACL anchor |
| 2026-09-16 | MXC scoped to containers + JSON policy | Skip enterprise plane for v1; graceful degradation on Home |
| 2026-09-16 | Static-first append-only prompts; llama.cpp first-class | Prefix-cache discipline; `cache_prompt`, template hygiene |
| 2026-09-16 | Hybrid + JIT manifest | Core inline, tail indexed, JIT injection on use/failure; marginal tool cost ~zero |
| 2026-09-16 | Small TCB: no Rust tool wrappers | Rust = supervisor at the trust boundary; capabilities = PowerShell; wrappers are memory-safety theater |
| 2026-09-16 | Harness modes are user preferences | Error action, approval, sandbox, backend = config-driven, enforced by the runner |
| 2026-09-16 | Secrets in Credential Manager; Rust-only secret access | DPAPI-backed OS facility, config holds references; pwsh envelope carries no secrets (exfiltration closed by construction); transcripts scrubbed |
| 2026-09-16 | Shell execution topology | Persistent managed runspace, fresh child scope per invocation; per-command raw shells (+fresh MXC); separate user terminal — lifetimes match trust |
| 2026-09-16 | Isolation ladder; execution backend as trait | Scope (syntactic) → process → MXC → VM (future); VMs arrive as a new backend impl, no restructure |
| 2026-09-16 | Session log data model + two-level versioning | SQLite/WAL, append-only events with per-type payload versions, content-addressed blobs; record richly, render defensively |
| 2026-09-16 | 1:1:1 — process per session; multiplexed serve retired | Workstation-normal: no multiplex daemon; tray supervisor keeps per-session processes alive |
| 2026-09-16 | Mediator = agent process; named-mutex single ownership | Many speakers, one writer; kernel-reaped mutex, no timeouts; --acp loser becomes proxy |
| 2026-09-16 | Write-ahead intent; txn states; no auto-heal | Intent logged before action → certain about our uncertainty; takeover derives unknown; observed-ok/failed asserted by agent/operator; engine honest, not smart |
| 2026-09-16 | Blocking over deadlines; first-writer-wins | No timeouts on log/queue/IPC/approval; mediator serializes by arrival |
| 2026-09-16 | Subagent DBs never reaped | Real work stays; user deletes; parent links child id, reads child DB |
| 2026-09-16 | MCP scoped down: subcommand or URL, session-registered | No packages/provenance for v1; built-ins win on name overlap |
| 2026-09-16 | User tools = files; built-ins always win; lint at load | Search path is config; collisions fail loudly; native PowerShell linter at load |
| 2026-09-16 | MSIX capability/ACL details deferred | No conflict foreseen: immutable image + runtime user-data search paths |
| 2026-09-16 | Config: discrete files (conf.d pattern); local-first default | `config/providers/<name>.toml` + `preferences.toml`; UI-writable; secrets stay in Credential Manager; agent-controlled state may be monolithic |
| 2026-09-16 | MCP-serve deferred to v2; provenance = Authenticode | Scripts mean no box-in; production signing with our cert, loader trusts our signature |
| 2026-09-16 | Engine named **FactorAgent**; WinAgent32 kept for distribution + WinUI client | Decided after bike-shedding: collision peek eliminated HermesAgent (active NousResearch/hermes-agent), LegatusAgent (yevkavaliou/legatus-agent), and ActaAgent (acta-dotnet/acta in adjacent durable-jobs space); NuntiusAgent clean but thinner; FactorAgent has no shipped product under the name (code-level class names only, no Gallery module). Note: an earlier premature commit was reverted same-day before the real decision. Derived identifiers in §1; pre-publish recheck + trademark screen still required |
| 2026-09-16 | Standalone MCP server deferral confirmed | v2, unblocked; `fa32 mcp-serve` subcommand name reserved |
| 2026-09-16 | User-tool format fully specified | One file = one function, name match, approved verb, FA prefix reserved, required help fields, Test-FAToolScript error/warning contract, New-FATool scaffolder (§4.9.1–§4.9.4) |
| 2026-09-16 | MSIX capability / pipe-ACL deferral confirmed | No conflicts foreseen; intended shape (runFullTrust, package-SID pipe DACL, Local\ mutexes) recorded in §9.1 |
| 2026-09-16 | Built-in tool inventory (v1) locked | Core vocabulary (Block A) + JIT tail; allowlist = Core only; built-ins dogfood user-tool rules; one-line source-tagged index entries; no Invoke-FAExpression in managed (§4.3.1) |
| 2026-09-16 | Toolchain environments: buy-the-ps1 | Toolchain = ps1 env file applied as-is; New-FAToolchainEnv capture skill for bat/sh vendors; toolchains are terminal decorators, never ambient state (§4.11) |
| 2026-09-17 | Session transport: named pipe on Windows, Unix socket on Unix | `fa-core` RPC framing is transport-agnostic; platform halves are type-erased (`Pin<Box<dyn>>`). Pipe naming locked to `\\.\pipe\WinAgent32\<session-id>` (sanitized). Session lock: `flock` on Unix, exclusive-share (`share_mode(0)`) file handle on Windows — both kernel-reaped. Bridge tree-kill: `killpg` on Unix, `taskkill /T /F` on Windows as the prototype spelling; v1 hardening is a Job Object owned by the bridge |
| 2026-09-16 | Raw terminals replace fork-per-command | Persistent LLM-managed terminals (New/Get/Invoke/Remove-FATerminal); lazy default terminal living with the agent process; per-session limit, no silent reaping; approval names the terminal; covert-channel tradeoff recorded (§4.12) |
| 2026-09-16 | Scopes: user / project / session | `.factoragent/project.toml`; precedence args > session > project > user; scopes feed Block B; scopes are the memory filing system (§4.13) |
| 2026-09-16 | Isolation strategy = framework-owned session invariant | Set at spawn from operator policy + machine capability, immutable; -Backend not in agent vocabulary; agent chooses what (toolchain), framework chooses where (§7.1, §4.12) |
| 2026-09-16 | Trust model: one boundary, one guard | Isolation invariant is the only hard boundary; env/prefs/toolchains are semantic state (anti-theater); project→user writes need default approval; multi-user out of scope for v1 (§7.5) |
| 2026-09-18 | File tools confined to the session workspace | The host passes its cwd as `FA_SESSION_ROOT`; a module-private `Assert-SessionPath` (never exported, never in the manifest, unreachable over RPC) resolves every file-cmdlet `-Path` and throws outside the root — fail-closed when unset. The "workspace" the model sees in Block B is now a boundary, not a suggestion. `Invoke-FACommand`/terminals remain the known open sandbox problem (§7.1) |
| 2026-09-18 | No unbounded tool returns: page + summarize, never blast | After a live run serialized ~9k repo paths (~200k tokens) verbatim into the next model request, the rule is: every collection/large output gets junk pruning, a default page cap, stateless paging, and a truncation note with the true total plus a shape summary (top subtrees / hardest-hit files) so the agent navigates instead of guessing. `Find-FAFile`/`Find-FAText` gained `-Skip`; `Read-FAFile` gained `-Offset` and caps bare reads at 2000 lines; `Invoke-FACommand` keeps the last 2000 output lines. Paging is stateless because page-walking accumulates every page in history — the summary makes narrowing the cheap path, paging the deliberate one |
| 2026-09-18 | No canonical hidden-file concept; inherit .gitignore via git itself | Dotfiles are usually the *target* for a coding agent (`.env`, `.github/`, `.vscode/`) — pruning them by default trades noise for the worse failure mode of files that exist but the tool denies. Instead, `Find-FAFile`/`Find-FAText` filter through `git check-ignore --stdin` (batched, module-private `Get-GitIgnoredPaths`): the repo's own noise list, with real gitignore semantics instead of a reimplemented matcher, best-effort and silent outside a work tree. Default-on; the hidden count is always reported (`+N hidden by .gitignore`) so nothing vanishes silently, and `-IncludeIgnored` bypasses. Known limit: gitignore is a relevance post-filter, not traversal pruning — a huge ignored dir outside the junk list is still walked (add it via `-Exclude`) |
| 2026-09-16 | Containment boundary at FactorAgent | Engine draws the boundary around its pwsh captive; WinAgent32 supplies identity + policy JSON (§7.1) |
| 2026-09-16 | Distribution CLI renamed `wa32` → `fa32` | Bare `FactorAgent` rejected: collides with the `factoragent` service binary; `fa32` keeps the 32-branding and the engine/distribution split |
| 2026-09-17 | Tool-call protocol: fenced text, not structured JSON | Model emits fenced `fa` code blocks; each non-empty line is one call: `call <Verb>-FA<Noun> {json-args}`. Prose outside fences is ignored; a malformed line never panics the parser — it becomes a warning fed back to the model for self-correction. Tool names validated for shape in the parser; the bridge `*-FA*` allowlist is the real enforcement point (§4.5, M2) |
| 2026-09-17 | Collection cmdlets always return arrays on the wire | PowerShell unwraps single-element output to a scalar, so Find-FAText/Find-FAFile/Read-FAFile/Get-FATerminal emitted object-or-array depending on hit count. They now emit via `Write-Output -NoEnumerate`, making the JSON-RPC result shape count-independent. Deterministic wire shapes are a protocol property, not a convenience (§4.5, M1) |
| 2026-09-17 | WhatIf previews bypass the engine's ShouldProcess message | The console host writes the "What if:" line straight to stdout, bypassing every PowerShell stream (`6>$null` and `$InformationPreference` cannot catch it) — it corrupted the one-JSON-line framing. For `_WhatIf` calls the bridge now sets a module-scope `$script:FAForceWhatIf` flag and invokes WITHOUT `-WhatIf`; cmdlets short-circuit to their Preview object before `ShouldProcess`. Human `-WhatIf`/`-Confirm` behavior is unchanged (§4.5, M1) |
| 2026-09-17 | StopAndReport and denial surface as real errors | `AgentLoop::run_prompt` returned `Ok` with a paraphrase for tool failures and operator denials. Per §4.6 ("first failure aborts the chain, real error surfaces") it now returns `Err(FaError::ToolFailed{..})` / `Err(FaError::Denied)`; the frontend reports, the operator decides (§4.6, M2) |
| 2026-09-17 | RPC read loop dispatches every message independently | `read_loop` awaited `on_request` inline, so `session.prompt` (which blocks on the client's `event.approval_resolved` notification) deadlocked: the notification could never be read. Each request/notification is now spawned as its own task; response writes stay serialized via the peer writer mutex and responses are matched by id (M2/M3) |
| 2026-09-17 | Block A teaches JSON call shapes, not just JSON args | Qwen3-4B wrote PowerShell syntax (`-Path foo`) for tool args — twice, without self-correcting — because Block A was saturated with PowerShell (cmdlet names, `-Param` help, PS `.EXAMPLE` snippets) and showed JSON only in the preamble. Every tool now renders a synthesized `Call as: call <Name> {"Req": "..."}` line (required params only) under its heading: 10 JSON shapes to imitate. Plus an explicit anti-example in the preamble and a targeted parser warning when args look like `-Param value` syntax. After the fix: valid JSON on the first try (M3) |
| 2026-09-17 | Common parameters excluded from the reflected manifest | The manifest reflected the PS common parameter `ProgressAction` (it was missing from the `$skip` list — "a newer common parameter"), typed as a free string; the model filled it with a prose description ("Creating terminal 'default'") and parameter binding failed. `ActionPreference` is harness territory, not agent vocabulary: `ProgressAction` added to `$skip`, completing the 14-parameter common set |
| 2026-09-17 | 4B-class models drive the protocol once taught; fidelity is loose | Empirical M3 result: Qwen3-4B-Q4_K_M follows the fenced-JSON protocol reliably after the Block A fix, but merges multi-step instructions (two terminal commands became one `$env:` assignment) and drops content details. Protocol compliance ≠ instruction fidelity — the M4 battery must measure task fidelity separately, not just call validity |
| 2026-09-17 | M4 battery declared PARTIAL; v1 model story gated on re-run | 14/18 cells infra-failed, not model-failed: qwen3-4b t5/t6 died with LLM `Connection refused`, all 12 smollm2-1.7b and qwen3-1.7b cells died with session-socket spawn failure (zero model turns each). 4/4 measured cells (qwen3-4b t1–t4) PASS, 100% first-try validity, approvals engaged. Local-first stays the working hypothesis for the 4B class only; no sub-4B support claim in v1 until the harness flakiness is fixed and the 14 cells re-run clean (§4.5, M4) |
| 2026-09-17 | M4 re-run declared PARTIAL (12/18 real); 1.7B floor named: clears protocol, fails strategy | qwen3-4b 6/6 PASS incl. t5 error-recovery via heal-and-continue (tool error absorbed) — 4B class clears the full bar, local-first working hypothesis holds for 4B. qwen3-1.7b: 96% first-try validity but 3/6 ground-truth completion; failures are wrong-target/wrong-tool answers, not syntax — protocol compliance ≠ task completion. v1 model story: 4B-class and up; no 1.7B support claim. smollm2-1.7b t1–t6 NO DATA again (SUN_LEN infra, below) (PROTOTYPE.md App. A, M4) |
| 2026-09-17 | Hermetic session dirs exceed Unix socket SUN_LEN — session paths must stay under the socket budget | The 238ec10 repair's hermetic session dirs pushed `session.sock` past 108 bytes (`.../state/smollm2-1.7b/sessions/m4-smollm2-1.7b-t6/session.sock` ≈110 chars), killing all 6 smollm2-1.7b re-run cells before spawn. Fix before a third M4 attempt: shorten/hash session dir names or anchor session state under a short TMPDIR — the socket path is a hard 108-byte budget, not a soft one (§4.5, M4) |
| 2026-09-18 | Named-pipe server/client tightened to the MSDN pattern (fixed ERROR_PIPE_BUSY on session start) | First live Windows run died with `All pipe instances are busy (231)`: `serve()` raced each `accept()` against a 200 ms timer in `select!`, dropping the in-flight accept — on Windows the accept future *owns* the pipe instance, so the listener was deleted most of the time; then `fa32`'s `listening()` probe consumed the surviving instance and the real `connect()` landed on a connected-but-dead instance. Fixes: (1) `bind` creates the first instance eagerly with `FILE_FLAG_FIRST_PIPE_INSTANCE` (fails fast on squat); (2) `accept` installs the replacement instance *before* handing the connected one out (MSDN multithreaded-server ordering); (3) `serve` pins one accept future per iteration — the 200 ms tick only re-polls `should_stop()`, never cancels the accept; (4) `connect` retries `ERROR_PIPE_BUSY` with 50 ms backoff up to 10 s (the `WaitNamedPipe` pattern; tokio exposes no `WaitNamedPipe`). Unix side untouched (M5) |
| 2026-09-18 | Collection tools rebuilt on the catalogue contract; display layer split out | The per-tool special cases (junk lists, paging params, note dialects) were converging on the same knowledge in N copies. New shape: tools are *collectors* (one shared pruned traversal `Get-FileCandidate`, one gitignore filter) and a single module-private *display layer* (`Format-ResultView`) owns the whole rendering grammar. Small result sets return items; large ones return an INDEX — directory/file cards with counts, samples, and call numbers (`-Path`/`-FilePattern` spellings that open each drawer) — instead of page 1 of the ocean. New public tool `Get-FATree`: the subject catalogue (dirs with recursive counts, bounded depth/nodes). Navigation is always a new lookup, never a page turn; `-Skip` is the raw-paging escape hatch, and explicitly passing it (even 0) opts out of the catalogue. The frame: the file tools are a text adventure for the model — the wire carries the maze, not the database (§4.3.1, 11 tools) |
| 2026-09-18 | Relevance filters fail open; boundaries fail closed | `Get-GitIgnoredPaths` collapsed to one git spawn per call by reading `check-ignore`'s own exit codes (128 = not a work tree → skip silently); any git failure returns "nothing ignored" rather than erroring, because a relevance filter must never invent failures. Contrast `Assert-SessionPath`, which throws — boundaries fail closed, filters fail open. The junk-dir list is documented as traversal-performance (never descend), not relevance; relevance is gitignore's job, and it lives in exactly one place |
