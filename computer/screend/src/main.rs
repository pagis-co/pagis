//! The agent computer's screen daemon: one continuous
//! ext-image-copy-capture-v1 session against the headless labwc
//! compositor keeps the latest frame in memory; a control endpoint
//! serves it, and the WebRTC pipeline streams it — screenshots and the
//! stream can never disagree. The daemon reaches this endpoint through
//! the published container port. Media publishes no port: each viewer
//! session registers outbound with the daemon's Media Relay.
//!
//! The control endpoint listens on 0.0.0.0. Docker forwards a
//! published port to the container's own interface address, so a
//! listener on the container's loopback alone would answer nothing the
//! daemon sends; what keeps another tenant out is the per-tenant Docker
//! network and the bearer token on every request, not a narrower bind
//! address.
//!
//! Every endpoint but `GET /healthz` needs `Authorization: Bearer
//! <token>`, where the token is the file the daemon mounts into this
//! container (see `auth`). `/healthz` stays open because it carries
//! nothing of the person and because the image's readiness probe and
//! the daemon's boot probe call it before a token is in hand.
//!
//! Endpoints:
//!   GET /healthz    -> {ok, browser} once the compositor session is
//!                      up; `browser` reports whether the supervised
//!                      browser is alive. The one open endpoint.
//!   GET /frame.png  -> the latest frame, PNG-encoded
//!   POST /input     -> execute a batch of input operations
//!   POST /offer     -> WebRTC signaling: {sdp, candidate, relay, token}
//!                      -> {sdp}
//!   GET /windows    -> [{title, app_id}]: the compositor's window
//!                      list, through ext-foreign-toplevel-list-v1
//!   GET /holder     -> {holder, idle_ms}: the input switch and the
//!                      time since the last applied user input
//!   POST /holder    -> {holder}: the daemon flips the input switch
//!   POST /browser/open -> {url}: open the address in the daemon's own
//!                      tab and wait for the load event -> {url}, the
//!                      top-level address after every redirect
//!   GET /browser/page -> {url}: the top-level address of that tab
//!   POST /browser/fill -> {origin, fields}: write the values into
//!                      verified fields of that tab (see `fill`)
//!   GET /exit       -> {mode, connections}: the mode of the Exit Proxy
//!                      and the client connections it holds now
//!   POST /exit      -> {mode} -> {mode, closed}: set the mode of the
//!                      Exit Proxy and close every connection it holds
//!                      (see `exit`)
//!
//! Pointer operations inject natively through zwlr_virtual_pointer_v1
//! (absolute coordinates); text and key operations inject through a
//! second zwp_virtual_keyboard_v1 that carries a generated keymap,
//! so the image needs no `wtype` an agent shell could also run.
//! `POST /input` declares who it speaks for and is refused unless that
//! holder holds the switch, so the switch means the same at both ends.
//! The daemon sends no input batch: a Vault fill writes through the
//! browser channel, and the `/browser/` endpoints answer only while the
//! daemon holds the switch. screend starts the browser and holds its
//! DevTools pipe (see `browser`).
//! User input arrives over each viewer's WebRTC data channel and
//! is dropped unless the user holds the switch; its keyboard events
//! inject through zwp_virtual_keyboard_v1 with a fixed US keymap and a
//! static code→evdev map.
//!
//! screend also runs the Exit Proxy on loopback, on a runtime thread of
//! its own (see `exit`). Every connection of the browser and of the
//! shells goes through it.

mod auth;
mod browser;
mod cdp;
mod exit;
mod fill;
mod keymap;
mod typing;
mod webrtc;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool},
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};

const PORT: u16 = 7900;
/// How long the page of a fill gets to load.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a fill waits for the page to give the focus to a field.
const FOCUS_WAIT: Duration = Duration::from_secs(3);
/// One wheel click in wl_pointer axis units.
const WHEEL_STEP: f64 = 15.0;

/// One input operation, the shared wire shape with the daemon.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum InputOp {
    /// Absolute pointer move, in capture pixels.
    Move { x: f64, y: f64 },
    /// Press or release one pointer button.
    Button { button: String, down: bool },
    /// Scroll by wheel clicks (positive = right/down).
    Scroll { dx: f64, dy: f64 },
    /// Type text through the keyboard.
    Text { text: String },
    /// One key chord: modifiers held around the final keys, all xkb
    /// keysym names. `hold_ms` holds the chord before release.
    Key {
        keys: Vec<String>,
        #[serde(default)]
        hold_ms: Option<u64>,
    },
}

#[derive(Debug, Deserialize)]
struct InputBatch {
    /// Who the batch speaks for.
    holder: Holder,
    ops: Vec<InputOp>,
}

/// An op handed to the wayland thread, answered when injected.
type InputRequest = (InputOp, mpsc::Sender<Result<(), String>>);

