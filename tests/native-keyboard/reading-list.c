#include <windows.h>
#include <commctrl.h>
#include <stdio.h>

static HWND find_window(void) {
    for (int i = 0; i < 150; ++i) {
        HWND hwnd = FindWindowW(L"RocSignalsWindow", L"reading-list");
        if (hwnd && IsWindowVisible(FindWindowExW(hwnd, NULL, L"SysListView32", NULL)))
            return hwnd;
        Sleep(40);
    }
    return NULL;
}

static void key(WORD code, int shift) {
    if (shift) keybd_event(VK_SHIFT, 0, 0, 0);
    keybd_event(code, 0, 0, 0);
    keybd_event(code, 0, KEYEVENTF_KEYUP, 0);
    if (shift) keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, 0);
    Sleep(80);
}

static HWND focus(HWND hwnd) {
    GUITHREADINFO info = { .cbSize = sizeof(info) };
    GetGUIThreadInfo(GetWindowThreadProcessId(hwnd, NULL), &info);
    return info.hwndFocus;
}

static int visible(HWND parent, HWND child) {
    RECT bounds, area;
    GetWindowRect(child, &bounds);
    MapWindowPoints(NULL, parent, (POINT *)&bounds, 2);
    GetClientRect(parent, &area);
    return bounds.top >= 0 && bounds.top < area.bottom && bounds.right <= area.right;
}

static int verify(HWND hw) {
    HWND edit = FindWindowExW(hw, NULL, L"EDIT", NULL);
    HWND add = FindWindowExW(hw, NULL, L"BUTTON", L"Add");
    HWND combo = FindWindowExW(hw, NULL, L"COMBOBOX", NULL);
    HWND list = FindWindowExW(hw, NULL, L"SysListView32", NULL);
    HWND tools = FindWindowExW(hw, NULL, TOOLBARCLASSNAMEW, NULL);
    if (!edit || !add || !combo || !list || !tools) {
        puts("did not find the Reading List controls");
        return 2;
    }

    SetForegroundWindow(hw);
    SetWindowPos(hw, NULL, 0, 0, 205, 205, SWP_NOMOVE | SWP_NOZORDER);
    for (int i = 0; focus(hw) != edit && i < 8; ++i)
        key(VK_TAB, 0);
    if (focus(hw) != edit || !visible(hw, edit)) {
        printf("did not focus and reveal Title: current=%p target=%p\n", focus(hw), edit);
        return 3;
    }
    key('X', 0);
    if (!IsWindowEnabled(add) || focus(hw) != edit) {
        puts("typing a title did not enable Add while preserving focus");
        return 4;
    }
    key(VK_TAB, 0);
    if (focus(hw) != add || !visible(hw, add)) {
        printf("did not focus and reveal enabled Add: current=%p target=%p\n", focus(hw), add);
        return 5;
    }
    key(VK_SPACE, 0);
    if (IsWindowEnabled(add) || focus(hw) != combo || !visible(hw, combo)) {
        printf("disabled Add did not repair focus to Status filter: %p wanted %p\n", focus(hw), combo);
        return 6;
    }
    key(VK_TAB, 0);
    if (focus(hw) != tools || !visible(hw, tools)) {
        printf("did not focus and reveal Toolbar: current=%p target=%p\n", focus(hw), tools);
        return 7;
    }
    key(VK_TAB, 0);
    if (focus(hw) != list || !visible(hw, list)) {
        printf("did not focus and reveal ListView: current=%p target=%p\n", focus(hw), list);
        return 8;
    }
    key(VK_TAB, 1);
    if (focus(hw) != tools || !visible(hw, tools)) {
        puts("Shift+Tab did not return to the Toolbar");
        return 9;
    }
    key(VK_TAB, 1);
    if (focus(hw) != combo || !visible(hw, combo)) {
        puts("Shift+Tab did not return to the Status filter");
        return 10;
    }
    for (int i = 0; i < 2; ++i) {
        key(VK_F4, 0);
        if (SendMessageW(combo, CB_GETDROPPEDSTATE, 0, 0) == 0) {
            puts("F4 did not open the native list");
            return 11;
        }
        key(i == 0 ? VK_RETURN : VK_ESCAPE, 0);
        if (SendMessageW(combo, CB_GETDROPPEDSTATE, 0, 0) != 0 || focus(hw) != combo) {
            puts("the native list did not handle Enter or Escape");
            return 12;
        }
    }
    puts("Reading List passed Tab traversal, reactive focus fallback, scroll revelation, and native popup keys");
    return 0;
}

int main(void) {
    HWND window = find_window();
    if (!window) {
        puts("did not find the reading-list window");
        return 1;
    }
    int result = verify(window);
    PostMessageW(window, WM_CLOSE, 0, 0);
    return result;
}
