function New-FATerminal {
    <#
    .SYNOPSIS
        Creates a named persistent terminal (a long-lived pwsh process).
    .DESCRIPTION
        Spawns one pwsh child running the terminal loop and registers it under
        -Name. The terminal persists across commands: state (variables, working
        directory, history) survives between Invoke-FACommand calls. Terminals
        live with the session; Remove-FATerminal reaps them. When the session
        reaches its terminal limit, creation fails and names Remove-FATerminal.
        Supports ShouldProcess: -WhatIf previews without spawning.
    .PARAMETER Name
        Terminal name, unique within the session.
    .PARAMETER WorkingDirectory
        Starting directory. Defaults to the session working directory.
    .PARAMETER Toolchain
        Accepted and recorded on the terminal. Toolchain application is
        deferred in the prototype (no environments are applied yet).
    .EXAMPLE
        New-FATerminal -Name build
        Creates a terminal named "build" in the working directory.
    .OUTPUTS
        PSCustomObject with Name, State, WorkingDirectory, Toolchain, Backend,
        CreatedAt, Preview.
    #>
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [Parameter(Mandatory)][string]$Name,
        [string]$WorkingDirectory = (Get-Location).Path,
        [string]$Toolchain = ''
    )
    if ($script:FATerminals.ContainsKey($Name)) {
        throw "New-FATerminal: terminal '$Name' already exists."
    }
    $max = if ($env:FA_MAX_TERMINALS) { [int]$env:FA_MAX_TERMINALS } else { 8 }
    if ($script:FATerminals.Count -ge $max) {
        throw ("New-FATerminal: terminal limit ($max) reached. " +
            "Remove-FATerminal one you no longer need, then retry.")
    }
    $wd = (Resolve-Path -LiteralPath $WorkingDirectory -ErrorAction Stop).Path
    $preview = "Create terminal '$Name' in '$wd'"
    if ($Toolchain) { $preview += " (toolchain '$Toolchain' recorded; application deferred in prototype)" }

    # Bridge-driven preview (see Write-FAFile): skip ShouldProcess entirely
    # so the engine's unredirectable "What if:" line never hits stdout.
    $previewOnly = [bool]$script:FAForceWhatIf
    if (-not $previewOnly -and $PSCmdlet.ShouldProcess($Name, "Create terminal in '$wd'")) {
        $terminalPs1 = Join-Path $script:FAModuleRoot 'terminal.ps1'
        if (-not (Test-Path -LiteralPath $terminalPs1)) {
            throw "New-FATerminal: terminal loop script not found at '$terminalPs1'."
        }
        $psi = [System.Diagnostics.ProcessStartInfo]::new()
        $psi.FileName = 'pwsh'
        $psi.ArgumentList.Add('-NoProfile')
        $psi.ArgumentList.Add('-NonInteractive')
        $psi.ArgumentList.Add('-File')
        $psi.ArgumentList.Add($terminalPs1)
        $psi.RedirectStandardInput = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $false
        $psi.UseShellExecute = $false
        $psi.CreateNoWindow = $true
        $psi.WorkingDirectory = $wd
        $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
        $proc = [System.Diagnostics.Process]::Start($psi)
        $proc.StandardInput.AutoFlush = $true
        $hello = $proc.StandardOutput.ReadLine()
        if ($null -eq $hello -or -not ($hello | ConvertFrom-Json -ErrorAction SilentlyContinue).ready) {
            try { $proc.Kill() } catch {}
            throw "New-FATerminal: terminal process failed its startup handshake."
        }
        $script:FATerminals[$Name] = [PSCustomObject]@{
            Name             = $Name
            Process          = $proc
            Reader           = $proc.StandardOutput
            Writer           = $proc.StandardInput
            WorkingDirectory = $wd
            Toolchain        = $Toolchain
            CreatedAt        = [DateTime]::UtcNow
            NextId           = 1
        }
    }
    [PSCustomObject]@{
        Name             = $Name
        State            = 'running'
        WorkingDirectory = $wd
        Toolchain        = $Toolchain
        Backend          = 'host'
        CreatedAt        = [DateTime]::UtcNow.ToString('o')
        Preview          = $preview
    }
}