/// Who drives the screen: the daemon-set input switch.
/// `Daemon` is the vault's held switch: the daemon fills a secret
/// through the browser channel, and nothing else may reach the keyboard
/// while it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Holder {
    Agent,
    User,
    Daemon,
}

impl Holder {
    fn as_str(self) -> &'static str {
        match self {
            Holder::Agent => "agent",
            Holder::User => "user",
            Holder::Daemon => "daemon",
        }
    }
}

/// The input switch plus the last time user input was applied, shared
/// between the control endpoint and the wayland thread.
struct HolderState {
    holder: Holder,
    last_input: std::time::Instant,
}

type SharedHolder = Arc<Mutex<HolderState>>;

/// One user input event from a viewer's data channel.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum UserInput {
    Move { x: f64, y: f64 },
    Button { button: String, down: bool },
    Scroll { dx: f64, dy: f64 },
    /// One raw key transition, as a browser `KeyboardEvent.code`.
    Key { code: String, down: bool },
}

/// One open window, as the compositor reports it.
#[derive(Debug, Default, Clone, Serialize)]
struct Window {
    title: String,
    app_id: String,
}

/// Every open window, keyed by the toplevel handle's protocol id, so
/// the list keeps the order the windows opened in.
type Windows = Arc<Mutex<BTreeMap<u32, Window>>>;

/// The latest complete frame, tightly packed RGB.
struct Frame {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

type Latest = Arc<Mutex<Option<Frame>>>;

struct App {
    conn: Connection,
    shm: wl_shm::WlShm,
    session: Option<ExtImageCopyCaptureSessionV1>,
    qh: QueueHandle<App>,
    width: u32,
    height: u32,
    format: Option<wl_shm::Format>,
    buffer: Option<wl_buffer::WlBuffer>,
    mmap_ptr: *mut u8,
    mmap_len: usize,
    latest: Latest,
    vpointer: ZwlrVirtualPointerV1,
    vkeyboard: ZwpVirtualKeyboardV1,
    /// The typing keyboard: its keymap is regenerated per
    /// operation, so it never shares state with the user's keyboard.
    typer: ZwpVirtualKeyboardV1,
    /// xkb modifier state for the virtual keyboard.
    mods_depressed: u32,
    mods_locked: u32,
    holder: SharedHolder,
    /// The compositor's window list, shared with the endpoint.
    windows: Windows,
    input: mpsc::Receiver<InputRequest>,
    pipeline: webrtc::Pipeline,
    offers: mpsc::Receiver<webrtc::OfferRequest>,
}

impl App {
    /// Create the shm buffer once the session announced size and
    /// format, then request the first frame.
    fn ensure_buffer(&mut self) {
        if self.buffer.is_some() || self.width == 0 || self.format.is_none() {
            return;
        }
        let stride = self.width * 4;
        let size = (stride * self.height) as usize;
        let fd = rustix::fs::memfd_create("screend-shm", rustix::fs::MemfdFlags::CLOEXEC)
            .expect("memfd");
        rustix::fs::ftruncate(&fd, size as u64).expect("ftruncate");
        let ptr = unsafe {
            rustix::mm::mmap(
                std::ptr::null_mut(),
                size,
                rustix::mm::ProtFlags::READ | rustix::mm::ProtFlags::WRITE,
                rustix::mm::MapFlags::SHARED,
                &fd,
                0,
            )
            .expect("mmap")
        };
        use std::os::fd::AsFd;
        let pool = self.shm.create_pool(fd.as_fd(), size as i32, &self.qh, ());
        let buffer = pool.create_buffer(
            0,
            self.width as i32,
            self.height as i32,
            stride as i32,
            self.format.expect("format announced"),
            &self.qh,
            (),
        );
        pool.destroy();
        self.mmap_ptr = ptr as *mut u8;
        self.mmap_len = size;
        self.buffer = Some(buffer);
        eprintln!(
            "[screend] capture {}x{} format {:?}",
            self.width, self.height, self.format
        );
        self.next_frame();
    }

    fn next_frame(&mut self) {
        let session = self.session.as_ref().expect("session exists");
        let frame = session.create_frame(&self.qh, ());
        frame.attach_buffer(self.buffer.as_ref().expect("buffer exists"));
        frame.capture();
    }

