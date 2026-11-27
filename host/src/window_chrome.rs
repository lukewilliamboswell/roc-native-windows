use crate::Result;
use crate::progress::Progress;
use crate::style::Accent;
use crate::wire::{Key, Kind, Tree};

pub(crate) struct WindowDeclaration<'a> {
    pub(crate) content: Key,
    pub(crate) status: Option<StatusDeclaration<'a>>,
    pub(crate) taskbar_progress: Progress,
    pub(crate) accent: Accent,
}

pub(crate) struct StatusDeclaration<'a> {
    primary: &'a str,
    trailing: Vec<(&'a str, u32)>,
}

impl<'a> WindowDeclaration<'a> {
    pub(crate) fn read(tree: &'a Tree, wrapper: Option<Key>, content_root: Key) -> Result<Self> {
        let Some(wrapper) = wrapper else {
            return Ok(Self {
                content: content_root,
                status: None,
                taskbar_progress: Progress::Idle,
                accent: Accent::default(),
            });
        };

        let mut content = None;
        let mut status = None;
        let children = elements(tree, wrapper);
        for (child, tag) in children {
            match tag {
                "ui-window-content" if content.is_none() => content = Some(child),
                "ui-status-bar" if status.is_none() => {
                    status = Some(StatusDeclaration::read(tree, child)?)
                }
                _ => return Err(format!("unexpected or repeated window declaration {tag:?}")),
            }
        }
        Ok(Self {
            content: content.ok_or("Ui.window needs a content element")?,
            status,
            taskbar_progress: Progress::parse(
                tree.node(wrapper).attr("taskbar-progress").unwrap_or(""),
            )?,
            accent: Accent::parse(tree.node(wrapper).attr("accent").unwrap_or(""))?,
        })
    }
}

impl<'a> StatusDeclaration<'a> {
    fn read(tree: &'a Tree, key: Key) -> Result<Self> {
        let primary = tree.node(key).attr("text").unwrap_or("");
        let mut trailing = Vec::new();
        for (part, tag) in elements(tree, key) {
            if tag != "ui-status-part" {
                return Err(format!("unexpected {tag:?} under a status bar"));
            }
            let part = tree.node(part);
            let text = part.attr("text").unwrap_or("");
            let width = part
                .attr("width")
                .unwrap_or("128")
                .parse::<u32>()
                .map_err(|_| "invalid status pane width")?
                .clamp(1, 4096);
            trailing.push((text, width));
        }
        if trailing.len() > 254 {
            return Err("a status bar accepts at most 254 trailing panes".into());
        }
        Ok(Self { primary, trailing })
    }
}

pub(crate) fn elements(tree: &Tree, parent: Key) -> impl Iterator<Item = (Key, &str)> + Clone {
    tree.node(parent)
        .children
        .iter()
        .filter_map(|&key| match &tree.node(key).kind {
            Kind::Element(tag) => Some((key, tag.as_str())),
            _ => None,
        })
}

fn pane_edges(
    width: i32,
    minimum_primary: i32,
    trailing: impl IntoIterator<Item = i32>,
) -> Vec<i32> {
    let mut primary = width;
    let mut remaining = Vec::new();
    for part in trailing {
        if primary - part < minimum_primary {
            break;
        }
        primary -= part;
        remaining.push(part);
    }
    let mut edges = Vec::with_capacity(remaining.len() + 1);
    edges.push(primary);
    for part in remaining {
        primary += part;
        edges.push(primary);
    }
    *edges.last_mut().unwrap() = -1;
    edges
}

#[cfg(windows)]
pub(crate) mod native {
    use super::{StatusDeclaration, pane_edges};
    use crate::Result;
    use crate::view::{child, position, scale, wide};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::HFONT;
    use windows_sys::Win32::UI::Controls::{
        CCS_NOPARENTALIGN, CCS_NORESIZE, SB_GETPARTS, SB_GETTEXTLENGTHW, SB_GETTEXTW, SB_SETPARTS,
        SB_SETTEXTW, STATUSCLASSNAMEW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, HWND_TOP, SendMessageW, WM_SETFONT,
    };

    pub(crate) struct StatusBar {
        pub(crate) handle: HWND,
        primary: String,
        trailing: Vec<(String, u32)>,
        edges: Vec<i32>,
        displayed: Vec<String>,
    }

    impl StatusBar {
        pub(crate) fn new(window: HWND, font: HFONT) -> Result<Self> {
            let handle = child(
                window,
                font,
                0,
                STATUSCLASSNAMEW,
                (CCS_NORESIZE | CCS_NOPARENTALIGN) as u32,
            )?;
            Ok(Self {
                handle,
                primary: String::new(),
                trailing: Vec::new(),
                edges: Vec::new(),
                displayed: Vec::new(),
            })
        }

        pub(crate) fn update(&mut self, declaration: StatusDeclaration<'_>) {
            if self.primary != declaration.primary {
                self.primary = declaration.primary.into();
            }
            if !self
                .trailing
                .iter()
                .map(|(text, width)| (text.as_str(), *width))
                .eq(declaration.trailing.iter().copied())
            {
                self.trailing = declaration
                    .trailing
                    .into_iter()
                    .map(|(text, width)| (text.into(), width))
                    .collect();
            }
        }

        pub(crate) fn font(&self, font: HFONT) {
            // SAFETY: The child status bar and its new font are live.
            unsafe { SendMessageW(self.handle, WM_SETFONT, font as usize, 0) };
        }

