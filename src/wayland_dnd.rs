//! Files dropped on the window from other apps, on Wayland.
//!
//! winit 0.30 has no drag and drop on Wayland, so Lumen keeps its own queue on
//! winit's display: a `wl_data_device` that accepts `text/uri-list`, reads the
//! list once the files are dropped and hands the paths to the app. A thread
//! owns the queue: whoever reads the socket first (winit, usually) files our
//! events in our queue without waking the UI, so only a thread of ours sees them
//! and can ask for a repaint.

use std::io::Read;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use eframe::egui;

use crate::fs::transfer::Mode;
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use wayland_client::backend::{Backend, WaylandError};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop, event_created_child};

/// The kind of data a file manager offers for the files it drags.
const URI_LIST: &str = "text/uri-list";
/// The most of a list that is read: a drop of a few thousand files fits.
const MAX_LIST: u64 = 4 << 20;
/// How long the thread waits on the socket before it looks at the stop flag and
/// at events another reader filed in its queue.
const TICK: Duration = Duration::from_millis(50);

/// The local paths a `text/uri-list` names, in order. Comment lines, blank
/// lines and anything that is not a `file:` URI of this machine are skipped.
pub fn parse_uri_list(data: &[u8]) -> Vec<PathBuf> {
    data.split(|&b| b == b'\n').filter_map(|line| file_path(line.trim_ascii())).collect()
}

/// The path a `file:` URI names, when it is on this machine.
fn file_path(uri: &[u8]) -> Option<PathBuf> {
    const SCHEME: &[u8] = b"file://";
    if uri.len() < SCHEME.len() || !uri[..SCHEME.len()].eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    let rest = &uri[SCHEME.len()..];
    // `file://host/path`: only an empty host or this machine's own.
    let slash = rest.iter().position(|&b| b == b'/')?;
    let host = &rest[..slash];
    if !host.is_empty() && !host.eq_ignore_ascii_case(b"localhost") {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(unescape(&rest[slash..]))))
}

/// `%XX` escapes turned into their bytes; a `%` not followed by two hex digits
/// stays as it is.
fn unescape(text: &[u8]) -> Vec<u8> {
    let hex = |b: u8| char::from(b).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'%'
            && let (Some(high), Some(low)) = (text.get(i + 1).copied().and_then(hex), text.get(i + 2).copied().and_then(hex))
        {
            out.push(high << 4 | low);
            i += 3;
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    out
}

/// Reads a whole list from the pipe the dragging app writes to.
fn read_list(source: impl Read) -> std::io::Result<Vec<PathBuf>> {
    let mut data = Vec::new();
    source.take(MAX_LIST).read_to_end(&mut data)?;
    Ok(parse_uri_list(&data))
}

/// What the thread hands to the frames.
struct Shared {
    ctx: egui::Context,
    /// One entry per drop, in order, until a frame takes them.
    drops: Mutex<Vec<(Vec<PathBuf>, Mode)>>,
}

/// The Wayland side of drag and drop in: nothing until the first frame shows
/// a Wayland window, nothing at all on X11 and Windows.
#[derive(Default)]
pub struct Receiver {
    state: Setup,
}

#[derive(Default)]
enum Setup {
    /// No frame has told yet what the display is.
    #[default]
    Waiting,
    /// Not Wayland, or Wayland without what is needed: nothing to do.
    Off,
    Running(Worker),
}

struct Worker {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Receiver {
    /// Starts listening on the first frame whose window is a Wayland one, and
    /// returns the file lists dropped since the last call, each with the
    /// action agreed with the app they came from.
    pub fn poll(&mut self, frame: &eframe::Frame, ctx: &egui::Context) -> Vec<(Vec<PathBuf>, Mode)> {
        if matches!(self.state, Setup::Waiting) {
            match frame.display_handle().map(|handle| handle.as_raw()) {
                Ok(RawDisplayHandle::Wayland(wayland)) => self.state = Worker::start(wayland.display.as_ptr() as usize, ctx),
                Ok(_) => self.state = Setup::Off,
                // No handle yet: look again next frame.
                Err(_) => {}
            }
        }
        let Setup::Running(worker) = &self.state else { return Vec::new() };
        worker.shared.drops.lock().map(|mut drops| std::mem::take(&mut *drops)).unwrap_or_default()
    }

    /// Stops the thread. Before the window goes: it must not touch the display
    /// once winit has closed it.
    pub fn stop(&mut self) {
        if let Setup::Running(worker) = std::mem::take(&mut self.state) {
            worker.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = worker.thread {
                let _ = thread.join();
            }
        }
        self.state = Setup::Off;
    }
}

impl Worker {
    fn start(display: usize, ctx: &egui::Context) -> Setup {
        let shared = Arc::new(Shared { ctx: ctx.clone(), drops: Mutex::new(Vec::new()) });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop) = (shared.clone(), stop.clone());
            std::thread::Builder::new().name("lumen-wayland-dnd".into()).spawn(move || run(display, shared, stop))
        };
        match thread {
            Ok(thread) => Setup::Running(Worker { shared, stop, thread: Some(thread) }),
            Err(err) => {
                eprintln!("[lumen] drag and drop from other apps is off: {err}");
                Setup::Off
            }
        }
    }
}

