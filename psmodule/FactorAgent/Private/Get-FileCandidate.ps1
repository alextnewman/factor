# Module-private: the single traversal behind every file-discovery tool.
# Pruned stack recursion yielding FULL paths of files whose names match
# -FilePattern. Never exported, never in the tool manifest, unreachable
# over RPC.
#
# The junk-directory list lives HERE and only here. It is a
# traversal-performance concern (never descend into build trees, VCS
# metadata, or dependency forests), not a relevance judgment — relevance
# is the repo's own .gitignore, applied by callers through
# Get-GitIgnoredPaths. One list, one place, one documented reason.
function Get-FileCandidate {
    [CmdletBinding()]
    param(
        # Already session-asserted full directory path to walk.
        [Parameter(Mandatory)][string]$Root,
        [string]$FilePattern = '*',
        [switch]$Recurse,
        [string[]]$Exclude = @('target', 'bin', 'obj', '.git', '.hg', '.svn',
            'node_modules', '__pycache__', '.venv', 'venv', 'dist', 'build', 'out')
    )
    $excludeSet = [System.Collections.Generic.HashSet[string]]::new(
        [string[]]@($Exclude | ForEach-Object { $_.ToLowerInvariant() }),
        [System.StringComparer]::OrdinalIgnoreCase)
    $out = [System.Collections.Generic.List[string]]::new()
    # Manual stack recursion: excluded directories are pruned, never walked.
    # (Get-ChildItem -Exclude filters results but still descends in PS 7.)
    $stack = [System.Collections.Generic.Stack[string]]::new()
    $stack.Push($Root)
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
                $out.Add($child.FullName)
            }
        }
    }
    return [string[]]$out
}
