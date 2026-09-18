using Microsoft.UI.Xaml;

namespace FactorClient;

/// <summary>
/// Entry point. The window it opens is real WinUI 3 on real Mica —
/// this prototype is the client, not a picture of the client.
/// </summary>
public partial class App : Application
{
    private Window? m_window;

    public App()
    {
        this.InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        m_window = new MainWindow();
        m_window.Activate();
    }

    public MainWindow? MainWindow => m_window as MainWindow;
}
