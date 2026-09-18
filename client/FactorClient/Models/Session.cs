namespace FactorClient.Models;

/// <summary>
/// The kinds of events the wire carries. The Chronicle renders this stream;
/// the mock below stands in for the live named-pipe feed until the client
/// binds to fa32 for real. Same beats as the textual specimen's hello.ps1 run.
/// </summary>
public enum FeedKind
{
    Info,
    AgentText,
    ToolDispatch,
    ToolResult,
    MazeIndex,
}

public sealed record FeedItem(
    FeedKind Kind,
    string Kicker,      // e.g. "◆ Agent", "⚙ Tool dispatch"
    string Body,        // human text
    string? Detail = null,   // mono block: args, output, index card
    bool Success = true);

public sealed record ProposedCall(string Name, string Arguments);

public static class MockSession
{
    public const string SessionId = "sess-18d67ef79cadf12c-33616";
    public const string Pipe = @"\\.\pipe\WinAgent32\sess-18d67ef79cadf12c-33616";

    public static List<FeedItem> Chronicle() => new()
    {
        new(FeedKind.Info, "⬡ Terminal cell opened",
            "The default cell was materialized for this session — a persistent PowerShell, warm and waiting. Nothing about it needed your judgment; empty rooms don't ask permission.",
            $"# New-FATerminal -Name default\n# pipe {Pipe} acquired in 774 ms"),
        new(FeedKind.AgentText, "◆ Agent",
            "The file doesn't exist yet. I'll create it, then run it — the run is the part I'll bring to you first."),
        new(FeedKind.ToolDispatch, "⚙ Tool dispatch — Write-FAFile",
            "Write-FAFile -Path .\\hello.ps1 -Content \"Write-Host 'hello'\"",
            null, Success: true),
        new(FeedKind.MazeIndex, "▦ Maze — drawer opened",
            "A search returned more than a page, so the wire carried a map instead of the maze: grouped index cards with counts and narrowing calls.",
            "INDEX src/ (34 files)\n  drawer: bridge (6)   narrow: Find-FAFile -Pattern bridge*\n  drawer: engine (11)  narrow: Find-FAFile -Pattern engine*"),
    };

    public static List<ProposedCall> PendingApproval() => new()
    {
        new("Invoke-FACommand", @".\hello.ps1"),
    };

    public static FeedItem ApprovedDispatch() => new(
        FeedKind.ToolDispatch, "⚙ Tool dispatch — Invoke-FACommand",
        "Invoke-FACommand .\\hello.ps1 -Terminal default",
        "hello\n\n# exit 0 — 41 ms in the warm cell",
        Success: true);

    public static FeedItem DeniedFollowUp() => new(
        FeedKind.AgentText, "◆ Agent — context updated",
        "Understood — the run never happened. The file is written; I'll wait for your direction before executing anything.");
}
