# M4 battery on Windows

This runs the same battery as `run.sh`, natively on Windows: `llama-server.exe`
(CUDA build) serves the models, `fa32.exe` drives the sessions, and the
fa32↔factoragent channel uses the **named pipe** `\\.\pipe\WinAgent32\<session-id>`
instead of a Unix socket. Tasks, prompts, fixtures, and LLM parameters are
identical to the Linux battery, so the numbers are directly comparable —
the GPU just makes them arrive faster.

Running this on Windows is also the first live validation of the
named-pipe transport.

## Prerequisites

- PowerShell 7: `winget install Microsoft.PowerShell`
- Rust: install from [rustup.rs](https://rustup.rs) (`rustup-init.exe`), then
  restart the shell so `cargo` is on PATH
- Python 3: `winget install Python.Python.3.12` (for `check.py` / `score.py`)
- git

## 1. Clone and build

```powershell
git clone https://github.com/alextnewman/factor.git
cd factor\winagent32          # the repo root IS winagent32
cargo build --bins
```

This produces `target\debug\fa32.exe`, `target\debug\factoragent.exe`, and
`target\debug\m4score.exe`.

## 2. llama.cpp (CUDA)

1. Go to [llama.cpp releases](https://github.com/ggml-org/llama.cpp/releases).
   (If `releases/latest` shows a `vX.Y.Z` tag with no binaries, open its notes
   for the `Nightly build: bNNNNN` line and download from that `bNNNNN` tag.)
2. Download **both**
   - `llama-b<N>-bin-win-cuda-<X.Y>-x64.zip` (the binaries), and
   - `cudart-llama-bin-win-cuda-<X.Y>-x64.zip` (CUDA runtime DLLs),
   
   where `<X.Y>` is **not higher** than the CUDA version `nvidia-smi` reports.
3. Extract **both** zips into `.tools\llamacpp\` inside the repo, so that
   `.tools\llamacpp\llama-server.exe` exists alongside its DLLs.

## 3. Models

Put the three GGUF files in `.tools\models\`, fetched **only** from the
publisher's official HuggingFace repos:

| file | repo |
|------|------|
| `Qwen3-4B-Q4_K_M.gguf` | `Qwen/Qwen3-4B-GGUF` |
| `Qwen3-1.7B-Q8_0.gguf` | `Qwen/Qwen3-1.7B-GGUF` |
| `smollm2-1.7b-instruct-q4_k_m.gguf` | `HuggingFaceTB/SmolLM2-1.7B-Instruct-GGUF` |

```powershell
pip install huggingface_hub
huggingface-cli download Qwen/Qwen3-4B-GGUF Qwen3-4B-Q4_K_M.gguf --local-dir .tools\models
huggingface-cli download Qwen/Qwen3-1.7B-GGUF Qwen3-1.7B-Q8_0.gguf --local-dir .tools\models
huggingface-cli download HuggingFaceTB/SmolLM2-1.7B-Instruct-GGUF smollm2-1.7b-instruct-q4_k_m.gguf --local-dir .tools\models
```

(Browser download works too — the filenames must match `models.tsv` exactly.)

## 4. Run

```powershell
.\scripts\m4-battery\Run-M4Battery.ps1
```

One model only, or a task subset:

```powershell
.\scripts\m4-battery\Run-M4Battery.ps1 -Models qwen3-4b
$env:M4_TASKS = "t1 t2"; .\scripts\m4-battery\Run-M4Battery.ps1
```

Each model gets full GPU offload (`--n-gpu-layers 999`); override with
`-GpuLayers`. Per-cell timeout is 30 minutes (`-TaskTimeoutSec`).

## 5. Score

```powershell
python .\scripts\m4-battery\score.py
```

Per-cell artifacts (fa32 logs, session DBs, score JSON, ground-truth checks)
land in `.state\m4-battery\`, same layout as the Linux run.

## Troubleshooting

- `missing ... fa32.exe` — run `cargo build --bins` from the repo root first.
- Cell fails immediately with a connection error — read
  `.state\m4-battery\server-<tag>.log`; the usual cause is a CUDA-version
  mismatch between the two zips and the installed driver.
- A runaway cell: the script kills `fa32` after the timeout and moves on;
  stale state is cleaned per cell, so rerunning is safe.