    /// A frame landed in the shm buffer: publish it as the latest
    /// frame (xrgb8888 little-endian bytes are B,G,R,X) and stream it
    /// to the connected viewers.
    fn on_ready(&mut self) {
        let (w, h) = (self.width as usize, self.height as usize);
        let src = unsafe { std::slice::from_raw_parts(self.mmap_ptr, self.mmap_len) };
        let mut rgb = vec![0u8; w * h * 3];
        for (px, out) in src.chunks_exact(4).zip(rgb.chunks_exact_mut(3)) {
            out[0] = px[2];
            out[1] = px[1];
            out[2] = px[0];
        }
        self.pipeline.send_frame(&rgb, self.width, self.height);
        let inputs = self.pipeline.drive();
        self.apply_user_input(inputs);
        *self.latest.lock().expect("latest frame lock") = Some(Frame {
            width: self.width,
            height: self.height,
            rgb,
        });
        self.next_frame();
    }

    /// Answer queued offers and push a stored frame to viewers that
    /// joined a static screen (no damage, so no capture is coming).
    fn drive_webrtc(&mut self) {
        while let Ok(request) = self.offers.try_recv() {
            let answer = self.pipeline.accept_offer(&request);
            let _ = request.reply.send(answer);
        }
        let inputs = self.pipeline.drive();
        self.apply_user_input(inputs);
        if self.pipeline.needs_refresh() {
            let frame = {
                let latest = self.latest.lock().expect("latest frame lock");
                latest
                    .as_ref()
                    .map(|frame| (frame.rgb.clone(), frame.width, frame.height))
            };
            if let Some((rgb, width, height)) = frame {
                self.pipeline.send_frame(&rgb, width, height);
                let inputs = self.pipeline.drive();
                self.apply_user_input(inputs);
            }
        }
    }

    /// Apply viewer data-channel input. Enforcement lives here:
    /// every event is dropped unless the user holds the switch, so a
    /// watching viewer can never accidentally drive the screen.
    fn apply_user_input(&mut self, inputs: Vec<UserInput>) {
        for input in inputs {
            {
                let mut holder = self.holder.lock().expect("holder lock");
                if holder.holder != Holder::User {
                    continue;
                }
                holder.last_input = std::time::Instant::now();
            }
            let result = match input {
                UserInput::Move { x, y } => self.pointer_op(&InputOp::Move { x, y }),
                UserInput::Button { button, down } => {
                    self.pointer_op(&InputOp::Button { button, down })
                }
                UserInput::Scroll { dx, dy } => self.pointer_op(&InputOp::Scroll { dx, dy }),
                UserInput::Key { code, down } => self.user_key(&code, down),
            };
            if let Err(error) = result {
                eprintln!("[screend] user input dropped: {error}");
            }
        }
    }

    /// Inject one raw user key transition through the virtual keyboard:
    /// the static code→evdev map plus tracked modifier state.
    fn user_key(&mut self, code: &str, down: bool) -> Result<(), String> {
        let key = keymap::code_to_evdev(code).ok_or_else(|| format!("unknown key {code:?}"))?;
        // wl_keyboard key states: 0 released, 1 pressed.
        self.vkeyboard.key(0, key, u32::from(down));
        if code == "CapsLock" {
            if down {
                self.mods_locked ^= keymap::MOD_LOCK;
                self.vkeyboard
                    .modifiers(self.mods_depressed, 0, self.mods_locked, 0);
            }
        } else if let Some(mask) = keymap::modifier_mask(code) {
            if down {
                self.mods_depressed |= mask;
            } else {
                self.mods_depressed &= !mask;
            }
            self.vkeyboard
                .modifiers(self.mods_depressed, 0, self.mods_locked, 0);
        }
        Ok(())
    }
}

impl App {
    /// Inject one pointer operation. Runs on the wayland thread.
    fn pointer_op(&mut self, op: &InputOp) -> Result<(), String> {
        match op {
            InputOp::Move { x, y } => {
                if self.width == 0 {
                    return Err("no output size yet".to_string());
                }
                self.vpointer.motion_absolute(
                    0,
                    x.max(0.0) as u32,
                    y.max(0.0) as u32,
                    self.width,
                    self.height,
                );
            }
            InputOp::Button { button, down } => {
                let code = match button.as_str() {
                    "left" => 0x110,
                    "right" => 0x111,
                    "middle" | "wheel" => 0x112,
                    other => return Err(format!("unknown button {other:?}")),
                };
                let state = if *down {
                    wl_pointer::ButtonState::Pressed
                } else {
                    wl_pointer::ButtonState::Released
                };
                self.vpointer.button(0, code, state);
            }
            InputOp::Scroll { dx, dy } => {
                if *dy != 0.0 {
                    self.vpointer
                        .axis(0, wl_pointer::Axis::VerticalScroll, dy * WHEEL_STEP);
                }
                if *dx != 0.0 {
                    self.vpointer
                        .axis(0, wl_pointer::Axis::HorizontalScroll, dx * WHEEL_STEP);
                }
            }
            InputOp::Text { .. } | InputOp::Key { .. } => {
                return Err("keyboard op sent to the pointer".to_string());
            }
        }
        self.vpointer.frame();
        Ok(())
    }

