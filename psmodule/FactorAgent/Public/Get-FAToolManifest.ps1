function Get-FAToolManifest {
    <#
    .SYNOPSIS
        Emits the tool manifest as JSON (harness use; not an agent tool).
    .DESCRIPTION
        Reflects every *-FA* function in the FactorAgent module (except
        itself) into a JSON array: name, synopsis, description, parameters
        (name, JSON type, required, enum, description), examples, outputs,
        source tag. The Rust harness calls this once per session to build
        Block A. Single source of truth: the manifest IS the module.
    .EXAMPLE
        Get-FAToolManifest | ConvertFrom-Json
    .OUTPUTS
        String. Compressed JSON array of tool schemas.
    #>
    [CmdletBinding()]
    param()
    $skip = @('Verbose', 'Debug', 'ErrorAction', 'WarningAction', 'InformationAction',
        'ErrorVariable', 'WarningVariable', 'InformationVariable', 'OutVariable',
        'OutBuffer', 'PipelineVariable', 'ProgressAction', 'WhatIf', 'Confirm')
    $tools = foreach ($cmd in (Get-Command -Module FactorAgent -CommandType Function |
            Where-Object { $_.Name -like '*-FA*' -and $_.Name -ne 'Get-FAToolManifest' } |
            Sort-Object Name)) {
        $help = Get-Help $cmd.Name
        $params = foreach ($p in ($cmd.Parameters.Values | Sort-Object Name)) {
            if ($p.Name -in $skip) { continue }
            $typeName = $p.ParameterType.Name
            $isSwitch = $typeName -eq 'SwitchParameter'
            $pattrs = @($p.Attributes | Where-Object {
                $_ -is [System.Management.Automation.ParameterAttribute] })
            $mandatory = ($pattrs | Where-Object { $_.Mandatory }).Count -gt 0
            $vs = $p.Attributes | Where-Object {
                $_ -is [System.Management.Automation.ValidateSetAttribute] } |
                Select-Object -First 1
            $ph = $help.parameters.parameter | Where-Object { $_.name -eq $p.Name } |
                Select-Object -First 1
            $jtype = if ($isSwitch) { 'boolean' } else {
                switch -Regex ($typeName) {
                    '^String' { 'string' }
                    '^Int(32|64)?$' { 'integer' }
                    '^Bool(ean)?$' { 'boolean' }
                    'String\[\]' { 'array(string)' }
                    default { 'string' }
                }
            }
            [PSCustomObject]@{
                name        = $p.Name
                type        = $jtype
                required    = ($mandatory -and -not $isSwitch)
                enum        = if ($vs) { @($vs.ValidValues) } else { $null }
                description = if ($ph) { (($ph.description.Text) -join ' ').Trim() } else { '' }
            }
        }
        $examples = @($help.examples.example | ForEach-Object {
            (($_.code) -join "`n").Trim() } | Where-Object { $_ })
        [PSCustomObject]@{
            name        = $cmd.Name
            synopsis    = $help.Synopsis.Trim()
            description = (($help.description.Text) -join "`n").Trim()
            parameters  = @($params)
            examples    = $examples
            outputs     = ($help.returnValues.returnValue.type.name | Select-Object -First 1)
            source      = 'builtin'
        }
    }
    $tools | ConvertTo-Json -Depth 6 -Compress
}
