function Find-FAFile {
    <#
    .SYNOPSIS
        Finds files by glob pattern.
    .DESCRIPTION
        Searches for files matching a glob pattern (e.g. "*.ps1", "demo-*").
        Returns paths relative to -Path. Add -Recurse to search subdirectories.

        Junk directories (build output, VCS metadata, dependency trees) are
        pruned during recursion so a bare recursive listing doesn't flood the
        agent's context with tens of thousands of artifact paths. Override
        with -Exclude, or pass -Exclude @() to disable pruning entirely.
        Results are capped at -MaxResults; overflow appends a truncation note
        instead of silently dropping matches.
    .PARAMETER Pattern
        Glob pattern, e.g. "*.md".
    .PARAMETER Path
        Directory to search. Defaults to the session working directory.
        Confined to the session workspace root (FA_SESSION_ROOT): paths
        outside it are rejected. The root itself is always searchable, even
        if its name appears in -Exclude.
    .PARAMETER Recurse
        Search subdirectories too.
    .PARAMETER Exclude
        Directory names to prune during recursion (case-insensitive, matched
        at any depth). Defaults to a junk list: target, bin, obj, .git, .hg,
        .svn, node_modules, __pycache__, .venv, venv, dist, build, out.
        Pass @() to disable pruning.
    .PARAMETER MaxResults
        Cap on returned paths. When exceeded, the final element is a
        truncation note stating the true total. Default 500.
    .EXAMPLE
        Find-FAFile -Pattern "demo-*" -Recurse
        Finds all files starting with demo- under the working directory,
        skipping target/, .git/, node_modules/, etc.
    .OUTPUTS
        String[]. Relative file paths, one per line.
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Pattern,
        [string]$Path = (Get-Location).Path,
        [switch]$Recurse,
        [string[]]$Exclude = @('target', 'bin', 'obj', '.git', '.hg', '.svn',
            'node_modules', '__pycache__', '.venv', 'venv', 'dist', 'build', 'out'),
        [ValidateRange(1, 100000)][int]$MaxResults = 500
    )
    # Workspace confinement: the session root is a boundary, not a suggestion.
    $root = Assert-SessionPath -Path $Path
    $excludeSet = [System.Collections.Generic.HashSet[string]]::new(
        [string[]]@($Exclude | ForEach-Object { $_.ToLowerInvariant() }),
        [System.StringComparer]::OrdinalIgnoreCase)

    $rel = [System.Collections.Generic.List[string]]::new()
    $total = 0
    # Manual stack recursion: excluded directories are pruned, never walked.
    # (Get-ChildItem -Exclude filters results but still descends in PS 7.)
    $stack = [System.Collections.Generic.Stack[string]]::new()
    $stack.Push($root)
    while ($stack.Count -gt 0) {
        $dir = $stack.Pop()
        $children = Get-ChildItem -LiteralPath $dir -ErrorAction SilentlyContinue
        foreach ($child in $children) {
            if ($child.PSIsContainer) {
                if ($Recurse -and -not $excludeSet.Contains($child.Name)) {
                    $stack.Push($child.FullName)
                }
            }
            elseif ($child.Name -like $Pattern) {
                $total++
                if ($rel.Count -lt $MaxResults) {
                    $rel.Add([System.IO.Path]::GetRelativePath($root, $child.FullName))
                }
            }
        }
    }
    if ($total -gt $MaxResults) {
        $rel.Add("... (truncated: showing first $MaxResults of $total matches; narrow -Pattern or -Path)")
    }
    # Always emit an array on the wire, even for 0 or 1 matches.
    Write-Output -NoEnumerate ([string[]]$rel)
}