/// What the dispatch handlers keep between events.
struct State {
    shared: Arc<Shared>,
    conn: Connection,
    /// The offer of the drag over the window, when it carries a file list.
    entered: Option<WlDataOffer>,
}

/// What an offer tells before the drag enters: its kinds of data, and the
/// action the compositor settled on.
#[derive(Default)]
struct OfferData {
    mimes: Mutex<Vec<String>>,
    action: Mutex<Option<DndAction>>,
}

impl OfferData {
    fn offers(&self, mime: &str) -> bool {
        self.mimes.lock().is_ok_and(|mimes| mimes.iter().any(|m| m == mime))
    }

    /// Copy or move was chosen (a version 1 or 2 offer has no actions to choose).
    fn accepted(&self, offer: &WlDataOffer) -> bool {
        offer.version() < 3 || self.action.lock().is_ok_and(|action| action.is_some_and(|a| a.intersects(DndAction::Copy | DndAction::Move)))
    }
}

/// The thread: binds the seat's data device, then lives in the queue until told to stop.
fn run(display: usize, shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    // SAFETY: the display is winit's and outlives this thread: the app joins it
    // in `Receiver::stop` before the window is closed.
    let conn = Connection::from_backend(unsafe { Backend::from_foreign_display(display as *mut _) });
    let mut state = State { shared, conn: conn.clone(), entered: None };
    let (globals, mut queue) = match registry_queue_init::<State>(&conn) {
        Ok(init) => init,
        Err(err) => return eprintln!("[lumen] drag and drop from other apps is off: {err}"),
    };
    let qh = queue.handle();
    let (Ok(seat), Ok(manager)) = (globals.bind::<WlSeat, _, _>(&qh, 1..=1, ()), globals.bind::<WlDataDeviceManager, _, _>(&qh, 1..=3, ())) else {
        return eprintln!("[lumen] drag and drop from other apps is off: the compositor has no seat or data device manager");
    };
    let _device = manager.get_data_device(&seat, &qh, ());

    while !stop.load(Ordering::Relaxed) {
        if let Err(err) = queue.dispatch_pending(&mut state) {
            return eprintln!("[lumen] drag and drop from other apps stopped: {err}");
        }
        match conn.flush() {
            Ok(()) | Err(WaylandError::Io(_)) => {}
            Err(err) => return eprintln!("[lumen] drag and drop from other apps stopped: {err}"),
        }
        // None: events are waiting in the queue already.
        let Some(guard) = queue.prepare_read() else { continue };
        let mut pollfd = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd, and the count says so.
        let ready = unsafe { libc::poll(&mut pollfd, 1, TICK.as_millis() as i32) };
        if ready > 0 {
            match guard.read() {
                Ok(_) => {}
                // Another reader took what was there.
                Err(WaylandError::Io(err)) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return eprintln!("[lumen] drag and drop from other apps stopped: {err}"),
            }
        }
        // Without data the guard is dropped, which cancels the read.
    }
}

