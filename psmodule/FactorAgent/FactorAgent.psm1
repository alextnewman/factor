# FactorAgent.psm1 — module loader.
# One file per cmdlet under Public/; the loader dotsources them.
# Module-scoped state:
#   $script:FATerminals      - live terminal table (New/Get/Invoke/Remove-FATerminal)
#   $script:FAManifestVersion - pinned into Block A per session
$script:FATerminals = @{}
$script:FAManifestVersion = '0.1.0'
# Module root (one level above Public/): where bridge.ps1 and terminal.ps1 live.
$script:FAModuleRoot = $PSScriptRoot

$PublicDir = Join-Path $PSScriptRoot 'Public'
Get-ChildItem -Path $PublicDir -Filter '*.ps1' -ErrorAction SilentlyContinue |
    ForEach-Object { . $_.FullName }

# Best-effort reaping: if the host exits, take the terminals with it.
# (The Rust supervisor also kills the process group; this is the inner net.)
Register-EngineEvent -SourceIdentifier PowerShell.Exiting -Action {
    foreach ($t in $script:FATerminals.Values) {
        try { if (-not $t.Process.HasExited) { $t.Process.Kill() } } catch {}
    }
} | Out-Null

Export-ModuleMember -Function '*-FA*'
