function Read-FAFile {
    <#
    .SYNOPSIS
        Reads a text file and returns numbered lines.
    .DESCRIPTION
        Returns the file's lines prefixed with 1-based line numbers in the
        form "12: text". Line numbers always reflect the original file, even
        when paging. Use -Lines for a window size and -Offset to page through
        long files; -Tail takes the last N lines. With no paging arguments,
        the first 2000 lines are returned with a truncation note when the
        file is longer, so a stray read of a huge file can't flood the
        agent's context.
    .PARAMETER Path
        File to read. Relative paths resolve against the session working directory.
        Confined to the session workspace root (FA_SESSION_ROOT): paths
        outside it are rejected.
    .PARAMETER Lines
        Window size: return N lines starting at -Offset (default 0).
    .PARAMETER Offset
        Skip the first N lines before the window (stateless paging with
        -Lines). Default 0.
    .PARAMETER Tail
        Return only the last N lines. Mutually exclusive with -Lines/-Offset.
    .EXAMPLE
        Read-FAFile -Path README.md -Lines 40
        Reads the first 40 numbered lines of README.md.
    .EXAMPLE
        Read-FAFile -Path big.log -Lines 100 -Offset 200
        Reads lines 201-300 of big.log.
    .OUTPUTS
        String[]. Each element is "<line-number>: <content>".
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Path,
        [int]$Lines = 0,
        [int]$Tail = 0,
        [ValidateRange(0, 10000000)][int]$Offset = 0
    )
    if ($Tail -gt 0 -and ($Lines -gt 0 -or $Offset -gt 0)) {
        throw "Read-FAFile: -Tail is mutually exclusive with -Lines/-Offset."
    }
    # Workspace confinement: the session root is a boundary, not a suggestion.
    $full = Assert-SessionPath -Path $Path
    $item = Get-Item -LiteralPath $full -ErrorAction Stop
    if ($item.PSIsContainer) {
        throw "Read-FAFile: '$Path' is a directory, not a file."
    }
    $all = [System.IO.File]::ReadAllLines($item.FullName)
    $total = $all.Count
    $note = $null
    $explicitPage = $PSBoundParameters.ContainsKey('Lines') -or
        $PSBoundParameters.ContainsKey('Offset')
    if ($Tail -gt 0) {
        $from = [Math]::Max(0, $total - $Tail)
        $to = $total - 1
    }
    else {
        $window = if ($Lines -gt 0) { $Lines } else { 2000 }
        if ($Offset -ge $total -and $total -gt 0) {
            $note = "... (offset $Offset is past the end of the file: $total lines total)"
            $from = $total
            $to = $total - 1
        }
        else {
            $from = [Math]::Min($Offset, $total)
            $to = [Math]::Min($from + $window, $total) - 1
            # The note is the safety net for the implicit default window.
            # Explicit paging (-Lines/-Offset) needs no narration.
            if (-not $explicitPage -and $from + $window -lt $total) {
                $note = "... (showing lines $($from + 1)-$($to + 1) of $total; " +
                    "use -Offset/-Lines to page)"
            }
        }
    }
    # Always an array on the wire, even for a one-line file.
    $numbered = @(for ($i = $from; $i -le $to; $i++) {
        "{0}: {1}" -f ($i + 1), $all[$i]
    })
    if ($note) { $numbered += $note }
    Write-Output -NoEnumerate ([string[]]$numbered)
}
