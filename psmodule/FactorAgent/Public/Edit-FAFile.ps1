function Edit-FAFile {
    <#
    .SYNOPSIS
        Replaces one anchored span of text in a file.
    .DESCRIPTION
        Replaces -OldText with -NewText using exact ordinal matching. The
        anchor must occur exactly once: zero matches or more than one match
        is a loud error, so the agent is forced to make the anchor specific.
        Supports ShouldProcess: -WhatIf previews without touching the disk.
    .PARAMETER Path
        File to edit. Relative paths resolve against the session working directory.
        Confined to the session workspace root (FA_SESSION_ROOT): paths
        outside it are rejected.
    .PARAMETER OldText
        Exact text to find. Must occur exactly once in the file.
    .PARAMETER NewText
        Replacement text.
    .EXAMPLE
        Edit-FAFile -Path app.txt -OldText "color: red" -NewText "color: blue"
        Replaces the one occurrence of "color: red".
    .OUTPUTS
        PSCustomObject with Path, Replacements, Preview.
    #>
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$OldText,
        [Parameter(Mandatory)][string]$NewText
    )
    if ([string]::IsNullOrEmpty($OldText)) {
        throw "Edit-FAFile: -OldText must not be empty."
    }
    # Workspace confinement: the session root is a boundary, not a suggestion.
    $full = Assert-SessionPath -Path $Path
    $item = Get-Item -LiteralPath $full -ErrorAction Stop
    if ($item.PSIsContainer) {
        throw "Edit-FAFile: '$Path' is a directory, not a file."
    }
    $text = [System.IO.File]::ReadAllText($item.FullName)
    $count = 0
    $idx = 0
    while (($idx = $text.IndexOf($OldText, $idx, [StringComparison]::Ordinal)) -ge 0) {
        $count++
        $idx += $OldText.Length
    }
    if ($count -eq 0) {
        throw "Edit-FAFile: anchor text not found in '$Path' (0 matches). Quote more context from Read-FAFile."
    }
    if ($count -gt 1) {
        throw "Edit-FAFile: anchor text is ambiguous in '$Path' ($count matches). Make -OldText more specific."
    }
    $preview = "Replace 1 anchored occurrence in '$Path'"
    # Bridge-driven preview (see Write-FAFile): skip ShouldProcess entirely
    # so the engine's unredirectable "What if:" line never hits stdout.
    $previewOnly = [bool]$script:FAForceWhatIf
    if (-not $previewOnly -and $PSCmdlet.ShouldProcess($item.FullName, "Replace 1 anchored occurrence")) {
        [System.IO.File]::WriteAllText($item.FullName, $text.Replace($OldText, $NewText))
    }
    [PSCustomObject]@{
        Path         = $item.FullName
        Replacements = 1
        Preview      = $preview
    }
}
