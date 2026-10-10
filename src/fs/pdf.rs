//! A PDF's first page drawn by Windows itself (Windows.Data.Pdf, built into
//! Windows 10 and later): no PDF app needed, nothing added to Lumen.

use std::path::Path;

use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Storage::StorageFile;
use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::core::HSTRING;

/// The first page as PNG bytes, its longest side `side` pixels, on white.
pub fn first_page_png(path: &Path, side: u32) -> windows::core::Result<Vec<u8>> {
    // SAFETY: joins (or finds) the preview thread's COM apartment, which
    // Windows Runtime calls need.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))?.join()?;
    let document = PdfDocument::LoadFromFileAsync(&file)?.join()?;
    let page = document.GetPage(0)?;
    let size = page.Size()?;
    let scale = side as f32 / size.Width.max(size.Height).max(1.0);
    let options = PdfPageRenderOptions::new()?;
    options.SetDestinationWidth(((size.Width * scale).round() as u32).max(1))?;
    options.SetDestinationHeight(((size.Height * scale).round() as u32).max(1))?;
    let stream = InMemoryRandomAccessStream::new()?;
    page.RenderWithOptionsToStreamAsync(&stream, &options)?.join()?;
    let length = u32::try_from(stream.Size()?).unwrap_or(u32::MAX);
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    let loaded = reader.LoadAsync(length)?.join()?;
    let mut png = vec![0; loaded as usize];
    reader.ReadBytes(&mut png)?;
    Ok(png)
}
