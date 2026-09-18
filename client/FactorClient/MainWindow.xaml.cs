using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using FactorClient.Views;

namespace FactorClient;

/// <summary>
/// Shell: NavigationView over a Frame, on real Mica.
/// One wire, many renderers — this window is View 2 (Fluent materialist);
/// the terminal's textual classicist is View 1. Same events, both.
/// </summary>
public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        this.InitializeComponent();
        this.Title = "Factor";

        // The material itself, from dwm — not a gradient pretending to be one.
        this.SystemBackdrop = new MicaBackdrop();

        NavView.SelectedItem = NavView.MenuItems[0];
        ContentFrame.Navigate(typeof(ChroniclePage));
    }

    public void ClearApprovalBadge() => ApprovalBadge.Value = 0;

    private void NavView_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.IsSettingsSelected)
        {
            ContentFrame.Navigate(typeof(SettingsPage));
            return;
        }
        if (args.SelectedItemContainer is NavigationViewItem item && item.Tag is string tag)
        {
            var page = tag switch
            {
                "chronicle" => typeof(ChroniclePage),
                "terminals" => typeof(TerminalsPage),
                "maze" => typeof(MazePage),
                "approvals" => typeof(ApprovalsPage),
                _ => typeof(ChroniclePage),
            };
            ContentFrame.Navigate(page);
        }
    }
}