        pub(crate) fn arrange(
            &mut self,
            width: i32,
            height: i32,
            dpi: u32,
            text_scale: f64,
        ) -> Result<i32> {
            let bar_height = scale(dpi, (24.0 * text_scale.max(1.0)).round() as i32).min(height);
            let edges = pane_edges(
                width,
                scale(dpi, 72),
                self.trailing
                    .iter()
                    .map(|(_, width)| scale(dpi, *width as i32)),
            );
            if self.edges != edges {
                // SAFETY: Each native right edge remains readable for the synchronous call.
                if unsafe {
                    SendMessageW(
                        self.handle,
                        SB_SETPARTS,
                        edges.len(),
                        edges.as_ptr() as isize,
                    )
                } == 0
                {
                    return Err("could not lay out the native status bar panes".into());
                }
                self.displayed.truncate(edges.len());
                self.edges = edges;
            }
            let texts = std::iter::once(self.primary.as_str())
                .chain(self.trailing.iter().map(|(text, _)| text.as_str()));
            for (index, text) in texts.take(self.edges.len()).enumerate() {
                if self
                    .displayed
                    .get(index)
                    .is_some_and(|previous| previous == text)
                {
                    continue;
                }
                let native = wide(text);
                // SAFETY: The status bar copies the terminated UTF-16 text synchronously.
                if unsafe {
                    SendMessageW(self.handle, SB_SETTEXTW, index, native.as_ptr() as isize)
                } == 0
                {
                    return Err(format!("could not set text for status pane {index}"));
                }
                if let Some(previous) = self.displayed.get_mut(index) {
                    *previous = text.into();
                } else {
                    self.displayed.push(text.into());
                }
            }
            position(
                self.handle,
                HWND_TOP,
                0,
                height - bar_height,
                width,
                bar_height,
            );
            Ok(bar_height)
        }

        pub(crate) fn texts(&self) -> Vec<String> {
            // SAFETY: This HWND is a live status control on the UI thread.
            let count =
                unsafe { SendMessageW(self.handle, SB_GETPARTS, 0, 0) }.clamp(0, 255) as usize;
            (0..count)
                .map(|index| {
                    // SAFETY: The selected part exists, and the two messages run without an intervening update.
                    unsafe {
                        let len =
                            SendMessageW(self.handle, SB_GETTEXTLENGTHW, index, 0) as u16 as usize;
                        let mut text = vec![0u16; len + 1];
                        SendMessageW(self.handle, SB_GETTEXTW, index, text.as_mut_ptr() as isize);
                        String::from_utf16_lossy(&text[..len])
                    }
                })
                .collect()
        }
    }

    impl Drop for StatusBar {
        fn drop(&mut self) {
            // SAFETY: This status bar owns its child window on the UI thread.
            unsafe { DestroyWindow(self.handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{ROOT, Reader, put_str};

    fn add_element(tree: &mut Tree, id: u32, parent: u32, tag: &str) {
        let mut command = Vec::new();
        for number in [id, parent, 0] {
            command.extend(number.to_le_bytes());
        }
        command.push(0);
        put_str(&mut command, "");
        put_str(&mut command, tag);
        command.push(0);
        assert!(tree.command(8, &mut Reader::new(&command)).unwrap());
    }

    #[test]
    fn places_the_root_wrapper_and_restores_implicit_content_when_removed() {
        let mut tree = Tree::new();
        add_element(&mut tree, 1, 0, "ui-window");
        add_element(&mut tree, 2, 1, "ui-window-content");
        add_element(&mut tree, 3, 1, "ui-status-bar");
        add_element(&mut tree, 4, 3, "ui-status-part");
        let window = WindowDeclaration::read(&tree, Some(1), ROOT).unwrap();
        assert_eq!(window.content, tree.node(1).children[0]);
        assert!(window.accent == Accent::default());
        let status = window.status.unwrap();
        assert_eq!(status.primary, "");
        assert_eq!(status.trailing, vec![("", 128)]);

        tree.command(5, &mut Reader::new(&1u32.to_le_bytes()))
            .unwrap();
        let window = WindowDeclaration::read(&tree, None, ROOT).unwrap();
        assert_eq!(window.content, ROOT);
        assert!(window.status.is_none());
        assert_eq!(window.taskbar_progress, Progress::Idle);
    }

    #[test]
    fn rejects_multiple_chrome_declarations() {
        let mut tree = Tree::new();
        add_element(&mut tree, 1, 0, "ui-window");
        add_element(&mut tree, 3, 1, "ui-window-content");
        add_element(&mut tree, 4, 1, "ui-status-bar");
        add_element(&mut tree, 5, 1, "ui-status-bar");
        assert!(WindowDeclaration::read(&tree, Some(1), ROOT).is_err());
    }

    #[test]
    fn fits_primary_and_keeps_only_trailing_panes_that_fit_in_order() {
        assert_eq!(pane_edges(300, 72, [100, 70]), [130, 230, -1]);
        assert_eq!(pane_edges(242, 72, [100, 70]), [72, 172, -1]);
        assert_eq!(pane_edges(241, 72, [100, 70]), [141, -1]);
        assert_eq!(pane_edges(171, 72, [100, 70]), [-1]);
        assert_eq!(pane_edges(48, 72, []), [-1]);
    }
}
