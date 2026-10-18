//! Files dragged over the window from other apps, and dropped on it, on Wayland.
//!
//! winit 0.30 has no drag and drop on Wayland, so Lumen keeps its own queue on
//! winit's display: a `wl_data_device` that reads the `text/uri-list` of a drag
//! when it enters the window, follows its position and hands both to the
//! frames, which light the folder under it as for a drag inside Lumen and
//! answer whether it would be taken. A thread owns the queue: whoever reads the
//! socket first (winit, usually) files our events in our queue without waking
//! the UI, so only a thread of ours sees them and can ask for a repaint.
//!
//! During such a drag the compositor owns the pointer: winit and egui get no
//! pointer events, so the position here is the only one there is.
//!
//! The other way, files out: an entry dragged inside Lumen whose pointer goes
//! past the window's edge becomes a Wayland drag, a `wl_data_source` offering
//! `text/uri-list` that the compositor carries to whatever app is under the
//! pointer; that app copies or moves the files, Lumen deletes nothing. The
//! compositor only starts a drag for the serial of the button press that holds
//! the implicit grab, which winit does not give out, so the thread keeps its
//! own `wl_pointer` on the seat for that.

use std::io::Read;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::drag::{Drag, Keys};
use crate::fs::transfer::Mode;
use crate::portal::{self, Transfer};
use crate::ui::dnd::Outside;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_client::backend::{Backend, ObjectId, WaylandError};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_pointer::{self, ButtonState, WlPointer};
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child};

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

/// Reads a whole list from the pipe the dragging app writes to. One that
/// reaches the cap is refused: its last line may be cut short into another
/// file's name.
fn read_list(source: impl Read) -> std::io::Result<Vec<PathBuf>> {
    let mut data = Vec::new();
    source.take(MAX_LIST).read_to_end(&mut data)?;
    if data.len() as u64 >= MAX_LIST {
        return Err(std::io::Error::other("the list of files is too long"));
    }
    Ok(parse_uri_list(&data))
}

/// What a drag out of Lumen offers, in the order of preference: the files as
/// URIs, the same files through the document portal (what a sandboxed app, a
/// Flatpak, needs: the host paths mean nothing to it; `filetransfer` then
/// `files`, the order Firefox asks in), then their paths as text (an editor or
/// a terminal takes those). Nautilus lists the text first; here the files stay
/// ahead of it so an app that takes both asks for the files.
const OFFERED: [&str; 5] = [URI_LIST, portal::FILE_TRANSFER, portal::FILES, "text/plain;charset=utf-8", "text/plain"];
/// How long a receiving app has to read what is written to it.
const WRITE_LIMIT: Duration = Duration::from_secs(10);
/// The left button of the pointer, in Linux input codes.
const BTN_LEFT: u32 = 0x110;

