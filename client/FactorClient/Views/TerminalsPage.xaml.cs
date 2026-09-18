using Microsoft.UI.Xaml.Controls;

namespace FactorClient.Views;

/// <summary>
/// The Cells: persistent terminals as a real TabView.
/// (Prototype tabs are mock panes; the real client hosts the PTY-backed cells.)
/// </summary>
public sealed partial class TerminalsPage : Page
{
    private int _cellCount = 1;

    public TerminalsPage()
    {
        this.InitializeComponent();
    }

    private void Cells_AddTabButtonClick(TabView sender, object args)
    {
        _cellCount++;
        sender.TabItems.Add(new TabViewItem
        {
            Header = $"cell-{_cellCount}",
            Content = new TextBlock
            {
                FontFamily = new Microsoft.UI.Xaml.Media.FontFamily("Consolas"),
                FontSize = 12.5,
                Padding = new Microsoft.UI.Xaml.Thickness(12),
                Text = $"PS V:\\Sources\\factor>  # cell-{_cellCount} — warm and waiting",
            },
        });
    }

    private void Cells_TabCloseRequested(TabView sender, TabViewTabCloseRequestedEventArgs args)
    {
        sender.TabItems.Remove(args.Tab);
    }
}
