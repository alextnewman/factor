# Wire-encoding regression tests: the JSON-RPC loops in bridge.ps1 and
# terminal.ps1 must speak UTF-8 unconditionally. [Console]::In/Out follow the
# legacy Windows console code page (e.g. cp437), which maps characters like
# § (U+00A7) to raw C0 control bytes (0x15) — silently corrupting the JSON
# framing for any non-ASCII payload. These tests drive the real scripts as
# child processes (stdin from a file, stdout to a file, raw bytes asserted).
BeforeAll {
    $script:BridgePs1 = Join-Path $PSScriptRoot '..' 'bridge.ps1'
    $script:TerminalPs1 = Join-Path $PSScriptRoot '..' 'terminal.ps1'
    $script:Pwsh = (Get-Command pwsh -ErrorAction Stop).Source

    # Spawn a wire script: stdin comes from a UTF-8 file (EOF = clean shutdown),
    # stdout is captured to a file so we can assert on the raw bytes.
    function Invoke-WireScript {
        param(
            [Parameter(Mandatory)][string]$Script,
            [Parameter(Mandatory)][string]$RequestText
        )
        $reqFile = Join-Path $TestDrive 'wire-req.txt'
        $respFile = Join-Path $TestDrive 'wire-resp.bin'
        if (Test-Path $respFile) { Remove-Item $respFile -Force }
        [System.IO.File]::WriteAllText($reqFile, $RequestText, [System.Text.UTF8Encoding]::new($false))
        $p = Start-Process $script:Pwsh -ArgumentList @(
            '-NoProfile', '-NonInteractive', '-File', $Script
        ) -RedirectStandardInput $reqFile -RedirectStandardOutput $respFile `
            -WorkingDirectory $TestDrive -NoNewWindow -PassThru
        if (-not $p.WaitForExit(20000)) {
            try { $p.Kill() } catch {}
            throw "wire script did not exit: $Script"
        }
        if ($p.ExitCode -ne 0) { throw "wire script exited $($p.ExitCode): $Script" }
        return [System.IO.File]::ReadAllBytes($respFile)
    }

    # No raw C0 control byte (0x00-0x1F) may appear on the wire outside the line
    # terminator: inside JSON, a raw control is never valid in a string.
    # Returns the response lines as text.
    function Assert-NoRawControls {
        param([Parameter(Mandatory)][byte[]]$Raw)
        $bad = @($Raw | Where-Object { $_ -lt 0x20 -and $_ -ne 0x0A -and $_ -ne 0x0D })
        $bad | Should -BeNullOrEmpty -Because 'the wire must be clean UTF-8 JSON'
        $text = [System.Text.Encoding]::UTF8.GetString($Raw)
        return @($text -split "`r?`n" | Where-Object { $_ -ne '' })
    }
}

Describe 'bridge.ps1 wire encoding' {
    BeforeAll {
        $env:FA_SESSION_ROOT = $TestDrive
        $env:FA_SESSION_JSON = '{"id":"t","mode":"managed","backend":"host"}'
        $env:FA_ERROR_ACTION = 'Continue'
        $env:FA_MAX_TERMINALS = '8'
    }
    It 'round-trips non-ASCII file content as valid UTF-8 JSON' {
        # § (U+00A7) is the canary: under cp437 it encodes to byte 0x15.
        [System.IO.File]::WriteAllLines(
            (Join-Path $TestDrive 'unicode.txt'),
            @('plain', 'section §4.9', 'emdash — here', 'CJK: 日本語'),
            [System.Text.UTF8Encoding]::new($false))
        $req = '{"jsonrpc":"2.0","id":1,"method":"Read-FAFile","params":{"Path":"unicode.txt"}}' + "`n"
        $raw = Invoke-WireScript -Script $script:BridgePs1 -RequestText $req
        $lines = @(Assert-NoRawControls -Raw $raw)
        $lines.Count | Should -Be 1
        $v = $lines[0] | ConvertFrom-Json -ErrorAction Stop
        $v.result[0] | Should -Be '1: plain'
        $v.result[1] | Should -Be '2: section §4.9'
        $v.result[2] | Should -Be '3: emdash — here'
        $v.result[3] | Should -Be '4: CJK: 日本語'
    }
}

Describe 'terminal.ps1 wire encoding' {
    It 'round-trips non-ASCII command output as valid UTF-8 JSON' {
        $req = '{"id":1,"command":"''§ — 日本語''"}' + "`n"
        $raw = Invoke-WireScript -Script $script:TerminalPs1 -RequestText $req
        $lines = @(Assert-NoRawControls -Raw $raw)
        $lines.Count | Should -Be 2
        $lines[0] | Should -Be '{"ready":true}'
        $v = $lines[1] | ConvertFrom-Json -ErrorAction Stop
        $v.id | Should -Be 1
        $v.output | Should -Match '§ — 日本語'
    }
    It 'captures Write-Host and warning streams instead of leaking them onto the wire' {
        # Write-Host writes to the information stream (6); an uncaptured
        # stream lands on stdout raw, so the next ReadLine gets
        # "hello from..." instead of the JSON response and the framing dies.
        $cmd = "Write-Host 'hello-info'; Write-Warning 'warn-here'; 'plain-output'"
        $req = (@{ id = 1; command = $cmd } | ConvertTo-Json -Compress) + "`n"
        $raw = Invoke-WireScript -Script $script:TerminalPs1 -RequestText $req
        $lines = @(Assert-NoRawControls -Raw $raw)
        $lines.Count | Should -Be 2
        $lines[0] | Should -Be '{"ready":true}'
        $v = $lines[1] | ConvertFrom-Json -ErrorAction Stop
        $v.id | Should -Be 1
        $v.output | Should -Match 'hello-info'
        $v.output | Should -Match 'warn-here'
        $v.output | Should -Match 'plain-output'
    }
}
