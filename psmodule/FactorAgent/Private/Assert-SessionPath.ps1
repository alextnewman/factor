# Module-private guard: confines file-cmdlet paths to the session workspace.
# Deliberately NOT named *-FA*: it is never exported, never appears in the
# tool manifest, and the bridge (which only dispatches *-FA* methods) can
# never be steered into invoking it.
function Assert-SessionPath {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Path
    )
    $root = $env:FA_SESSION_ROOT
    if ([string]::IsNullOrWhiteSpace($root)) {
        throw "FA file tools require FA_SESSION_ROOT to be set (session root not configured)."
    }
    # Lexical normalization first (.. segments resolved textually).
    $full = [System.IO.Path]::GetFullPath($Path, (Get-Location).Path)
    # Then resolve symlinks / provider quirks where the target exists.
    try { $full = (Resolve-Path -LiteralPath $full -ErrorAction Stop).Path } catch {}
    $rootFull = try {
        (Resolve-Path -LiteralPath $root -ErrorAction Stop).Path
    } catch {
        [System.IO.Path]::GetFullPath($root)
    }
    $rootWithSep = $rootFull.TrimEnd([System.IO.Path]::DirectorySeparatorChar) +
        [System.IO.Path]::DirectorySeparatorChar
    $inside = $full.Equals($rootFull, [System.StringComparison]::OrdinalIgnoreCase) -or
        $full.StartsWith($rootWithSep, [System.StringComparison]::OrdinalIgnoreCase)
    if (-not $inside) {
        throw "Path '$Path' is outside the session root '$rootFull'. File tools are confined to the session workspace."
    }
    return $full
}
