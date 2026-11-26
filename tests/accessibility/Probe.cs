using System;
using System.Diagnostics;
using System.Linq;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Automation;

public static class RocWindowsAccessibility
{
    public static void Check(Process process, string example)
    {
        Require(Thread.CurrentThread.GetApartmentState() == ApartmentState.MTA,
                "The UI Automation client requires an MTA thread.");
        var window = WaitFor("visible app window", () =>
        {
            process.Refresh();
            Require(!process.HasExited, "Application exited before its visible window was ready.");
            if (process.MainWindowHandle == IntPtr.Zero) return null;
            var element = AutomationElement.FromHandle(process.MainWindowHandle);
            return element.Current.ProcessId == process.Id && !element.Current.IsOffscreen
                ? element : null;
        });

        switch (example)
        {
            case "counter": Counter(window); break;
            case "starter": Starter(window); break;
            case "todos": Todos(window, process.Id); break;
            case "files": Files(window); break;
            case "http": Http(window); break;
            case "progress": Progress(window); break;
            case "workers": Workers(window); break;
            default: throw new ArgumentOutOfRangeException("example");
        }
    }

    private static void Counter(AutomationElement root)
    {
        Find(root, "Decrease", ControlType.Button);
        Find(root, "Reset", ControlType.Button);
        Invoke(Find(root, "Increase", ControlType.Button));
        Find(root, "Count: 1", ControlType.Text);
    }

    private static void Starter(AutomationElement root)
    {
        var title = Find(root, "Title", ControlType.Edit);
        var label = title.GetCurrentPropertyValue(AutomationElement.LabeledByProperty) as AutomationElement;
        Require(label != null && label.Current.Name == "Title", "Title has no associated visible label.");
        title.SetFocus();
        WaitFor("Title keyboard focus", () => title.Current.HasKeyboardFocus ? title : null);
        using (var change = new AutoResetEvent(false))
        {
            AutomationPropertyChangedEventHandler handler = (sender, args) =>
            {
                if (Equals(args.NewValue, "Accessibility proof"))
                {
                    try { change.Set(); } catch (ObjectDisposedException) {}
                }
            };
            Automation.AddAutomationPropertyChangedEventHandler(title, TreeScope.Element, handler, ValuePattern.ValueProperty);
            try
            {
                SetValue(title, "Accessibility proof");
                Require(change.WaitOne(4000), "Title Value did not raise its property change event.");
                Require(Value(title) == "Accessibility proof" && title.Current.Name == "Title",
                        "Title did not retain its name when its Value changed.");
            }
            finally { Automation.RemoveAutomationPropertyChangedEventHandler(title, handler); }
        }
        Find(root, "Status filter", ControlType.ComboBox);
        Invoke(Find(root, "Add", ControlType.Button));
        var report = Find(root, "Titles", ControlType.List, ControlType.DataGrid, ControlType.Table);
        Find(report, "Status", ControlType.HeaderItem);
        var row = Find(report, "Accessibility proof", ControlType.DataItem, ControlType.ListItem);
        var selection = Pattern<SelectionItemPattern>(row, SelectionItemPattern.Pattern);
        selection.Select();
        WaitFor("Reading List row selected", () => selection.Current.IsSelected ? row : null);
        // The Win32 toolbar proxy can expose the same command as MenuItem
        // while it is text-only and as Button while it has an icon.
        var commands = new[] { ControlType.Button, ControlType.MenuItem };
        var mark = Find(root, "Mark read", commands);
        WaitFor("Mark read enabled after selection", () => mark.Current.IsEnabled ? mark : null);
        Invoke(mark);
        Func<string> describe = () => "Buttons: " + Describe(root, ControlType.Button) +
            ". Menu items: " + Describe(root, ControlType.MenuItem) +
            ". Status: " + Describe(root, ControlType.StatusBar);
        var unmark = Find(root, "Mark unread", commands, describe);
        Require(unmark.Current.IsEnabled, "Mark unread is disabled for the selected title.");
        Invoke(unmark);
        Find(root, "Mark read", commands, describe);
    }

