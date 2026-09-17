function Remove-FATerminal {
    <#
    .SYNOPSIS
        Removes a terminal, killing its process.
    .DESCRIPTION
        Kills the terminal's pwsh process and unregisters the name. Removing
        a terminal discards its state (variables, working directory). The
        default terminal can be removed like any other; it will be lazily
        recreated on the next Invoke-FACommand without -Terminal.
        Supports ShouldProcess: -WhatIf previews without killing anything.
    .PARAMETER Name
        Terminal name to remove.
    .EXAMPLE
        Remove-FATerminal -Name build
        Kills and unregisters the 'build' terminal.
    .OUTPUTS
        PSCustomObject with Name, Removed, Preview.
    #>
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [Parameter(Mandatory)][string]$Name
    )
    if (-not $script:FATerminals.ContainsKey($Name)) {
        throw "Remove-FATerminal: terminal '$Name' does not exist."
    }
    $t = $script:FATerminals[$Name]
    $preview = "Remove terminal '$Name' (kill pid $($t.Process.Id))"
    # Bridge-driven preview (see Write-FAFile): skip ShouldProcess entirely
    # so the engine's unredirectable "What if:" line never hits stdout.
    $previewOnly = [bool]$script:FAForceWhatIf
    if (-not $previewOnly -and $PSCmdlet.ShouldProcess($Name, "Remove terminal (kill process)")) {
        try {
            if (-not $t.Process.HasExited) {
                $t.Process.Kill()
                $t.Process.WaitForExit(5000) | Out-Null
            }
        }
        catch {}
        try { $t.Writer.Close() } catch {}
        try { $t.Reader.Close() } catch {}
        try { $t.Process.Dispose() } catch {}
        $script:FATerminals.Remove($Name)
    }
    [PSCustomObject]@{
        Name    = $Name
        Removed = $true
        Preview = $preview
    }
}
