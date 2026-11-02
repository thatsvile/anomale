mod niri;
mod pixels;

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, LoopHandle, LoopSignal};
use image::RgbaImage;
use log::{info, warn};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::pointer::{
    CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, registry_handlers};
use vile_protocol::{load_config, ScreenshotConfig};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

const BTN_LEFT: u32 = 0x110;
const MAX_OVERLAY_BUFFERS: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Full,
    Crop,
}

enum Phase {
    Capturing,
    Cropping,
    Clipboard,
}

fn main() -> Result<()> {
    ignore_sigpipe();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mode = parse_args(std::env::args().skip(1))?;
    let config = load_config()?;
    let wanted = niri::pick_output_name()?;
    info!("capturing output {wanted}");

    let conn = Connection::connect_to_env().context("connecting to Wayland")?;
    let (globals, mut event_queue) = registry_queue_init(&conn).context("wayland registry")?;
    let qh = event_queue.handle();

    let compositor =
        CompositorState::bind(&globals, &qh).context("wl_compositor is not available")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context("layer shell is not available")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm is not available")?;
    let pool = SlotPool::new(256 * 256 * 4, &shm).context("creating shm pool")?;
    let screencopy: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 1..=3, ())
        .context("wlr-screencopy is not available")?;
    let data_control: ZwlrDataControlManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .context("wlr-data-control is not available")?;
    let mut event_loop = EventLoop::<App>::try_new().context("creating event loop")?;
    let handle = event_loop.handle();
    let signal = event_loop.get_signal();

    let mut app = App {
        qh: qh.clone(),
        handle: handle.clone(),
        signal,
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        screencopy,
        data_control,
        orig_bgra: Vec::new(),
        dim_bgra: Vec::new(),
        overlay_bufs: Vec::new(),
        overlay_buf_w: 0,
        overlay_buf_h: 0,
        drawing: false,
        mode,
        screenshot: config.screenshot,
        wanted,
        target: None,
        capture: None,
        capture_frame: None,
        capture_buffer: None,
        capture_format: None,
        capture_width: 0,
        capture_height: 0,
        capture_stride: 0,
        y_invert: false,
        copy_started: false,
        layer: None,
        width: 0,
        height: 0,
        scale: 1,
        pointer: None,
        keyboard: None,
        dragging: false,
        sel_start: None,
        sel_end: None,
        device: None,
        source: None,
        png: None,
        phase: Phase::Capturing,
        error: None,
    };

    event_queue
        .roundtrip(&mut app)
        .context("wayland roundtrip")?;
    app.ensure_clipboard_device();
    let target = app
        .find_output()
        .with_context(|| format!("wayland output {} not found", app.wanted))?;
    app.target = Some(target.clone());
    app.start_capture(&target);

    calloop_wayland_source::WaylandSource::new(conn, event_queue)
        .insert(handle)
        .map_err(|err| anyhow::anyhow!("inserting wayland source: {err}"))?;

    event_loop
        .run(None, &mut app, |_| {})
        .context("event loop")?;
    if let Some(err) = app.error {
        bail!("{err}");
    }
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Mode> {
    let args: Vec<String> = args.into_iter().collect();
    if args.len() != 1 {
        bail!("{}", usage());
    }
    match args[0].as_str() {
        "full" | "--full" => Ok(Mode::Full),
        "crop" | "--crop" => Ok(Mode::Crop),
        "-h" | "--help" => bail!("{}", usage()),
        _ => bail!("{}", usage()),
    }
}

fn usage() -> &'static str {
    "usage: vileshot full | vileshot crop"
}

fn ignore_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

fn write_clipboard_fd(fd: OwnedFd, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::from(fd);
    unsafe {
        let raw = file.as_raw_fd();
        let flags = libc::fcntl(raw, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(raw, libc::F_SETFL, flags & !libc::O_NONBLOCK);
        }
    }
    file.write_all(bytes)?;
    file.flush()
}

