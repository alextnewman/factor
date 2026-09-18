function Get-FASession {
    <#
    .SYNOPSIS
        Returns read-only facts about the current FactorAgent session.
    .DESCRIPTION
        Reports the session id, mode, error mode, execution backend, tool
        manifest version, and working directory. All values are read-only;
        mode dials stay operator-owned and cannot be changed from here.
        Facts come from the harness (FA_SESSION_JSON); outside a session it
        reports sensible interactive defaults.
    .EXAMPLE
        Get-FASession
        Shows the current session facts.
    .OUTPUTS
        PSCustomObject with Id, Mode, ErrorMode, Backend, ManifestVersion,
        WorkingDirectory.
    #>
    # .PRINTFORM: Session status
    [CmdletBinding()]
    param()
    $s = $null
    if ($env:FA_SESSION_JSON) {
        $s = $env:FA_SESSION_JSON | ConvertFrom-Json -ErrorAction SilentlyContinue
    }
    [PSCustomObject]@{
        Id              = if ($s -and $s.id) { [string]$s.id } else { 'interactive' }
        Mode            = if ($s -and $s.mode) { [string]$s.mode } else { 'managed' }
        ErrorMode       = if ($env:FA_ERROR_ACTION) { $env:FA_ERROR_ACTION } else { 'Stop' }
        Backend         = if ($s -and $s.backend) { [string]$s.backend } else { 'host' }
        ManifestVersion = $script:FAManifestVersion
        WorkingDirectory = (Get-Location).Path
    }
}
