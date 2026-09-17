function Invoke-FACommand {
    <#
    .SYNOPSIS
        Runs a command in a persistent terminal.
    .DESCRIPTION
        Sends -Command to the named terminal's pwsh process and returns its
        output, exit code, and duration. Terminal state persists between
        calls. If -Terminal is omitted, the session's default terminal is
        used, lazily created on first use. Commands must not read from stdin
        (no interactive prompts) in the prototype. Supports ShouldProcess:
        -WhatIf previews without running anything (and without creating the
        lazy default terminal).
    .PARAMETER Terminal
        Terminal name. Defaults to 'default' (lazily created).
    .PARAMETER Command
        PowerShell command text to run.
    .EXAMPLE
        Invoke-FACommand -Command "Get-Date -Format 'yyyy-MM-dd'"
        Runs Get-Date in the default terminal.
    .EXAMPLE
        Invoke-FACommand -Terminal build -Command "cargo build"
        Runs cargo build in the 'build' terminal.
    .OUTPUTS
        PSCustomObject with Terminal, ExitCode, Output, DurationMs, Preview.
    #>
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [string]$Terminal = 'default',
        [Parameter(Mandatory)][string]$Command
    )
    $preview = "Run in terminal '$Terminal': $Command"
    # Bridge-driven preview (see Write-FAFile): skip ShouldProcess entirely
    # so the engine's unredirectable "What if:" line never hits stdout.
    $previewOnly = [bool]$script:FAForceWhatIf
    if (-not $previewOnly -and $PSCmdlet.ShouldProcess($Terminal, "Run command")) {
        if (-not $script:FATerminals.ContainsKey($Terminal)) {
            if ($Terminal -eq 'default') {
                New-FATerminal -Name 'default' | Out-Null
            }
            else {
                throw "Invoke-FACommand: terminal '$Terminal' does not exist."
            }
        }
        $t = $script:FATerminals[$Terminal]
        if ($t.Process.HasExited) {
            throw "Invoke-FACommand: terminal '$Terminal' has exited."
        }
        $id = $t.NextId
        $t.NextId++
        $req = @{ id = $id; command = $Command } | ConvertTo-Json -Compress -Depth 5
        try {
            $t.Writer.WriteLine($req)
        }
        catch {
            throw "Invoke-FACommand: failed writing to terminal '$Terminal': $($_.Exception.Message)"
        }
        $line = $t.Reader.ReadLine()
        if ($null -eq $line) {
            throw "Invoke-FACommand: terminal '$Terminal' closed the pipe unexpectedly."
        }
        $resp = $line | ConvertFrom-Json -ErrorAction Stop
        if ($resp.PSObject.Properties.Name -contains 'error') {
            throw "Invoke-FACommand: terminal '$Terminal' reported: $($resp.error)"
        }
        [PSCustomObject]@{
            Terminal   = $Terminal
            ExitCode   = [int]$resp.exitCode
            Output     = [string]$resp.output
            DurationMs = [long]$resp.durationMs
            Preview    = $preview
        }
    }
    else {
        [PSCustomObject]@{
            Terminal = $Terminal
            Preview  = $preview
        }
    }
}
