# Module-private: the display layer for collection-returning tools.
# Never exported, never in the tool manifest, unreachable over RPC.
#
# PowerShell separates what a tool does (emit objects) from how it is
# displayed (the formatting subsystem). This is the model-legible half of
# that split: a text-adventure renderer. Tools collect and filter; this
# function owns the entire display grammar — grouping, cards, call numbers,
# notes, and the wire shape per Kind. Nothing about presentation lives in
# the tools themselves.
#
# The catalogue contract it renders:
#   * Small result sets (<= -MaxResults) return ITEMS — the cards are the
#     things themselves. No narration when the answer fits.
#   * Large result sets return an INDEX — grouped cards (key, count,
#     samples, call number) instead of page 1 of the ocean. Each card is a
#     room description with its exits: enough to decide whether to open
#     that drawer, and the drawer opens with a new lookup, never a page.
# No cursors, no sessions: every view is self-contained. -Skip is the raw-
# paging escape hatch — explicitly passing it (even -Skip 0) yields items
# and never the index — for the rare caller that wants the raw list rather
# than the catalogue.
function Format-ResultView {
    [CmdletBinding()]
    param(
        # Post-filter items, in the tool's display order. Empty is fine:
        # zero items is the Empty view, not an error.
        [array]$Items = @(),
        # 'File': items are relative path strings; wire shape is String[].
        # 'Text': items are hit objects (Path/LineNumber/Line/Context);
        # wire shape is PSCustomObject[] with the same fields.
        [ValidateSet('File', 'Text')][string]$Kind = 'File',
        [int]$Skip = 0,
        [int]$MaxResults = 500,
        [string]$Unit = 'matches',
        # Raw-paging request: the caller explicitly passed -Skip (even 0),
        # opting out of the index for this call. The tools set this from
        # $PSBoundParameters so the default view stays the catalogue.
        [switch]$RawPage,
        # Pre-rendered hidden-count clause, e.g.
        # "+3 hidden by .gitignore (-IncludeIgnored to show)". Empty = none.
        [string]$HiddenNote = '',
        # Tool-specific onward guidance, appended when the view is an index
        # or a raw page (never when the full answer fit).
        [string]$Guidance = '',
        [int]$MaxCards = 12,
        [int]$SamplesPerCard = 3
    )
    $sep = [System.IO.Path]::DirectorySeparatorChar
    $total = $Items.Count
    $view = @{
        Mode           = 'Empty'
        Page           = @()
        Cards          = @()
        Total          = $total
        OverflowGroups = 0
        Note           = ''
        Lines          = @()   # rendered wire output
    }

    # Note assembly: window/index clause, then hidden count, then guidance.
    $noteOf = {
        param($Head, $Hidden, $Guide)
        $parts = @()
        if ($Head -ne '') { $parts += $Head }
        if ($Hidden -ne '') { $parts += $Hidden }
        if ($Guide -ne '') { $parts += $Guide }
        return ($parts -join '; ')
    }
    # Trailing note element, shaped for the wire.
    $noteLine = {
        param($Text)
        if ($Kind -eq 'File') { return "... ($Text)" }
        return [PSCustomObject]@{
            Path = '...'; LineNumber = 0; Line = "... ($Text)"; Context = ''
        }
    }

    if ($total -eq 0) {
        if ($HiddenNote -ne '') {
            $view.Note = "no visible $Unit; $HiddenNote"
            $view.Lines = @(& $noteLine $view.Note)
        }
        return $view
    }

    if ($RawPage) {
        # Escape hatch: raw paging, always items, never the index.
        # Explicitly passing -Skip is how the caller starts (or continues)
        # a raw walk instead of taking the catalogue view.
        $view.Mode = 'Items'
        if ($Skip -ge $total) {
            $view.Note = "no more ${Unit}: -Skip $Skip is past the $total total"
        }
        else {
            $end = [Math]::Min($Skip + $MaxResults, $total)
            $view.Page = @($Items[$Skip..($end - 1)])
            $view.Note = & $noteOf "showing $($Skip + 1)-$end of $total $Unit" `
                $HiddenNote $Guidance
        }
        $view.Lines = @($view.Page)
        if ($view.Note -ne '') { $view.Lines += & $noteLine $view.Note }
        return $view
    }

    if ($total -le $MaxResults) {
        $view.Mode = 'Items'
        $view.Page = @($Items)
        $view.Lines = @($view.Page)
        if ($HiddenNote -ne '') {
            $view.Note = $HiddenNote
            $view.Lines += & $noteLine $view.Note
        }
        return $view
    }

    # Index mode: group into cards, largest first, key as tiebreak.
    $groups = @{}
    foreach ($item in $Items) {
        if ($Kind -eq 'File') {
            $key = if ($item.Contains($sep)) {
                ($item -split [regex]::Escape($sep))[0] + $sep
            } else { '(top level)' }
        }
        else {
            $key = $item.Path
        }
        if (-not $groups.ContainsKey($key)) {
            $groups[$key] = @{
                Count   = 0
                Samples = [System.Collections.Generic.List[object]]::new()
            }
        }
        $g = $groups[$key]
        $g.Count++
        if ($g.Samples.Count -lt $SamplesPerCard) { $g.Samples.Add($item) }
    }
    $sorted = $groups.GetEnumerator() | Sort-Object -Property `
        @{ Expression = { $_.Value.Count }; Descending = $true }, `
        @{ Expression = { $_.Key } }
    $view.Cards = @($sorted | Select-Object -First $MaxCards | ForEach-Object {
        [PSCustomObject]@{
            Key     = $_.Key
            Count   = $_.Value.Count
            Samples = @($_.Value.Samples)
        }
    })
    $view.OverflowGroups = $groups.Count - $view.Cards.Count
    $view.Mode = 'Index'

    $lines = [System.Collections.Generic.List[object]]::new()
    foreach ($c in $view.Cards) {
        if ($Kind -eq 'File') {
            $samples = ($c.Samples -join ', ')
            if ($c.Key -eq '(top level)') {
                $lines.Add("(top level) - $($c.Count) matches (e.g. $samples)")
            }
            else {
                $dir = $c.Key.TrimEnd($sep)
                $lines.Add("$($c.Key) - $($c.Count) matches (e.g. $samples) - narrow: -Path `"$dir`"")
            }
        }
        else {
            $first = $c.Samples[0]
            $sample = $first.Line
            if ($sample.Length -gt 100) { $sample = $sample.Substring(0, 97) + '...' }
            $dir = Split-Path $c.Key -Parent
            $base = Split-Path $c.Key -Leaf
            $narrow = if ([string]::IsNullOrEmpty($dir)) {
                "-FilePattern `"$base`""
            } else {
                "-Path `"$dir`" -FilePattern `"$base`""
            }
            $lines.Add([PSCustomObject]@{
                Path       = $c.Key
                LineNumber = 0
                Line       = "index: $($c.Count) hits (e.g. L$($first.LineNumber): $sample) - narrow: $narrow"
                Context    = ''
            })
        }
    }
    if ($view.OverflowGroups -gt 0) {
        # Method-call arguments can't hold a command invocation: render first.
        $overflowNote = & $noteLine "+$($view.OverflowGroups) more groups below the card cap; narrow -Path to see them"
        $lines.Add($overflowNote)
    }
    $head = "index of $total $Unit in $($groups.Count) groups"
    $view.Note = & $noteOf $head $HiddenNote $Guidance
    $finalNote = & $noteLine $view.Note
    $lines.Add($finalNote)
    $view.Lines = @($lines)
    return $view
}