    /// Execute the requests the control endpoint queued.
    fn drain_input(&mut self) {
        while let Ok((op, reply)) = self.input.try_recv() {
            let result = match &op {
                InputOp::Text { text } => self.type_text(text),
                InputOp::Key { keys, hold_ms } => self.key_chord(keys, *hold_ms),
                pointer => self.pointer_op(pointer),
            };
            let _ = reply.send(result);
        }
    }

    /// Upload one generated keymap to the typing keyboard. The
    /// compositor applies it before the key events that follow it,
    /// because Wayland keeps requests in order on the wire.
    fn load_typing_keymap(&self, keysyms: &[String]) {
        let keymap = typing::keymap_for(keysyms);
        let bytes = keymap.as_bytes();
        let size = bytes.len() + 1; // NUL-terminated, per the protocol.
        let fd = rustix::fs::memfd_create("screend-typing", rustix::fs::MemfdFlags::CLOEXEC)
            .expect("typing keymap memfd");
        rustix::fs::ftruncate(&fd, size as u64).expect("typing keymap ftruncate");
        let ptr = unsafe {
            rustix::mm::mmap(
                std::ptr::null_mut(),
                size,
                rustix::mm::ProtFlags::READ | rustix::mm::ProtFlags::WRITE,
                rustix::mm::MapFlags::SHARED,
                &fd,
                0,
            )
            .expect("typing keymap mmap")
        };
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
            *(ptr as *mut u8).add(bytes.len()) = 0;
            rustix::mm::munmap(ptr, size).expect("typing keymap munmap");
        }
        use std::os::fd::AsFd;
        // Format 1 is xkb_v1.
        self.typer.keymap(1, fd.as_fd(), size as u32);
    }

    /// Type one string. The text itself is never logged:
    /// this is the path a vault fill runs through (ADR-0013).
    fn type_text(&mut self, text: &str) -> Result<(), String> {
        let keysyms: Vec<String> = text.chars().map(typing::char_keysym).collect();
        for (table, codes) in typing::passes(&keysyms) {
            self.load_typing_keymap(&table);
            self.typer.modifiers(0, 0, 0, 0);
            for code in codes {
                self.typer.key(0, code, 1);
                self.typer.key(0, code, 0);
            }
        }
        Ok(())
    }

    /// Press one chord: modifiers held around the plain keys.
    fn key_chord(&mut self, keys: &[String], hold_ms: Option<u64>) -> Result<(), String> {
        let mask = keys
            .iter()
            .filter_map(|key| typing::modifier_mask(key))
            .fold(0u32, |mask, bit| mask | bit);
        let plain: Vec<String> = keys
            .iter()
            .filter(|key| typing::modifier_mask(key).is_none())
            .map(|key| typing::key_keysym(key))
            .collect();
        let table: Vec<String> = typing::passes(&plain)
            .into_iter()
            .next()
            .map(|(table, _)| table)
            .unwrap_or_default();
        if table.len() != plain.iter().collect::<std::collections::BTreeSet<_>>().len() {
            return Err("chord needs more keys than one keymap holds".to_string());
        }
        self.load_typing_keymap(&table);
        self.typer.modifiers(mask, 0, 0, 0);
        let codes: Vec<u32> = plain
            .iter()
            .map(|keysym| {
                table
                    .iter()
                    .position(|known| known == keysym)
                    .expect("keysym in the table") as u32
                    + 1
            })
            .collect();
        for code in &codes {
            self.typer.key(0, *code, 1);
        }
        if let Some(hold) = hold_ms {
            self.flush_typing();
            std::thread::sleep(Duration::from_millis(hold));
        }
        for code in codes.iter().rev() {
            self.typer.key(0, *code, 0);
        }
        self.typer.modifiers(0, 0, 0, 0);
        Ok(())
    }

    /// Push queued requests to the compositor now. A held chord needs
    /// its presses to land before the hold, not with the release.
    fn flush_typing(&self) {
        if let Err(error) = self.conn.flush() {
            eprintln!("[screend] typing flush failed: {error}");
        }
    }
}

fn encode_png(frame: &Frame) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(Cursor::new(&mut out), frame.width, frame.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&frame.rgb).expect("png data");
    }
    out
}

/// Execute one batch strictly in order. Every op round-trips through
/// the wayland thread, which owns both virtual input devices.
fn run_batch(input: &mpsc::Sender<InputRequest>, batch: InputBatch) -> Result<(), String> {
    for op in batch.ops {
        let (reply, done) = mpsc::channel();
        input
            .send((op, reply))
            .map_err(|_| "wayland thread is gone".to_string())?;
        done.recv_timeout(Duration::from_secs(30))
            .map_err(|_| "input injection timed out".to_string())??;
    }
    Ok(())
}

