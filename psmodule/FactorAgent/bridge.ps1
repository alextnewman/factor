#Requires -Version 7.0
<#
.SYNOPSIS
    M0 spike: newline-delimited JSON-RPC 2.0 bridge over stdin/stdout.
.DESCRIPTION
    Reads one JSON-RPC request per line on stdin, dispatches to an FA cmdlet,
    writes one JSON response per line on stdout. EOF on stdin = shutdown.

    Request:  {"jsonrpc":"2.0","id":1,"method":"Read-FAFile","params":{"Path":"..."}}
    Response: {"jsonrpc":"2.0","id":1,"result":"..."}
    Error:    {"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"..."}}

    Method names are restricted to *-FA* so the bridge can never be steered
    into arbitrary invocation. Windows will use a named pipe with the same
    framing; the spike validates framing, latency, streaming, and kill.
#>
[CmdletBinding()]
param(
    [string]$ModulePath = (Join-Path $PSScriptRoot 'FactorAgent.psd1')
)

$ErrorActionPreference = 'Stop'
Import-Module $ModulePath -Force -ErrorAction Stop

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
        $result = & $method @params
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
