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

use std::io::Read;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use eframe::egui;

use crate::drag::{Drag, Keys};
use crate::fs::transfer::Mode;
use crate::ui::dnd::Outside;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_client::backend::{Backend, WaylandError};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;
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
}

impl Shared {
    fn flush(&self) {
        if let Some(conn) = self.conn.get() {
            let _ = conn.flush();
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
    fn start(display: usize, surface: usize, ctx: &egui::Context) -> Setup {
        let shared = Arc::new(Shared { ctx: ctx.clone(), hover: Mutex::new(None), conn: OnceLock::new(), surface });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop) = (shared.clone(), stop.clone());
            std::thread::Builder::new().name("lumen-wayland-dnd".into()).spawn(move || run(display, shared, stop))
        };
        match thread {
            Ok(thread) => Setup::Running(Worker { shared, stop, thread: Some(thread), releasing: false }),
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
    let mut state = State { shared, conn: conn.clone(), last: 0 };
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
    /// A drag enters the window at `at` with `offer`: follows it when it
    /// carries files over the window itself, and starts reading the list (it
    /// may be read before the drop) so the frames know the files while it hovers.
    fn entered(&mut self, serial: u32, surface: &WlSurface, at: (f64, f64), offer: WlDataOffer) {
        // Over a decoration the position is the decoration's: no target there.
        let own = self.shared.surface == 0 || surface.id().as_ptr() as usize == self.shared.surface;
        if !own || !offer.data::<OfferData>().is_some_and(|data| data.offers(URI_LIST)) {
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

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

delegate_noop!(State: ignore WlSeat);
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
}
