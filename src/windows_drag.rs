//! Dragging entries out of Lumen on Windows. A drag carried inside Lumen
//! whose pointer leaves the window is handed to the system (OLE) with the
//! data object File Explorer offers for those paths: file drops for browser
//! uploads and editors, shell items for Explorer, and its drag image.

use std::path::PathBuf;

use eframe::egui;

/// After the list is drawn: when an entry carried inside Lumen has its
/// pointer past the window's edge, hands it to Windows as a drag out.
/// `window` is Lumen's window, the drag's owner. Returns whether it did.
pub fn carry_out(ctx: &egui::Context, window: Option<isize>) -> bool {
    carry_out_with(ctx, |paths| drag_out(paths, window))
}

/// `carry_out` with the system's drag as `start`, which returns once the
/// button is let go or Escape is pressed.
fn carry_out_with(
    ctx: &egui::Context,
    start: impl FnOnce(&[PathBuf]) -> Result<(), String>,
) -> bool {
    let Some(drag) = crate::ui::dnd::carried_inside(ctx) else {
        return false;
    };
    if !crate::ui::dnd::left_window(ctx) {
        return false;
    }
    // The system's drag keeps the release from egui, and a failed one must
    // not be tried again every frame: Lumen's drag ends here either way.
    crate::ui::dnd::hand_off(ctx);
    if let Err(err) = start(&drag.paths) {
        eprintln!("[lumen] dragging files out failed: {err}");
    }
    true
}

/// Runs the system's drag of `paths` until it ends. On winit's thread, which
/// has OLE: the drag's modal loop hands the window's messages to winit,
/// which keeps them until this frame returns.
fn drag_out(paths: &[PathBuf], window: Option<isize>) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Ole::{
        DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, OleInitialize, OleUninitialize,
    };
    use windows::Win32::UI::Shell::SHDoDragDrop;
    // SAFETY: OLE is counted per call, so this one is undone below; winit's
    // own stays. The window handle is Lumen's, alive for the whole drag.
    unsafe {
        let ole = OleInitialize(None).is_ok();
        let object = data_object(paths);
        // Every effect File Explorer offers: the app it lands on picks one
        // (Explorer moves on the same disk itself, a browser copies).
        let dragged = object.and_then(|object| {
            let window = window.map(|handle| HWND(handle as *mut core::ffi::c_void));
            SHDoDragDrop(
                window,
                &object,
                None,
                DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
            )
            .map(|_| ())
            .map_err(|e| format!("drag: {e}"))
        });
        if ole {
            OleUninitialize();
        }
        dragged
    }
}