struct App {
    qh: QueueHandle<App>,
    handle: LoopHandle<'static, App>,
    signal: LoopSignal,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    screencopy: ZwlrScreencopyManagerV1,
    data_control: ZwlrDataControlManagerV1,
    orig_bgra: Vec<u8>,
    dim_bgra: Vec<u8>,
    overlay_bufs: Vec<Buffer>,
    overlay_buf_w: u32,
    overlay_buf_h: u32,
    drawing: bool,
    mode: Mode,
    screenshot: ScreenshotConfig,
    wanted: String,
    target: Option<wl_output::WlOutput>,
    capture: Option<RgbaImage>,
    capture_frame: Option<ZwlrScreencopyFrameV1>,
    capture_buffer: Option<Buffer>,
    capture_format: Option<wl_shm::Format>,
    capture_width: u32,
    capture_height: u32,
    capture_stride: u32,
    y_invert: bool,
    copy_started: bool,
    layer: Option<LayerSurface>,
    width: u32,
    height: u32,
    scale: u32,
    pointer: Option<ThemedPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    dragging: bool,
    sel_start: Option<(f64, f64)>,
    sel_end: Option<(f64, f64)>,
    device: Option<ZwlrDataControlDeviceV1>,
    source: Option<ZwlrDataControlSourceV1>,
    png: Option<Vec<u8>>,
    phase: Phase,
    error: Option<String>,
}

impl App {
    fn find_output(&self) -> Option<wl_output::WlOutput> {
        self.output_state.outputs().find(|output| {
            self.output_state
                .info(output)
                .and_then(|info| info.name)
                .as_deref()
                == Some(self.wanted.as_str())
        })
    }

    fn start_capture(&mut self, output: &wl_output::WlOutput) {
        let frame = self.screencopy.capture_output(0, output, &self.qh, ());
        self.capture_frame = Some(frame);
        self.phase = Phase::Capturing;
        self.copy_started = false;
    }

    fn begin_copy(&mut self) {
        if self.copy_started {
            return;
        }
        let Some(frame) = self.capture_frame.as_ref() else {
            return;
        };
        let Some(buffer) = self.capture_buffer.as_ref() else {
            return;
        };
        frame.copy(buffer.wl_buffer());
        self.copy_started = true;
    }

    fn on_capture_ready(&mut self) {
        let Some(format) = self.capture_format else {
            self.fail("missing screenshot format");
            return;
        };
        let pixels = {
            let Some(buffer) = self.capture_buffer.as_ref() else {
                self.fail("missing screenshot buffer");
                return;
            };
            let Some(canvas) = buffer.canvas(&mut self.pool) else {
                self.fail("screenshot buffer is still busy");
                return;
            };
            canvas.to_vec()
        };
        if let Some(frame) = self.capture_frame.take() {
            frame.destroy();
        }
        self.capture_buffer = None;
        match pixels::shm_to_rgba(
            self.capture_width,
            self.capture_height,
            self.capture_stride,
            format,
            self.y_invert,
            &pixels,
        ) {
            Ok(image) => {
                info!("captured {}x{}", image.width(), image.height());
                self.capture = Some(image);
                match self.mode {
                    Mode::Full => self.finish_with_capture(),
                    Mode::Crop => self.start_crop(),
                }
            }
            Err(err) => self.fail(err.to_string()),
        }
    }

