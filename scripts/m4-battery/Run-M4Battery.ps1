<#
.SYNOPSIS
    M4 battery orchestrator — Windows/PowerShell port of run.sh.
.DESCRIPTION
    For each model in models.tsv and each task: fresh session, fresh workdir,
    `fa32 run --message` single-shot with --auto-approve --error-mode heal
    --turn-cap 6. Captures the fa32 log, the session DB, an m4score JSON and a
    ground-truth check JSON per (model, task) under .state/m4-battery/.

    Tasks, prompts, fixtures, and LLM parameters are identical to run.sh;
    only the platform glue differs (llama-server.exe, named-pipe sessions).
    Idempotent per (model,task): rerunning overwrites that cell's outputs.

    Usage:
        .\scripts\m4-battery\Run-M4Battery.ps1
        .\scripts\m4-battery\Run-M4Battery.ps1 -Models qwen3-4b
        $env:M4_TASKS = "t1 t2"; .\scripts\m4-battery\Run-M4Battery.ps1
#>
[CmdletBinding()]
param(
    [string[]]$Models = @(),
    [string]$Tasks = "",
    [string]$LlamaServerExe = "",
    [string]$ModelsDir = "",
    [string]$BinDir = "",
    [int]$Port = 8080,
    [int]$TurnCap = 6,
    [string]$ErrorMode = "heal",
    [int]$TaskTimeoutSec = 1800,   # 30 min: a model slower than this IS the data point
    [int]$GpuLayers = 999,         # full GPU offload on CUDA builds
    [string]$Python = "python"
)

$ErrorActionPreference = "Stop"
$ROOT = (Resolve-Path (Join-Path $PSScriptRoot ".." "..")).Path
$BAT = Join-Path $ROOT "scripts\m4-battery"
$TOOLS = Join-Path $ROOT ".tools"
$OUT = Join-Path $ROOT ".state\m4-battery"
if (-not $LlamaServerExe) { $LlamaServerExe = Join-Path $TOOLS "llamacpp\llama-server.exe" }
if (-not $ModelsDir)     { $ModelsDir = Join-Path $TOOLS "models" }
if (-not $BinDir)        { $BinDir = Join-Path $ROOT "target\debug" }
$FA32 = Join-Path $BinDir "fa32.exe"
$M4SCORE = Join-Path $BinDir "m4score.exe"

foreach ($d in @("$OUT\state", "$OUT\work", "$OUT\logs", "$OUT\scores", "$OUT\checks")) {
    New-Item -ItemType Directory -Path $d -Force | Out-Null
}

# --- task definitions -------------------------------------------------------
# Fixtures are written with LF newlines ([IO.File]::WriteAllText does no
# newline translation); check.py splits on "\n", so CRLF would fail cells.
function Install-M4Fixture([string]$task, [string]$cwd) {
    switch ($task) {
        "t2" {
            $docs = Join-Path $cwd "docs"; New-Item -ItemType Directory -Path $docs -Force | Out-Null
            [IO.File]::WriteAllText((Join-Path $docs "a.md"), "nothing here`n")
            [IO.File]::WriteAllText((Join-Path $docs "b.md"), "line1`nline2`nmarker-T2-5517`nline4`n")
            [IO.File]::WriteAllText((Join-Path $docs "c.md"), "nothing`n")
        }
        "t3" {
            [IO.File]::WriteAllText((Join-Path $cwd "t3.txt"), "KEEP-ONE`nREPLACE-ME`nKEEP-THREE`n")
        }
    }
}

function Get-M4Prompt([string]$task) {
    switch ($task) {
        "t1" { 'Create a file named t1.txt in the working directory with exactly two lines: first line ALPHA-ONE, second line marker-T1-8821. Emit one fenced fa block with a single Write-FAFile call, then report the two lines back in plain prose with no fenced block.' }
        "t2" { 'Search under the working directory for the text marker-T2-5517. Report the filename and the matching line in plain prose with no fenced block.' }
        "t3" { 'In the file t3.txt in the working directory, change line 2 from REPLACE-ME to marker-T3-9034, leaving the other lines untouched. Report the new line 2 in plain prose with no fenced block.' }
        "t4" { 'In a persistent terminal, set the variable $m4t4 to the value persist-4417. This MUST be its own Invoke-FACommand call. Then, in a SEPARATE Invoke-FACommand call, read $m4t4 back. Report the value you read in plain prose with no fenced block.' }
        "t5" { 'Read the file no-such-file-xyz.txt in the working directory and report its contents in plain prose.' }
        "t6" { 'Do these three steps in one fenced fa block, one call per line: 1) Create t6.txt in the working directory containing exactly chain-6601. 2) Search for chain-6601 under the working directory. 3) In the persistent terminal, echo the text chain-6601-done. Then report each step outcome in plain prose with no fenced block.' }
        default { throw "unknown task $task" }
    }
}

if (-not $Tasks) {
    $Tasks = if ($env:M4_TASKS) { $env:M4_TASKS } else { "t1 t2 t3 t4 t5 t6" }
}
$taskList = @($Tasks -split '\s+' | Where-Object { $_ })

