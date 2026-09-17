# FactorAgent.psm1 — M0 spike loader.
# M1 will grow this into the full module; the rule stays: one file per cmdlet
# under Public/, loader dotsources them, nothing else lives here.
$PublicDir = Join-Path $PSScriptRoot 'Public'
Get-ChildItem -Path $PublicDir -Filter '*.ps1' -ErrorAction SilentlyContinue |
    ForEach-Object { . $_.FullName }

Export-ModuleMember -Function '*-FA*'
