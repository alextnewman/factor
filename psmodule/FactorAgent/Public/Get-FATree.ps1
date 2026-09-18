function Get-FATree {
    <#
    .SYNOPSIS
        Shows the shape of a directory tree: a map, not a listing.
    .DESCRIPTION
        The subject catalogue of the file tools. Returns directories with
        recursive file counts plus a bounded sample of files per directory —
        the answer to "what does this look like", structurally incapable of
        listing the ocean. Subtrees below -Depth are folded into counts, and
        output stops at -MaxNodes lines. Junk directories are pruned like
        the find tools, and the visible set matches theirs: gitignored files
        are filtered (and counted) unless -IncludeIgnored is given.
        Pair with Find-FAFile -Path <dir> once a region is chosen.
    .PARAMETER Path
        Directory to map. Defaults to the session working directory.
        Confined to the session workspace root (FA_SESSION_ROOT): paths
        outside it are rejected.
    .PARAMETER Depth
        Directory levels to expand (1-10). Deeper subtrees are folded into
        their parent's count. Default 3.
    .PARAMETER MaxNodes
        Cap on emitted tree lines (1-2000). Default 200.
    .PARAMETER Exclude
        Directory names to prune during the walk (case-insensitive, matched
        at any depth). Defaults to a junk list: target, bin, obj, .git, .hg,
        .svn, node_modules, __pycache__, .venv, venv, dist, build, out.
        Pass @() to disable pruning.
    .PARAMETER IncludeIgnored
        Include files hidden by .gitignore. By default they are filtered
        out (and counted in the trailing note).
    .EXAMPLE
        Get-FATree -Path V:\Sources\factor -Depth 2
        Maps the top two directory levels of the repo with file counts.
    .EXAMPLE
        Get-FATree -Path .\src -MaxNodes 50
        Compact map of one subtree, at most 50 lines.
    .OUTPUTS
        String[]. Indented tree lines; directories carry recursive file
        counts, folded subtrees and caps are reported in trailing notes.
    #>
    # .PRINTFORM: Tree {Path}
    [CmdletBinding()]
    param(
        [string]$Path = (Get-Location).Path,
        [ValidateRange(1, 10)][int]$Depth = 3,
        [ValidateRange(1, 2000)][int]$MaxNodes = 200,
        [string[]]$Exclude = @('target', 'bin', 'obj', '.git', '.hg', '.svn',
            'node_modules', '__pycache__', '.venv', 'venv', 'dist', 'build', 'out'),
        [switch]$IncludeIgnored
    )
    # Workspace confinement: the session root is a boundary, not a suggestion.
    $root = Assert-SessionPath -Path $Path
    $sep = [System.IO.Path]::DirectorySeparatorChar
    $full = Get-FileCandidate -Root $root -FilePattern '*' -Recurse -Exclude $Exclude
    $rel = [string[]]@($full | ForEach-Object {
        [System.IO.Path]::GetRelativePath($root, $_) })

    # Same visible set as the find tools: gitignore is a relevance filter.
    $hiddenCount = 0
    if (-not $IncludeIgnored -and $rel.Count -gt 0) {
        $ignored = Get-GitIgnoredPaths -Root $root -Paths $rel
        if ($ignored.Count -gt 0) {
            $iset = [System.Collections.Generic.HashSet[string]]::new(
                [string[]]$ignored, [System.StringComparer]::Ordinal)
            $rel = [string[]]@($rel | Where-Object { -not $iset.Contains($_) })
            $hiddenCount = $full.Count - $rel.Count
        }
    }

    # Build the nested catalogue from flat relative paths.
    $newNode = {
        @{ Files = [System.Collections.Generic.List[string]]::new()
           Dirs  = [ordered]@{} }
    }
    $treeRoot = & $newNode
    foreach ($p in ($rel | Sort-Object)) {
        $parts = $p -split [regex]::Escape($sep)
        $node = $treeRoot
        for ($i = 0; $i -lt $parts.Count - 1; $i++) {
            $d = $parts[$i]
            if (-not $node.Dirs.Contains($d)) { $node.Dirs[$d] = & $newNode }
            $node = $node.Dirs[$d]
        }
        $node.Files.Add($parts[-1])
    }

    # Recursive helpers as scriptblocks (the lint contract allows exactly
    # one *function* per Public file; scriptblocks are not functions).
    $countFiles = {
        param([hashtable]$Node)
        $n = $Node.Files.Count
        foreach ($c in $Node.Dirs.Values) { $n += & $countFiles $c }
        return $n
    }
    $ctx = @{
        Lines       = [System.Collections.Generic.List[string]]::new()
        MaxNodes    = $MaxNodes
        Depth       = $Depth
        Folded      = 0
        Capped      = $false
        FilesPerDir = 20
    }
    $emit = {
        param([string]$Line, [hashtable]$Ctx)
        if ($Ctx.Lines.Count -ge $Ctx.MaxNodes) { $Ctx.Capped = $true; return $false }
        $Ctx.Lines.Add($Line)
        return $true
    }
    $render = {
        param([hashtable]$Node, [string]$Indent, [int]$DepthNow, [hashtable]$Ctx)
        foreach ($d in ($Node.Dirs.Keys | Sort-Object)) {
            $child = $Node.Dirs[$d]
            $count = & $countFiles $child
            if (-not (& $emit "$Indent$d/ - $count files" $Ctx)) { return }
            if ($DepthNow + 1 -ge $Ctx.Depth) { $Ctx.Folded++; continue }
            & $render $child ($Indent + '  ') ($DepthNow + 1) $Ctx
            if ($Ctx.Capped) { return }
        }
        $files = @($Node.Files | Sort-Object)
        $shown = [Math]::Min($Ctx.FilesPerDir, $files.Count)
        for ($i = 0; $i -lt $shown; $i++) {
            if (-not (& $emit "$Indent$($files[$i])" $Ctx)) { return }
        }
        if ($files.Count -gt $shown -and -not $Ctx.Capped) {
            & $emit "$Indent... (+$($files.Count - $shown) more)" $Ctx | Out-Null
        }
    }

    $totalFiles = & $countFiles $treeRoot
    $lines = $ctx.Lines
    $lines.Add("Tree: $root ($totalFiles files)")
    if ($totalFiles -eq 0) {
        $lines.Add('(empty)')
    }
    else {
        & $render $treeRoot '' 0 $ctx
    }
    if ($ctx.Capped) {
        $lines.Add("... (node cap $MaxNodes reached; narrow -Path)")
    }
    if ($ctx.Folded -gt 0) {
        $lines.Add("... (+$($ctx.Folded) dirs folded below depth $Depth; use -Depth to expand)")
    }
    if ($hiddenCount -gt 0) {
        $lines.Add("... (+$hiddenCount hidden by .gitignore (-IncludeIgnored to show))")
    }
    # Always emit an array on the wire, even for an empty tree.
    Write-Output -NoEnumerate ([string[]]$lines)
}