impl State {
    /// A drag over the window offers `offer`: takes it when it carries files.
    fn entered(&mut self, serial: u32, offer: WlDataOffer) {
        if !offer.data::<OfferData>().is_some_and(|data| data.offers(URI_LIST)) {
            offer.destroy();
            return;
        }
        offer.accept(serial, Some(URI_LIST.into()));
        if offer.version() >= 3 {
            // Copy is preferred: without keys the dragging app keeps its files.
            // Shift there (a move) is honoured at the drop.
            offer.set_actions(DndAction::Copy | DndAction::Move, DndAction::Copy);
        }
        self.entered = Some(offer);
    }

    /// The files were let go over the window: reads their list, then reports it.
    fn dropped(&mut self) {
        let Some(offer) = self.entered.take() else { return };
        if !offer.data::<OfferData>().is_some_and(|data| data.accepted(&offer)) {
            offer.destroy();
            return;
        }
        let mode = mode_of(offer.data::<OfferData>().and_then(|data| data.action.lock().ok().and_then(|action| *action)));
        let Some((read, write)) = pipe() else {
            offer.destroy();
            return;
        };
        offer.receive(URI_LIST.into(), write.as_fd());
        // The other end is the source's now; ours must close or the read never ends.
        drop(write);
        let _ = self.conn.flush();
        let (shared, conn) = (self.shared.clone(), self.conn.clone());
        let reader = {
            let offer = offer.clone();
            std::thread::Builder::new().name("lumen-wayland-dnd-read".into()).spawn(move || {
                match read_list(std::fs::File::from(read)) {
                    Ok(paths) => {
                        if !paths.is_empty() {
                            if let Ok(mut drops) = shared.drops.lock() {
                                drops.push((paths, mode));
                            }
                            shared.ctx.request_repaint();
                        }
                        // The list is read: the drop is done as far as the source cares.
                        if offer.version() >= 3 {
                            offer.finish();
                        }
                    }
                    Err(err) => eprintln!("[lumen] reading the dropped files failed: {err}"),
                }
                offer.destroy();
                let _ = conn.flush();
            })
        };
        if let Err(err) = reader {
            eprintln!("[lumen] reading the dropped files failed: {err}");
            offer.destroy();
        }
    }
}

/// What Lumen does with dropped files: a move only when the compositor
/// settled on one (Shift in the dragging app), else a copy, which leaves the
/// dragging app's files where they are.
fn mode_of(action: Option<DndAction>) -> Mode {
    match action {
        Some(action) if action == DndAction::Move => Mode::Move,
        _ => Mode::Copy,
    }
}

/// A pipe: the end to read from, and the end the dragging app writes to.
fn pipe() -> Option<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors pipe2 fills in.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return None;
    }
    // SAFETY: pipe2 just made both, and nothing else owns them.
    Some(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(state: &mut Self, _: &WlDataDevice, event: wl_data_device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_device::Event::Enter { serial, id, .. } => {
                if let Some(old) = state.entered.take() {
                    old.destroy();
                }
                if let Some(offer) = id {
                    state.entered(serial, offer);
                }
            }
            wl_data_device::Event::Leave => {
                if let Some(offer) = state.entered.take() {
                    offer.destroy();
                }
            }
            wl_data_device::Event::Drop => state.dropped(),
            // The clipboard's offer is of no use here.
            wl_data_device::Event::Selection { id: Some(offer) } => offer.destroy(),
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, OfferData::default()),
    ]);
}

