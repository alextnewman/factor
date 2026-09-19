function Find-FAText {
    <#
    .SYNOPSIS
        Searches file contents for a pattern: a concordance lookup.
    .DESCRIPTION
        Searches files under -Path for -Pattern (regex by default; -SimpleMatch
        for literal text). Optionally recurses and filters by file glob.
        Returns one object per matching line with the relative path, line
        number, and the trimmed line.

        The catalogue contract: small result sets return the hits themselves;
        large ones return an INDEX — file cards with hit counts, a sample
        line, and the narrower -Path/-FilePattern that opens each drawer —
        instead of page 1 of the ocean. Navigation is always a new lookup,
        never a page turn; -Skip is the raw-paging escape hatch.

        Junk directories (build output, VCS metadata, dependency trees) are
        pruned during recursion — a traversal-performance concern owned by
        the shared walker, not a per-tool judgment. Override with -Exclude,
        or pass -Exclude @() to disable pruning entirely. Inside a git repo,
        gitignored files are additionally filtered through `git check-ignore`
        (one spawn; the repo's own notion of noise, with correct ignore
        semantics) unless -IncludeIgnored is given; the hidden count is
        always reported so nothing vanishes silently.
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
        Result-set size at which the catalogue flips from hits to the
        index view. Default 500. (Explicitly passing -Skip always opts
        into raw paging instead.)
    .PARAMETER Skip
        Raw-paging escape hatch: skip the first N hits and return the
        next -MaxResults as plain hit objects, bypassing the index.
        Explicitly passing -Skip (even 0) opts out of the catalogue view
        for this call. Default 0.
    .PARAMETER IncludeIgnored
        Include hits from gitignored files. By default, inside a git repo,
        gitignored files are filtered out (and counted in the note).
    .EXAMPLE
        Find-FAText -Pattern "TODO" -Recurse -FilePattern "*.ps1"
        Finds TODO comments in PowerShell files, skipping target/, .git/,
        node_modules/, etc.
    .EXAMPLE
        Find-FAText -Pattern "TODO" -Recurse -Path .\src -FilePattern "App.ps1"
        Opens one drawer of the concordance: TODOs in a single file.
    .OUTPUTS
        PSCustomObject[] with Path, LineNumber, Line, Context — or, for
        large result sets, index cards (LineNumber 0) naming the files
        that hold the hits.
    #>
    # .PRINTFORM: Find text {Pattern}
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

    # One shared traversal; deterministic order for stable cards and pages.
    $files = [string[]]@(Get-FileCandidate -Root $root -FilePattern $FilePattern `
        -Recurse:$Recurse -Exclude $Exclude | Sort-Object)

    # Gitignore relevance filter on the candidate files, before grepping:
    # the repo's own noise list, via git itself. Best-effort and fail-open.
    $hiddenNote = ''
    if (-not $IncludeIgnored -and $files.Count -gt 0) {
        $relFiles = [string[]]@($files | ForEach-Object {
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
            $hiddenNote = "+$($files.Count - $kept.Count) files hidden by .gitignore (-IncludeIgnored to search them)"
            $files = [string[]]$kept
        }
    }

    $hits = [System.Collections.Generic.List[PSCustomObject]]::new()
    foreach ($f in $files) {
        $matches = Select-String -LiteralPath $f -Pattern $Pattern `
            -SimpleMatch:$SimpleMatch -Context $Context -ErrorAction SilentlyContinue
        foreach ($m in $matches) {
            $ctx = ''
            if ($Context -gt 0 -and $m.Context) {
                $ctx = (($m.Context.PreContext + $m.Context.PostContext) -join "`n")
            }
            $hits.Add([PSCustomObject]@{
                Path       = [System.IO.Path]::GetRelativePath($root, $m.Path)
                LineNumber = $m.LineNumber
                Line       = $m.Line.Trim()
                Context    = $ctx
            })
        }
    }

    $view = Format-ResultView -Items ([array]$hits) -Kind Text -Skip $Skip `
        -MaxResults $MaxResults -Unit 'hits' -HiddenNote $hiddenNote `
        -Guidance 'open a drawer with its -Path/-FilePattern' `
        -RawPage:$($PSBoundParameters.ContainsKey('Skip'))

    # The display layer owns the wire shape; the tool only casts it.
    # Always emit an array (even for 0 or 1 hits): the JSON-RPC wire shape
    # must not depend on the hit count.
    Write-Output -NoEnumerate ([PSCustomObject[]]$view.Lines)
}
