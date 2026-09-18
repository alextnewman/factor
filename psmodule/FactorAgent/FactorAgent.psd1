@{
    ModuleVersion = '0.1.0'
    RootModule    = 'FactorAgent.psm1'
    Description   = 'FactorAgent engine tool module (M1 prototype: 11 agent tools + harness reflector)'
    FunctionsToExport = @(
        'Get-FASession',
        'Read-FAFile',
        'Write-FAFile',
        'Edit-FAFile',
        'Find-FAFile',
        'Find-FAText',
        'Get-FATree',
        'New-FATerminal',
        'Get-FATerminal',
        'Invoke-FACommand',
        'Remove-FATerminal',
        'Get-FAToolManifest'
    )
}
