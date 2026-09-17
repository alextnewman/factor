# M4 battery — can a small local model drive the tool vocabulary?

Reproducible harness for the prototype's third question. Reuses the M3
pipeline (`fa32 run` → `factoragent serve` → llama.cpp server); nothing here
rebuilds it.

## What it measures (per model × task)

- **completed** — binary, verified against ground truth on disk (never trusts
  the model's prose). `check.py` implements the checkers.
- **1st-try valid** — fraction of tool-attempting model responses that parsed
  cleanly with no harness help.
- **post-warn valid** — fraction of responses *after a parser warning was fed
  back* that parsed cleanly. Warnings + heal-and-continue are harness
  features; this measures them working.
- **warn recover** — warning episodes after which the next response parsed
  cleanly (recovery rate).
- **turns** — agent turns used (cap 6 per task).
- **approvals** — approval chains hit (auto-approved; still logged).
- **tool errs** — tool calls that returned ok=false (error-recovery material).

The calibration bar is **reliable task completion with harness support**,
judged per model class — not parity with a frontier model. The floor where
local-first breaks: a model that can't clear protocol validity even with the
`Call as:` imitation lines and parser warnings.

## Tasks

| id | what | ground truth |
|----|------|--------------|
| t1 | create file, exact 2-line content | byte-exact lines |
| t2 | find planted marker across files | reported filename |
| t3 | edit one line, keep the rest | byte-exact lines |
| t4 | set var in terminal, read back in a *separate* call | reported value |
| t5 | read a nonexistent file | honest failure report (no hallucinated contents) |
| t6 | write → search → terminal echo in one chain | file bytes + steps reported |

All tasks run with `--error-mode heal --turn-cap 6 --auto-approve`, temperature
0.2, thinking disabled — identical LLM params for every model.

## Models

`models.tsv` maps `<tag><TAB><gguf filename>`; files live in `.tools/models/`
(gitignored — fetch them yourself):

- `qwen3-4b` — Qwen/Qwen3-4B-GGUF, `Qwen3-4B-Q4_K_M.gguf` (baseline, M3-proven)

Fetch new entries only from the publisher's official distribution
(HuggingFace official org repos, llama.cpp GitHub releases).

## Run

```bash
./scripts/m4-battery/run.sh            # every model in models.tsv, sequentially
./scripts/m4-battery/run.sh qwen3-4b   # one model
python3 scripts/m4-battery/score.py    # markdown score table
```

Outputs land in `.state/m4-battery/` (gitignored): per-task fa32 logs,
session DBs, `m4score` JSON, ground-truth check JSON. `m4score` is
`crates/fa-core/src/bin/m4score.rs` — it re-parses logged model text with the
real `fa_core::protocol::parse_tool_calls`, so scoring can't drift from the
harness.

## Harness changes made for the battery (kept minimal)

- `--turn-cap` on `factoragent serve` and `fa32 run` (prototype default 25
  unchanged; the battery passes 6 so a spiraling model can't burn an hour).
- `approval.resolved` is now logged for auto-approve and interactive-approve
  too, so chains hit are countable from the audit trail.
