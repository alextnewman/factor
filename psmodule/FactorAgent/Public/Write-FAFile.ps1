function Write-FAFile {
    <#
    .SYNOPSIS
        Writes text to a file, creating parent directories as needed.
    .DESCRIPTION
        Writes content as UTF-8 without BOM (default). Missing parent
        directories are created automatically. With -Append the content is
        appended instead of replacing the file. Supports ShouldProcess:
        -WhatIf previews the write without touching the disk.
    .PARAMETER Path
        Destination file path. Relative paths resolve against the session
        working directory. Confined to the session workspace root
        (FA_SESSION_ROOT): paths outside it are rejected.
    .PARAMETER Content
        The text to write.
    .PARAMETER Append
        Append to the file instead of overwriting it.
    .PARAMETER Encoding
        Text encoding. One of utf8NoBOM (default), utf8, ascii.
    .EXAMPLE
        Write-FAFile -Path notes/todo.txt -Content "buy milk"
        Writes (or replaces) notes/todo.txt.
    .EXAMPLE
        Write-FAFile -Path log.txt -Content "done`n" -Append
        Appends a line to log.txt.
    .OUTPUTS
        PSCustomObject with Path, BytesWritten, Created, Preview.
    #>
    # .PRINTFORM: Write {Path}
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Content,
        [switch]$Append,
        [ValidateSet('utf8NoBOM', 'utf8', 'ascii')][string]$Encoding = 'utf8NoBOM'
    )
    $enc = switch ($Encoding) {
        'utf8NoBOM' { [System.Text.UTF8Encoding]::new($false) }
        'utf8'      { [System.Text.UTF8Encoding]::new($true) }
        'ascii'     { [System.Text.Encoding]::ASCII }
    }
    $bytes = $enc.GetBytes($Content)
    $action = if ($Append) { "Append $($bytes.Count) bytes" } else { "Write $($bytes.Count) bytes" }
    # Workspace confinement: the session root is a boundary, not a suggestion.
    # Assert before ShouldProcess so even -WhatIf previews can't leak paths.
    $full = Assert-SessionPath -Path $Path
    $preview = "$action to '$full'"
    $created = -not (Test-Path -LiteralPath $Path -PathType Leaf)
    # $script:FAForceWhatIf is set by bridge.ps1 for _WhatIf calls. It
    # short-circuits BEFORE ShouldProcess so the engine never prints its
    # "What if:" line: the console host writes that straight to stdout,
    # bypassing every stream, which would corrupt the JSON-RPC framing.
    # Human -WhatIf/-Confirm still flow through ShouldProcess untouched.
    $previewOnly = [bool]$script:FAForceWhatIf
    if (-not $previewOnly -and $PSCmdlet.ShouldProcess($Path, $action)) {
        $dir = [System.IO.Path]::GetDirectoryName([System.IO.Path]::GetFullPath($Path))
        if ($dir -and -not (Test-Path -LiteralPath $dir)) {
            New-Item -ItemType Directory -Path $dir -Force | Out-Null
        }
        if ($Append -and (Test-Path -LiteralPath $Path -PathType Leaf)) {
            [System.IO.File]::AppendAllText($Path, $Content, $enc)
        }
        else {
            [System.IO.File]::WriteAllText($Path, $Content, $enc)
        }
    }
    [PSCustomObject]@{
        Path         = $Path
        BytesWritten = $bytes.Count
        Created      = $created
        Preview      = $preview
    }
}
