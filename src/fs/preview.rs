//! What the preview panel shows for an entry, loaded on a background thread.
//!
//! One worker thread serves every request and always jumps to the newest one,
//! so holding an arrow key over a folder of photos never decodes them all.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use super::entry::Entry;
use super::lister::read_entries;

/// Text files are read up to this many bytes; the rest is not shown.
pub const TEXT_LIMIT: usize = 256 * 1024;
/// Image files above this size are not decoded.
pub const IMAGE_LIMIT: u64 = 50 * 1024 * 1024;
/// RAM the decoder may use. A small file can claim a huge picture; this still
/// admits a 48-megapixel photo (8000 x 6000 RGBA is 183 MiB).
pub const IMAGE_MEMORY_LIMIT: u64 = 192 * 1024 * 1024;
/// Decoded images are scaled down so their longest side fits this.
pub const IMAGE_MAX_SIDE: u32 = 2048;

/// Decoded pixels, ready for the UI to upload as a texture.
pub struct Picture {
    /// Width and height of `rgba`, after scaling down.
    pub size: [usize; 2],
    /// Width and height of the file itself.
    pub original: [u32; 2],
    pub rgba: Vec<u8>,
}

pub enum Content {
    Folder(Result<Vec<Entry>, String>),
    Text { text: String, truncated: bool },
    Image(Picture),
    /// Nothing to show but the entry's details; the reason says why.
    Info(String),
}

/// Reads what to show for `path`. Runs on the worker thread.
pub fn load(path: &Path) -> Content {
    // Follows links, so a link to a folder previews the folder.
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) => return Content::Info(format!("Cannot read: {err}")),
    };
    if meta.is_dir() {
        return Content::Folder(read_entries(path).map_err(|e| e.to_string()));
    }
    // FIFOs and devices block or never end when read.
    if !meta.is_file() {
        return Content::Info("Special file: no preview".into());
    }
    if is_image(path) {
        if meta.len() > IMAGE_LIMIT {
            return Content::Info("Image larger than 50 MB: no preview".into());
        }
        return match decode_image(path) {
            Ok(picture) => Content::Image(picture),
            Err(image::ImageError::Limits(_)) => Content::Info("Image too large to preview".into()),
            Err(err) => Content::Info(format!("Cannot read image: {err}")),
        };
    }
    match read_text(path) {
        Ok(Some((text, truncated))) => Content::Text { text, truncated },
        Ok(None) => Content::Info("Binary file: no preview".into()),
        Err(err) => Content::Info(format!("Cannot read: {err}")),
    }
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["png", "jpg", "jpeg"].contains(&e.to_ascii_lowercase().as_str()))
}

fn decode_image(path: &Path) -> Result<Picture, image::ImageError> {
    // The format comes from the bytes, so a PNG named .jpg still works. The
    // size in the header is checked against the limit before any pixel is read.
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(IMAGE_MEMORY_LIMIT);
    reader.limits(limits);
    let mut image = reader.decode()?;
    let original = [image.width(), image.height()];
    if original.iter().any(|&side| side > IMAGE_MAX_SIDE) {
        image = image.thumbnail(IMAGE_MAX_SIDE, IMAGE_MAX_SIDE);
    }
    let rgba = image.into_rgba8();
    Ok(Picture {
        size: [rgba.width() as usize, rgba.height() as usize],
        original,
        rgba: rgba.into_raw(),
    })
}

/// The first `TEXT_LIMIT` bytes as text and whether there was more, or `None`
/// for binary content (a NUL byte or invalid UTF-8).
fn read_text(path: &Path) -> std::io::Result<Option<(String, bool)>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(TEXT_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > TEXT_LIMIT;
    bytes.truncate(TEXT_LIMIT);
    if bytes.contains(&0) {
        return Ok(None);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Some((text, truncated))),
        // The limit cut a multi-byte character: drop its first half.
        Err(err) if truncated && err.utf8_error().error_len().is_none() => {
            let valid = err.utf8_error().valid_up_to();
            let mut bytes = err.into_bytes();
            bytes.truncate(valid);
            Ok(String::from_utf8(bytes).ok().map(|text| (text, true)))
        }
        Err(_) => Ok(None),
    }
}

/// Result of one request.
pub struct Preview {
    pub id: u64,
    pub path: PathBuf,
    pub content: Content,
}

type Wake = Box<dyn FnOnce() + Send>;

pub struct Previewer {
    requests: Sender<(u64, PathBuf, Wake)>,
    results: Receiver<Preview>,
    latest_id: u64,
}

