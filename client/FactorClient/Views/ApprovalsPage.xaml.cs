using Microsoft.UI.Xaml.Controls;

namespace FactorClient.Views;

/// <summary>
/// Approvals: the judgment gate, full-page. A second live instance of the
/// same control the Chronicle embeds — judgment is one metaphor, everywhere.
/// </summary>
public sealed partial class ApprovalsPage : Page
{
    public ApprovalsPage()
    {
        this.InitializeComponent();
        Gate.Resolved += (_, approved) =>
        {
            Echo.Text = approved
                ? "✓ The chain above ran to completion. Every call, in order."
                : "✗ Zero calls executed. The denial is now part of the model's context.";
        };
    }
}
