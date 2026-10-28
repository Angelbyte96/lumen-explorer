//! A frame of a video file, for the Icons cards and the preview panel, taken
//! by the system as PDF pages are: ffmpeg on Linux, the shell's own thumbnail
//! on Windows. Nothing is added to Lumen.

use std::path::Path;

use super::preview::Picture;

/// Video files, by extension. `.ts` is left out: it is mostly TypeScript.
const EXTENSIONS: &[&str] = &["mp4", "m4v", "mkv", "webm", "mov", "avi", "wmv", "flv", "mpg", "mpeg", "3gp", "ogv"];

/// A video that takes longer to give a frame is left without one: one limit
/// for ffprobe and every ffmpeg run together.
#[cfg(not(windows))]
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Why a video has no frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// ffmpeg is not installed.
    #[cfg(not(windows))]
    Missing,
    /// ffmpeg ran past the time allowed and was stopped.
    #[cfg(not(windows))]
    TimedOut,
    /// The video could not be read.
    Failed,
}

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// A frame of the video at `path`, scaled down so its longest side fits
/// `side`; `original` is the video's own size.
#[cfg(not(windows))]
pub fn frame(path: &Path, side: u32) -> Result<Picture, Failure> {
    frame_with("ffmpeg", "ffprobe", path, side, TIMEOUT)
}

/// Where to take the frame from, in seconds: a tenth of the way in, as
/// ffmpegthumbnailer does, past a black or fading start; the start when the
/// duration is unknown.
#[cfg(not(windows))]
fn seek_for(duration: Option<f64>) -> Option<f64> {
    duration.map(|seconds| seconds / 10.0)
}

/// The video's length in seconds, read by ffprobe (installed with ffmpeg).
#[cfg(not(windows))]
fn duration_with(program: &str, path: &Path, timeout: std::time::Duration) -> Option<f64> {
    let mut command = std::process::Command::new(program);
    command.args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"]);
    command.arg(Path::new(".").join(path));
    let out = super::preview::run_for(command, timeout).ok()?;
    // "N/A" when the container does not say.
    String::from_utf8_lossy(&out).trim().parse::<f64>().ok().filter(|seconds| seconds.is_finite() && *seconds > 0.0)
}