/// One signaling request body, the wire shape with the daemon.
#[derive(Debug, Deserialize)]
struct OfferBody {
    sdp: String,
    candidate: String,
    relay: String,
    token: String,
}

/// One input-switch body, the wire shape with the daemon.
#[derive(Debug, Deserialize)]
struct HolderBody {
    holder: Holder,
}

/// One `POST /browser/open` body, the wire shape with the daemon.
#[derive(Deserialize)]
struct OpenBody {
    url: String,
}

/// One `POST /browser/fill` body, the wire shape with the daemon. It
/// carries a secret, so nothing prints it.
#[derive(Deserialize)]
struct FillBody {
    origin: String,
    fields: Vec<fill::Field>,
}

/// The request's `Authorization` header value, if it carries one.
fn authorization(request: &tiny_http::Request) -> Option<&str> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Authorization"))
        .map(|header| header.value.as_str())
}

/// One request of the daemon's browser channel, on a thread of its own.
fn answer_browser(
    mut request: tiny_http::Request,
    url: &str,
    browser: &browser::Browser,
    tab: &fill::Tab,
) {
    let answer: Result<Option<String>, (u16, String)> = match browser.connection() {
        None => Err((503, "the browser is not running".to_string())),
        Some(cdp) => {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            match (request.method(), url) {
                (tiny_http::Method::Post, "/browser/open") => {
                    match serde_json::from_str::<OpenBody>(&body) {
                        Ok(open) => tab.open(&cdp, &open.url).map(Some),
                        Err(_) => Err("bad open body".to_string()),
                    }
                    .map_err(|reason| (422, reason))
                }
                (tiny_http::Method::Get, "/browser/page") => {
                    tab.page(&cdp).map(Some).map_err(|reason| (422, reason))
                }
                // The body carries a secret, so a parse error never
                // repeats any of it.
                (tiny_http::Method::Post, "/browser/fill") => {
                    match serde_json::from_str::<FillBody>(&body) {
                        Ok(fill) => tab.fill(&cdp, &fill.origin, &fill.fields).map(|()| None),
                        Err(_) => Err("bad fill body".to_string()),
                    }
                    .map_err(|reason| (422, reason))
                }
                _ => Err((404, "not found".to_string())),
            }
        }
    };
    let response = match answer {
        Ok(Some(page)) => {
            tiny_http::Response::from_string(serde_json::json!({ "url": page }).to_string())
                .with_status_code(200)
                .with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .expect("static header"),
                )
        }
        Ok(None) => tiny_http::Response::from_string("ok").with_status_code(200),
        Err((status, reason)) => {
            eprintln!("[screend] {url} failed: {reason}");
            tiny_http::Response::from_string(reason).with_status_code(status)
        }
    };
    let _ = request.respond(response);
}

