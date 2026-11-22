using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace HLSR.Core;

/// <summary>Диалоги в духе <c>MessageDialog</c>/<c>ContentDialog</c> Windows 10.</summary>
public static class Dialogs
{
    private static XamlRoot? Root => App.Window?.Content?.XamlRoot;

    public static async Task Message(string title, string text)
    {
        if (Root is null) return;
        var dialog = new ContentDialog
        {
            XamlRoot = Root,
            Title = title,
            Content = new TextBlock { Text = text, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true },
            CloseButtonText = Loc.T("#L_CLOSE"),
            DefaultButton = ContentDialogButton.Close,
        };
        await Show(dialog);
    }

    public static async Task<bool> Confirm(string title, string text, string confirm)
    {
        if (Root is null) return false;
        var dialog = new ContentDialog
        {
            XamlRoot = Root,
            Title = title,
            Content = new TextBlock { Text = text, TextWrapping = TextWrapping.Wrap },
            PrimaryButtonText = confirm,
            CloseButtonText = Loc.T("#L_CANCEL"),
            DefaultButton = ContentDialogButton.Close,
        };
        return await Show(dialog) == ContentDialogResult.Primary;
    }

    // Один диалог за раз: второй ContentDialog поверх первого WinUI не открывает.
    private static readonly SemaphoreSlim Gate = new(1, 1);

    private static async Task<ContentDialogResult> Show(ContentDialog dialog)
    {
        await Gate.WaitAsync();
        try { return await dialog.ShowAsync(); }
        finally { Gate.Release(); }
    }
}
