# Module-private: batch gitignore filtering via git itself.
# Reimplementing .gitignore matching (negations, dir-only patterns, subdir
# files) is a bug farm; `git check-ignore --stdin` gives correct semantics
# for free. Best-effort and silent: returns an empty set when git is missing
# or the root is not inside a work tree. Never exported, never in the tool
# manifest, unreachable over RPC.
#
# Cost: exactly ONE git spawn per call, and only when there are candidates
# at all. check-ignore reads from stdin, so there is no argv to keep sane
# and no reason to batch. check-ignore's own exit codes do the work-tree
# detection: 128 means "not a git repository" (or git is broken),
# 1 means "nothing ignored". Any fatal error fails OPEN — no filtering —
# because this is a relevance filter, not a security boundary. (Boundaries
# like Assert-SessionPath fail closed; relevance filters must never invent
# errors the user didn't ask about.)
function Get-GitIgnoredPaths {
    [CmdletBinding()]
    param(
        # Directory to run git in; -Paths are relative to it.
        [Parameter(Mandatory)][string]$Root,
        [string[]]$Paths = @()
    )
    if ($Paths.Count -eq 0) { return [string[]]@() }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { return [string[]]@() }
    # Exit 0: some ignored (stdout lists them verbatim). Exit 1: none
    # ignored — normal. Exit 128: not a work tree or git failed.
    $out = $Paths | git -C $Root check-ignore --stdin 2>$null
    if ($LASTEXITCODE -eq 128) { return [string[]]@() }
    $ignored = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal)
    foreach ($p in $out) { $ignored.Add($p) | Out-Null }
    return [string[]]$ignored
}
