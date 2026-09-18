#Requires -Version 7.0
<#
.SYNOPSIS
    M1: newline-delimited JSON-RPC 2.0 bridge over stdin/stdout.
.DESCRIPTION
    Reads one JSON-RPC request per line on stdin, dispatches to an FA cmdlet
    in a FRESH CHILD SCOPE per invocation (arena-style: the runspace lives
    long, scopes are freed per use), writes one JSON response per line on
    stdout. EOF on stdin = shutdown.

    Request:  {"jsonrpc":"2.0","id":1,"method":"Read-FAFile","params":{"Path":"..."}}
    Response: {"jsonrpc":"2.0","id":1,"result":"..."}
    Error:    {"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"..."}}

    Special param "_WhatIf": when true and the cmdlet supports ShouldProcess,
    the bridge sets a module-scope preview flag and invokes the cmdlet
    WITHOUT -WhatIf, so the engine's unredirectable "What if:" line never
    reaches stdout. The cmdlet returns its Preview object; nothing mutates.

    Environment:
      FA_SESSION_JSON  - JSON session facts for Get-FASession
      FA_SESSION_ROOT  - session workspace root; the file cmdlets
                         (Assert-SessionPath) confine -Path to this tree
      FA_ERROR_ACTION  - "Continue" or anything else (= Stop); scoped per call
      FA_MAX_TERMINALS - terminal limit enforced by New-FATerminal

    Method names are restricted to *-FA* so the bridge can never be steered
    into arbitrary invocation. Windows will use a named pipe with the same
    framing.
#>
[CmdletBinding()]
param(
    [string]$ModulePath = (Join-Path $PSScriptRoot 'FactorAgent.psd1')
)

$ErrorActionPreference = 'Stop'
Import-Module $ModulePath -Force -ErrorAction Stop

$faErrorAction = if ($env:FA_ERROR_ACTION -eq 'Continue') { 'Continue' } else { 'Stop' }

function ConvertTo-FlatHashtable {
    param($Object)
    $ht = @{}
    if ($null -eq $Object) { return $ht }
    foreach ($prop in $Object.PSObject.Properties) {
        $ht[$prop.Name] = $prop.Value
    }
    return $ht
}

$stdin  = [Console]::In
$stdout = [Console]::Out

while ($true) {
    $line = $stdin.ReadLine()
    if ($null -eq $line) { break }  # EOF: parent went away, shut down
    if ([string]::IsNullOrWhiteSpace($line)) { continue }

    $id = $null
    try {
        $req = $line | ConvertFrom-Json -ErrorAction Stop
        $id = $req.id
        $method = [string]$req.method
        if ($method -notlike '*-FA*') {
            throw "bridge: refusing method '$method' (not an FA cmdlet)"
        }
        $params = ConvertTo-FlatHashtable $req.params
        $whatIf = $false
        if ($params.ContainsKey('_WhatIf')) {
            $whatIf = [bool]$params['_WhatIf']
            $params.Remove('_WhatIf')
        }
        # Fresh child scope per invocation; error action scoped to the call.
        $result = & {
            param($__m, $__p, $__w)
            $ErrorActionPreference = $faErrorAction
            $cmd = Get-Command $__m -ErrorAction Stop
            if ($__w -and $cmd.Parameters.ContainsKey('WhatIf')) {
                # Bridge-driven preview: set $script:FAForceWhatIf in module
                # scope and invoke WITHOUT -WhatIf. The engine's "What if:"
                # text is written by the console host straight to stdout,
                # bypassing every PowerShell stream (6>$null cannot catch
                # it); letting it print would corrupt the one-JSON-line
                # framing. The cmdlet returns its Preview object instead.
                $faModule = Get-Module FactorAgent -ErrorAction Stop
                & $faModule { $script:FAForceWhatIf = $true }
                try {
                    & $__m @__p
                }
                finally {
                    & $faModule { $script:FAForceWhatIf = $false }
                }
            }
            else {
                & $__m @__p
            }
        } $method $params $whatIf
        $response = @{ jsonrpc = '2.0'; id = $id; result = $result }
    }
    catch {
        $response = @{
            jsonrpc = '2.0'
            id      = $id
            error   = @{ code = -32000; message = $_.Exception.Message }
        }
    }
    $json = $response | ConvertTo-Json -Compress -Depth 10
    $stdout.WriteLine($json)
    $stdout.Flush()
}
