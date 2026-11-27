#include <windows.h>
#include <stdio.h>
#include <wchar.h>

static HWND focus(HWND window) {
    GUITHREADINFO info = { .cbSize = sizeof(info) };
    GetGUIThreadInfo(GetWindowThreadProcessId(window, NULL), &info);
    return info.hwndFocus;
}

static void key(WORD code, int shift) {
    if (shift) keybd_event(VK_SHIFT, 0, 0, 0);
    keybd_event(code, 0, 0, 0);
    keybd_event(code, 0, KEYEVENTF_KEYUP, 0);
    if (shift) keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, 0);
    Sleep(100);
}

static void type_date(HWND edit, const WCHAR *date) {
    SendMessageW(edit, EM_SETSEL, 0, -1);
    for (; *date; ++date)
        SendMessageW(edit, WM_CHAR, *date, 1);
}

static void read_text(HWND window, WCHAR text[128]) {
    SendMessageW(window, WM_GETTEXT, 128, (LPARAM)text);
}

static int has_caption(HWND window, const WCHAR *caption) {
    WCHAR text[128] = {0};
    read_text(window, text);
    return wcscmp(text, caption) == 0;
}

static HWND picker(void) {
    return FindWindowW(L"#32770", L"Choose date for New due date");
}

static int verify(HWND owner, HWND choose) {
    HWND title = FindWindowExW(owner, NULL, L"EDIT", NULL);
    HWND date = FindWindowExW(owner, title, L"EDIT", NULL);
    if (!date) return 2;

    SetForegroundWindow(owner);
    for (int i = 0; focus(owner) != date && i < 8; ++i)
        key(VK_TAB, 0);
    if (focus(owner) != date) return 3;
    type_date(date, L"2028-01-31");
    key(VK_TAB, 0);
    if (focus(owner) != choose) return 4;
    WCHAR before[128] = {0};
    read_text(date, before);

    key(VK_RETURN, 0);
    HWND calendar = picker();
    if (!calendar || GetWindow(calendar, GW_OWNER) != owner || IsWindowEnabled(owner)) return 5;
    if (!has_caption(focus(calendar), L"31")) return 6;
    key(VK_NEXT, 0);
    if (!has_caption(focus(calendar), L"29")) return 7;
    key(VK_RIGHT, 0);
    if (!has_caption(focus(calendar), L"1")) return 8;
    key(VK_PRIOR, 0);
    key(VK_END, 0);
    if (!has_caption(focus(calendar), L"29")) return 9;
    key(VK_TAB, 0);
    if (!has_caption(focus(calendar), L"Cancel")) return 10;
    key(VK_TAB, 1);
    if (!has_caption(focus(calendar), L"29")) return 11;
    key(VK_ESCAPE, 0);
    if (picker() || !IsWindowEnabled(owner) || focus(owner) != choose || !has_caption(date, before))
        return 12;

    key(VK_SPACE, 0);
    calendar = picker();
    if (!calendar || !has_caption(focus(calendar), L"31")) return 13;
    key(VK_NEXT, 0);
    key(VK_RETURN, 0);
    if (picker() || focus(owner) != choose || has_caption(date, before)) return 14;

    key(VK_RETURN, 0);
    calendar = picker();
    if (!calendar || !has_caption(focus(calendar), L"29")) return 15;
    key(VK_ESCAPE, 0);
    if (picker()) return 16;
    puts("Calendar passed leap-month navigation, one date Tab stop, selection, cancellation, and opener focus");
    return 0;
}

int main(void) {
    for (int i = 0; i < 150; ++i) {
        HWND owner = FindWindowW(L"RocSignalsWindow", L"todos");
        HWND choose = FindWindowExW(owner, NULL, L"BUTTON", L"Choose date");
        if (owner && IsWindowVisible(choose)) {
            int result = verify(owner, choose);
            if (result) printf("calendar keyboard check failed at step %d\n", result);
            if (picker()) SendMessageW(picker(), WM_CLOSE, 0, 0);
            PostMessageW(owner, WM_CLOSE, 0, 0);
            return result;
        }
        Sleep(40);
    }
    puts("did not find the Todos date field");
    return 1;
}
