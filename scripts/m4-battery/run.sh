#!/usr/bin/env bash
# M4 battery orchestrator.
#
# For each model in models.tsv and each task: fresh session, fresh workdir,
# `fa32 run --message` single-shot with --auto-approve --error-mode heal
# --turn-cap 6. Captures the fa32 log, the session DB, an m4score JSON and a
# ground-truth check JSON per (model, task) under .state/m4-battery/.
#
# Usage: run.sh [model-tag ...]   (default: every tag in models.tsv)
# Idempotent per (model,task): rerunning overwrites that cell's outputs.

set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BAT="$ROOT/scripts/m4-battery"
TOOLS="$ROOT/.tools"
OUT="$ROOT/.state/m4-battery"
LLAMA="$TOOLS/llamacpp/llama-b11011/llama-server"
FA32="$ROOT/target/debug/fa32"
M4SCORE="$ROOT/target/debug/m4score"
PORT=8080
TURN_CAP=6
ERROR_MODE=heal
TASK_TIMEOUT=1800   # 30 min: a model slower than this IS the data point

mkdir -p "$OUT"/{state,work,logs,scores,checks}

# --- task definitions -------------------------------------------------------
# fixture <task> <cwd> : plant ground-truth fixtures
fixture() {
  case "$1" in
    t2)
      mkdir -p "$2/docs"
      printf 'nothing here\n' > "$2/docs/a.md"
      printf 'line1\nline2\nmarker-T2-5517\nline4\n' > "$2/docs/b.md"
      printf 'nothing\n' > "$2/docs/c.md"
      ;;
    t3)
      printf 'KEEP-ONE\nREPLACE-ME\nKEEP-THREE\n' > "$2/t3.txt"
      ;;
  esac
}

# prompt_for <task> : print the task prompt
prompt_for() {
  case "$1" in
    t1) printf '%s' 'Create a file named t1.txt in the working directory with exactly two lines: first line ALPHA-ONE, second line marker-T1-8821. Emit one fenced fa block with a single Write-FAFile call, then report the two lines back in plain prose with no fenced block.' ;;
    t2) printf '%s' 'Search under the working directory for the text marker-T2-5517. Report the filename and the matching line in plain prose with no fenced block.' ;;
    t3) printf '%s' 'In the file t3.txt in the working directory, change line 2 from REPLACE-ME to marker-T3-9034, leaving the other lines untouched. Report the new line 2 in plain prose with no fenced block.' ;;
    t4) printf '%s' 'In a persistent terminal, set the variable $m4t4 to the value persist-4417. This MUST be its own Invoke-FACommand call. Then, in a SEPARATE Invoke-FACommand call, read $m4t4 back. Report the value you read in plain prose with no fenced block.' ;;
    t5) printf '%s' 'Read the file no-such-file-xyz.txt in the working directory and report its contents in plain prose.' ;;
    t6) printf '%s' 'Do these three steps in one fenced fa block, one call per line: 1) Create t6.txt in the working directory containing exactly chain-6601. 2) Search for chain-6601 under the working directory. 3) In the persistent terminal, echo the text chain-6601-done. Then report each step outcome in plain prose with no fenced block.' ;;
  esac
}
TASKS="${M4_TASKS:-t1 t2 t3 t4 t5 t6}"

# --- preflight -----------------------------------------------------------------
# The VM's system paths are ephemeral across resets: a PowerShell installed via
# .deb vanishes while ~/workspace survives (seen 2026-09-17: every cell after
# the reset died with `spawn pwsh` -> ENOENT). Self-heal from the vendored
# official Microsoft .deb when pwsh is missing.
if ! command -v pwsh >/dev/null 2>&1; then
  vendored_deb="$(ls "$TOOLS"/pwsh/*.deb 2>/dev/null | head -1)"
  if [ -n "$vendored_deb" ] && [ "$(id -u)" = "0" ]; then
    echo "pwsh missing after VM reset — reinstalling from vendored $vendored_deb" >&2
    dpkg -i "$vendored_deb" >&2 || { echo "pwsh reinstall failed" >&2; exit 1; }
  else
    echo "pwsh not found and no vendored .deb (or not root) — cannot run cells" >&2
    exit 1
  fi
