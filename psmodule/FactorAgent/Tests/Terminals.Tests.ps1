BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..' 'FactorAgent.psd1') -Force
}

Describe 'Terminal lifecycle' {
    AfterEach {
        foreach ($t in (Get-FATerminal)) {
            if ($t.Name -like 'pester-*') { Remove-FATerminal -Name $t.Name }
        }
    }

    It 'New/Get/Invoke/Remove round-trip' {
        $n = New-FATerminal -Name 'pester-a' -WorkingDirectory $TestDrive
        $n.State | Should -Be 'running'
        $n.Backend | Should -Be 'host'

        $all = (Get-FATerminal)
        ($all.Name) | Should -Contain 'pester-a'

        $r = Invoke-FACommand -Terminal 'pester-a' -Command "Write-Output 'term-ok'"
        $r.ExitCode | Should -Be 0
        $r.Output | Should -Match 'term-ok'

        $rm = Remove-FATerminal -Name 'pester-a'
        $rm.Removed | Should -Be $true
        { Get-FATerminal -Name 'pester-a' } | Should -Throw '*does not exist*'
    }

    It 'terminal state persists between commands' {
        New-FATerminal -Name 'pester-state' -WorkingDirectory $TestDrive | Out-Null
        Invoke-FACommand -Terminal 'pester-state' -Command '$FAProbe = 41 + 1' | Out-Null
        $r = Invoke-FACommand -Terminal 'pester-state' -Command '$FAProbe'
        $r.Output | Should -Match '42'
    }

    It 'lazy default terminal is created on first Invoke-FACommand' {
        $r = Invoke-FACommand -Command "Write-Output 'lazy-ok'"
        $r.Terminal | Should -Be 'default'
        $r.Output | Should -Match 'lazy-ok'
        (Get-FATerminal -Name 'default').State | Should -Be 'running'
        Remove-FATerminal -Name 'default'
    }

    It 'unknown terminal is a loud error' {
        { Invoke-FACommand -Terminal 'pester-ghost' -Command 'x' } |
            Should -Throw "*does not exist*"
    }

    It 'truncates long output, keeping the tail' {
        $r = Invoke-FACommand -Command "1..3000 | ForEach-Object { 'line ' + `$_ }"
        $lines = $r.Output -split "`n"
        $lines.Count | Should -Be 2001
        $lines[0] | Should -Match 'output truncated.*last 2000 of 3000 lines'
        $lines[-1].Trim() | Should -Be 'line 3000'
        Remove-FATerminal -Name 'default'
    }

    It 'duplicate names are refused' {
        New-FATerminal -Name 'pester-dup' | Out-Null
        { New-FATerminal -Name 'pester-dup' } | Should -Throw '*already exists*'
    }

    It '-WhatIf previews without spawning or running' {
        $before = (Get-FATerminal).Count
        $n = New-FATerminal -Name 'pester-whatif' -WhatIf
        $n.Preview | Should -Match "Create terminal 'pester-whatif'"
        (Get-FATerminal).Count | Should -Be $before

        New-FATerminal -Name 'pester-wi2' | Out-Null
        $r = Invoke-FACommand -Terminal 'pester-wi2' -Command 'Remove-Item C:\' -WhatIf
        $r.Preview | Should -Match "Run in terminal 'pester-wi2'"
        # real run still works afterwards (nothing ran under WhatIf)
        $r2 = Invoke-FACommand -Terminal 'pester-wi2' -Command "Write-Output 'still-here'"
        $r2.Output | Should -Match 'still-here'
        Remove-FATerminal -Name 'pester-wi2'
    }

    It 'command failure surfaces the exit code, not a throw' {
        New-FATerminal -Name 'pester-fail' | Out-Null
        # NOTE: bare `exit N` would kill the terminal process itself; a nested
        # pwsh is the portable way to produce a real native nonzero exit.
        $r = Invoke-FACommand -Terminal 'pester-fail' -Command 'pwsh -NoProfile -NonInteractive -Command "exit 3"'
        $r.ExitCode | Should -Be 3
    }

    It 'stale $LASTEXITCODE does not leak into the next command' {
        New-FATerminal -Name 'pester-ec' | Out-Null
        Invoke-FACommand -Terminal 'pester-ec' -Command 'pwsh -NoProfile -NonInteractive -Command "exit 3"' | Out-Null
        $r = Invoke-FACommand -Terminal 'pester-ec' -Command "Write-Output 'pure-posh'"
        $r.ExitCode | Should -Be 0
        $r.Output | Should -Match 'pure-posh'
    }

    It 'honors FA_MAX_TERMINALS' {
        $env:FA_MAX_TERMINALS = '1'
        try {
            New-FATerminal -Name 'pester-lim1' | Out-Null
            { New-FATerminal -Name 'pester-lim2' } | Should -Throw '*terminal limit*'
        }
        finally { Remove-Item Env:\FA_MAX_TERMINALS -ErrorAction SilentlyContinue }
    }
}

Describe 'Get-FASession' {
    It 'reports read-only facts' {
        $env:FA_SESSION_JSON = '{"id":"sess-123","mode":"managed","backend":"host"}'
        try {
            $s = Get-FASession
            $s.Id | Should -Be 'sess-123'
            $s.Mode | Should -Be 'managed'
            $s.Backend | Should -Be 'host'
            $s.ManifestVersion | Should -Not -BeNullOrEmpty
            $s.WorkingDirectory | Should -Not -BeNullOrEmpty
        }
        finally { Remove-Item Env:\FA_SESSION_JSON -ErrorAction SilentlyContinue }
    }
}

Describe 'Get-FAToolManifest' {
    It 'emits all 11 agent tools as JSON' {
        $m = Get-FAToolManifest | ConvertFrom-Json
        $names = @($m.name)
        $names.Count | Should -Be 11
        foreach ($n in @('Get-FASession','Read-FAFile','Write-FAFile','Edit-FAFile',
                'Find-FAFile','Find-FAText','Get-FATree','New-FATerminal','Get-FATerminal',
                'Invoke-FACommand','Remove-FATerminal')) {
            $names | Should -Contain $n
        }
    }
    It 'every tool has synopsis, parameters with types, and examples' {
        $m = Get-FAToolManifest | ConvertFrom-Json
        foreach ($t in $m) {
            $t.synopsis | Should -Not -BeNullOrEmpty
            @($t.examples).Count | Should -BeGreaterThan 0 -Because "$($t.name) needs examples"
            foreach ($p in $t.parameters) {
                $p.type | Should -Match '^(string|integer|boolean|array)'
            }
        }
    }
}