/// The control endpoint, on its own thread over the shared frame.
#[allow(clippy::too_many_arguments)]
fn serve(
    latest: Latest,
    input: mpsc::Sender<InputRequest>,
    offers: mpsc::Sender<webrtc::OfferRequest>,
    holder: SharedHolder,
    windows: Windows,
    guard: auth::Guard,
    browser: Arc<browser::Browser>,
    tab: Arc<fill::Tab>,
    exit_proxy: Arc<exit::ExitProxy>,
) {
    let server = tiny_http::Server::http(("0.0.0.0", PORT)).expect("bind control port");
    eprintln!("[screend] control endpoint on :{PORT}");
    for mut request in server.incoming_requests() {
        let url = request.url().to_string();
        // The token is the boundary between tenants: reaching
        // the port is not holding it. `/healthz` is the one open path.
        if !guard.allows(&url, authorization(&request)) {
            let _ = request.respond(
                tiny_http::Response::from_string("unauthorized").with_status_code(401),
            );
            continue;
        }
        // The daemon's browser channel answers only while the daemon
        // holds the switch. A page can take its whole load time, so each
        // request runs on a thread of its own and the endpoint goes on
        // answering.
        if url.starts_with("/browser/") {
            let current = holder.lock().expect("holder lock").holder;
            if current != Holder::Daemon {
                let _ = request.respond(
                    tiny_http::Response::from_string(format!(
                        "the browser channel refused: {} holds the switch",
                        current.as_str()
                    ))
                    .with_status_code(409),
                );
                continue;
            }
            let browser = Arc::clone(&browser);
            let tab = Arc::clone(&tab);
            std::thread::spawn(move || answer_browser(request, &url, &browser, &tab));
            continue;
        }
        if url == "/offer" && request.method() == &tiny_http::Method::Post {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let answer = serde_json::from_str::<OfferBody>(&body)
                .map_err(|err| format!("bad offer body: {err}"))
                .and_then(|offer| {
                    let (reply, answered) = mpsc::channel();
                    offers
                        .send(webrtc::OfferRequest {
                            sdp: offer.sdp,
                            candidate: offer.candidate,
                            relay: offer.relay,
                            token: offer.token,
                            reply,
                        })
                        .map_err(|_| "wayland thread is gone".to_string())?;
                    answered
                        .recv_timeout(Duration::from_secs(5))
                        .map_err(|_| "signaling timed out".to_string())?
                });
            let response = match answer {
                Ok(sdp) => tiny_http::Response::from_string(
                    serde_json::json!({ "sdp": sdp }).to_string(),
                )
                .with_status_code(200)
                .with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .expect("static header"),
                ),
                Err(error) => tiny_http::Response::from_string(error).with_status_code(500),
            };
            let _ = request.respond(response);
            continue;
        }
        if url == "/input" && request.method() == &tiny_http::Method::Post {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let batch = serde_json::from_str::<InputBatch>(&body)
                .map_err(|err| format!("bad input batch: {err}"));
            // The switch means the same at both ends: a batch is
            // refused unless the holder it speaks for holds the switch.
            let current = holder.lock().expect("holder lock").holder;
            let response = match batch {
                // A Vault fill writes through the browser channel, so the
                // focused window never gets a keystroke of the daemon.
                Ok(batch) if batch.holder == Holder::Daemon => tiny_http::Response::from_string(
                    "input refused: the daemon writes through the browser channel",
                )
                .with_status_code(409),
                Ok(batch) if batch.holder != current => tiny_http::Response::from_string(format!(
                    "input refused: {} holds the switch",
                    current.as_str()
                ))
                .with_status_code(409),
                Ok(batch) => match run_batch(&input, batch) {
                    Ok(()) => tiny_http::Response::from_string("ok").with_status_code(200),
                    Err(error) => tiny_http::Response::from_string(error).with_status_code(500),
                },
                Err(error) => tiny_http::Response::from_string(error).with_status_code(400),
            };
            let _ = request.respond(response);
            continue;
        }
        if url == "/holder" && request.method() == &tiny_http::Method::Post {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let response = match serde_json::from_str::<HolderBody>(&body) {
                Ok(HolderBody { holder: wanted }) => {
                    let mut state = holder.lock().expect("holder lock");
                    state.holder = wanted;
                    // A fresh takeover starts its inactivity clock now.
                    state.last_input = std::time::Instant::now();
                    eprintln!("[screend] input holder: {wanted:?}");
                    tiny_http::Response::from_string("ok").with_status_code(200)
                }
                Err(err) => tiny_http::Response::from_string(format!("bad holder body: {err}"))
                    .with_status_code(400),
            };
            let _ = request.respond(response);
            continue;
        }
        if url == "/exit" && request.method() == &tiny_http::Method::Post {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let response = match serde_json::from_str::<exit::SwitchBody>(&body) {
                Ok(exit::SwitchBody { mode }) => {
                    let switched = exit_proxy.switch(mode);
                    eprintln!(
                        "[screend] exit mode: {mode:?}; {} connections closed",
                        switched.closed
                    );
                    tiny_http::Response::from_string(
                        serde_json::to_string(&switched).expect("the switch answer is JSON"),
                    )
                    .with_status_code(200)
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json"[..],
                        )
                        .expect("static header"),
                    )
                }
                Err(err) => tiny_http::Response::from_string(format!("bad exit body: {err}"))
                    .with_status_code(400),
            };
            let _ = request.respond(response);
            continue;
        }
        let response = match url.as_str() {
            "/healthz" => tiny_http::Response::from_string(
                serde_json::json!({
                    "ok": true,
                    "browser": if browser_running() { "up" } else { "down" },
                })
                .to_string(),
            )
            .with_status_code(200)
            .with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .expect("static header"),
            ),
            "/holder" => {
                let state = holder.lock().expect("holder lock");
                let name = state.holder.as_str();
                tiny_http::Response::from_string(
                    serde_json::json!({
                        "holder": name,
                        "idle_ms": state.last_input.elapsed().as_millis() as u64,
                    })
                    .to_string(),
                )
                .with_status_code(200)
                .with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .expect("static header"),
                )
            }
            "/exit" => tiny_http::Response::from_string(
                serde_json::to_string(&exit_proxy.status()).expect("the exit status is JSON"),
            )
            .with_status_code(200)
            .with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .expect("static header"),
            ),
            "/windows" => {
                let open: Vec<Window> = windows
                    .lock()
                    .expect("window list lock")
                    .values()
                    .cloned()
                    .collect();
                tiny_http::Response::from_string(
                    serde_json::to_string(&open).expect("the window list is JSON"),
                )
                .with_status_code(200)
                .with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .expect("static header"),
                )
            }
            "/frame.png" => match latest.lock().expect("latest frame lock").as_ref() {
                Some(frame) => {
                    let png = encode_png(frame);
                    tiny_http::Response::from_data(png)
                        .with_status_code(200)
                        .with_header(
                            tiny_http::Header::from_bytes(
                                &b"Content-Type"[..],
                                &b"image/png"[..],
                            )
                            .expect("static header"),
                        )
                }
                None => tiny_http::Response::from_string("no frame yet").with_status_code(503),
            },
            _ => tiny_http::Response::from_string("not found").with_status_code(404),
        };
        let _ = request.respond(response);
    }
}

