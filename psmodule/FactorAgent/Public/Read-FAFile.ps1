function Read-FAFile {
    <#
    .SYNOPSIS
        Reads a text file and returns numbered lines.
    .DESCRIPTION
        Returns the file's lines prefixed with 1-based line numbers in the
        form "12: text". Line numbers always reflect the original file, even
        when -Lines or -Tail selects a window. Use -Lines for the first N
        lines or -Tail for the last N lines; the two are mutually exclusive.
    .PARAMETER Path
        File to read. Relative paths resolve against the session working directory.
        Confined to the session workspace root (FA_SESSION_ROOT): paths
        outside it are rejected.
    .PARAMETER Lines
        Return only the first N lines.
    .PARAMETER Tail
        Return only the last N lines.
    .EXAMPLE
        Read-FAFile -Path README.md -Lines 40
        Reads the first 40 numbered lines of README.md.
    .OUTPUTS
        String[]. Each element is "<line-number>: <content>".
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Path,
        [int]$Lines = 0,
        [int]$Tail = 0
    )
    if ($Lines -gt 0 -and $Tail -gt 0) {
        throw "Read-FAFile: -Lines and -Tail are mutually exclusive."
    }
    # Workspace confinement: the session root is a boundary, not a suggestion.
    $full = Assert-SessionPath -Path $Path
    $item = Get-Item -LiteralPath $full -ErrorAction Stop
    if ($item.PSIsContainer) {
        throw "Read-FAFile: '$Path' is a directory, not a file."
    }
    $all = [System.IO.File]::ReadAllLines($item.FullName)
    $total = $all.Count
    $from = 0
    $to = $total - 1
    if ($Lines -gt 0) { $to = [Math]::Min($Lines, $total) - 1 }
    elseif ($Tail -gt 0) { $from = [Math]::Max(0, $total - $Tail) }
    # Always an array on the wire, even for a one-line file.
    $numbered = @(for ($i = $from; $i -le $to; $i++) {
        "{0}: {1}" -f ($i + 1), $all[$i]
    })
    Write-Output -NoEnumerate $numbered
}