    fn start_crop(&mut self) {
        let Some(output) = self.target.clone() else {
            self.fail("no target output for crop");
            return;
        };
        let scale = self
            .output_state
            .info(&output)
            .map(|info| info.scale_factor.max(1) as u32)
            .unwrap_or(1);
        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Overlay,
            Some("vileshot"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        let _ = layer.set_buffer_scale(1);
        layer.commit();
        self.scale = scale;
        self.layer = Some(layer);
        self.phase = Phase::Cropping;
    }

    fn prepare_overlay_bitmaps(&mut self) {
        let Some(capture) = self.capture.as_ref() else {
            return;
        };
        let buf_w = self.width.max(1);
        let buf_h = self.height.max(1);
        if self.overlay_buf_w == buf_w && self.overlay_buf_h == buf_h && !self.orig_bgra.is_empty()
        {
            return;
        }
        self.orig_bgra = pixels::rgba_to_bgra_scaled(capture, buf_w, buf_h);
        self.dim_bgra = pixels::dim_bgra(&self.orig_bgra);
        self.overlay_buf_w = buf_w;
        self.overlay_buf_h = buf_h;
        self.overlay_bufs.clear();
    }

    fn finish_with_capture(&mut self) {
        let Some(image) = self.capture.take() else {
            self.fail("missing capture");
            return;
        };
        self.finish_image(image);
    }

    fn finish_crop(&mut self) {
        let Some(image) = self.capture.take() else {
            self.fail("missing capture");
            return;
        };
        let Some((x0, y0)) = self.sel_start else {
            self.capture = Some(image);
            return;
        };
        let (x1, y1) = self.sel_end.unwrap_or((x0, y0));
        let cx0 = pixels::map_point(x0, self.width, image.width());
        let cy0 = pixels::map_point(y0, self.height, image.height());
        let cx1 = pixels::map_point(x1, self.width, image.width());
        let cy1 = pixels::map_point(y1, self.height, image.height());
        let cropped = pixels::crop_rgba(&image, cx0, cy0, cx1, cy1);
        self.release_overlay();
        self.finish_image(cropped);
    }

    fn finish_image(&mut self, image: RgbaImage) {
        match pixels::encode_png(&image) {
            Ok(png) => {
                if self.screenshot.save {
                    let dir = pixels::expand_directory(&self.screenshot.directory);
                    match pixels::save_png(&dir, &png) {
                        Ok(path) => info!("saved {}", path.display()),
                        Err(err) => {
                            self.fail(err.to_string());
                            return;
                        }
                    }
                }
                self.png = Some(png);
                self.offer_clipboard();
            }
            Err(err) => self.fail(err.to_string()),
        }
    }

    fn offer_clipboard(&mut self) {
        self.release_overlay();
        self.ensure_clipboard_device();
        let Some(device) = self.device.as_ref() else {
            self.fail("no seat for clipboard");
            return;
        };
        let source = self.data_control.create_data_source(&self.qh, ());
        source.offer("image/png".into());
        device.set_selection(Some(&source));
        self.source = Some(source);
        self.phase = Phase::Clipboard;
        let secs = self.screenshot.clipboard_seconds;
        if secs == 0 {
            info!("clipboard ready (holding until replaced)");
            return;
        }
        info!("clipboard ready ({secs}s)");
        if let Err(err) = self.handle.insert_source(
            Timer::from_duration(Duration::from_secs(secs)),
            |_, _, app| {
                info!("clipboard hold expired");
                app.signal.stop();
                TimeoutAction::Drop
            },
        ) {
            warn!("clipboard timer: {err}");
        }
    }

    fn release_overlay(&mut self) {
        self.layer = None;
        self.overlay_bufs.clear();
        self.orig_bgra.clear();
        self.dim_bgra.clear();
        self.overlay_buf_w = 0;
        self.overlay_buf_h = 0;
        self.drawing = false;
        self.dragging = false;
    }

    fn ensure_clipboard_device(&mut self) {
        if self.device.is_some() {
            return;
        }
        let Some(seat) = self.seat_state.seats().next() else {
            return;
        };
        self.device = Some(self.data_control.get_data_device(&seat, &self.qh, ()));
    }

    fn draw_crop(&mut self) {
        if self.drawing || !matches!(self.phase, Phase::Cropping) {
            return;
        }
        if self.width == 0 || self.height == 0 {
            return;
        }
        self.drawing = true;
        self.prepare_overlay_bitmaps();
        let buf_w = self.overlay_buf_w;
        let buf_h = self.overlay_buf_h;
        if buf_w == 0 || buf_h == 0 || self.orig_bgra.is_empty() {
            self.drawing = false;
            return;
        }
        let sel = self.selection_buffer(buf_w, buf_h);
        let stride = buf_w as i32 * 4;
        let free = self
            .overlay_bufs
            .iter()
            .position(|buffer| !buffer.slot().has_active_buffers());
        if free.is_none() && self.overlay_bufs.len() >= MAX_OVERLAY_BUFFERS {
            self.drawing = false;
            return;
        }
        if free.is_none() {
            match self.pool.create_buffer(
                buf_w as i32,
                buf_h as i32,
                stride,
                wl_shm::Format::Argb8888,
            ) {
                Ok((buffer, _)) => self.overlay_bufs.push(buffer),
                Err(err) => {
                    warn!("overlay buffer: {err}");
                    self.drawing = false;
                    return;
                }
            }
        }
        let index = free.unwrap_or(self.overlay_bufs.len() - 1);
        {
            let Some(canvas) = self.overlay_bufs[index].canvas(&mut self.pool) else {
                self.drawing = false;
                return;
            };
            pixels::blit_crop_overlay(canvas, &self.orig_bgra, &self.dim_bgra, buf_w, buf_h, sel);
        }
        let Some(layer) = self.layer.as_ref() else {
            self.drawing = false;
            return;
        };
        let _ = layer.set_buffer_scale(1);
        layer
            .wl_surface()
            .damage_buffer(0, 0, buf_w as i32, buf_h as i32);
        if let Err(err) = self.overlay_bufs[index].attach_to(layer.wl_surface()) {
            warn!("overlay attach: {err}");
            self.drawing = false;
            return;
        }
        layer.commit();
        self.drawing = false;
    }

    fn selection_buffer(&self, buf_w: u32, buf_h: u32) -> Option<(u32, u32, u32, u32)> {
        let (x0, y0) = self.sel_start?;
        let (x1, y1) = self.sel_end.unwrap_or((x0, y0));
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let left = pixels::map_point(x0.min(x1), self.width, buf_w).min(buf_w.saturating_sub(1));
        let top = pixels::map_point(y0.min(y1), self.height, buf_h).min(buf_h.saturating_sub(1));
        let right = pixels::map_point(x0.max(x1), self.width, buf_w)
            .max(left.saturating_add(1))
            .min(buf_w);
        let bottom = pixels::map_point(y0.max(y1), self.height, buf_h)
            .max(top.saturating_add(1))
            .min(buf_h);
        Some((left, top, right, bottom))
    }

    fn handle_pointer(&mut self, conn: &Connection, events: &[PointerEvent]) {
        if !matches!(self.phase, Phase::Cropping) {
            return;
        }
        for event in events {
            if matches!(event.kind, PointerEventKind::Enter { .. }) {
                if let Some(pointer) = self.pointer.as_ref() {
                    let _ = pointer.set_cursor(conn, CursorIcon::Crosshair);
                }
            }
            match event.kind {
                PointerEventKind::Press { button, .. } if button == BTN_LEFT => {
                    self.dragging = true;
                    self.sel_start = Some(event.position);
                    self.sel_end = Some(event.position);
                    self.draw_crop();
                }
                PointerEventKind::Motion { .. } if self.dragging => {
                    self.sel_end = Some(event.position);
                    self.draw_crop();
                }
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    if self.dragging {
                        self.sel_end = Some(event.position);
                        self.dragging = false;
                        self.finish_crop();
                    }
                }
                _ => {}
            }
        }
    }