    private static void Todos(AutomationElement root, int pid)
    {
        var date = Find(root, "New due date", ControlType.Edit);
        var time = Find(root, "New due time", ControlType.Edit);
        foreach (var field in new[] { date, time })
        {
            var label = field.GetCurrentPropertyValue(AutomationElement.LabeledByProperty) as AutomationElement;
            Require(label != null && label.Current.Name == field.Current.Name,
                    field.Current.Name + " has no associated visible label.");
            Require(field.Current.HelpText.Contains("Example"),
                    field.Current.Name + " has no format hint.");
        }
        date.SetFocus();
        SetValue(date, "invalid");
        PostKey(date, 0x0d); // Enter validates the draft in the app's message loop.
        WaitFor("date validation is exposed on the edit", () =>
            date.Current.HelpText.Contains("Enter a valid date") ? date : null);
        PostKey(date, 0x1b); // Escape restores the committed value and hint.
        WaitFor("date validation clears on Escape", () =>
            !date.Current.HelpText.Contains("Enter a valid date") ? date : null);
        SetValue(Find(root, "New to-do", ControlType.Edit), "Accessibility task");
        Invoke(Find(root, "Add", ControlType.Button));
        var opener = Find(root, "Notes for Accessibility task", ControlType.Button);
        opener.SetFocus();
        Invoke(opener);
        var dialog = WaitFor("owned Notes window", () => TopLevel(pid, "Notes for Accessibility task"));
        var notes = Find(dialog, "Notes", ControlType.Document, ControlType.Edit);
        Pattern<TextPattern>(notes, TextPattern.Pattern);
        Invoke(Find(dialog, "Cancel", ControlType.Button));
        WaitFor("focus restored to the Notes opener", () =>
            TopLevel(pid, "Notes for Accessibility task") == null && opener.Current.HasKeyboardFocus
            ? opener : null);
    }

    private static void Files(AutomationElement root)
    {
        Find(root, "Picked files", ControlType.List);
        var tabs = Find(root, "File details", ControlType.Tab);
        var fingerprint = Find(tabs, "Fingerprint", ControlType.TabItem);
        Pattern<SelectionItemPattern>(fingerprint, SelectionItemPattern.Pattern).Select();
        var copy = Find(root, "Copy SHA-256", ControlType.Button);
        Require(!copy.Current.IsEnabled, "Copy SHA-256 should be disabled without a file.");
        var image = Find(root, "A document in a folder", ControlType.Image);
        Require(image.Current.IsControlElement && image.Current.IsContentElement &&
                !image.Current.IsKeyboardFocusable, "The informative image has incorrect view membership or focusability.");
    }

    private static void Http(AutomationElement root)
    {
        using (var response = new LoopbackResponse())
        using (var pending = new AutoResetEvent(false))
        using (var changed = new AutoResetEvent(false))
        {
            var url = Find(root, "URL", ControlType.Edit);
            SetValue(url, response.Url);
            url.SetFocus();
            var live = AutomationElementIdentifiers.LiveRegionChangedEvent;
            Require(live != null, "This UI Automation client does not support LiveRegionChanged.");
            int completed = 0;
            AutomationEventHandler handler = (source, args) =>
            {
                try
                {
                    var item = source as AutomationElement;
                    if (item == null) return;
                    var name = item.Current.Name;
                    if (name.StartsWith("Fetching", StringComparison.Ordinal)) pending.Set();
                    if (name.StartsWith("HTTP 200", StringComparison.Ordinal))
                    {
                        Interlocked.Increment(ref completed);
                        changed.Set();
                    }
                }
                catch (ElementNotAvailableException) {}
                catch (ObjectDisposedException) {}
            };
            Automation.AddAutomationEventHandler(live, root, TreeScope.Subtree, handler);
            try
            {
                var fetch = Find(root, "Fetch", ControlType.Button);
                Invoke(fetch);
                Require(response.RequestReceived.WaitOne(4000), "The HTTP request did not reach loopback.");
                Require(pending.WaitOne(4000), "HTTP pending status did not raise a live-region event.");
                var animations = ClientAnimationsEnabled();
                var progress = Find(root, animations ? "Request progress" : "Request progress (busy)",
                    animations ? ControlType.ProgressBar : ControlType.Text);
                Require(!progress.Current.IsOffscreen, "The request progress indicator is offscreen.");
                var focused = AutomationElement.FocusedElement;
                response.Complete.Set();
                Require(changed.WaitOne(8000), "HTTP completion did not raise a live-region event.");
                Require(!changed.WaitOne(350) && completed == 1, "HTTP completion was announced more than once.");
                Require(focused != null && Automation.Compare(focused, AutomationElement.FocusedElement),
                        "Status feedback moved focus from the control that held it during the request.");
            }
            finally { Automation.RemoveAutomationEventHandler(live, root, handler); }
        }
    }

