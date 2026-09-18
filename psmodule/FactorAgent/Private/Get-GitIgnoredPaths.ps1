# Module-private: batch gitignore filtering via git itself.
# Reimplementing .gitignore matching (negations, dir-only patterns, subdir
# files) is a bug farm; `git check-ignore --stdin` gives correct semantics
# for free. Best-effort and silent: returns an empty set when git is missing
# or the root is not inside a work tree. Never exported, never in the tool
# manifest, unreachable over RPC.
function Get-GitIgnoredPaths {
    [CmdletBinding()]
    param(
        # Directory to run git in; -Paths are relative to it.
        [Parameter(Mandatory)][string]$Root,
        [string[]]$Paths = @()
    )
    if ($Paths.Count -eq 0) { return [string[]]@() }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { return [string[]]@() }
    $inside = git -C $Root rev-parse --is-inside-work-tree 2>$null
    if ($LASTEXITCODE -ne 0 -or $inside -ne 'true') { return [string[]]@() }
    # check-ignore echoes the ignored inputs verbatim; exit code 1 when none
    # match is fine (we only read stdout). Batched to keep argv sane.
    $ignored = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal)
    $batchSize = 500
    for ($i = 0; $i -lt $Paths.Count; $i += $batchSize) {
        $end = [Math]::Min($i + $batchSize - 1, $Paths.Count - 1)
        $out = $Paths[$i..$end] | git -C $Root check-ignore --stdin 2>$null
        foreach ($p in $out) { $ignored.Add($p) | Out-Null }
    }
    return [string[]]$ignored
}