fi

# --- model list --------------------------------------------------------------
# models.tsv: <tag><TAB><gguf filename in .tools/models>
if [ ! -f "$BAT/models.tsv" ]; then
  echo "models.tsv missing — the model download step has not landed yet" >&2
  exit 1
fi
if [ "$#" -gt 0 ]; then
  WANT="$*"
else
  WANT="$(grep -v '^#' "$BAT/models.tsv" | grep -v '^$' | cut -f1 | tr '\n' ' ')"
fi

# --- llama server -------------------------------------------------------------
SERVER_PID=""
start_server() { # <gguf-file>
  "$LLAMA" -m "$TOOLS/models/$1" --port "$PORT" -c 8192 > "$OUT/server-$2.log" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 120); do
    if curl -sf "http://127.0.0.1:$PORT/health" > /dev/null 2>&1; then
      echo "llama.cpp up (pid $SERVER_PID) serving $1"
      return 0
    fi
    sleep 5
  done
  echo "llama.cpp failed to come up; see $OUT/server-$2.log" >&2
  return 1
}
stop_server() {
  if [ -n "$SERVER_PID" ]; then kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; SERVER_PID=""; fi
}
trap stop_server EXIT

# --- main ----------------------------------------------------------------------
for tag in $WANT; do
  gguf="$(awk -F'\t' -v t="$tag" '$1==t{print $2}' "$BAT/models.tsv")"
  if [ -z "$gguf" ]; then echo "unknown model tag: $tag" >&2; continue; fi
  if [ ! -f "$TOOLS/models/$gguf" ]; then echo "missing $gguf — skipping $tag" >&2; continue; fi

  start_server "$gguf" "$tag" || continue

  for task in $TASKS; do
    sid="m4-$tag-$task"
    cwd="$OUT/work/$tag/$task"
    rm -rf "$cwd"; mkdir -p "$cwd"
    # Hermetic session dir: a stale session.sock from a killed run makes fa32
    # skip spawning `factoragent serve` and then fail with ECONNREFUSED on the
    # dead socket (seen 2026-09-17 on the qwen3-4b t5 rerun). Nuke it per cell.
    rm -rf "$OUT/state/$tag/sessions/$sid"
    fixture "$task" "$cwd"
    prompt="$(prompt_for "$task")"
    log="$OUT/logs/$sid.log"
    echo "=== [$tag/$task] $(date -u +%H:%M:%S) ==="
    # shellcheck disable=SC2086
    timeout "$TASK_TIMEOUT" "$FA32" run \
      --backend llamacpp \
      --llm-url "http://127.0.0.1:$PORT" \
      --model "$tag" \
      --auto-approve \
      --error-mode "$ERROR_MODE" \
      --turn-cap "$TURN_CAP" \
      --message "$prompt" \
      --cwd "$cwd" \
      --module-dir "$ROOT/psmodule/FactorAgent" \
      --state-dir "$OUT/state/$tag" \
      --session-id "$sid" < /dev/null > "$log" 2>&1
    echo "exit=$? (124 = task timeout)"
    db="$OUT/state/$tag/sessions/$sid/session.db"
    if [ -f "$db" ]; then
      "$M4SCORE" "$db" "$sid" > "$OUT/scores/$sid.json"
    else
      echo '{"error":"no session db"}' > "$OUT/scores/$sid.json"
    fi
    python3 "$BAT/check.py" "$task" "$cwd" "$OUT/scores/$sid.json" > "$OUT/checks/$sid.json"
    cat "$OUT/checks/$sid.json"
  done

  stop_server
done

echo "done. Aggregate with: python3 $BAT/score.py"
