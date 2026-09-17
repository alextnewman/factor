function Find-FAFile {
    <#
    .SYNOPSIS
        Finds files by glob pattern.
    .DESCRIPTION
        Searches for files matching a glob pattern (e.g. "*.ps1", "demo-*").
        Returns paths relative to -Path. Add -Recurse to search subdirectories.
    .PARAMETER Pattern
        Glob pattern, e.g. "*.md".
    .PARAMETER Path
        Directory to search. Defaults to the session working directory.
    .PARAMETER Recurse
        Search subdirectories too.
    .EXAMPLE
        Find-FAFile -Pattern "demo-*" -Recurse
        Finds all files starting with demo- under the working directory.
    .OUTPUTS
        String[]. Relative file paths, one per line.
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Pattern,
        [string]$Path = (Get-Location).Path,
        [switch]$Recurse
    )
    $root = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
    $items = Get-ChildItem -LiteralPath $root -Filter $Pattern -Recurse:$Recurse -File -ErrorAction Stop
    # Always emit an array on the wire, even for 0 or 1 matches.
    $rel = @($items | ForEach-Object { [System.IO.Path]::GetRelativePath($root, $_.FullName) })
    Write-Output -NoEnumerate $rel
}
