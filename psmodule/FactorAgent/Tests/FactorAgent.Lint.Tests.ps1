BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..' 'FactorAgent.psd1') -Force
    $script:PublicDir = Join-Path $PSScriptRoot '..' 'Public'
    $script:Files = Get-ChildItem -Path $script:PublicDir -Filter '*-FA*.ps1'
}

Describe 'Module lint — built-ins dogfood §4.9.3' {
    It 'every Public/*.ps1 defines exactly one function matching the file base name' {
        foreach ($f in $script:Files) {
            $ast = [System.Management.Automation.Language.Parser]::ParseFile(
                $f.FullName, [ref]$null, [ref]$null)
            $funcs = @($ast.FindAll(
                { param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $true))
            $funcs.Count | Should -Be 1 -Because "$($f.Name) must define exactly one function"
            $funcs[0].Name | Should -Be $f.BaseName -Because 'file/function name must match'
        }
    }

    It 'every function has [CmdletBinding()]' {
        foreach ($f in $script:Files) {
            $ast = [System.Management.Automation.Language.Parser]::ParseFile(
                $f.FullName, [ref]$null, [ref]$null)
            $func = $ast.Find(
                { param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $true)
            $func.Body.ParamBlock.Attributes.TypeName.FullName |
                Should -Contain 'CmdletBinding' -Because "$($f.Name) needs [CmdletBinding()]"
        }
    }

    It 'every function has required help fields (SYNOPSIS, DESCRIPTION, PARAMETER per param, EXAMPLE, OUTPUTS)' {
        $harnessOnly = @('Get-FAToolManifest')
        foreach ($f in $script:Files) {
            $name = $f.BaseName
            if ($name -in $harnessOnly) { continue }
            $h = Get-Help $name
            $h.Synopsis.Trim() | Should -Not -BeNullOrEmpty -Because "$name needs .SYNOPSIS"
            (($h.description.Text) -join "`n").Trim() |
                Should -Not -BeNullOrEmpty -Because "$name needs .DESCRIPTION"
            $cmd = Get-Command $name
            $userParams = @($cmd.Parameters.Values | Where-Object {
                $_.Name -notin @('Verbose','Debug','ErrorAction','WarningAction','InformationAction',
                    'ErrorVariable','WarningVariable','InformationVariable','OutVariable',
                    'OutBuffer','PipelineVariable','WhatIf','Confirm','ProgressAction') })
            foreach ($p in $userParams) {
                $ph = $h.parameters.parameter | Where-Object { $_.name -eq $p.Name }
                $ph | Should -Not -BeNullOrEmpty -Because "$name needs .PARAMETER $($p.Name)"
            }
            @($h.examples.example).Count | Should -BeGreaterThan 0 -Because "$name needs .EXAMPLE"
            $h.returnValues | Should -Not -BeNullOrEmpty -Because "$name needs .OUTPUTS"
        }
    }

    It 'verbs are PowerShell-approved' {
        $approved = (Get-Verb).Verb
        foreach ($f in $script:Files) {
            $verb = $f.BaseName.Split('-')[0]
            $approved | Should -Contain $verb -Because "$($f.Name) uses verb $verb"
        }
    }

    It 'all parameters are explicitly typed' {
        foreach ($f in $script:Files) {
            $ast = [System.Management.Automation.Language.Parser]::ParseFile(
                $f.FullName, [ref]$null, [ref]$null)
            $params = $ast.FindAll(
                { param($n) $n -is [System.Management.Automation.Language.ParameterAst] }, $true)
            foreach ($p in $params) {
                $p.StaticType.Name | Should -Not -Be 'Object' -Because "$($f.Name): $($p.Name) must be typed"
            }
        }
    }

    It 'no Invoke-Expression / Add-Type anywhere in the module' {
        foreach ($f in $script:Files) {
            if ($f.BaseName -eq 'terminal') { continue }
            $tokens = $null
            [System.Management.Automation.Language.Parser]::ParseFile(
                $f.FullName, [ref]$tokens, [ref]$null) | Out-Null
            $bad = @($tokens | Where-Object {
                $_.Kind -eq 'Identifier' -and $_.Text -in @('Invoke-Expression', 'iex', 'Add-Type') })
            $bad.Count | Should -Be 0 -Because "$($f.Name) must not use Invoke-Expression/Add-Type"
        }
    }
}
