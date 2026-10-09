//! The thumbnail File Explorer shows for a file (Windows only): what the
//! preview panel draws for PDFs, from whatever PDF handler the PC has.

use std::path::Path;

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT, SIIGBF_THUMBNAILONLY};
use windows::core::HSTRING;

use super::preview::Picture;

/// The file's thumbnail, its longest side at most `side`. Only a real
/// thumbnail: a file whose type has no thumbnail handler gives the reason.
pub fn of(path: &Path, side: u32) -> Result<Picture, String> {
    let none = || "No thumbnail for this PDF: install a PDF app that gives File Explorer one".to_owned();
    // SAFETY: COM on the preview thread (this call joins its apartment, or
    // finds it joined); the bitmap is ours and deleted once read.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).map_err(|_| none())?;
        let size = SIZE { cx: side as i32, cy: side as i32 };
        let bitmap = factory.GetImage(size, SIIGBF_RESIZETOFIT | SIIGBF_THUMBNAILONLY).map_err(|_| none())?;
        let pixels = crate::shell_menu::native::pixels(bitmap, side as i32);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let (size, rgba) = pixels.ok_or_else(none)?;
        Ok(Picture { size, original: [size[0] as u32, size[1] as u32], rgba })
    }
}