    fn cancel_crop(&mut self) {
        if !matches!(self.phase, Phase::Cropping) {
            return;
        }
        info!("crop cancelled");
        self.layer = None;
        self.overlay_bufs.clear();
        self.signal.stop();
    }

    fn fail(&mut self, message: impl Into<String>) {
        let message = message.into();
        warn!("{message}");
        self.error = Some(message);
        self.signal.stop();
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        self.scale = new_factor.max(1) as u32;
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if self.target.as_ref() == Some(&output) {
            self.fail("target output disappeared");
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        if matches!(self.phase, Phase::Cropping) {
            self.cancel_crop();
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (width, height) = if configure.new_size.0 > 0 && configure.new_size.1 > 0 {
            configure.new_size
        } else if let Some(output) = self.target.as_ref() {
            self.output_state
                .info(output)
                .and_then(|info| {
                    let (w, h) = info.logical_size?;
                    Some((w.max(1) as u32, h.max(1) as u32))
                })
                .unwrap_or((1, 1))
        } else {
            (1, 1)
        };
        self.width = width.max(1);
        self.height = height.max(1);
        if matches!(self.phase, Phase::Cropping) {
            self.draw_crop();
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {
        self.ensure_clipboard_device();
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            let surface = self.compositor.create_surface(qh);
            match self.seat_state.get_pointer_with_theme::<_, ()>(
                qh,
                &seat,
                self.shm.wl_shm(),
                surface,
                ThemeSpec::default(),
            ) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(err) => warn!("failed to bind pointer: {err}"),
            }
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seat_state.get_keyboard(qh, &seat, None) {
                Ok(keyboard) => self.keyboard = Some(keyboard),
                Err(err) => warn!("failed to bind keyboard: {err}"),
            }
        }
        self.ensure_clipboard_device();
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.pointer().release();
            }
        }
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        if event.keysym == Keysym::Escape {
            self.cancel_crop();
        }
    }

