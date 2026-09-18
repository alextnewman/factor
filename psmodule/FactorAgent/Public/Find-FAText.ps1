function Find-FAText {
    <#
    .SYNOPSIS
        Searches file contents for a pattern.
    .DESCRIPTION
        Searches files under -Path for -Pattern (regex by default; -SimpleMatch
        for literal text). Optionally recurses and filters by file glob.
        Returns one object per matching line with the relative path, line
        number, and the trimmed line.

        Junk directories (build output, VCS metadata, dependency trees) are
        pruned during recursion so a broad search doesn't flood the agent's
        context with matches from artifacts. Override with -Exclude, or pass
        -Exclude @() to disable pruning entirely. Inside a git repo,
        gitignored files are additionally filtered through `git check-ignore`
        (the repo's own notion of noise, with correct ignore semantics)
        unless -IncludeIgnored is given; the hidden count is always reported
        so nothing vanishes silently.
        Hits are capped at -MaxResults and paged with -Skip; overflow
        appends a truncation note with the true total and the hardest-hit
        files so the next call can narrow or page instead of guessing.
    .PARAMETER Pattern
        Regex pattern, or literal text with -SimpleMatch.
    .PARAMETER Path
        Directory to search. Defaults to the session working directory.
        Confined to the session workspace root (FA_SESSION_ROOT): paths
        outside it are rejected.
    .PARAMETER Recurse
        Search subdirectories too.
    .PARAMETER FilePattern
        Glob limiting which files are searched, e.g. "*.md". Default "*".
    .PARAMETER Context
        Lines of context around each match to include in the Context field.
    .PARAMETER SimpleMatch
        Treat -Pattern as literal text instead of regex.
    .PARAMETER Exclude
        Directory names to prune during recursion (case-insensitive, matched
        at any depth). Defaults to a junk list: target, bin, obj, .git, .hg,
        .svn, node_modules, __pycache__, .venv, venv, dist, build, out.
        Pass @() to disable pruning.
    .PARAMETER MaxResults
        Cap on returned hits per page. When the total exceeds the page, the
        final element is a truncation note stating the true total and the
        hardest-hit files. Default 500.
    .PARAMETER Skip
        Skip the first N hits before paging (stateless paging with
        -MaxResults). Default 0.
    .PARAMETER IncludeIgnored
        Include hits from gitignored files. By default, inside a git repo,
        gitignored files are filtered out (and counted in the note).
    .EXAMPLE
        Find-FAText -Pattern "TODO" -Recurse -FilePattern "*.ps1"
        Finds TODO comments in PowerShell files, skipping target/, .git/,
        node_modules/, etc.
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
        [switch]$SimpleMatch,
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

    # Collect candidate files with pruned recursion, then grep. (Excluded
    # directories are pruned, never walked: Get-ChildItem -Exclude filters
    # results but still descends in PS 7.)
    $files = [System.Collections.Generic.List[string]]::new()
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
            elseif ($child.Name -like $FilePattern) {
                $files.Add($child.FullName)
            }
        }
    }

    # Gitignore relevance filter on the candidate files, before grepping:
    # the repo's own noise list, via git itself. Best-effort; skipped
    # outside a work tree or without git.
    $ignoredFileCount = 0
    if (-not $IncludeIgnored -and $files.Count -gt 0) {
        $relFiles = [string[]]($files | ForEach-Object {
            [System.IO.Path]::GetRelativePath($root, $_) })
        $ignored = Get-GitIgnoredPaths -Root $root -Paths $relFiles
        if ($ignored.Count -gt 0) {
            $ignoredSet = [System.Collections.Generic.HashSet[string]]::new(
                [string[]]$ignored, [System.StringComparer]::Ordinal)
            $kept = [System.Collections.Generic.List[string]]::new()
            foreach ($f in $files) {
                if (-not $ignoredSet.Contains(
                        [System.IO.Path]::GetRelativePath($root, $f))) {
                    $kept.Add($f)
                }
            }
            $ignoredFileCount = $files.Count - $kept.Count
            $files = $kept
        }
    }

    # Always emit an array (even for 0 or 1 hits): the JSON-RPC wire shape
    # must not depend on the hit count.
    $hits = [System.Collections.Generic.List[PSCustomObject]]::new()
    $total = 0
    $fileCounts = @{}
    foreach ($f in $files) {
        $matches = Select-String -LiteralPath $f -Pattern $Pattern `
            -SimpleMatch:$SimpleMatch -Context $Context -ErrorAction SilentlyContinue
        foreach ($m in $matches) {
            $total++
            $hitPath = [System.IO.Path]::GetRelativePath($root, $m.Path)
            $fileCounts[$hitPath] = [int]$fileCounts[$hitPath] + 1
            $idx = $total - 1
            if ($idx -ge $Skip -and $hits.Count -lt $MaxResults) {
                $ctx = ''
                if ($Context -gt 0 -and $m.Context) {
                    $ctx = (($m.Context.PreContext + $m.Context.PostContext) -join "`n")
                }
                $hits.Add([PSCustomObject]@{
                    Path       = $hitPath
                    LineNumber = $m.LineNumber
                    Line       = $m.Line.Trim()
                    Context    = $ctx
                })
            }
        }
    }
    $noteParts = @()
    if ($total -eq 0 -and $ignoredFileCount -eq 0) {
        # No hits: no note, just the empty array.
    }
    elseif ($total -eq 0) {
        $noteParts += "no visible hits; $ignoredFileCount files hidden by .gitignore (-IncludeIgnored to search them)"
    }
    else {
        if ($Skip -ge $total) {
            $noteParts += "no more matches: -Skip $Skip is past the $total total hits"
        }
        elseif ($total -gt $Skip + $hits.Count) {
            $from = $Skip + 1
            $to = $Skip + $hits.Count
            $top = $fileCounts.GetEnumerator() | Sort-Object Value -Descending |
                Select-Object -First 5 | ForEach-Object { "$($_.Key) ($($_.Value))" }
            $noteParts += "truncated: showing $from-$to of $total hits; hardest-hit: $($top -join ', ')"
        }
        if ($ignoredFileCount -gt 0) {
            $noteParts += "+$ignoredFileCount files hidden by .gitignore (-IncludeIgnored to search them)"
        }
    }
    if ($noteParts.Count -gt 0) {
        $hits.Add([PSCustomObject]@{
            Path       = '...'
            LineNumber = 0
            Line       = "... ($($noteParts -join '; '); use -Skip/-MaxResults to page, or narrow -Pattern/-FilePattern/-Path)"
            Context    = ''
        })
    }
    Write-Output -NoEnumerate ([PSCustomObject[]]$hits)
}
