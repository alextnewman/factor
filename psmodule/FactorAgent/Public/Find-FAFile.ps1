function Find-FAFile {
    <#
    .SYNOPSIS
        Finds files by glob pattern: a title lookup in the file catalogue.
    .DESCRIPTION
        Searches for files matching a glob pattern (e.g. "*.ps1", "demo-*").
        Returns paths relative to -Path. Add -Recurse to search subdirectories.

        The catalogue contract: small result sets return the matching paths
        themselves; large ones return an INDEX — directory cards with counts,
        sample paths, and the narrower -Path that opens each drawer — instead
        of page 1 of the ocean. Navigation is always a new lookup, never a
        page turn; -Skip is the raw-paging escape hatch.

        Junk directories (build output, VCS metadata, dependency trees) are
        pruned during recursion — a traversal-performance concern owned by
        the shared walker, not a per-tool judgment. Override with -Exclude,
        or pass -Exclude @() to disable pruning entirely. Inside a git repo,
        gitignored matches are additionally filtered through
        `git check-ignore` (one spawn; the repo's own notion of noise, with
        correct ignore semantics) unless -IncludeIgnored is given; the
        hidden count is always reported so nothing vanishes silently.
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
        Result-set size at which the catalogue flips from items to the
        index view. Default 500. (Explicitly passing -Skip always opts
        into raw paging instead.)
    .PARAMETER Skip
        Raw-paging escape hatch: skip the first N matches and return the
        next -MaxResults as plain paths, bypassing the index. Explicitly
        passing -Skip (even 0) opts out of the catalogue view for this
        call. Default 0.
    .PARAMETER IncludeIgnored
        Include matches hidden by .gitignore. By default, inside a git repo,
        gitignored matches are filtered out (and counted in the note).
    .EXAMPLE
        Find-FAFile -Pattern "demo-*" -Recurse
        Finds all files starting with demo- under the working directory,
        skipping target/, .git/, node_modules/, etc.
    .EXAMPLE
        Find-FAFile -Pattern "*.ps1" -Recurse -Path .\src
        Opens the src drawer of the catalogue for PowerShell files.
    .OUTPUTS
        String[]. Relative file paths — or, for large result sets, index
        cards naming the directory drawers that hold them.
    #>
    # .PRINTFORM: Find {Pattern}
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

    # One shared traversal; deterministic order for stable cards and pages.
    $full = Get-FileCandidate -Root $root -FilePattern $Pattern `
        -Recurse:$Recurse -Exclude $Exclude
    $all = [string[]]@($full | ForEach-Object {
        [System.IO.Path]::GetRelativePath($root, $_) } | Sort-Object)

    # Gitignore relevance filter: the repo's own noise list, via git itself.
    # Best-effort and fail-open; the hidden count keeps it honest.
    $hiddenNote = ''
    $visible = $all
    if (-not $IncludeIgnored -and $all.Count -gt 0) {
        $ignored = Get-GitIgnoredPaths -Root $root -Paths $all
        if ($ignored.Count -gt 0) {
            $ignoredSet = [System.Collections.Generic.HashSet[string]]::new(
                [string[]]$ignored, [System.StringComparer]::Ordinal)
            $visible = [string[]]@($all | Where-Object { -not $ignoredSet.Contains($_) })
            $hiddenNote = "+$($all.Count - $visible.Count) hidden by .gitignore (-IncludeIgnored to show)"
        }
    }

    $view = Format-ResultView -Items $visible -Kind File -Skip $Skip `
        -MaxResults $MaxResults -Unit 'matches' -HiddenNote $hiddenNote `
        -Guidance 'Get-FATree shows the shape; open a drawer with its -Path' `
        -RawPage:$($PSBoundParameters.ContainsKey('Skip'))

    # The display layer owns the wire shape; the tool only casts it.
    # Always emit an array on the wire, even for 0 or 1 matches.
    Write-Output -NoEnumerate ([string[]]$view.Lines)
}
