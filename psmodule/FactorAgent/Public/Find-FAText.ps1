function Find-FAText {
    <#
    .SYNOPSIS
        Searches file contents for a pattern.
    .DESCRIPTION
        Searches files under -Path for -Pattern (regex by default; -SimpleMatch
        for literal text). Optionally recurses and filters by file glob.
        Returns one object per matching line with the relative path, line
        number, and the trimmed line.
    .PARAMETER Pattern
        Regex pattern, or literal text with -SimpleMatch.
    .PARAMETER Path
        Directory to search. Defaults to the session working directory.
    .PARAMETER Recurse
        Search subdirectories too.
    .PARAMETER FilePattern
        Glob limiting which files are searched, e.g. "*.md". Default "*".
    .PARAMETER Context
        Lines of context around each match to include in the Context field.
    .PARAMETER SimpleMatch
        Treat -Pattern as literal text instead of regex.
    .EXAMPLE
        Find-FAText -Pattern "TODO" -Recurse -FilePattern "*.ps1"
        Finds TODO comments in PowerShell files.
    .OUTPUTS
        PSCustomObject[] with Path, LineNumber, Line, Context.
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Pattern,
        [string]$Path = (Get-Location).Path,
        [switch]$Recurse,
        [string]$FilePattern = '*',
        [int]$Context = 0,
        [switch]$SimpleMatch
    )
    $root = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
    $files = Get-ChildItem -LiteralPath $root -Filter $FilePattern -Recurse:$Recurse -File -ErrorAction Stop
    # Always emit an array (even for 0 or 1 hits): the JSON-RPC wire shape
    # must not depend on the hit count.
    $hits = @(foreach ($f in $files) {
        $matches = $f | Select-String -Pattern $Pattern -SimpleMatch:$SimpleMatch `
            -Context $Context -ErrorAction SilentlyContinue
        foreach ($m in $matches) {
            $ctx = ''
            if ($Context -gt 0 -and $m.Context) {
                $ctx = (($m.Context.PreContext + $m.Context.PostContext) -join "`n")
            }
            [PSCustomObject]@{
                Path       = [System.IO.Path]::GetRelativePath($root, $m.Path)
                LineNumber = $m.LineNumber
                Line       = $m.Line.Trim()
                Context    = $ctx
            }
        }
    })
    Write-Output -NoEnumerate $hits
}