/// `paths` as a `text/uri-list`: one `file://` URI per line, every byte that
/// is not a letter, digit or one of `-._~/` percent-encoded. A path that is
/// not absolute names no file for another app and is left out.
fn uri_list(paths: &[PathBuf]) -> Vec<u8> {
    const UNRESERVED: &[u8] = b"-._~/";
    let mut out = Vec::new();
    for path in paths {
        let bytes = path.as_os_str().as_bytes();
        if bytes.first() != Some(&b'/') {
            continue;
        }
        out.extend_from_slice(b"file://");
        for &byte in bytes {
            if byte.is_ascii_alphanumeric() || UNRESERVED.contains(&byte) {
                out.push(byte);
            } else {
                out.extend_from_slice(format!("%{byte:02X}").as_bytes());
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// `paths` as text, one per line.
fn path_list(paths: &[PathBuf]) -> Vec<u8> {
    paths.iter().map(|path| path.as_os_str().as_bytes()).collect::<Vec<_>>().join(&b'\n')
}

/// What to write for a receiving app that asked for `mime`; `None` for a kind
/// that was never offered, and for a portal kind when the portal took no
/// transfer. The portal's kinds open a transfer of their own each time and
/// block on the session bus while they do.
fn payload_for(mime: &str, paths: &[PathBuf], transfer: &Transfer) -> Option<Vec<u8>> {
    match mime {
        URI_LIST => Some(uri_list(paths)),
        kind if portal::is_portal(kind) => transfer.key(paths).map(|key| portal::framed(&key)),
        text if OFFERED.contains(&text) => Some(path_list(paths)),
        _ => None,
    }
}

/// Whether to hand a drag over to the compositor now: Lumen carries one, its
/// pointer is outside the window, the press that holds the grab is known (the
/// serial `start_drag` needs) and that grab has not started one already.
fn should_start(carried: bool, outside: bool, serial: Option<u32>, started: bool) -> bool {
    carried && outside && serial.is_some() && !started
}

/// Writes all of `data` to `fd` (a receiving app's pipe) without ever
/// waiting longer than `limit` for a slow reader.
fn write_within(fd: OwnedFd, data: &[u8], limit: Duration) -> std::io::Result<()> {
    let raw = fd.as_raw_fd();
    // SAFETY: `raw` is open for as long as `fd` is.
    unsafe {
        let flags = libc::fcntl(raw, libc::F_GETFL);
        if flags < 0 || libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    let end = Instant::now() + limit;
    let mut sent = 0;
    while sent < data.len() {
        // SAFETY: the buffer is `data[sent..]`, `raw` is open.
        let wrote = unsafe { libc::write(raw, data[sent..].as_ptr().cast(), data.len() - sent) };
        if wrote >= 0 {
            sent += wrote as usize;
            continue;
        }
        let err = std::io::Error::last_os_error();
        match err.kind() {
            std::io::ErrorKind::Interrupted => {}
            std::io::ErrorKind::WouldBlock => {
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(std::io::ErrorKind::TimedOut.into());
                }
                let mut pollfd = libc::pollfd { fd: raw, events: libc::POLLOUT, revents: 0 };
                // SAFETY: one valid pollfd, and the count says so. Rounded up so a
                // fraction of a millisecond left is not a busy loop.
                unsafe { libc::poll(&mut pollfd, 1, left.as_millis().min(i32::MAX as u128) as i32 + 1) };
            }
            _ => return Err(err),
        }
    }
    Ok(())
}

/// The pointer of Lumen's own on the seat, as far as a drag out needs it: the
/// serial of the left button's press made over the window, while it is held.
#[derive(Default)]
struct Grab {
    /// The pointer is over the window's own surface (not its decorations).
    focus: bool,
    press: Option<u32>,
}

impl Grab {
    // The press outlives the focus: while the button is held the compositor
    // may move the focus onto the window's frame (a subsurface of it, on
    // GNOME) and back, and the grab, with its serial, goes on.
    fn entered(&mut self, main: bool) {
        self.focus = main;
    }

    fn left(&mut self) {
        self.focus = false;
    }

    fn button(&mut self, button: u32, pressed: bool, serial: u32) {
        if button == BTN_LEFT {
            self.press = (pressed && self.focus).then_some(serial);
        }
    }
}

/// What the thread and the frames share.
struct Shared {
    ctx: egui::Context,
    /// The drag over the window that carries files, while there is one.
    hover: Mutex<Option<Hover>>,
    /// The thread's connection once it has one: the frames flush their requests with it.
    conn: OnceLock<Connection>,
    /// winit's surface of the window (its decorations are surfaces of their
    /// own, with positions of their own); 0 when it is not known.
    surface: usize,
    /// What a drag out needs, once the thread has bound it.
    wire: OnceLock<Wire>,
    /// The thread's own pointer: the press that holds the grab.
    grab: Mutex<Grab>,
    /// The drag out that was handed to the compositor and has not ended.
    out: Mutex<Option<Out>>,
    /// The id of the last drag out.
    last_out: AtomicU64,
}

/// The objects a drag out is started with.
struct Wire {
    manager: WlDataDeviceManager,
    device: WlDataDevice,
    qh: QueueHandle<State>,
}

/// A drag out: the source is destroyed under the lock that holds it, so
/// nothing is ever requested of a dead one.
struct Out {
    id: u64,
    source: WlDataSource,
    /// The press that started it.
    serial: u32,
    paths: Arc<Vec<PathBuf>>,
    /// The portal transfers of these paths, one per receiver's request.
    transfer: Arc<Transfer>,
}

/// The id of the drag out a source belongs to.
struct SourceData {
    id: u64,
}

impl Shared {
    fn flush(&self) {
        if let Some(conn) = self.conn.get() {
            let _ = conn.flush();
        }
    }

    /// `surface` is the window's own, not one of its decorations.
    fn is_main(&self, surface: &WlSurface) -> bool {
        self.surface == 0 || surface.id().as_ptr() as usize == self.surface
    }

    /// Whether the drag out started by the press `serial` is still going.
    fn started(&self, serial: Option<u32>) -> bool {
        self.out.lock().is_ok_and(|out| out.as_ref().is_some_and(|out| Some(out.serial) == serial))
    }

    /// Hands `paths` to the compositor as a drag that starts at the press
    /// `serial`, from the window's surface, with no icon of its own.
    fn begin(&self, paths: Vec<PathBuf>, serial: u32) -> Result<(), String> {
        let (Some(conn), Some(wire)) = (self.conn.get(), self.wire.get()) else { return Err("not ready yet".into()) };
        if self.surface == 0 {
            return Err("the window's surface is not known".into());
        }
        // SAFETY: the surface is winit's and outlives the frames that call this.
        let origin = unsafe { ObjectId::from_ptr(WlSurface::interface(), self.surface as *mut _) }
            .ok()
            .and_then(|id| WlSurface::from_id(conn, id).ok())
            .ok_or("the window's surface is not a wl_surface")?;
        let mut out = self.out.lock().map_err(|_| "poisoned")?;
        // A drag that never finished (its receiver did not say) is over.
        if let Some(old) = out.take() {
            old.source.destroy();
        }
        let id = self.last_out.fetch_add(1, Ordering::Relaxed) + 1;
        let source = wire.manager.create_data_source(&wire.qh, SourceData { id });
        for mime in OFFERED {
            source.offer(mime.into());
        }
        if wire.manager.version() >= 3 {
            source.set_actions(DndAction::Copy | DndAction::Move);
        }
        wire.device.start_drag(Some(&source), &origin, None, serial);
        *out = Some(Out { id, source, serial, paths: Arc::new(paths), transfer: Arc::new(Transfer::new()) });
        drop(out);
        conn.flush().map_err(|err| err.to_string())
    }

    /// Ends drag out `id`: finished, cancelled, or replaced.
    fn end_source(&self, id: u64) {
        if let Ok(mut out) = self.out.lock()
            && out.as_ref().is_some_and(|out| out.id == id)
            && let Some(out) = out.take()
        {
            out.source.destroy();
        }
        self.flush();
    }

    /// A receiving app asked for `mime` of drag out `id` and gave `fd` to
    /// write it to. Written on a thread of its own (the app may be slow, the
    /// queue and the frames must not wait for it) and given up on after
    /// `WRITE_LIMIT`.
    fn serve(&self, id: u64, mime: &str, fd: OwnedFd) {
        let drag = self.out.lock().ok().and_then(|out| out.as_ref().filter(|out| out.id == id).map(|out| (out.paths.clone(), out.transfer.clone())));
        // Not a kind offered, or a drag that is over: the descriptor closes.
        let (Some((paths, transfer)), true) = (drag, OFFERED.contains(&mime)) else { return };
        let mime = mime.to_owned();
        // The payload is made on the writer's thread too: a portal kind waits
        // for the session bus, and neither the queue nor the frames may.
        let writer = std::thread::Builder::new().name("lumen-wayland-dnd-write".into()).spawn(move || {
            // No payload (the portal took no transfer): the descriptor closes.
            let Some(data) = payload_for(&mime, &paths, &transfer) else { return };
            match write_within(fd, &data, WRITE_LIMIT) {
                // The receiver closed its end: it has what it wanted, or gave up.
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {}
                Err(err) => eprintln!("[lumen] sending the dragged files failed: {err}"),
            }
        });
        if let Err(err) = writer {
            eprintln!("[lumen] sending the dragged files failed: {err}");
        }
    }
}

/// A drag over the window that carries files. The offer is destroyed under
/// the lock that holds it, so a frame never sends a request to a dead one.
struct Hover {
    /// Tells this drag from the one before it to the thread reading the list.
    id: u64,
    offer: WlDataOffer,
    serial: u32,
    track: Track,
    /// The last verdict told to the compositor.
    answered: Option<Option<Mode>>,
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
    /// The frame that was just drawn saw the files let go: `answer` ends the drop.
    releasing: bool,
    /// A drag out could not be started: not tried again for this window.
    out_failed: bool,
}

impl Receiver {
    /// Starts listening on the first frame whose window is a Wayland one, and
    /// gives the drag from another app over the window, once its file list is
    /// read: where it is (in egui points), the keys that stand for the action
    /// agreed with the dragging app, and whether it was let go.
    pub fn poll(&mut self, frame: &eframe::Frame, ctx: &egui::Context) -> Option<Outside> {
        if matches!(self.state, Setup::Waiting) {
            match frame.display_handle().map(|handle| handle.as_raw()) {
                Ok(RawDisplayHandle::Wayland(wayland)) => {
                    let surface = match frame.window_handle().map(|handle| handle.as_raw()) {
                        Ok(RawWindowHandle::Wayland(window)) => window.surface.as_ptr() as usize,
                        _ => 0,
                    };
                    self.state = Worker::start(wayland.display.as_ptr() as usize, surface, ctx);
                }
                Ok(_) => self.state = Setup::Off,
                // No handle yet: look again next frame.
                Err(_) => {}
            }
        }
        let Setup::Running(worker) = &mut self.state else { return None };
        let ppp = ctx.pixels_per_point();
        let native = ctx.native_pixels_per_point().unwrap_or(ppp);
        let outside = {
            let hover = worker.shared.hover.lock().ok()?;
            hover.as_ref().and_then(|hover| outside_of(&hover.track, agreed(&hover.offer), native, ppp))
        };
        worker.releasing = outside.as_ref().is_some_and(|outside| outside.released);
        outside
    }

    /// After a frame, with the action its target takes with no key held
    /// (`None`: no target takes the files) and whether a target took the drop
    /// that frame. Tells the compositor when that changed, which shows the
    /// cursor of the action or the no-drop one; ends a drop that was let go.
    pub fn answer(&mut self, verdict: Option<Mode>, took: bool) {
        let Setup::Running(worker) = &mut self.state else { return };
        let releasing = std::mem::take(&mut worker.releasing);
        let Ok(mut hover) = worker.shared.hover.lock() else { return };
        if releasing {
            if let Some(hover) = hover.take() {
                end_drop(&hover, took);
            }
        // Not once let go: set_actions on a dropped offer is a protocol error,
        // which would close the connection (and the window). The next frame ends it.
        } else if let Some(hover) = hover.as_mut().filter(|hover| !hover.track.released && hover.track.paths.is_some() && hover.answered != Some(verdict)) {
            send_verdict(hover, verdict);
            hover.answered = Some(verdict);
        }
        drop(hover);
        worker.shared.flush();
    }

    /// After the list is drawn: when an entry carried inside Lumen has its
    /// pointer past the window's edge, hands it to the compositor as a drag out
    /// (once per press) and ends egui's own drag, whose release egui will never
    /// see. Returns whether it did.
    pub fn carry_out(&mut self, ctx: &egui::Context) -> bool {
        let Setup::Running(worker) = &mut self.state else { return false };
        if worker.out_failed {
            return false;
        }
        let drag = crate::ui::dnd::carried_inside(ctx);
        let serial = worker.shared.grab.lock().ok().and_then(|grab| grab.press);
        let outside = drag.is_some() && crate::ui::dnd::left_window(ctx);
        if !should_start(drag.is_some(), outside, serial, worker.shared.started(serial)) {
            return false;
        }
        let (Some(drag), Some(serial)) = (drag, serial) else { return false };
        match worker.shared.begin(drag.paths, serial) {
            Ok(()) => {
                crate::ui::dnd::hand_off(ctx);
                true
            }
            Err(err) => {
                eprintln!("[lumen] dragging files out is off: {err}");
                worker.out_failed = true;
                false
            }
        }
    }

    /// Stops the thread. Before the window goes: it must not touch the display
    /// once winit has closed it.
    pub fn stop(&mut self) {
        if let Setup::Running(worker) = std::mem::take(&mut self.state) {
            worker.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = worker.thread {
                let _ = thread.join();
            }
            // A drag out still going ends with the window.
            if let Ok(mut out) = worker.shared.out.lock()
                && let Some(out) = out.take()
            {
                out.source.destroy();
            }
            worker.shared.flush();
        }
        self.state = Setup::Off;
    }
}

impl Worker {
    fn start(display: usize, surface: usize, ctx: &egui::Context) -> Setup {
        let shared = Arc::new(Shared {
            ctx: ctx.clone(),
            hover: Mutex::new(None),
            conn: OnceLock::new(),
            surface,
            wire: OnceLock::new(),
            grab: Mutex::new(Grab::default()),
            out: Mutex::new(None),
            last_out: AtomicU64::new(0),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop) = (shared.clone(), stop.clone());
            std::thread::Builder::new().name("lumen-wayland-dnd".into()).spawn(move || run(display, shared, stop))
        };
        match thread {
            Ok(thread) => Setup::Running(Worker { shared, stop, thread: Some(thread), releasing: false, out_failed: false }),
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
    /// The id of the last drag that carried files.
    last: u64,
    /// Lumen's own pointer on the seat, while the seat has one.
    pointer: Option<WlPointer>,
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

/// The action agreed with the dragging app. A version 1 or 2 offer agrees on
/// nothing: its files are copied, which leaves them where they are.
fn agreed(offer: &WlDataOffer) -> Option<DndAction> {
    if offer.version() < 3 {
        return Some(DndAction::Copy);
    }
    offer.data::<OfferData>().and_then(|data| data.action.lock().ok().and_then(|action| *action))
}

/// The thread: binds the seat's data device, then lives in the queue until told to stop.
fn run(display: usize, shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    // SAFETY: the display is winit's and outlives this thread: the app joins it
    // in `Receiver::stop` before the window is closed.
    let conn = Connection::from_backend(unsafe { Backend::from_foreign_display(display as *mut _) });
    let _ = shared.conn.set(conn.clone());
    let mut state = State { shared, conn: conn.clone(), last: 0, pointer: None };
    let (globals, mut queue) = match registry_queue_init::<State>(&conn) {
        Ok(init) => init,
        Err(err) => return eprintln!("[lumen] drag and drop from other apps is off: {err}"),
    };
    let qh = queue.handle();
    let (Ok(seat), Ok(manager)) = (globals.bind::<WlSeat, _, _>(&qh, 1..=5, ()), globals.bind::<WlDataDeviceManager, _, _>(&qh, 1..=3, ())) else {
        return eprintln!("[lumen] drag and drop from other apps is off: the compositor has no seat or data device manager");
    };
    let device = manager.get_data_device(&seat, &qh, ());
    let _ = state.shared.wire.set(Wire { manager, device, qh: qh.clone() });

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
    /// A drag enters the window at `at` with `offer`: follows it when it
    /// carries files over the window itself, and starts reading the list (it
    /// may be read before the drop) so the frames know the files while it hovers.
    fn entered(&mut self, serial: u32, surface: &WlSurface, at: (f64, f64), offer: WlDataOffer) {
        // Over a decoration the position is the decoration's: no target there.
        if !self.shared.is_main(surface) || !offer.data::<OfferData>().is_some_and(|data| data.offers(URI_LIST)) {
            offer.destroy();
            return;
        }
        let Some((read, write)) = pipe() else {
            offer.destroy();
            return;
        };
        offer.receive(URI_LIST.into(), write.as_fd());
        // The other end is the source's now; ours must close or the read never ends.
        drop(write);
        let _ = self.conn.flush();
        self.last += 1;
        let id = self.last;
        match self.shared.hover.lock() {
            Ok(mut hover) => *hover = Some(Hover { id, offer: offer.clone(), serial, track: Track { at, paths: None, released: false }, answered: None }),
            Err(_) => return offer.destroy(),
        }
        let shared = self.shared.clone();
        let reader = std::thread::Builder::new().name("lumen-wayland-dnd-read".into()).spawn(move || {
            let list = read_list(std::fs::File::from(read));
            shared.listed(id, list);
        });
        if let Err(err) = reader {
            eprintln!("[lumen] reading the dragged files failed: {err}");
            self.shared.forget(self.last);
        }
    }

    /// The drag is over the window no more (or it was cancelled).
    fn left(&mut self) {
        let Ok(mut hover) = self.shared.hover.lock() else { return };
        // Let go: the frame that saw it ends the drop, which a leave sent after
        // the drop must not take away.
        if hover.as_ref().is_some_and(|hover| hover.track.released) {
            return;
        }
        if let Some(hover) = hover.take() {
            hover.offer.destroy();
        }
        drop(hover);
        self.shared.flush();
        self.shared.ctx.request_repaint();
    }

    /// The pointer of the drag moved.
    fn moved(&mut self, at: (f64, f64)) {
        if let Ok(mut hover) = self.shared.hover.lock()
            && let Some(hover) = hover.as_mut()
        {
            hover.track.at = at;
            self.shared.ctx.request_repaint();
        }
    }

    /// The files were let go over the window: the next frame decides whether a
    /// target takes them. Not if no action was agreed (nothing was accepted).
    fn dropped(&mut self) {
        let Ok(mut hover) = self.shared.hover.lock() else { return };
        let accepted = hover.as_ref().is_some_and(|hover| hover.offer.data::<OfferData>().is_some_and(|data| data.accepted(&hover.offer)));
        if accepted {
            if let Some(hover) = hover.as_mut() {
                hover.track.released = true;
            }
        } else if let Some(hover) = hover.take() {
            hover.offer.destroy();
        }
        drop(hover);
        self.shared.flush();
        self.shared.ctx.request_repaint();
    }
}

impl Shared {
    /// The reader of drag `id` is done: its files are known, or the list could not be read.
    fn listed(&self, id: u64, list: std::io::Result<Vec<PathBuf>>) {
        match list {
            Ok(paths) => {
                if let Ok(mut hover) = self.hover.lock()
                    && let Some(hover) = hover.as_mut().filter(|hover| hover.id == id)
                {
                    hover.track.paths = Some(paths);
                }
            }
            Err(err) => {
                eprintln!("[lumen] reading the dragged files failed: {err}");
                self.forget(id);
            }
        }
        self.ctx.request_repaint();
    }

    /// Drops drag `id`, if it still is the one over the window.
    fn forget(&self, id: u64) {
        if let Ok(mut hover) = self.hover.lock()
            && hover.as_ref().is_some_and(|hover| hover.id == id)
            && let Some(hover) = hover.take()
        {
            hover.offer.destroy();
        }
        self.flush();
    }
}

/// Tells the compositor whether the files would be taken, and with which action preferred.
fn send_verdict(hover: &Hover, verdict: Option<Mode>) {
    let (accept, actions, preferred) = answer_of(verdict);
    if hover.offer.version() >= 3 {
        hover.offer.set_actions(actions, preferred);
    }
    hover.offer.accept(hover.serial, accept.then(|| URI_LIST.into()));
}

/// The drop is over for the dragging app: finished when a target took it, else
/// cancelled by destroying the offer. `finish` is a protocol error, which
/// ends the connection, unless an action was agreed.
fn end_drop(hover: &Hover, took: bool) {
    let settled = hover.offer.version() >= 3 && hover.offer.data::<OfferData>().is_some_and(|data| data.accepted(&hover.offer));
    if took && settled {
        hover.offer.finish();
    }
    hover.offer.destroy();
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

/// Where a drag over the window is and what has arrived of it, as the thread
/// learned it.
#[derive(Default)]
struct Track {
    /// Surface-local, logical pixels, as the compositor sends them.
    at: (f64, f64),
    /// The files; `None` until their list is read.
    paths: Option<Vec<PathBuf>>,
    /// Let go over the window.
    released: bool,
}

/// What the frames see of `track` given the action agreed with the dragging
/// app, or `None` while there is nothing to light (the list is not read yet,
/// or names no file here).
fn outside_of(track: &Track, action: Option<DndAction>, native: f32, ppp: f32) -> Option<Outside> {
    let paths = track.paths.clone().filter(|paths| !paths.is_empty())?;
    Some(Outside { drag: Drag { paths }, at: to_points(track.at, native, ppp), keys: keys_of(action, track.released), released: track.released })
}

/// The keys that stand for the action agreed with the dragging app. While
/// hovering only a settled copy or move counts; once let go, as `mode_of`
/// has it: a move only if one was settled, else a copy.
fn keys_of(action: Option<DndAction>, dropped: bool) -> Keys {
    if dropped {
        return match mode_of(action) {
            Mode::Move => Keys { ctrl: false, shift: true },
            Mode::Copy => Keys { ctrl: true, shift: false },
        };
    }
    match action {
        Some(action) if action == DndAction::Move => Keys { ctrl: false, shift: true },
        Some(action) if action == DndAction::Copy => Keys { ctrl: true, shift: false },
        _ => Keys::default(),
    }
}

/// A surface-local position in logical pixels, in egui points. A window at
/// scale 2 and zoom 1 has `native` 2 and `ppp` 2: the points are the pixels.
fn to_points(at: (f64, f64), native: f32, ppp: f32) -> egui::Pos2 {
    if ppp <= 0.0 {
        return egui::pos2(at.0 as f32, at.1 as f32);
    }
    egui::pos2(at.0 as f32 * native / ppp, at.1 as f32 * native / ppp)
}

/// What to tell the compositor of a verdict: whether to accept `text/uri-list`,
/// the actions the offer takes and the one it prefers.
fn answer_of(verdict: Option<Mode>) -> (bool, DndAction, DndAction) {
    match verdict {
        Some(Mode::Move) => (true, DndAction::Copy | DndAction::Move, DndAction::Move),
        Some(Mode::Copy) => (true, DndAction::Copy | DndAction::Move, DndAction::Copy),
        None => (false, DndAction::empty(), DndAction::empty()),
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
            wl_data_device::Event::Enter { serial, surface, x, y, id } => {
                // A drag still held from before is over.
                if let Ok(mut hover) = state.shared.hover.lock()
                    && let Some(old) = hover.take()
                {
                    old.offer.destroy();
                }
                if let Some(offer) = id {
                    state.entered(serial, &surface, (x, y), offer);
                }
                state.shared.ctx.request_repaint();
            }
            wl_data_device::Event::Motion { x, y, .. } => state.moved((x, y)),
            wl_data_device::Event::Leave => state.left(),
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
    fn event(state: &mut Self, _: &WlDataOffer, event: wl_data_offer::Event, data: &OfferData, _: &Connection, _: &QueueHandle<Self>) {
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
                // Ctrl or Shift in the dragging app: the label names the new action.
                state.shared.ctx.request_repaint();
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        let wl_seat::Event::Capabilities { capabilities: WEnum::Value(capabilities) } = event else { return };
        match (capabilities.contains(Capability::Pointer), state.pointer.is_some()) {
            (true, false) => state.pointer = Some(seat.get_pointer(qh, ())),
            (false, true) => {
                if let Some(pointer) = state.pointer.take().filter(|pointer| pointer.version() >= 3) {
                    pointer.release();
                }
                if let Ok(mut grab) = state.shared.grab.lock() {
                    *grab = Grab::default();
                }
            }
            _ => {}
        }
    }
}

/// Lumen's own pointer: only what a drag out needs of it, the press that holds the grab.
impl Dispatch<WlPointer, ()> for State {
    fn event(state: &mut Self, _: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Ok(mut grab) = state.shared.grab.lock() else { return };
        match event {
            wl_pointer::Event::Enter { surface, .. } => grab.entered(state.shared.is_main(&surface)),
            wl_pointer::Event::Leave { .. } => grab.left(),
            wl_pointer::Event::Button { serial, button, state: pressed, .. } => {
                grab.button(button, pressed == WEnum::Value(ButtonState::Pressed), serial);
                // A drag may already be past the edge, waiting for this serial.
                state.shared.ctx.request_repaint();
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, SourceData> for State {
    fn event(state: &mut Self, _: &WlDataSource, event: wl_data_source::Event, data: &SourceData, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => state.shared.serve(data.id, &mime_type, fd),
            // Another app took the files (or Lumen's own window did), or the
            // drag was cancelled (Escape, let go over nothing): either way it is over.
            wl_data_source::Event::DndFinished | wl_data_source::Event::Cancelled => state.shared.end_source(data.id),
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

delegate_noop!(State: WlDataDeviceManager);

#[cfg(test)]
mod tests {
    use super::*;

    fn track(paths: Option<&[&str]>, released: bool) -> Track {
        Track { at: (100.0, 50.0), paths: paths.map(|p| p.iter().map(PathBuf::from).collect()), released }
    }

    #[test]
    fn nothing_is_lit_until_the_list_has_arrived() {
        assert!(outside_of(&track(None, false), None, 1.0, 1.0).is_none());
        assert!(outside_of(&track(Some(&[]), false), None, 1.0, 1.0).is_none(), "a list naming no file here");
        assert!(outside_of(&track(None, true), Some(DndAction::Move), 1.0, 1.0).is_none(), "let go before the list: wait for it");
        let outside = outside_of(&track(Some(&["/a", "/b"]), false), None, 1.0, 1.0).expect("the list is known");
        assert_eq!(outside.drag.paths, [PathBuf::from("/a"), PathBuf::from("/b")]);
        assert!(!outside.released);
        assert_eq!(outside.at, egui::pos2(100.0, 50.0));
    }

    #[test]
    fn the_keys_follow_the_agreed_action_while_hovering() {
        let keys = |action| keys_of(action, false);
        assert_eq!(keys(Some(DndAction::Move)), Keys { ctrl: false, shift: true });
        assert_eq!(keys(Some(DndAction::Copy)), Keys { ctrl: true, shift: false });
        assert_eq!(keys(None), Keys::default(), "nothing agreed yet: the disks decide");
        assert_eq!(keys(Some(DndAction::Ask)), Keys::default());
        assert_eq!(keys(Some(DndAction::empty())), Keys::default());
    }

    #[test]
    fn a_drop_moves_only_if_a_move_was_agreed_else_it_copies() {
        assert_eq!(keys_of(Some(DndAction::Move), true), Keys { ctrl: false, shift: true });
        for action in [Some(DndAction::Copy), Some(DndAction::Ask), None] {
            assert_eq!(keys_of(action, true), Keys { ctrl: true, shift: false }, "{action:?}");
        }
        let outside = outside_of(&track(Some(&["/a"]), true), Some(DndAction::Move), 1.0, 1.0).expect("known");
        assert!(outside.released);
        assert_eq!(outside.keys, Keys { ctrl: false, shift: true });
    }

    #[test]
    fn positions_become_points_whatever_the_scale_and_the_zoom() {
        assert_eq!(to_points((100.0, 50.0), 1.0, 1.0), egui::pos2(100.0, 50.0));
        assert_eq!(to_points((100.0, 50.0), 2.0, 2.0), egui::pos2(100.0, 50.0), "scale 2: logical pixels are points");
        assert_eq!(to_points((100.0, 50.0), 2.0, 3.0), egui::pos2(100.0 * 2.0 / 3.0, 50.0 * 2.0 / 3.0), "zoomed in 1.5: fewer points");
        assert_eq!(to_points((100.0, 50.0), 1.0, 0.5), egui::pos2(200.0, 100.0), "zoomed out 0.5");
        assert_eq!(to_points((100.0, 50.0), 1.0, 0.0), egui::pos2(100.0, 50.0), "no scale yet: unchanged");
    }

    #[test]
    fn a_verdict_tells_the_compositor_to_accept_with_the_action_or_to_refuse() {
        let both = DndAction::Copy | DndAction::Move;
        assert_eq!(answer_of(Some(Mode::Move)), (true, both, DndAction::Move));
        assert_eq!(answer_of(Some(Mode::Copy)), (true, both, DndAction::Copy));
        assert_eq!(answer_of(None), (false, DndAction::empty(), DndAction::empty()), "over nothing that takes it: no-drop");
    }

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
    fn a_list_that_reaches_the_cap_is_refused_whole() {
        /// A source that keeps writing file lines.
        struct Endless;
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                for (i, byte) in buf.iter_mut().enumerate() {
                    *byte = b"file:///tmp/a long name\n"[i % 24];
                }
                Ok(buf.len())
            }
        }
        // The read ends at the cap instead of hanging or filling memory, and
        // the last line may be cut short (another file's name): nothing is taken.
        assert!(read_list(Endless).is_err());
        // Just under the cap: taken.
        let line = b"file:///tmp/x\n";
        let fits = line.repeat((MAX_LIST as usize - 1) / line.len());
        assert_eq!(read_list(&fits[..]).unwrap().len(), fits.len() / line.len());
    }

    fn uris(paths: &[&str]) -> String {
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        String::from_utf8(uri_list(&paths)).expect("a list is ASCII")
    }

    #[test]
    fn a_path_becomes_a_percent_encoded_file_uri_on_its_own_line() {
        assert_eq!(uris(&["/home/me/a.txt"]), "file:///home/me/a.txt\r\n");
        assert_eq!(uris(&["/tmp/a b.txt", "/tmp/c#d", "/tmp/100%", "/tmp/q?x"]), "file:///tmp/a%20b.txt\r\nfile:///tmp/c%23d\r\nfile:///tmp/100%25\r\nfile:///tmp/q%3Fx\r\n");
        assert_eq!(uris(&["/tmp/ñandú"]), "file:///tmp/%C3%B1and%C3%BA\r\n");
        assert_eq!(uris(&["/a-b_c.d~e/f"]), "file:///a-b_c.d~e/f\r\n", "unreserved characters and the slash stay");
        assert_eq!(uris(&[]), "");
    }

    #[test]
    fn the_list_is_ascii_and_a_path_that_is_not_absolute_is_left_out() {
        assert_eq!(uris(&["relative/a", "/ok", ""]), "file:///ok\r\n");
        let list = uris(&["/tmp/a b#c%d?e&f=g+h;i,j'k(l)m[n]o{p}q|r\\s\"t<u>v^w`x"]);
        assert!(list.is_ascii());
        let line = list.trim_end();
        assert!(!line.contains([' ', '#', '?', '\\', '\"', '<', '>', '|', '^', '`', '[', ']', '{', '}']), "{line}");
    }

    #[test]
    fn what_is_offered_reads_back_as_the_same_paths() {
        use std::os::unix::ffi::OsStringExt;
        let paths: Vec<PathBuf> = [
            "/tmp/a b.txt",
            "/tmp/ñandú ☃",
            "/tmp/c#d",
            "/tmp/100%",
            "/tmp/%41 not an escape",
            "/tmp/q?x=1&y=2",
            "/tmp/line\nbreak",
            "/tmp/tab\there",
            "/tmp/ü+plus",
            "/tmp/[x]{y}",
            "/tmp/file:///inner",
        ]
        .iter()
        .map(PathBuf::from)
        .chain([PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/a\xffb".to_vec()))])
        .collect();
        assert_eq!(parse_uri_list(&uri_list(&paths)), paths);
    }

    #[test]
    fn the_paths_are_offered_as_text_one_per_line_and_nothing_else_is_asked_for() {
        let paths = [PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/c")];
        assert_eq!(path_list(&paths), b"/tmp/a b\n/tmp/c");
        let transfer = Transfer::with(Box::new(|_| Ok("key".into())));
        assert_eq!(payload_for(URI_LIST, &paths, &transfer), Some(uri_list(&paths)));
        for text in ["text/plain;charset=utf-8", "text/plain"] {
            assert_eq!(payload_for(text, &paths, &transfer), Some(path_list(&paths)), "{text}");
        }
        assert_eq!(payload_for("image/png", &paths, &transfer), None);
        assert!(OFFERED.iter().all(|mime| payload_for(mime, &paths, &transfer).is_some()), "all that is offered can be served");
        assert_eq!(OFFERED[0], URI_LIST, "the files come first");
    }

    #[test]
    fn sandboxed_apps_are_offered_the_portal_kinds_after_the_files_and_before_the_text() {
        assert_eq!(
            OFFERED[..],
            [URI_LIST, "application/vnd.portal.filetransfer", "application/vnd.portal.files", "text/plain;charset=utf-8", "text/plain"]
        );
    }

    #[test]
    fn a_portal_kind_is_answered_with_the_transfer_key_and_its_nul() {
        let paths = [PathBuf::from("/tmp/a b")];
        let transfer = Transfer::with(Box::new(|paths| {
            assert_eq!(paths, [PathBuf::from("/tmp/a b")], "the dragged paths are what the transfer holds");
            Ok("k42".into())
        }));
        for mime in ["application/vnd.portal.filetransfer", "application/vnd.portal.files"] {
            assert_eq!(payload_for(mime, &paths, &transfer), Some(b"k42\0".to_vec()), "{mime}");
        }
    }

    #[test]
    fn without_a_portal_nothing_is_written_for_its_kinds_and_the_other_kinds_still_are() {
        let paths = [PathBuf::from("/tmp/a")];
        let transfer = Transfer::with(Box::new(|_| Err("no session bus".into())));
        for mime in ["application/vnd.portal.filetransfer", "application/vnd.portal.files"] {
            assert_eq!(payload_for(mime, &paths, &transfer), None, "{mime}");
        }
        assert_eq!(payload_for(URI_LIST, &paths, &transfer), Some(uri_list(&paths)));
        assert_eq!(payload_for("text/plain", &paths, &transfer), Some(path_list(&paths)));
    }

    #[test]
    fn the_files_and_the_text_never_open_a_transfer() {
        let paths = [PathBuf::from("/tmp/a")];
        let transfer = Transfer::with(Box::new(|_| panic!("no transfer for this kind")));
        for mime in [URI_LIST, "text/plain;charset=utf-8", "text/plain", "image/png"] {
            let _ = payload_for(mime, &paths, &transfer);
        }
    }

    #[test]
    fn a_drag_is_handed_over_only_when_carried_out_of_the_window_with_a_known_press() {
        assert!(should_start(true, true, Some(7), false));
        assert!(!should_start(false, true, Some(7), false), "nothing carried");
        assert!(!should_start(true, false, Some(7), false), "still inside the window");
        assert!(!should_start(true, true, None, false), "no press serial: start_drag would be refused");
        assert!(!should_start(true, true, Some(7), true), "this grab already started one");
    }

    #[test]
    fn the_press_of_the_left_button_is_kept_while_it_is_held_over_the_window() {
        let mut grab = Grab::default();
        grab.button(BTN_LEFT, true, 5);
        assert_eq!(grab.press, None, "no pointer over the window yet");
        grab.entered(true);
        grab.button(BTN_LEFT, true, 6);
        assert_eq!(grab.press, Some(6));
        grab.button(BTN_LEFT, true, 9);
        assert_eq!(grab.press, Some(9), "the last press");
        grab.button(0x111, true, 10);
        grab.button(0x111, false, 11);
        assert_eq!(grab.press, Some(9), "another button does not touch it");
        grab.button(BTN_LEFT, false, 12);
        assert_eq!(grab.press, None, "let go");
    }

    #[test]
    fn a_press_on_a_decoration_or_with_no_focus_is_no_drag_of_the_list() {
        let mut grab = Grab::default();
        grab.entered(true);
        grab.left();
        grab.button(BTN_LEFT, true, 4);
        assert_eq!(grab.press, None, "no focus after the leave");
        // Over the title bar (a surface of its own): a press there is no drag of the list.
        grab.entered(false);
        grab.button(BTN_LEFT, true, 5);
        assert_eq!(grab.press, None);
    }

    #[test]
    fn a_held_press_outlives_the_focus_passing_over_the_frame_and_back() {
        // A slow drag out on GNOME (M13, L35-L37): the frame's border is a
        // subsurface of the window, and the compositor moves the focus onto it
        // and back while the button is held.
        let mut grab = Grab::default();
        grab.entered(true);
        grab.button(BTN_LEFT, true, 7021);
        grab.left();
        assert_eq!(grab.press, Some(7021), "left for the frame's border");
        grab.entered(false);
        assert_eq!(grab.press, Some(7021), "over the frame's border");
        grab.left();
        grab.entered(true);
        assert_eq!(grab.press, Some(7021), "back on the window, past its edge");
        grab.button(BTN_LEFT, false, 7028);
        assert_eq!(grab.press, None, "let go");
    }

    fn reader_of(fd: OwnedFd) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut data = Vec::new();
            std::fs::File::from(fd).read_to_end(&mut data).unwrap();
            data
        })
    }

    #[test]
    fn what_is_written_reaches_a_reader_however_big() {
        let (read, write) = pipe().expect("a pipe");
        let reader = reader_of(read);
        // Far more than a pipe holds (64 KiB): the writer waits for the reader.
        let data: Vec<u8> = (0..1_500_000u32).map(|i| (i % 251) as u8).collect();
        write_within(write, &data, Duration::from_secs(10)).expect("written");
        assert_eq!(reader.join().unwrap(), data);
    }

    #[test]
    fn a_reader_that_never_reads_is_given_up_on() {
        let (read, write) = pipe().expect("a pipe");
        let started = std::time::Instant::now();
        let err = write_within(write, &vec![0u8; 1 << 20], Duration::from_millis(200)).expect_err("nobody reads");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5), "it does not hang");
        drop(read);
    }

    #[test]
    fn a_reader_that_went_away_is_an_error_not_a_crash() {
        let (read, write) = pipe().expect("a pipe");
        drop(read);
        let err = write_within(write, b"file:///a\r\n", Duration::from_secs(1)).expect_err("nobody to read");
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    }
}
