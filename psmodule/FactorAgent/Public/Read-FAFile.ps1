function Read-FAFile {
    <#
    .SYNOPSIS
        Reads a text file (M0 spike implementation).
    .DESCRIPTION
        M0 stub: reads the whole file as text. M1 will add encoding,
        line ranges, and structured errors per the §4.9 help/lint contract.
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )
    $item = Get-Item -LiteralPath $Path -ErrorAction Stop
    if ($item.PSIsContainer) {
        throw "Read-FAFile: '$Path' is a directory, not a file."
    }
    [System.IO.File]::ReadAllText($item.FullName)
}
