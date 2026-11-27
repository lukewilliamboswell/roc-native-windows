using System;
using System.Diagnostics;
using System.Linq;
using System.Net;
using System.Net.Sockets;
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
        Pattern<SelectionItemPattern>(row, SelectionItemPattern.Pattern).Select();
        var mark = Find(root, "Mark read", ControlType.Button);
        WaitFor("Mark read enabled after selection", () => mark.Current.IsEnabled ? mark : null);
        Invoke(mark);
        Find(root, "Mark unread", ControlType.Button);
    }

    private static void Todos(AutomationElement root, int pid)
    {
        Find(root, "New due date", ControlType.Edit);
        Find(root, "New due time", ControlType.Edit);
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
                    if (item != null && item.Current.Name.StartsWith("HTTP 200"))
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

    private static void Workers(AutomationElement root)
    {
        var grid = Find(root, "Stock counts", ControlType.DataGrid);
        var data = Pattern<GridPattern>(grid, GridPattern.Pattern);
        Require(data.Current.RowCount > 0 && data.Current.ColumnCount == 3, "Stock counts has incorrect grid dimensions.");
        var cell = data.GetItem(0, 2);
        Require(Value(cell) == "4", "Counted cell did not expose the committed value.");
        Require(!Pattern<ValuePattern>(cell, ValuePattern.Pattern).Current.IsReadOnly, "Counted is read-only.");
        Require(Pattern<ValuePattern>(data.GetItem(0, 1), ValuePattern.Pattern).Current.IsReadOnly,
                "Expected should be read-only.");
        var table = Pattern<TableItemPattern>(cell, TableItemPattern.Pattern);
        Require(table.Current.GetColumnHeaderItems().Any(item => item.Current.Name == "Counted"),
                "Counted cell has no Counted header.");
        int[] identity = cell.GetRuntimeId();
        SetValue(cell, "7");
        WaitFor("Counted committed from UI Automation", () => Value(data.GetItem(0, 2)) == "7" ? grid : null);
        SetValue(cell, "-2");
        WaitFor("Counted validation exposed", () =>
            data.GetItem(0, 2).Current.HelpText.Contains("whole number") ? grid : null);
        var sameCell = data.GetItem(0, 2);
        Require(Value(sameCell) == "7" && identity.SequenceEqual(sameCell.GetRuntimeId()),
                "Rejected edit changed the committed value or live cell identity.");
        Invoke(Find(root, "Reverse order", ControlType.Button));
        WaitFor("old cell retired after sort and paging", () =>
        {
            try { cell.GetCurrentPropertyValue(AutomationElement.NameProperty); return null; }
            catch (ElementNotAvailableException) { return grid; }
        });
    }

    private static AutomationElement TopLevel(int process, string caption)
    {
        var condition = new AndCondition(
            new PropertyCondition(AutomationElement.ProcessIdProperty, process),
            new PropertyCondition(AutomationElement.NameProperty, caption));
        return AutomationElement.RootElement.FindFirst(TreeScope.Children, condition);
    }

    private static AutomationElement Find(AutomationElement parent, string name, params ControlType[] types)
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
        });
    }

    private static AutomationElement WaitFor(string checkpoint, Func<AutomationElement> attempt)
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
        throw new InvalidOperationException("UI Automation checkpoint timed out: " + checkpoint);
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
        private readonly TcpListener listener = new TcpListener(IPAddress.Loopback, 0);
        private readonly Thread worker;
        public string Url { get; private set; }
        public ManualResetEvent RequestReceived { get; private set; } = new ManualResetEvent(false);
        public ManualResetEvent Complete { get; private set; } = new ManualResetEvent(false);

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