/// Upload the fixed US keymap to the virtual keyboard.
fn upload_keymap(vkeyboard: &ZwpVirtualKeyboardV1) {
    let bytes = keymap::US_KEYMAP.as_bytes();
    let size = bytes.len() + 1; // NUL-terminated, per the protocol.
    let fd = rustix::fs::memfd_create("screend-keymap", rustix::fs::MemfdFlags::CLOEXEC)
        .expect("keymap memfd");
    rustix::fs::ftruncate(&fd, size as u64).expect("keymap ftruncate");
    let ptr = unsafe {
        rustix::mm::mmap(
            std::ptr::null_mut(),
            size,
            rustix::mm::ProtFlags::READ | rustix::mm::ProtFlags::WRITE,
            rustix::mm::MapFlags::SHARED,
            &fd,
            0,
        )
        .expect("keymap mmap")
    };
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
        *(ptr as *mut u8).add(bytes.len()) = 0;
        rustix::mm::munmap(ptr, size).expect("keymap munmap");
    }
    use std::os::fd::AsFd;
    // Format 1 is xkb_v1.
    vkeyboard.keymap(1, fd.as_fd(), size as u32);
}

/// Is the supervised browser alive? screend starts a browser that
/// exits again (see `browser`), and the health report says what it
/// finds, so a browser that cannot start at all is visible to the daemon
/// instead of only to whoever looks at a screenshot. The container runs one
/// process tree and no `hidepid`, so `/proc` is the whole answer.
fn browser_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let is_pid = name.to_str().is_some_and(|name| {
            !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit())
        });
        is_pid
            && std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| comm.trim() == "chromium")
    })
}

fn main() {
    let latest: Latest = Arc::new(Mutex::new(None));
    let holder: SharedHolder = Arc::new(Mutex::new(HolderState {
        holder: Holder::Agent,
        last_input: std::time::Instant::now(),
    }));
    let windows: Windows = Arc::new(Mutex::new(BTreeMap::new()));
    let (input_tx, input_rx) = mpsc::channel::<InputRequest>();
    let (offer_tx, offer_rx) = mpsc::channel::<webrtc::OfferRequest>();
    let guard = auth::Guard::from_env();
    if guard.is_closed() {
        eprintln!(
            "[screend] no control token: {} is absent or empty, so every request but {} is refused",
            auth::token_path().display(),
            auth::OPEN_PATH,
        );
    }
    // The Exit Proxy listens before the browser starts, so the first
    // page of the browser finds it.
    let exit_proxy = Arc::new(exit::ExitProxy::new());
    {
        let listener = exit::listen().expect("bind the Exit Proxy port");
        let exit_proxy = Arc::clone(&exit_proxy);
        std::thread::spawn(move || exit::run(exit_proxy, listener));
    }
    // The browser, with the DevTools pipe of the daemon's fills.
    let browser = browser::Browser::start(&["pagis-browser"]);
    let tab = Arc::new(fill::Tab::new(LOAD_TIMEOUT, FOCUS_WAIT));
    {
        let latest = Arc::clone(&latest);
        let holder = Arc::clone(&holder);
        let windows = Arc::clone(&windows);
        std::thread::spawn(move || {
            serve(
                latest, input_tx, offer_tx, holder, windows, guard, browser, tab, exit_proxy,
            )
        });
    }

    let conn = Connection::connect_to_env().expect("WAYLAND_DISPLAY reachable");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("wayland registry");
    let qh = queue.handle();

    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
    let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).expect("wl_output");
    let source_mgr: ExtOutputImageCaptureSourceManagerV1 =
        globals.bind(&qh, 1..=1, ()).expect("capture source manager");
    let capture_mgr: ExtImageCopyCaptureManagerV1 =
        globals.bind(&qh, 1..=1, ()).expect("capture manager");
    let source = source_mgr.create_source(&output, &qh, ());
    let vpointer_mgr: ZwlrVirtualPointerManagerV1 =
        globals.bind(&qh, 1..=2, ()).expect("virtual pointer manager");
    let vpointer = vpointer_mgr.create_virtual_pointer(None, &qh, ());
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).expect("wl_seat");
    let vkeyboard_mgr: ZwpVirtualKeyboardManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .expect("virtual keyboard manager");
    let vkeyboard = vkeyboard_mgr.create_virtual_keyboard(&seat, &qh, ());
    upload_keymap(&vkeyboard);
    // The typing keyboard gets a fresh keymap per operation.
    let typer = vkeyboard_mgr.create_virtual_keyboard(&seat, &qh, ());
    // The window list. The compositor sends one toplevel handle
    // per open window and keeps its title current, so the list needs no
    // polling.
    let _toplevels: ExtForeignToplevelListV1 =
        globals.bind(&qh, 1..=1, ()).expect("foreign toplevel list");

    let mut app = App {
        conn: conn.clone(),
        shm,
        session: None,
        qh: qh.clone(),
        width: 0,
        height: 0,
        format: None,
        buffer: None,
        mmap_ptr: std::ptr::null_mut(),
        mmap_len: 0,
        latest,
        vpointer,
        vkeyboard,
        typer,
        mods_depressed: 0,
        mods_locked: 0,
        holder,
        windows,
        input: input_rx,
        pipeline: webrtc::Pipeline::new(),
        offers: offer_rx,
    };
    let session = capture_mgr.create_session(&source, Options::PaintCursors, &qh, ());
    app.session = Some(session);

    loop {
        app.drain_input();
        app.drive_webrtc();
        queue.flush().expect("wayland flush");
        queue.dispatch_pending(&mut app).expect("wayland dispatch");
        if let Some(guard) = conn.prepare_read() {
            let fd = guard.connection_fd();
            use std::os::fd::AsFd;
            let udp = app.pipeline.socket().as_fd();
            let mut fds = [
                rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN),
                rustix::event::PollFd::new(&udp, rustix::event::PollFlags::IN),
            ];
            // Short poll: queued input ops and str0m timers wait at
            // most this long.
            let ts = rustix::fs::Timespec {
                tv_sec: 0,
                tv_nsec: 20_000_000,
            };
            if rustix::event::poll(&mut fds, Some(&ts)).expect("poll") > 0 {
                guard.read().ok();
            }
        }
        queue.dispatch_pending(&mut app).expect("wayland dispatch");
    }
}

