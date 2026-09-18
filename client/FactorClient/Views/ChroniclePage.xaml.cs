using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using FactorClient.Controls;
using FactorClient.Models;

namespace FactorClient.Views;

/// <summary>
/// The Chronicle: the session's memory as cards. Built from the same event
/// stream the terminal classicist renders — one wire, two renderers.
/// (Prototype builds cards in code-behind; the real client binds the feed.)
/// </summary>
public sealed partial class ChroniclePage : Page
{
    private JudgmentCard? _judgment;

    public ChroniclePage()
    {
        this.InitializeComponent();
        BuildFeed();
    }

    private void BuildFeed()
    {
        var items = MockSession.Chronicle();
        // info, agent, write-dispatch …
        foreach (var item in items.Take(3))
            Feed.Children.Add(MakeCard(item));
        // … then the judgment gate …
        _judgment = new JudgmentCard { Margin = new Thickness(0, 0, 0, 12) };
        _judgment.Resolved += OnJudgmentResolved;
        Feed.Children.Add(_judgment);
        // … then the maze card.
        Feed.Children.Add(MakeCard(items[3]));
    }

    private void OnJudgmentResolved(object? sender, bool approved)
    {
        var card = MakeCard(approved ? MockSession.ApprovedDispatch() : MockSession.DeniedFollowUp());
        var idx = Feed.Children.IndexOf(_judgment!);
        Feed.Children.Insert(idx + 1, card);
        ((App)Application.Current).MainWindow?.ClearApprovalBadge();
    }

    private Border MakeCard(FeedItem item)
    {
        var res = Application.Current.Resources;
        var stack = new StackPanel { Spacing = 8 };

        var kicker = new TextBlock
        {
            Text = item.Kicker,
            Style = (Style)res["CaptionTextBlockStyle"],
            Opacity = 0.7,
        };
        stack.Children.Add(kicker);

        var body = new TextBlock
        {
            Text = item.Body,
            Style = (Style)res["BodyTextBlockStyle"],
            TextWrapping = TextWrapping.WrapWholeWords,
        };
        stack.Children.Add(body);

        if (item.Detail is not null)
        {
            var detail = new TextBlock
            {
                Text = item.Detail,
                FontFamily = new FontFamily("Consolas"),
                FontSize = 12.5,
                TextWrapping = TextWrapping.Wrap,
            };
            var inset = new Border
            {
                Background = new SolidColorBrush(Microsoft.UI.Colors.Black) { Opacity = 0.35 },
                CornerRadius = new CornerRadius(6),
                Padding = new Thickness(10, 8, 10, 8),
                Child = detail,
            };
            stack.Children.Add(inset);
        }

        if (item.Kind is FeedKind.ToolDispatch or FeedKind.ToolResult)
        {
            var status = new TextBlock
            {
                Text = item.Success ? "✓ approved by operator" : "✗ failed",
                FontSize = 12.5,
                Opacity = 0.75,
                Foreground = new SolidColorBrush(item.Success
                    ? Microsoft.UI.Colors.LightGreen : Microsoft.UI.Colors.Salmon),
            };
            stack.Children.Add(status);
        }

        return new Border
        {
            Background = (Brush)res["CardBackgroundFillColorDefaultBrush"],
            BorderBrush = (Brush)res["CardStrokeColorDefaultBrush"],
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(8),
            Padding = new Thickness(16),
            Margin = new Thickness(0, 0, 0, 12),
            Child = stack,
        };
    }
}
