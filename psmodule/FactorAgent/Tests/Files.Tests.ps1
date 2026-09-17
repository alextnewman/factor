BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..' 'FactorAgent.psd1') -Force
}

Describe 'Read-FAFile' {
    It 'returns numbered lines' {
        Set-Content -Path (Join-Path $TestDrive 'a.txt') -Value @('alpha', 'beta', 'gamma')
        $r = Read-FAFile -Path (Join-Path $TestDrive 'a.txt')
        $r.Count | Should -Be 3
        $r[0] | Should -Be '1: alpha'
        $r[2] | Should -Be '3: gamma'
    }
    It '-Lines takes the head with original numbering' {
        1..10 | ForEach-Object { "line $_" } | Set-Content -Path (Join-Path $TestDrive 'b.txt')
        $r = Read-FAFile -Path (Join-Path $TestDrive 'b.txt') -Lines 3
        $r.Count | Should -Be 3
        $r[-1] | Should -Be '3: line 3'
    }
    It '-Tail takes the tail with original numbering' {
        1..10 | ForEach-Object { "line $_" } | Set-Content -Path (Join-Path $TestDrive 'b.txt')
        $r = Read-FAFile -Path (Join-Path $TestDrive 'b.txt') -Tail 2
        $r.Count | Should -Be 2
        $r[0] | Should -Be '9: line 9'
        $r[1] | Should -Be '10: line 10'
    }
    It '-Lines and -Tail are mutually exclusive' {
        { Read-FAFile -Path (Join-Path $TestDrive 'b.txt') -Lines 2 -Tail 2 } |
            Should -Throw '*mutually exclusive*'
    }
    It 'throws on missing file and on directories' {
        { Read-FAFile -Path (Join-Path $TestDrive 'nope.txt') } | Should -Throw
        { Read-FAFile -Path $TestDrive } | Should -Throw '*directory*'
    }
}

Describe 'Write-FAFile' {
    It 'writes content and creates parent dirs' {
        $p = Join-Path $TestDrive 'deep' 'sub' 'f.txt'
        $r = Write-FAFile -Path $p -Content 'hello'
        $r.BytesWritten | Should -Be 5
        $r.Created | Should -Be $true
        Get-Content -Raw -Path $p | Should -Be 'hello'
    }
    It '-Append appends' {
        $p = Join-Path $TestDrive 'app.txt'
        Write-FAFile -Path $p -Content "one`n" | Out-Null
        Write-FAFile -Path $p -Content "two`n" -Append | Out-Null
        (Get-Content -Raw -Path $p) | Should -Be "one`ntwo`n"
    }
    It '-WhatIf does not touch the disk' {
        $p = Join-Path $TestDrive 'whatif.txt'
        $r = Write-FAFile -Path $p -Content 'x' -WhatIf
        Test-Path $p | Should -Be $false
        $r.Preview | Should -Match 'Write 1 bytes'
    }
}

Describe 'Edit-FAFile' {
    BeforeEach {
        $script:ep = Join-Path $TestDrive 'edit.txt'
        Set-Content -Path $script:ep -Value @('color: red', 'size: big', 'color: red-ish')
    }
    It 'replaces exactly one anchored occurrence' {
        $r = Edit-FAFile -Path $script:ep -OldText 'size: big' -NewText 'size: small'
        $r.Replacements | Should -Be 1
        (Get-Content -Raw -Path $script:ep) | Should -Match 'size: small'
    }
    It 'fails loudly on zero matches' {
        { Edit-FAFile -Path $script:ep -OldText 'nope' -NewText 'x' } |
            Should -Throw '*0 matches*'
    }
    It 'fails loudly on ambiguous matches' {
        { Edit-FAFile -Path $script:ep -OldText 'color: red' -NewText 'x' } |
            Should -Throw '*ambiguous*2 matches*'
    }
    It 'rejects empty anchor' {
        { Edit-FAFile -Path $script:ep -OldText '' -NewText 'x' } | Should -Throw
    }
    It '-WhatIf does not modify' {
        Edit-FAFile -Path $script:ep -OldText 'size: big' -NewText 'size: small' -WhatIf | Out-Null
        (Get-Content -Raw -Path $script:ep) | Should -Match 'size: big'
    }
}

Describe 'Find-FAFile' {
    BeforeAll {
        $d = Join-Path $TestDrive 'find'
        New-Item -ItemType Directory -Path $d -Force | Out-Null
        Set-Content -Path (Join-Path $d 'demo-one.txt') -Value 'x'
        Set-Content -Path (Join-Path $d 'demo-two.md') -Value 'x'
        Set-Content -Path (Join-Path $d 'other.txt') -Value 'x'
        $script:fd = $d
    }
    It 'finds by glob, non-recursive' {
        $r = (Find-FAFile -Pattern 'demo-*' -Path $script:fd)
        $r.Count | Should -Be 2
    }
    It '-Recurse descends' {
        $sub = Join-Path $script:fd 'sub'
        New-Item -ItemType Directory -Path $sub -Force | Out-Null
        Set-Content -Path (Join-Path $sub 'demo-deep.txt') -Value 'x'
        $r = (Find-FAFile -Pattern 'demo-*' -Path $script:fd -Recurse)
        $r.Count | Should -Be 3
    }
    It 'returns relative paths' {
        $r = (Find-FAFile -Pattern 'other.txt' -Path $script:fd)
        $r[0] | Should -Be 'other.txt'
    }
}

Describe 'Find-FAText' {
    BeforeAll {
        $d = Join-Path $TestDrive 'grep'
        New-Item -ItemType Directory -Path $d -Force | Out-Null
        Set-Content -Path (Join-Path $d 'a.ps1') -Value @('# TODO: fix this', 'Write-Output ok')
        Set-Content -Path (Join-Path $d 'b.md') -Value @('nothing here', 'TODO later')
        $script:gd = $d
    }
    It 'finds matching lines with numbers' {
        $r = (Find-FAText -Pattern 'TODO' -Path $script:gd -Recurse)
        $r.Count | Should -Be 2
        $r[0].LineNumber | Should -Be 1
        $r[0].Line | Should -Match 'TODO'
    }
    It '-FilePattern filters files' {
        $r = (Find-FAText -Pattern 'TODO' -Path $script:gd -Recurse -FilePattern '*.md')
        $r.Count | Should -Be 1
        $r[0].Path | Should -Be 'b.md'
    }
    It 'a single hit is still an array (no scalar unwrap on the wire)' {
        $r = Find-FAText -Pattern 'TODO' -Path $script:gd -Recurse -FilePattern '*.md'
        ($r -is [array]) | Should -BeTrue
        $r.Count | Should -Be 1
    }
    It 'zero hits is an empty array, not $null' {
        $r = Find-FAText -Pattern 'ZZZ_NO_MATCH' -Path $script:gd -Recurse
        ($r -is [array]) | Should -BeTrue
        $r.Count | Should -Be 0
    }
    It '-SimpleMatch treats pattern literally' {
        $r = (Find-FAText -Pattern 'TODO:' -Path $script:gd -SimpleMatch)
        $r.Count | Should -Be 1
    }
}
