using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using FactorClient.Models;

namespace FactorClient.Controls;

/// <summary>
/// The judgment gate: a proposed approval chain with real consequences.
/// Approve → every call in the chain runs. Deny → zero execute, and the
/// denial enters the model's context. Edit is single-call only; anything
/// else fails closed as a denial.
/// </summary>
public sealed partial class JudgmentCard : UserControl
{
    /// <summary>True = approved, False = denied.</summary>
    public event EventHandler<bool>? Resolved;

    public JudgmentCard()
    {
        this.InitializeComponent();
        ChainList.ItemsSource = MockSession.PendingApproval();
        CallCountText.Text = MockSession.PendingApproval().Count == 1
            ? "1 call" : $"{MockSession.PendingApproval().Count} calls";
    }

    private void Settle(string verdict, bool approved)
    {
        Verdict.Text = verdict;
        Verdict.Visibility = Visibility.Visible;
        foreach (var child in Actions.Children)
            if (child is Button b) b.IsEnabled = false;
        Resolved?.Invoke(this, approved);
    }

    private void Approve_Click(object sender, RoutedEventArgs e) =>
        Settle("✓ Approved by operator — the chain ran to completion.", approved: true);

    private void Deny_Click(object sender, RoutedEventArgs e) =>
        Settle("✗ Denied by operator — zero calls executed. The agent was told nothing ran.", approved: false);

    private void Edit_Click(object sender, RoutedEventArgs e)
    {
        // Prototype honesty: edits are single-call only; multi-call edit fails closed.
        Verdict.Text = "Edit is supported on single-call chains only — a multi-call edit fails closed as a denial.";
        Verdict.Visibility = Visibility.Visible;
    }
}
