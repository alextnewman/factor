using Microsoft.UI.Xaml.Controls;

namespace FactorClient.Views;

/// <summary>
/// The Maze: catalogue navigation as a real TreeView. Drawers carry their
/// narrowing calls — the model walks map → drawer → item → read.
/// </summary>
public sealed partial class MazePage : Page
{
    public MazePage()
    {
        this.InitializeComponent();
    }

    private void MazeTree_ItemInvoked(TreeView sender, TreeViewItemInvokedEventArgs args)
    {
        var label = (args.InvokedItem as TreeViewNode)?.Content?.ToString() ?? "";
        NarrowDetail.Text = label switch
        {
            var s when s.StartsWith("bridge") => "Find-FAFile -Pattern bridge* -Recurse",
            var s when s.StartsWith("engine") => "Find-FAFile -Pattern engine* -Recurse",
            var s when s.StartsWith("loop") => "Find-FAFile -Pattern loop* -Recurse",
            var s when s.StartsWith("src") => "Get-FATree -Path src",
            var s when s.StartsWith("psmodule") => "Get-FATree -Path psmodule",
            var s when s.StartsWith("scripts") => "Get-FATree -Path scripts",
            _ => "Get-FATree — the room map.",
        };
    }
}