impl Dispatch<WlDataOffer, OfferData> for State {
    fn event(_: &mut Self, _: &WlDataOffer, event: wl_data_offer::Event, data: &OfferData, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_offer::Event::Offer { mime_type } => {
                if let Ok(mut mimes) = data.mimes.lock() {
                    mimes.push(mime_type);
                }
            }
            wl_data_offer::Event::Action { dnd_action } => {
                if let Ok(mut action) = data.action.lock() {
                    *action = dnd_action.into_result().ok();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: WlDataDeviceManager);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_settled_move_moves() {
        assert_eq!(mode_of(Some(DndAction::Move)), Mode::Move);
        assert_eq!(mode_of(Some(DndAction::Copy)), Mode::Copy);
        assert_eq!(mode_of(Some(DndAction::Ask)), Mode::Copy);
        assert_eq!(mode_of(None), Mode::Copy, "a version 1 or 2 offer agrees on nothing");
    }

    fn paths(text: &str) -> Vec<PathBuf> {
        parse_uri_list(text.as_bytes())
    }

    #[test]
    fn each_file_uri_is_a_path_in_order() {
        assert_eq!(
            paths("file:///home/me/a.txt\r\nfile:///home/me/b.txt\r\n"),
            [PathBuf::from("/home/me/a.txt"), PathBuf::from("/home/me/b.txt")]
        );
    }

    #[test]
    fn lines_end_with_a_bare_newline_or_none() {
        assert_eq!(paths("file:///a\nfile:///b"), [PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn percent_escapes_are_decoded() {
        assert_eq!(paths("file:///tmp/a%20b%23c%25.txt"), [PathBuf::from("/tmp/a b#c%.txt")]);
        assert_eq!(paths("file:///tmp/%C3%B1and%C3%BA"), [PathBuf::from("/tmp/ñandú")]);
    }

    #[test]
    fn a_bad_escape_stays_as_it_is() {
        assert_eq!(paths("file:///tmp/100%\nfile:///tmp/a%zzb\nfile:///tmp/c%4"), [
            PathBuf::from("/tmp/100%"),
            PathBuf::from("/tmp/a%zzb"),
            PathBuf::from("/tmp/c%4")
        ]);
    }

    #[test]
    fn bytes_that_are_not_utf8_survive() {
        use std::os::unix::ffi::OsStrExt;
        let got = parse_uri_list(b"file:///tmp/a%FFb");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].as_os_str().as_bytes(), b"/tmp/a\xffb");
    }

    #[test]
    fn comments_blank_lines_and_other_schemes_are_skipped() {
        let list = "# dropped from the browser\r\n\r\nhttps://example.com/a.png\r\nfile:///keep\r\nftp://host/x\r\n  \r\n#file:///no\r\n";
        assert_eq!(paths(list), [PathBuf::from("/keep")]);
    }

    #[test]
    fn only_this_machines_host_is_accepted() {
        assert_eq!(paths("file://localhost/a\nfile://other-pc/b\nfile:///c"), [PathBuf::from("/a"), PathBuf::from("/c")]);
    }

    #[test]
    fn a_uri_that_names_no_absolute_path_is_skipped() {
        assert_eq!(paths("file:\nfile://\nfile://host\nfile:relative\nfile:///ok"), [PathBuf::from("/ok")]);
    }

    #[test]
    fn the_scheme_is_case_insensitive_and_the_line_is_trimmed() {
        assert_eq!(paths("  FILE:///a \t\r\nFile:///b"), [PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn nothing_listed_is_an_empty_list() {
        assert!(paths("").is_empty());
        assert!(paths("\r\n# only a comment\r\n").is_empty());
    }

    #[test]
    fn a_list_is_read_up_to_its_end() {
        let got = read_list(&b"file:///a\r\nfile:///b%20c\r\n"[..]).unwrap();
        assert_eq!(got, [PathBuf::from("/a"), PathBuf::from("/b c")]);
    }

    #[test]
    fn a_list_that_never_ends_is_cut_at_the_cap() {
        /// A source that keeps writing the same line.
        struct Endless;
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                buf.fill(b'\n');
                Ok(buf.len())
            }
        }
        // Blank lines only: the read ends at the cap instead of hanging or filling memory.
        assert!(read_list(Endless).unwrap().is_empty());
    }
}