# --- preflight ---------------------------------------------------------------
foreach ($f in @($FA32, $M4SCORE, $LlamaServerExe)) {
    if (-not (Test-Path $f)) { throw "missing $f" }
}
$tsv = Join-Path $BAT "models.tsv"
if (-not (Test-Path $tsv)) { throw "models.tsv missing — the model download step has not landed" }

$ggufs = @{}
foreach ($line in (Get-Content $tsv)) {
    if ($line -match '^\s*#' -or $line -match '^\s*$') { continue }
    $parts = $line -split "`t"
    $ggufs[$parts[0].Trim()] = $parts[1].Trim()
}
if (-not $Models -or $Models.Count -eq 0) { $Models = @($ggufs.Keys) }

# --- llama server -------------------------------------------------------------
$serverProc = $null
function Start-M4Server([string]$gguf, [string]$tag) {
    $log = Join-Path $OUT "server-$tag.log"
    $exeDir = Split-Path -Parent $LlamaServerExe
    $p = Start-Process -FilePath $LlamaServerExe `
        -ArgumentList @('-m', (Join-Path $ModelsDir $gguf), '--port', "$Port", '-c', '8192',
                        '--n-gpu-layers', "$GpuLayers") `
        -WorkingDirectory $exeDir -NoNewWindow -PassThru `
        -RedirectStandardOutput $log -RedirectStandardError "$log.err"
    for ($i = 0; $i -lt 120; $i++) {
        try {
            $r = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/health" -UseBasicParsing -TimeoutSec 5
            if ($r.StatusCode -eq 200) { Write-Host "llama.cpp up (pid $($p.Id)) serving $gguf"; return $p }
        } catch { }
        Start-Sleep -Seconds 5
    }
    throw "llama.cpp failed to come up; see $log"
}
function Stop-M4Server {
    if ($script:serverProc) {
        try { Stop-Process -InputObject $script:serverProc -Force } catch { }
        $script:serverProc = $null
    }
}

# --- one cell -----------------------------------------------------------------
function Invoke-M4Cell([string]$tag, [string]$task) {
    $sid = "m4-$tag-$task"
    $cwd = Join-Path $OUT "work\$tag\$task"
    if (Test-Path $cwd) { Remove-Item -Recurse -Force $cwd }
    New-Item -ItemType Directory -Path $cwd | Out-Null
    # Hermetic session dir: a stale session dir from a killed run must not
    # contaminate the cell (socket/DB/lock leftovers).
    $sessionDir = Join-Path $OUT "state\$tag\sessions\$sid"
    if (Test-Path $sessionDir) { Remove-Item -Recurse -Force $sessionDir }
    Install-M4Fixture $task $cwd
    $prompt = Get-M4Prompt $task
    $log = Join-Path $OUT "logs\$sid.log"
    Write-Host "=== [$tag/$task] $(Get-Date -Format HH:mm:ss) ==="

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $script:FA32
    foreach ($a in @('--backend', 'llamacpp',
                    '--llm-url', "http://127.0.0.1:$Port",
                    '--model', $tag,
                    '--auto-approve',
                    '--error-mode', $script:ErrorMode,
                    '--turn-cap', "$script:TurnCap",
                    '--message', $prompt,
                    '--cwd', $cwd,
                    '--module-dir', (Join-Path $script:ROOT 'psmodule\FactorAgent'),
                    '--state-dir', (Join-Path $script:OUT "state\$tag"),
                    '--session-id', $sid)) {
        $psi.ArgumentList.Add($a)
    }
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $proc = [System.Diagnostics.Process]::Start($psi)
    $timedOut = -not $proc.WaitForExit($script:TaskTimeoutSec * 1000)
    if ($timedOut) {
        try { $proc.Kill($true) } catch { }
        Write-Host "exit=TIMEOUT (task timeout)"
    } else {
        Write-Host "exit=$($proc.ExitCode)"
    }
    $out = $proc.StandardOutput.ReadToEnd() + "`n`n--- stderr ---`n" + $proc.StandardError.ReadToEnd()
    [IO.File]::WriteAllText($log, $out)

    $db = Join-Path $sessionDir "session.db"
    $scoreFile = Join-Path $OUT "scores\$sid.json"
    if (Test-Path $db) {
        & $script:M4SCORE $db $sid | Set-Content -Path $scoreFile -Encoding utf8NoBOM
    } else {
        '{"error":"no session db"}' | Set-Content -Path $scoreFile -Encoding utf8NoBOM
    }
    $checkFile = Join-Path $OUT "checks\$sid.json"
    & $script:Python (Join-Path $script:BAT "check.py") $task $cwd $scoreFile |
        Set-Content -Path $checkFile -Encoding utf8NoBOM
    Get-Content $checkFile
}

# --- main ----------------------------------------------------------------------
try {
    foreach ($tag in $Models) {
        $gguf = $ggufs[$tag]
        if (-not $gguf) { Write-Warning "unknown model tag: $tag"; continue }
        if (-not (Test-Path (Join-Path $ModelsDir $gguf))) {
            Write-Warning "missing $gguf — skipping $tag"; continue
        }
        $script:serverProc = Start-M4Server $gguf $tag
        try {
            foreach ($task in $taskList) { Invoke-M4Cell $tag $task }
        } finally {
            Stop-M4Server
        }
    }
} finally {
    Stop-M4Server
}

Write-Host "done. Aggregate with: $Python $BAT\score.py"