/// The data object File Explorer offers for `paths` (they may be in
/// different folders, as in the Tree view). The caller's thread has OLE.
fn data_object(paths: &[PathBuf]) -> Result<windows::Win32::System::Com::IDataObject, String> {
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, ILFree, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
    };
    use windows::core::HSTRING;

    /// Item ids parsed from paths, freed when dropped.
    struct Pidls(Vec<*mut ITEMIDLIST>);
    impl Drop for Pidls {
        fn drop(&mut self) {
            for &pidl in &self.0 {
                // SAFETY: each came from SHParseDisplayName and is freed once.
                unsafe { ILFree(Some(pidl)) };
            }
        }
    }

    // SAFETY: every id outlives the calls that read it; the item array keeps
    // copies of its own.
    unsafe {
        let mut pidls = Pidls(Vec::with_capacity(paths.len()));
        for path in paths {
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut pidl, 0, None)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            pidls.0.push(pidl);
        }
        let ids: Vec<*const ITEMIDLIST> = pidls.0.iter().map(|&pidl| pidl as *const _).collect();
        let items = SHCreateShellItemArrayFromIDLists(&ids).map_err(|e| format!("items: {e}"))?;
        items
            .BindToHandler(None, &BHID_DataObject)
            .map_err(|e| format!("data: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drag::Drag;
    use crate::fs::ops::tests::fresh_fixture;
    use crate::ui::dnd::{carried_inside, end_hand_off, start};
    use crate::ui::testing::run;
    use eframe::egui::{Rect, pos2};
    use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
    use windows::Win32::System::Ole::{CF_HDROP, OleInitialize, OleUninitialize, ReleaseStgMedium};
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

    const ROW: Rect = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 130.0));

    /// A drag that began on a row at `ROW` and moved to `to`, button held.
    fn dragged_to(ctx: &egui::Context, paths: &[PathBuf], to: egui::Pos2) {
        let frame = |events: Vec<egui::Event>| {
            run(ctx, events, |ui| {
                let response = ui.allocate_rect(ROW, egui::Sense::click_and_drag());
                start(&response, || Drag {
                    paths: paths.to_vec(),
                });
            });
        };
        let press = egui::Event::PointerButton {
            pos: ROW.center(),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(ROW.center())]);
        frame(vec![press]);
        frame(vec![egui::Event::PointerMoved(pos2(500.0, 300.0))]);
        frame(vec![egui::Event::PointerMoved(to)]);
    }

    #[test]
    fn nothing_is_handed_to_windows_without_a_drag_or_inside_the_window() {
        let dir = fresh_fixture("windows-drag-inside");
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        let ctx = egui::Context::default();
        run(
            &ctx,
            vec![egui::Event::PointerMoved(pos2(900.0, 300.0))],
            |_| (),
        );
        let mut started = false;
        assert!(
            !carry_out_with(&ctx, |_| {
                started = true;
                Ok(())
            }),
            "no drag carried"
        );

        dragged_to(&ctx, &[dir.join("a.txt")], pos2(600.0, 300.0));
        assert!(
            !carry_out_with(&ctx, |_| {
                started = true;
                Ok(())
            }),
            "the pointer is still over the window"
        );
        assert!(!started, "the system's drag never began");
        assert!(carried_inside(&ctx).is_some(), "Lumen keeps its own drag");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_drag_past_the_edge_goes_to_windows_and_leaves_no_button_held() {
        let dir = fresh_fixture("windows-drag-out");
        let files = [dir.join("a.txt"), dir.join("b.png")];
        for file in &files {
            std::fs::write(file, "x").unwrap();
        }
        let ctx = egui::Context::default();
        dragged_to(&ctx, &files, pos2(850.0, 300.0));
        let mut given = None;
        assert!(carry_out_with(&ctx, |paths| {
            given = Some(paths.to_vec());
            Ok(())
        }));
        assert_eq!(
            given.as_deref(),
            Some(&files[..]),
            "the system's drag carries every dragged path"
        );
        assert!(carried_inside(&ctx).is_none(), "Lumen's own drag is over");
        // The release the system's drag kept from egui arrives the next frame.
        let mut raw = egui::RawInput::default();
        end_hand_off(&ctx, &mut raw);
        run(&ctx, raw.events, |_| ());
        assert!(
            !ctx.input(|i| i.pointer.any_down()),
            "egui does not think a button is held"
        );
        assert!(
            ctx.dragged_id().is_none(),
            "no widget is still being dragged"
        );
        // A failed system drag still ends Lumen's: it is not tried again every frame.
        dragged_to(&ctx, &files[..1], pos2(850.0, 300.0));
        assert!(carry_out_with(&ctx, |_| Err("refused".into())));
        assert!(carried_inside(&ctx).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The paths of the file drop (`CF_HDROP`) `object` gives to the app it lands on.
    fn file_drop(object: &IDataObject) -> Vec<PathBuf> {
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        // SAFETY: the medium is read while held and released once.
        unsafe {
            let mut medium = object.GetData(&format).expect("a file drop");
            let drop = HDROP(medium.u.hGlobal.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            let paths = (0..count)
                .map(|i| {
                    let mut name = vec![0u16; DragQueryFileW(drop, i, None) as usize + 1];
                    let len = DragQueryFileW(drop, i, Some(&mut name)) as usize;
                    PathBuf::from(String::from_utf16_lossy(&name[..len]))
                })
                .collect();
            ReleaseStgMedium(&mut medium);
            paths
        }
    }

    #[test]
    fn the_data_object_gives_every_path_as_a_file_drop_even_from_different_folders() {
        let dir = fresh_fixture("windows-drag-object");
        std::fs::create_dir(dir.join("sub")).unwrap();
        let paths = [
            dir.join("photo one.png"),
            dir.join("sub"),
            dir.join("sub").join("ñandú.jpg"),
        ];
        std::fs::write(&paths[0], "x").unwrap();
        std::fs::write(&paths[2], "x").unwrap();
        let given = paths.to_vec();
        let dropped = std::thread::spawn(move || {
            // SAFETY: OLE for this thread, undone once the object is gone.
            let _ = unsafe { OleInitialize(None) };
            let dropped = data_object(&given).map(|object| file_drop(&object));
            // SAFETY: undoes the call above.
            unsafe { OleUninitialize() };
            dropped
        })
        .join()
        .unwrap()
        .expect("the shell builds the data object");
        let real = |paths: &[PathBuf]| {
            paths
                .iter()
                .map(|path| std::fs::canonicalize(path).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(real(&dropped), real(&paths), "every path, in order");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
