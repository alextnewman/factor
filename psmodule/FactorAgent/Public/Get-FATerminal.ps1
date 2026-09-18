function Get-FATerminal {
    <#
    .SYNOPSIS
        Lists the session's terminals.
    .DESCRIPTION
        Returns one object per terminal: name, state (running/exited), working
        directory, toolchain, and the framework-owned backend (read-only).
        With -Name, returns just that terminal, or throws if it does not exist.
    .PARAMETER Name
        Optional terminal name to look up.
    .EXAMPLE
        Get-FATerminal
        Lists all terminals in the session.
    .OUTPUTS
        PSCustomObject[] with Name, State, WorkingDirectory, Toolchain,
        Backend, CreatedAt.
    #>
    # .PRINTFORM: List terminals
    [CmdletBinding()]
    param(
        [string]$Name = ''
    )
    $list = if ($Name) {
        if (-not $script:FATerminals.ContainsKey($Name)) {
            throw "Get-FATerminal: terminal '$Name' does not exist."
        }
        @($script:FATerminals[$Name])
    }
    else {
        @($script:FATerminals.Values)
    }
    # Always an array on the wire, even for one terminal.
    $out = @($list | ForEach-Object {
        $t = $_
        [PSCustomObject]@{
            Name             = $t.Name
            State            = if ($t.Process.HasExited) { 'exited' } else { 'running' }
            WorkingDirectory = $t.WorkingDirectory
            Toolchain        = $t.Toolchain
            Backend          = 'host'
            CreatedAt        = $t.CreatedAt.ToString('o')
        }
    })
    Write-Output -NoEnumerate $out
}