impl Previewer {
    /// Starts the worker; it stops when the previewer is dropped.
    pub fn new() -> Self {
        let (requests, inbox) = mpsc::channel::<(u64, PathBuf, Wake)>();
        let (outbox, results) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(mut request) = inbox.recv() {
                // Older requests are stale already: skip to the newest.
                while let Ok(newer) = inbox.try_recv() {
                    request = newer;
                }
                let (id, path, wake) = request;
                let content = load(&path);
                if outbox.send(Preview { id, path, content }).is_err() {
                    return;
                }
                wake();
            }
        });
        Self { requests, results, latest_id: 0 }
    }

    /// Asks for a preview of `path`; `on_done` runs on the worker when it is ready.
    pub fn request(&mut self, path: PathBuf, on_done: impl FnOnce() + Send + 'static) -> u64 {
        self.latest_id += 1;
        // The worker only stops when `self` is dropped, so this cannot fail.
        let _ = self.requests.send((self.latest_id, path, Box::new(on_done)));
        self.latest_id
    }

    /// The preview of the most recent request, once ready.
    pub fn poll(&self) -> Option<Preview> {
        let mut newest = None;
        while let Ok(preview) = self.results.try_recv() {
            if preview.id == self.latest_id {
                newest = Some(preview);
            }
        }
        newest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;
    use std::time::Duration;

    fn text_of(content: Content) -> (String, bool) {
        match content {
            Content::Text { text, truncated } => (text, truncated),
            _ => panic!("expected text"),
        }
    }

    fn info_of(content: Content) -> String {
        match content {
            Content::Info(reason) => reason,
            _ => panic!("expected info"),
        }
    }

    fn picture_of(content: Content) -> Picture {
        match content {
            Content::Image(picture) => picture,
            Content::Info(reason) => panic!("expected an image, got info: {reason}"),
            _ => panic!("expected an image"),
        }
    }

    #[test]
    fn a_folder_shows_its_entries_in_list_order() {
        let dir = fresh_fixture("preview-folder");
        std::fs::create_dir_all(dir.join("inner/zdir")).unwrap();
        std::fs::write(dir.join("inner/b.txt"), "").unwrap();
        std::fs::write(dir.join("inner/A.txt"), "").unwrap();
        let Content::Folder(Ok(entries)) = load(&dir.join("inner")) else { panic!("folder") };
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zdir", "A.txt", "b.txt"]);
    }

    #[test]
    fn a_small_text_file_is_shown_whole() {
        let dir = fresh_fixture("preview-text");
        std::fs::write(dir.join("main.ts"), "const a = 1;\nexport {};\n").unwrap();
        assert_eq!(text_of(load(&dir.join("main.ts"))), ("const a = 1;\nexport {};\n".into(), false));
        std::fs::write(dir.join("empty.sql"), "").unwrap();
        assert_eq!(text_of(load(&dir.join("empty.sql"))), (String::new(), false));
    }

    #[test]
    fn a_big_text_file_stops_at_the_limit_on_a_character_boundary() {
        let dir = fresh_fixture("preview-big-text");
        // "ñ" is two bytes; with one leading byte the limit falls inside one.
        let body = format!("x{}", "ñ".repeat(TEXT_LIMIT));
        std::fs::write(dir.join("big.txt"), &body).unwrap();
        let (text, truncated) = text_of(load(&dir.join("big.txt")));
        assert!(truncated);
        assert_eq!(text.len(), TEXT_LIMIT - 1, "the half character is dropped");
        assert!(body.starts_with(&text));
    }

    #[test]
    fn binary_files_are_not_shown_as_text() {
        let dir = fresh_fixture("preview-binary");
        std::fs::write(dir.join("app.bin"), b"ELF\x00\x01\x02").unwrap();
        assert_eq!(info_of(load(&dir.join("app.bin"))), "Binary file: no preview");
        std::fs::write(dir.join("latin1.txt"), b"caf\xE9 au lait").unwrap();
        assert_eq!(info_of(load(&dir.join("latin1.txt"))), "Binary file: no preview");
    }

    #[test]
    fn png_and_jpeg_images_are_decoded() {
        let dir = fresh_fixture("preview-images");
        image::RgbImage::from_pixel(30, 20, image::Rgb([200, 10, 10]))
            .save(dir.join("red.png"))
            .unwrap();
        image::RgbImage::from_pixel(16, 8, image::Rgb([10, 200, 10]))
            .save(dir.join("green.JPG"))
            .unwrap();
        let png = picture_of(load(&dir.join("red.png")));
        assert_eq!((png.size, png.original), ([30, 20], [30, 20]));
        assert_eq!(png.rgba.len(), 30 * 20 * 4);
        assert_eq!(&png.rgba[..4], [200, 10, 10, 255]);
        let jpeg = picture_of(load(&dir.join("green.JPG")));
        assert_eq!((jpeg.size, jpeg.original), ([16, 8], [16, 8]));
    }

    #[test]
    fn large_images_are_scaled_down_keeping_their_shape() {
        let dir = fresh_fixture("preview-wide");
        image::GrayImage::new(IMAGE_MAX_SIDE * 2, 512).save(dir.join("wide.png")).unwrap();
        let picture = picture_of(load(&dir.join("wide.png")));
        assert_eq!(picture.original, [IMAGE_MAX_SIDE * 2, 512]);
        assert_eq!(picture.size, [IMAGE_MAX_SIDE as usize, 256]);
        assert_eq!(picture.rgba.len(), IMAGE_MAX_SIDE as usize * 256 * 4);
    }

    #[test]
    fn huge_or_broken_images_are_not_decoded() {
        let dir = fresh_fixture("preview-bad-images");
        // Sparse: takes no disk space, but reports more than the limit.
        let huge = File::create(dir.join("huge.png")).unwrap();
        huge.set_len(IMAGE_LIMIT + 1).unwrap();
        assert_eq!(info_of(load(&dir.join("huge.png"))), "Image larger than 50 MB: no preview");
        std::fs::write(dir.join("broken.jpeg"), b"not really a jpeg").unwrap();
        assert!(info_of(load(&dir.join("broken.jpeg"))).starts_with("Cannot read image: "));
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    /// A PNG whose header claims `width` x `height` (8-bit, `color` type), with
    /// no real pixel data: enough to test the size check without the memory.
    fn png_header_only(path: &Path, width: u32, height: u32, color: u8) {
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend(width.to_be_bytes());
        ihdr.extend(height.to_be_bytes());
        ihdr.extend([8, color, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        // The decoder reads up to the first data chunk before it knows the size.
        for chunk in [ihdr, b"IDAT".to_vec(), b"IEND".to_vec()] {
            png.extend((chunk.len() as u32 - 4).to_be_bytes());
            png.extend(&chunk);
            png.extend(crc32(&chunk).to_be_bytes());
        }
        std::fs::write(path, png).unwrap();
    }

    #[test]
    fn images_that_would_take_too_much_memory_are_refused_before_decoding() {
        let dir = fresh_fixture("preview-bomb");
        // 7200 x 7200 RGBA needs 207 MB of RAM, over the limit.
        png_header_only(&dir.join("bomb.png"), 7200, 7200, 6);
        assert_eq!(info_of(load(&dir.join("bomb.png"))), "Image too large to preview");
        // A 48-megapixel photo (8000 x 6000 RGB, 144 MB) is still accepted:
        // here it fails later, only because the pixel data is missing.
        png_header_only(&dir.join("photo.png"), 8000, 6000, 2);
        let reason = info_of(load(&dir.join("photo.png")));
        assert!(reason.starts_with("Cannot read image: "), "{reason}");
    }

    #[test]
    #[cfg(unix)] // FIFOs
    fn special_files_are_never_opened() {
        let dir = fresh_fixture("preview-fifo");
        let made = std::process::Command::new("mkfifo").arg(dir.join("pipe")).status();
        assert!(made.unwrap().success());
        // Opening a FIFO with no writer would block this test forever.
        assert_eq!(info_of(load(&dir.join("pipe"))), "Special file: no preview");
    }

    #[test]
    fn a_missing_entry_says_so() {
        let dir = fresh_fixture("preview-missing");
        assert!(info_of(load(&dir.join("gone.txt"))).starts_with("Cannot read: "));
    }

    #[test]
    fn the_previewer_delivers_only_the_latest_request() {
        let dir = fresh_fixture("preview-latest");
        std::fs::write(dir.join("a.txt"), "A").unwrap();
        std::fs::write(dir.join("b.txt"), "B").unwrap();
        let mut previewer = Previewer::new();
        let (tx, done) = mpsc::channel();
        let tx2 = tx.clone();
        let first = previewer.request(dir.join("a.txt"), move || { let _ = tx2.send(()); });
        let second = previewer.request(dir.join("b.txt"), move || { let _ = tx.send(()); });
        assert!(second > first);
        done.recv_timeout(Duration::from_secs(5)).unwrap();
        // The first request may or may not have run; only the second is reported.
        let mut got = None;
        for _ in 0..50 {
            if let Some(preview) = previewer.poll() {
                got = Some(preview);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let preview = got.expect("latest preview");
        assert_eq!((preview.id, preview.path.clone()), (second, dir.join("b.txt")));
        assert_eq!(text_of(preview.content).0, "B");
        assert!(previewer.poll().is_none());
    }
}
