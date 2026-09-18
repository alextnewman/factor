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
        Inside a git repo, gitignored matches are additionally filtered
        through `git check-ignore` (the repo's own notion of noise, with
        correct ignore semantics) unless -IncludeIgnored is given; the hidden
        count is always reported so nothing vanishes silently.
        Results are capped at -MaxResults and paged with -Skip; overflow
        appends a truncation note with the true total and a per-subtree
        breakdown so the next call can narrow or page instead of guessing.
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
        Cap on returned paths per page. When the total exceeds the page, the
        final element is a truncation note stating the true total and the
        largest subtrees. Default 500.
    .PARAMETER Skip
        Skip the first N matches before paging (stateless paging with
        -MaxResults). Default 0.
    .PARAMETER IncludeIgnored
        Include matches hidden by .gitignore. By default, inside a git repo,
        gitignored matches are filtered out (and counted in the note).
    .EXAMPLE
        Find-FAFile -Pattern "demo-*" -Recurse
        Finds all files starting with demo- under the working directory,
        skipping target/, .git/, node_modules/, etc.
    .EXAMPLE
        Find-FAFile -Pattern "*.log" -Recurse -Skip 500 -MaxResults 500
        Second page of the recursive log listing.
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
        [ValidateRange(1, 100000)][int]$MaxResults = 500,
        [ValidateRange(0, 100000)][int]$Skip = 0,
        [switch]$IncludeIgnored
    )
    # Workspace confinement: the session root is a boundary, not a suggestion.
    $root = Assert-SessionPath -Path $Path
    $excludeSet = [System.Collections.Generic.HashSet[string]]::new(
        [string[]]@($Exclude | ForEach-Object { $_.ToLowerInvariant() }),
        [System.StringComparer]::OrdinalIgnoreCase)

    # Collect all matches first: gitignore filtering needs the full set
    # before paging, and the memory cost (paths only) is trivial next to
    # the context cost the page cap protects.
    $all = [System.Collections.Generic.List[string]]::new()
    $sep = [System.IO.Path]::DirectorySeparatorChar
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
                $all.Add([System.IO.Path]::GetRelativePath($root, $child.FullName))
            }
        }
    }

    # Gitignore relevance filter: the repo's own noise list, via git itself.
    $ignoredCount = 0
    $visible = $all
    if (-not $IncludeIgnored -and $all.Count -gt 0) {
        $ignored = Get-GitIgnoredPaths -Root $root -Paths ([string[]]$all)
        if ($ignored.Count -gt 0) {
            $ignoredSet = [System.Collections.Generic.HashSet[string]]::new(
                [string[]]$ignored, [System.StringComparer]::Ordinal)
            $visible = [System.Collections.Generic.List[string]]::new()
            foreach ($m in $all) {
                if (-not $ignoredSet.Contains($m)) { $visible.Add($m) }
            }
            $ignoredCount = $all.Count - $visible.Count
        }
    }

    # Page the visible set; the subtree breakdown is a map over what the
    # agent can actually see.
    $rel = [System.Collections.Generic.List[string]]::new()
    $dirCounts = @{}
    $total = $visible.Count
    for ($i = 0; $i -lt $total; $i++) {
        $relPath = $visible[$i]
        $key = if ($relPath.Contains($sep)) {
            ($relPath -split [regex]::Escape($sep))[0] + $sep
        } else { '.' }
        $dirCounts[$key] = [int]$dirCounts[$key] + 1
        if ($i -ge $Skip -and $rel.Count -lt $MaxResults) {
            $rel.Add($relPath)
        }
    }

    $noteParts = @()
    if ($total -eq 0 -and $ignoredCount -eq 0) {
        # No matches at all: no note, just the empty array.
    }
    elseif ($total -eq 0) {
        $noteParts += "no visible matches; $ignoredCount hidden by .gitignore (-IncludeIgnored to show)"
    }
    else {
        if ($Skip -ge $total) {
            $noteParts += "no more matches: -Skip $Skip is past the $total total matches"
        }
        elseif ($total -gt $Skip + $rel.Count) {
            $from = $Skip + 1
            $to = $Skip + $rel.Count
            $top = $dirCounts.GetEnumerator() | Sort-Object Value -Descending |
                Select-Object -First 8 | ForEach-Object { "$($_.Key) ($($_.Value))" }
            $noteParts += "truncated: showing $from-$to of $total matches; largest: $($top -join ', ')"
        }
        if ($ignoredCount -gt 0) {
            $noteParts += "+$ignoredCount hidden by .gitignore (-IncludeIgnored to show)"
        }
    }
    if ($noteParts.Count -gt 0) {
        $rel.Add("... ($($noteParts -join '; '); use -Skip/-MaxResults to page, or narrow -Pattern/-Path)")
    }
    # Always emit an array on the wire, even for 0 or 1 matches.
    Write-Output -NoEnumerate ([string[]]$rel)
}
