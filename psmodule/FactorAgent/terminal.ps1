#Requires -Version 7.0
<#
.SYNOPSIS
    Per-terminal command loop (spawned by New-FATerminal, never run by hand).
.DESCRIPTION
    Reads one JSON request per line on stdin: {"id":N,"command":"..."}.
    Executes the command DOT-SOURCED in this runspace's current scope, so
    state (variables, functions, working directory) survives between requests,
    and writes one JSON response per line on stdout:
    {"id":N,"output":"...","exitCode":0,"durationMs":12}
    or {"id":N,"error":"..."}.
    Emits {"ready":true} on startup as a handshake. EOF on stdin = shutdown.
    NOTE: commands must not read from stdin; there is no interactive input.
    Loop internals use __faterm_-prefixed names so dot-sourced commands are
    unlikely to clobber the loop's own variables.
#>
$__faterm_in  = [Console]::In
$__faterm_out = [Console]::Out
($__faterm_out.WriteLine('{"ready":true}'))
$__faterm_out.Flush()

while ($true) {
    $__faterm_line = $__faterm_in.ReadLine()
    if ($null -eq $__faterm_line) { break }
    if ([string]::IsNullOrWhiteSpace($__faterm_line)) { continue }
    $__faterm_id = $null
    try {
        $__faterm_req = $__faterm_line | ConvertFrom-Json -ErrorAction Stop
        $__faterm_id = $__faterm_req.id
        $__faterm_sw = [System.Diagnostics.Stopwatch]::StartNew()
        $__faterm_sb = [scriptblock]::Create([string]$__faterm_req.command)
        # Reset: a pure-PowerShell command must not inherit the previous
        # native command's exit code.
        $global:LASTEXITCODE = $null
        # Dot-source, NOT & : the command runs in this scope so its state
        # persists for the next command.
        $__faterm_out_text = . $__faterm_sb 2>&1 | Out-String
        $__faterm_ec = $LASTEXITCODE
        $__faterm_sw.Stop()
        $__faterm_resp = @{
            id         = $__faterm_id
            output     = [string]$__faterm_out_text
            exitCode   = if ($null -eq $__faterm_ec) { 0 } else { [int]$__faterm_ec }
            durationMs = $__faterm_sw.ElapsedMilliseconds
        }
    }
    catch {
        $__faterm_resp = @{ id = $__faterm_id; error = $_.Exception.Message }
    }
    $__faterm_out.WriteLine(($__faterm_resp | ConvertTo-Json -Compress -Depth 5))
    $__faterm_out.Flush()
}
