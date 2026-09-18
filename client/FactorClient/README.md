# FactorClient — the real WinUI 3 prototype (View 2)

This is not a mockup. It is a genuine WinUI 3 desktop app: real Mica on the
window (`MicaBackdrop`, resolved by dwm), real `AcrylicBrush` on the judgment
card, real `NavigationView` / `TabView` / `TreeView` / `InfoBadge`, real Reveal
on every button courtesy of the Fluent theme. View 1 (the textual classicist)
renders the same event stream in the terminal; this is View 2, the materialist.

## Metaphors under test

- **The Chronicle** — the session's memory as cards, in wire order.
- **The Judgment Gate** — approval as an elevated acrylic card (`Controls/JudgmentCard`).
  Approve runs the whole chain; deny executes zero calls and the denial enters
  the model's context. Reused on the Chronicle and Approvals pages.
- **The Cells** — terminals as `TabView` tabs. Bounded, never silently reaped,
  lifecycle ungated (matching the engine's trust policy).
- **The Maze** — catalogue drawers as a `TreeView`; each drawer shows its
  narrowing call.

## Data

`Models/Session.cs` carries the event model shaped like the wire's event
stream, with mock data (the same `hello.ps1` beats as the textual specimen)
standing in for the live named-pipe feed. Binding the real feed is later work.

## Build (on Windows)

Prerequisites: [.NET 8 SDK](https://dotnet.microsoft.com/download). No Visual
Studio required; the Windows App SDK comes from NuGet.

```powershell
cd client\FactorClient
dotnet build -c Release
```

Run the exe from `bin\Release\net8.0-windows10.0.19041.0\win-x64\FactorClient.exe`
(or `win-x86` / `win-arm64` per your platform).

Notes:

- Unpackaged deployment (`WindowsPackageType=None`): the Windows App SDK
  framework ships next to the exe, no MSIX needed for the prototype.
- The XAML compiler runs on Windows only — this project cannot build on Linux.
  XAML here is validated as well-formed XML; behavior is verified by running it.
- If `E756` renders as the wrong glyph on the Terminals nav item, say so —
  Segoe MDL2 coverage varies and we'll swap it.