#[cfg(not(windows))]
fn frame_with(program: &str, probe: &str, path: &Path, side: u32, limit: std::time::Duration) -> Result<Picture, Failure> {
    use super::preview::RunError;
    let deadline = std::time::Instant::now() + limit;
    let left = || deadline.saturating_duration_since(std::time::Instant::now());
    // ffprobe and the seek get half of what is left each, so a slow one
    // still leaves time to read the start.
    // A seek that lands past the last frame gives nothing (and exits fine),
    // so the video is read again from the start.
    let seek = seek_for(duration_with(probe, path, left() / 2)).map(|seconds| format!("{seconds:.3}"));
    let mut png = Vec::new();
    for seek in [seek.as_deref(), None] {
        let mut command = std::process::Command::new(program);
        command.args(["-v", "error", "-nostdin"]);
        if let Some(seek) = seek {
            command.args(["-ss", seek]);
        }
        // A path starting with "-" would be read as an option.
        command.arg("-i").arg(Path::new(".").join(path));
        command.args(["-map", "0:v:0", "-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "-"]);
        let time = if seek.is_some() { left() / 2 } else { left() };
        png = match super::preview::run_for(command, time) {
            Ok(png) => png,
            Err(RunError::Missing) => return Err(Failure::Missing),
            // A seek deep into a slow video is given up for the start.
            Err(RunError::TimedOut) if seek.is_some() => Vec::new(),
            Err(RunError::TimedOut) => return Err(Failure::TimedOut),
            Err(RunError::Failed) => return Err(Failure::Failed),
        };
        // Without a seek there is nothing else to try.
        if !png.is_empty() || seek.is_none() {
            break;
        }
    }
    super::preview::decode_reader_fit(image::ImageReader::new(std::io::Cursor::new(png)), side).map_err(|_| Failure::Failed)
}

/// The shell's thumbnail of the video at `path`, its longest side `side`
/// pixels. The shell does not tell the video's own size: `original` is the
/// frame's.
#[cfg(windows)]
pub fn frame(path: &Path, side: u32) -> Result<Picture, Failure> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_THUMBNAILONLY};
    use windows::core::HSTRING;

    let side = i32::try_from(side).unwrap_or(i32::MAX);
    // SAFETY: joins (or finds) this worker thread's COM apartment; the bitmap
    // is ours and freed once its pixels are read.
    let pixels = unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).map_err(|_| Failure::Failed)?;
        // Thumbnail only: no icon in its place when the shell has no frame.
        let bitmap = factory.GetImage(SIZE { cx: side, cy: side }, SIIGBF_THUMBNAILONLY).map_err(|_| Failure::Failed)?;
        let pixels = crate::shell_menu::native::pixels(bitmap, side);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        pixels
    };
    let (size, rgba) = pixels.ok_or(Failure::Failed)?;
    Ok(Picture { size, original: [size[0] as u32, size[1] as u32], rgba })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fs::ops::tests::fresh_fixture;

    /// A `seconds`-long red video, `width` x `height`, made by ffmpeg.
    #[cfg(not(windows))]
    pub(crate) fn write_video(path: &Path, width: u32, height: u32, seconds: f32) {
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i"])
            .arg(format!("color=c=red:size={width}x{height}:rate=10:duration={seconds}"))
            .args(["-pix_fmt", "yuv420p"])
            .arg(path)
            .status()
            .expect("the video tests need ffmpeg");
        assert!(made.success(), "ffmpeg made {}", path.display());
    }

    /// A 20 s video, 40 x 20, black for its first 1.5 s and red after.
    #[cfg(not(windows))]
    fn write_late_start(path: &Path) {
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-y"])
            .args(["-f", "lavfi", "-i", "color=c=black:size=40x20:rate=10:duration=1.5"])
            .args(["-f", "lavfi", "-i", "color=c=red:size=40x20:rate=10:duration=18.5"])
            .args(["-filter_complex", "[0][1]concat=n=2:v=1", "-pix_fmt", "yuv420p"])
            .arg(path)
            .status()
            .expect("the video tests need ffmpeg");
        assert!(made.success(), "ffmpeg made {}", path.display());
    }

    #[cfg(not(windows))]
    #[test]
    fn the_frame_comes_from_a_tenth_of_the_way_in_past_a_black_start() {
        let dir = fresh_fixture("video-late-start");
        write_late_start(&dir.join("intro.mp4"));
        let picture = frame(&dir.join("intro.mp4"), 256).expect("a frame");
        let pixel = &picture.rgba[(10 * 40 + 20) * 4..][..4];
        assert!(pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60, "red at 2 s, not the black start: {pixel:?}");
    }

    #[cfg(not(windows))]
    #[test]
    fn the_seek_point_is_a_tenth_of_the_duration_or_the_start_when_unknown() {
        assert_eq!(seek_for(Some(600.0)), Some(60.0));
        assert_eq!(seek_for(Some(0.3)), Some(0.03));
        assert_eq!(seek_for(None), None);
    }

    #[test]
    fn videos_are_told_by_their_extension_in_any_case() {
        for name in ["a.mp4", "b.MKV", "c.webm", "d.Mov", "e.avi", "f.m4v", "g.ogv"] {
            assert!(is_video(Path::new(name)), "{name}");
        }
        for name in ["a.ts", "b.png", "c.mp3", "mp4", "d"] {
            assert!(!is_video(Path::new(name)), "{name}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn a_video_gives_a_frame_scaled_to_fit_keeping_its_shape() {
        let dir = fresh_fixture("video-frame");
        write_video(&dir.join("wide.mp4"), 320, 160, 2.0);
        let whole = frame(&dir.join("wide.mp4"), 2048).expect("a frame");
        assert_eq!((whole.size, whole.original), ([320, 160], [320, 160]), "a small frame is not scaled up");
        let pixel = &whole.rgba[(80 * 320 + 160) * 4..][..4];
        assert!(pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60 && pixel[3] == 255, "red: {pixel:?}");
        let fitted = frame(&dir.join("wide.mp4"), 64).expect("a frame");
        assert_eq!((fitted.size, fitted.original), ([64, 32], [320, 160]), "longest side 64, the video's own size kept");
    }

    #[cfg(not(windows))]
    #[test]
    fn a_very_short_video_still_gives_a_frame() {
        let dir = fresh_fixture("video-short");
        write_video(&dir.join("short.webm"), 40, 20, 0.3);
        assert_eq!(frame(&dir.join("short.webm"), 256).expect("a frame").size, [40, 20]);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_broken_video_or_no_ffmpeg_says_why() {
        let dir = fresh_fixture("video-broken");
        std::fs::write(dir.join("bad.mp4"), "not a video").unwrap();
        assert_eq!(frame(&dir.join("bad.mp4"), 256).err(), Some(Failure::Failed));
        assert_eq!(frame(&dir.join("missing.mp4"), 256).err(), Some(Failure::Failed));
        write_video(&dir.join("good.mp4"), 40, 20, 1.0);
        assert_eq!(frame_with("lumen-no-such-ffmpeg", "lumen-no-such-ffprobe", &dir.join("good.mp4"), 256, TIMEOUT).err(), Some(Failure::Missing));
    }

    /// An executable shell script named `name` in `dir`, standing in for
    /// ffmpeg or ffprobe; its path as a string.
    #[cfg(not(windows))]
    fn write_stand_in(dir: &Path, name: &str, script: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// ffmpeg that hangs when asked to seek and is the real one otherwise.
    #[cfg(not(windows))]
    const HANGS_ON_SEEK: &str = "case \" $* \" in *\" -ss \"*) exec sleep 30;; esac\nexec ffmpeg \"$@\"";

    #[cfg(not(windows))]
    #[test]
    fn ffprobe_and_ffmpeg_share_one_time_limit() {
        let dir = fresh_fixture("video-shared-limit");
        write_video(&dir.join("clip.mp4"), 40, 20, 1.0);
        let probe = write_stand_in(&dir, "probe", "exec sleep 30");
        let ffmpeg = write_stand_in(&dir, "ffmpeg", "exec sleep 30");
        let start = std::time::Instant::now();
        let got = frame_with(&ffmpeg, &probe, &dir.join("clip.mp4"), 256, std::time::Duration::from_secs(1));
        let took = start.elapsed();
        assert_eq!(got.err(), Some(Failure::TimedOut));
        assert!(took < std::time::Duration::from_millis(1500), "one 1 s limit for both, not one each: {took:?}");
    }

    #[cfg(not(windows))]
    #[test]
    fn a_slow_ffprobe_still_leaves_time_to_read_the_start() {
        let dir = fresh_fixture("video-slow-probe");
        write_video(&dir.join("clip.mp4"), 40, 20, 1.0);
        let probe = write_stand_in(&dir, "probe", "exec sleep 30");
        let start = std::time::Instant::now();
        let got = frame_with("ffmpeg", &probe, &dir.join("clip.mp4"), 256, std::time::Duration::from_secs(2));
        let took = start.elapsed();
        assert_eq!(got.expect("a frame from the start").size, [40, 20]);
        assert!(took < std::time::Duration::from_secs(2), "within the limit: {took:?}");
    }

    #[cfg(not(windows))]
    #[test]
    fn a_seek_that_runs_out_of_time_is_retried_from_the_start() {
        let dir = fresh_fixture("video-slow-seek");
        write_video(&dir.join("clip.mp4"), 40, 20, 2.0);
        let ffmpeg = write_stand_in(&dir, "ffmpeg", HANGS_ON_SEEK);
        let start = std::time::Instant::now();
        let got = frame_with(&ffmpeg, "ffprobe", &dir.join("clip.mp4"), 256, std::time::Duration::from_secs(2));
        let took = start.elapsed();
        assert_eq!(got.expect("a frame from the start").size, [40, 20]);
        assert!(took < std::time::Duration::from_secs(2), "within the limit: {took:?}");
    }

    #[cfg(not(windows))]
    #[test]
    fn a_seek_past_the_last_frame_is_retried_from_the_start() {
        let dir = fresh_fixture("video-seek-past-end");
        write_video(&dir.join("clip.mp4"), 40, 20, 1.0);
        // A container that claims 1000 s sends the seek to 100 s, past the end.
        let probe = write_stand_in(&dir, "probe", "echo 1000");
        let got = frame_with("ffmpeg", &probe, &dir.join("clip.mp4"), 256, TIMEOUT);
        assert_eq!(got.expect("a frame from the start").size, [40, 20]);
    }

    #[cfg(windows)]
    #[test]
    fn a_broken_video_has_no_frame() {
        let dir = fresh_fixture("video-broken-windows");
        std::fs::write(dir.join("bad.mp4"), "not a video").unwrap();
        assert_eq!(frame(&dir.join("bad.mp4"), 256).err(), Some(Failure::Failed));
        assert_eq!(frame(&dir.join("missing.mp4"), 256).err(), Some(Failure::Failed));
    }
}