    fn repeat_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _modifiers: Modifiers,
        _raw_modifiers: RawModifiers,
        _layout: u32,
    ) {
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        self.handle_pointer(conn, events);
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrScreencopyManagerV1,
        _event: <ZwlrScreencopyManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let format = match format {
                    WEnum::Value(format) => format,
                    WEnum::Unknown(raw) => {
                        state.fail(format!("unknown screenshot format {raw}"));
                        return;
                    }
                };
                state.capture_format = Some(format);
                state.capture_width = width;
                state.capture_height = height;
                state.capture_stride = stride;
                match state
                    .pool
                    .create_buffer(width as i32, height as i32, stride as i32, format)
                {
                    Ok((buffer, _)) => state.capture_buffer = Some(buffer),
                    Err(err) => {
                        state.fail(format!("screenshot buffer: {err}"));
                        return;
                    }
                }
                if state.screencopy.version() < 3 {
                    state.begin_copy();
                }
            }
            zwlr_screencopy_frame_v1::Event::LinuxDmabuf { .. } => {}
            zwlr_screencopy_frame_v1::Event::BufferDone => state.begin_copy(),
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                state.y_invert = match flags {
                    WEnum::Value(flags) => flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert),
                    WEnum::Unknown(raw) => raw & 1 != 0,
                };
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => state.on_capture_ready(),
            zwlr_screencopy_frame_v1::Event::Failed => state.fail("screencopy failed"),
            zwlr_screencopy_frame_v1::Event::Damage { .. } => {}
            _ => {}
        }
    }
}

impl Dispatch<ZwlrDataControlManagerV1, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrDataControlManagerV1,
        _event: <ZwlrDataControlManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { .. } => {}
            zwlr_data_control_device_v1::Event::Selection { id } => {
                if let Some(id) = id {
                    id.destroy();
                }
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { id } => {
                if let Some(id) = id {
                    id.destroy();
                }
            }
            zwlr_data_control_device_v1::Event::Finished => {}
            _ => {}
        }
    }

    wayland_client::event_created_child!(App, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                let Some(png) = state.png.as_ref() else {
                    return;
                };
                let _ = mime_type;
                if let Err(err) = write_clipboard_fd(fd, png) {
                    warn!("clipboard send: {err}");
                }
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                info!("clipboard selection replaced");
                state.signal.stop();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrDataControlOfferV1,
        _event: <ZwlrDataControlOfferV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(App);

#[cfg(test)]
mod tests {
    use super::{parse_args, Mode};

    #[test]
    fn parses_full_and_crop() {
        assert!(matches!(parse_args(["full".into()]).unwrap(), Mode::Full));
        assert!(matches!(parse_args(["--crop".into()]).unwrap(), Mode::Crop));
        assert!(parse_args(Vec::<String>::new()).is_err());
        assert!(parse_args(["nope".into()]).is_err());
    }
}