// ---- Wayland dispatch ----

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

macro_rules! ignore {
    ($t:ty) => {
        impl Dispatch<$t, ()> for App {
            fn event(
                _: &mut Self,
                _: &$t,
                _: <$t as wayland_client::Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}
ignore!(wl_shm::WlShm);
ignore!(wl_shm_pool::WlShmPool);
ignore!(wl_buffer::WlBuffer);
ignore!(wl_output::WlOutput);
ignore!(ExtOutputImageCaptureSourceManagerV1);
ignore!(ExtImageCaptureSourceV1);
ignore!(ExtImageCopyCaptureManagerV1);
ignore!(ZwlrVirtualPointerManagerV1);
ignore!(ZwlrVirtualPointerV1);
ignore!(wl_seat::WlSeat);
ignore!(ZwpVirtualKeyboardManagerV1);
ignore!(ZwpVirtualKeyboardV1);

/// The window list. The compositor announces one handle per
/// open window and updates its title in place, so the map is the
/// current list at every moment.
impl Dispatch<ExtForeignToplevelListV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            app.windows
                .lock()
                .expect("window list lock")
                .insert(toplevel.id().protocol_id(), Window::default());
        }
    }

    event_created_child!(App, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for App {
    fn event(
        app: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_foreign_toplevel_handle_v1::Event;
        let key = handle.id().protocol_id();
        let mut windows = app.windows.lock().expect("window list lock");
        match event {
            Event::Title { title } => {
                windows.entry(key).or_default().title = title;
            }
            Event::AppId { app_id } => {
                windows.entry(key).or_default().app_id = app_id;
            }
            Event::Closed => {
                windows.remove(&key);
                handle.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event;
        match event {
            Event::BufferSize { width, height } => {
                app.width = width;
                app.height = height;
            }
            Event::ShmFormat { format } => {
                if app.format.is_none()
                    && let WEnum::Value(f) = format
                    && matches!(f, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888)
                {
                    app.format = Some(f);
                }
            }
            Event::Done => app.ensure_buffer(),
            Event::Stopped => {
                eprintln!("[screend] capture session stopped by the compositor");
                std::process::exit(1);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for App {
    fn event(
        app: &mut Self,
        frame: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event;
        match event {
            Event::Ready => {
                frame.destroy();
                app.on_ready();
            }
            Event::Failed { reason } => {
                eprintln!("[screend] frame failed: {reason:?}; retrying");
                frame.destroy();
                std::thread::sleep(Duration::from_millis(100));
                app.next_frame();
            }
            _ => {}
        }
    }
}