    private static void Progress(AutomationElement root)
    {
        var progress = Find(root, "Transfer progress", ControlType.ProgressBar);
        var range = Pattern<RangeValuePattern>(progress, RangeValuePattern.Pattern).Current;
        Require(range.IsReadOnly, "Transfer progress is writable through UI Automation.");
        Require(range.Maximum > range.Minimum &&
                Math.Abs((range.Value - range.Minimum) / (range.Maximum - range.Minimum) - 0.25) < 0.001,
                "Transfer progress does not expose the expected quarter-complete value.");
    }

    private static bool ClientAnimationsEnabled()
    {
        bool enabled;
        Require(SystemParametersInfo(0x1042, 0, out enabled, 0), "Could not read the Windows client animation setting.");
        return enabled;
    }

    [DllImport("user32.dll", EntryPoint = "SystemParametersInfoW")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SystemParametersInfo(uint action, uint parameter,
        [MarshalAs(UnmanagedType.Bool)] out bool value, uint flags);

    private static void Workers(AutomationElement root)
    {
        var grid = Find(root, "Stock counts", ControlType.DataGrid);
        var data = Pattern<GridPattern>(grid, GridPattern.Pattern);
        Require(data.Current.RowCount > 0 && data.Current.ColumnCount == 3, "Stock counts has incorrect grid dimensions.");
        var cell = GridItem(data, 0, 2);
        Require(Value(cell) == "4", "Counted cell did not expose the committed value.");
        Require(!Pattern<ValuePattern>(cell, ValuePattern.Pattern).Current.IsReadOnly, "Counted is read-only.");
        Require(Pattern<ValuePattern>(GridItem(data, 0, 1), ValuePattern.Pattern).Current.IsReadOnly,
                "Expected should be read-only.");
        var table = Pattern<TableItemPattern>(cell, TableItemPattern.Pattern);
        Require(ColumnHeaders(table).Any(item => item.Current.Name == "Counted"),
                "Counted cell has no Counted header.");
        int[] identity = cell.GetRuntimeId();
        SetValue(cell, "7");
        WaitFor("Counted committed from UI Automation", () => Value(GridItem(data, 0, 2)) == "7" ? grid : null);
        SetValue(cell, "-2");
        WaitFor("Counted validation exposed", () =>
            GridItem(data, 0, 2).Current.HelpText.Contains("whole number") ? grid : null);
        var sameCell = GridItem(data, 0, 2);
        Require(Value(sameCell) == "7" && identity.SequenceEqual(sameCell.GetRuntimeId()),
                "Rejected edit changed the committed value or live cell identity.");
        sameCell.SetFocus();
        WaitFor("Counted cell takes focus", () => CurrentFocus(sameCell));
        PostKey(grid, 0x71); // F2 enters a native EDIT.
        var editor = WaitFor("native Counted editor takes focus", () =>
        {
            var focused = AutomationElement.FocusedElement;
            return focused != null && focused.Current.ControlType == ControlType.Edit &&
                   focused.Current.ProcessId == root.Current.ProcessId &&
                   focused.Current.Name == "Counted" ? focused : null;
        });
        var parent = TreeWalker.RawViewWalker.GetParent(editor);
        Require(parent != null && Automation.Compare(parent, sameCell),
                "The focused EDIT is missing from the Counted cell's fragment.");
        Require(Value(editor) == "7" && editor.Current.HelpText.Contains("whole number"),
                "The EDIT did not expose its draft and the cell's validation.");
        Pattern<TextPattern>(editor, TextPattern.Pattern);
        SetValue(editor, "9");
        Require(Value(editor) == "9" && Value(sameCell) == "7",
                "The EDIT and the committed cell must expose their own values.");
        PostKey(editor, 0x1b); // Escape cancels the draft and refocuses the cell.
        WaitFor("Counted cell regains focus", () => CurrentFocus(sameCell));
        Require(Value(sameCell) == "7", "Escape committed the editor draft.");
        Invoke(Find(root, "Reverse order", ControlType.Button));
        WaitFor("old cell retired after sort and paging", () =>
        {
            try { cell.GetCurrentPropertyValue(AutomationElement.NameProperty); return null; }
            catch (ElementNotAvailableException) { return grid; }
        });
    }

    private static AutomationElement GridItem(GridPattern grid, int row, int column)
    {
        try { return grid.GetItem(row, column); }
        catch (ElementNotAvailableException) { throw; }
        catch (Exception error)
        {
            throw new InvalidOperationException(String.Format("Stock counts Grid.GetItem({0}, {1}) failed.", row, column), error);
        }
    }

    private static AutomationElement[] ColumnHeaders(TableItemPattern cell)
    {
        try { return cell.Current.GetColumnHeaderItems(); }
        catch (Exception error)
        {
            throw new InvalidOperationException("Counted TableItem.GetColumnHeaderItems failed.", error);
        }
    }

    private static AutomationElement CurrentFocus(AutomationElement expected)
    {
        var focused = AutomationElement.FocusedElement;
        return focused != null && Automation.Compare(focused, expected) ? focused : null;
    }

    private static void PostKey(AutomationElement target, int key)
    {
        var hwnd = new IntPtr(target.Current.NativeWindowHandle);
        Require(hwnd != IntPtr.Zero && PostMessage(hwnd, 0x100, new UIntPtr((uint)key), new IntPtr(1)),
                "Could not post a native key to " + target.Current.Name + ".");
    }

    [DllImport("user32.dll", EntryPoint = "PostMessageW", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PostMessage(IntPtr window, uint message, UIntPtr key, IntPtr flags);

    private static AutomationElement TopLevel(int process, string caption)
    {
        var condition = new AndCondition(
            new PropertyCondition(AutomationElement.ProcessIdProperty, process),
            new PropertyCondition(AutomationElement.NameProperty, caption));
        return AutomationElement.RootElement.FindFirst(TreeScope.Children, condition);
    }

    private static AutomationElement Find(AutomationElement parent, string name, params ControlType[] types)
    {
        return Find(parent, name, types, null);
    }

    private static AutomationElement Find(AutomationElement parent, string name, ControlType[] types, Func<string> describe)
    {
        return WaitFor(name + " [" + String.Join(", ", types.Select(type => type.ProgrammaticName)) + "]", () =>
        {
            foreach (var type in types)
            {
                var condition = new AndCondition(
                    new PropertyCondition(AutomationElement.NameProperty, name),
                    new PropertyCondition(AutomationElement.ControlTypeProperty, type));
                var result = parent.FindFirst(TreeScope.Descendants, condition);
                if (result != null) return result;
            }
            return null;
        }, describe);
    }

    private static string Describe(AutomationElement parent, ControlType type)
    {
        try
        {
            var condition = new PropertyCondition(AutomationElement.ControlTypeProperty, type);
            return String.Join(", ", parent.FindAll(TreeScope.Descendants, condition)
                .Cast<AutomationElement>().Take(12).Select(item =>
                {
                    var label = String.Format("'{0}' ({1})", item.Current.Name,
                                              item.Current.IsEnabled ? "enabled" : "disabled");
                    return type == ControlType.StatusBar ? label + " " + Describe(item, ControlType.Text) : label;
                }));
        }
        catch (Exception error) { return error.Message; }
    }

    private static AutomationElement WaitFor(string checkpoint, Func<AutomationElement> attempt, Func<string> describe = null)
    {
        var watch = Stopwatch.StartNew();
        while (watch.ElapsedMilliseconds < 5000)
        {
            try
            {
                var result = attempt();
                if (result != null) return result;
            }
            catch (ElementNotAvailableException) {}
            Thread.Sleep(50);
        }
        var details = describe == null ? "" : "\n" + describe();
        throw new InvalidOperationException("UI Automation checkpoint timed out: " + checkpoint + details);
    }

    private static T Pattern<T>(AutomationElement element, AutomationPattern pattern) where T : class
    {
        object value;
        Require(element.TryGetCurrentPattern(pattern, out value), element.Current.Name + " lacks " + pattern.ProgrammaticName);
        return (T)value;
    }

    private static void Invoke(AutomationElement element)
    {
        Pattern<InvokePattern>(element, InvokePattern.Pattern).Invoke();
    }

    private static void SetValue(AutomationElement element, string value)
    {
        Pattern<ValuePattern>(element, ValuePattern.Pattern).SetValue(value);
    }

    private static string Value(AutomationElement element)
    {
        return Pattern<ValuePattern>(element, ValuePattern.Pattern).Current.Value;
    }

    private static void Require(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }

    private sealed class LoopbackResponse : IDisposable
    {
        // This endpoint is declared in the HTTP example's capabilities.
        private readonly TcpListener listener = new TcpListener(IPAddress.Loopback, 48393);
        private readonly Thread worker;
        public string Url { get; private set; }
        public readonly ManualResetEvent RequestReceived = new ManualResetEvent(false);
        public readonly ManualResetEvent Complete = new ManualResetEvent(false);

        public LoopbackResponse()
        {
            listener.Start();
            Url = "http://127.0.0.1:" + ((IPEndPoint)listener.LocalEndpoint).Port + "/";
            worker = new Thread(Reply) { IsBackground = true };
            worker.Start();
        }

        private void Reply()
        {
            try
            {
                using (var client = listener.AcceptTcpClient())
                using (var stream = client.GetStream())
                {
                    client.ReceiveTimeout = 4000;
                    int matched = 0;
                    byte[] ending = { 13, 10, 13, 10 };
                    for (int read = 0; read < 8192 && matched < ending.Length; read++)
                    {
                        var next = stream.ReadByte();
                        if (next < 0) return;
                        matched = next == ending[matched] ? matched + 1 : next == 13 ? 1 : 0;
                    }
                    if (matched != ending.Length) return;
                    RequestReceived.Set();
                    if (!Complete.WaitOne(8000)) return;
                    var body = Encoding.ASCII.GetBytes("Local accessibility check.");
                    var headers = Encoding.ASCII.GetBytes(
                        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/plain\r\nContent-Length: "
                        + body.Length + "\r\n\r\n");
                    stream.Write(headers, 0, headers.Length);
                    stream.Write(body, 0, body.Length);
                }
            }
            catch (SocketException) {}
            catch (System.IO.IOException) {}
            catch (InvalidOperationException) {}
        }

        public void Dispose()
        {
            listener.Stop();
            Complete.Set();
            worker.Join(4500);
            RequestReceived.Dispose();
            Complete.Dispose();
        }
    }
}
