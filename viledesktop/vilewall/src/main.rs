mod blit;
mod decode;

use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use calloop::channel::{self, Event as ChannelEvent};
use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use image::RgbaImage;
use log::{info, warn};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, registry_handlers};
use vile_protocol::{
    load_config, read_json_lines, socket_path, write_json_line, BarToDaemon, Config, DaemonToBar,
    WallpaperMode,
};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use wayland_client::{Connection, Proxy, QueueHandle};

use crate::blit::{render_into, OutputRect};

struct Args {
    img: Option<PathBuf>,
    mode: Option<WallpaperMode>,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = parse_args(std::env::args().skip(1))?;
    let config = load_config()?;

    let conn = Connection::connect_to_env().context("connecting to Wayland")?;
    let (globals, event_queue) = registry_queue_init(&conn).context("wayland registry")?;
    let qh = event_queue.handle();

    let compositor =
        CompositorState::bind(&globals, &qh).context("wl_compositor is not available")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context("layer shell is not available")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm is not available")?;
    let pool = SlotPool::new(256 * 256 * 4, &shm).context("creating shm pool")?;

    let mut event_loop = EventLoop::<App>::try_new().context("creating event loop")?;
    let handle = event_loop.handle();
    calloop_wayland_source::WaylandSource::new(conn.clone(), event_queue)
        .insert(handle.clone())
        .map_err(|err| anyhow::anyhow!("inserting wayland source: {err}"))?;

    let (decode_tx, decode_rx) = channel::channel();
    handle
        .insert_source(decode_rx, |event, _, app| {
            if let ChannelEvent::Msg(result) = event {
                app.on_decoded(result);
            }
        })
        .map_err(|err| anyhow::anyhow!("inserting decode channel: {err}"))?;

    handle
        .insert_source(
            Timer::from_duration(Duration::from_millis(200)),
            |_, _, app| {
                app.retry_socket();
                TimeoutAction::ToDuration(Duration::from_millis(200))
            },
        )
        .map_err(|err| anyhow::anyhow!("inserting reconnect timer: {err}"))?;

    let mode = args.mode.unwrap_or(config.wallpaper.mode);
    let mut app = App {
        handle,
        qh: qh.clone(),
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        config,
        surfaces: Vec::new(),
        next_id: 1,
        writer: None,
        socket_token: None,
        read_buf: Vec::new(),
        image: None,
        wanted_path: args.img.as_ref().map(|p| p.to_string_lossy().into_owned()),
        mode,
        decode_tx,
    };

    if let Some(path) = app.wanted_path.clone() {
        app.request_decode(path);
    }

    info!("vilewall starting");
    event_loop
        .run(None, &mut app, |_| {})
        .context("event loop")?;
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args> {
    let args: Vec<String> = args.into_iter().collect();
    let mut img = None;
    let mut mode = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--img" {
            img = Some(
                args.get(i + 1)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow::anyhow!("{}", usage()))?,
            );
            i += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--img=") {
            img = Some(PathBuf::from(rest));
            i += 1;
            continue;
        }
        if arg == "--mode" {
            let spec = args.get(i + 1).map(String::as_str).unwrap_or("");
            mode = Some(WallpaperMode::parse(spec).ok_or_else(|| anyhow::anyhow!("{}", usage()))?);
            i += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--mode=") {
            mode = Some(WallpaperMode::parse(rest).ok_or_else(|| anyhow::anyhow!("{}", usage()))?);
            i += 1;
            continue;
        }
        if arg == "-h" || arg == "--help" {
            bail!("{}", usage());
        }
        bail!("{}", usage());
    }
    Ok(Args { img, mode })
}

fn usage() -> &'static str {
    "usage: vilewall [--img <path>] [--mode fill|fit|scale|stretch|center|tile|span]"
}

struct DecodeResult {
    path: String,
    image: std::result::Result<RgbaImage, String>,
    elapsed: Duration,
}

struct App {
    handle: LoopHandle<'static, App>,
    qh: QueueHandle<App>,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    config: Config,
    surfaces: Vec<WallSurface>,
    next_id: u64,
    writer: Option<UnixStream>,
    socket_token: Option<RegistrationToken>,
    read_buf: Vec<u8>,
    image: Option<RgbaImage>,
    wanted_path: Option<String>,
    mode: WallpaperMode,
    decode_tx: channel::Sender<DecodeResult>,
}

struct WallSurface {
    id: u64,
    output: wl_output::WlOutput,
    output_name: String,
    geom: OutputRect,
    layer: LayerSurface,
    width: u32,
    height: u32,
    scale: u32,
    configured: bool,
}

struct SocketConn {
    stream: UnixStream,
}

impl AsFd for SocketConn {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.stream.as_fd()
    }
}

impl App {
    fn decode_target_edge(&self) -> u32 {
        self.surfaces
            .iter()
            .filter(|s| s.configured)
            .map(|s| {
                let scale = s.scale.max(1);
                (s.width * scale).max(s.height * scale)
            })
            .max()
            .unwrap_or(2560)
            .max(1)
    }

    fn request_decode(&self, path: String) {
        let tx = self.decode_tx.clone();
        let target = self.decode_target_edge();
        thread::spawn(move || {
            let started = Instant::now();
            let image = crate::decode::load_wallpaper(std::path::Path::new(&path), target)
                .map_err(|err| err.to_string());
            if let Err(err) = tx.send(DecodeResult {
                path,
                image,
                elapsed: started.elapsed(),
            }) {
                warn!("decode result send failed: {err}");
            }
        });
    }

    fn on_decoded(&mut self, result: DecodeResult) {
        if self.wanted_path.as_deref() != Some(result.path.as_str()) {
            return;
        }
        match result.image {
            Ok(image) => {
                info!(
                    "decoded {} ({}x{}) in {:.0?}",
                    result.path,
                    image.width(),
                    image.height(),
                    result.elapsed
                );
                self.image = Some(image);
                self.draw_all();
            }
            Err(err) => warn!("decode {}: {err}", result.path),
        }
    }

    fn output_geom(&self, output: &wl_output::WlOutput) -> Option<(String, OutputRect, u32)> {
        let info = self.output_state.info(output)?;
        let name = info
            .name
            .clone()
            .unwrap_or_else(|| format!("output-{}", output.id().protocol_id()));
        let scale = info.scale_factor.max(1) as u32;
        let (x, y) = info.logical_position.unwrap_or(info.location);
        let (width, height) = if let Some((w, h)) = info.logical_size {
            (w.max(0) as u32, h.max(0) as u32)
        } else {
            let mode = info
                .modes
                .iter()
                .find(|m| m.current)
                .or(info.modes.first())?;
            let (pw, ph) = mode.dimensions;
            ((pw as u32 / scale).max(1), (ph as u32 / scale).max(1))
        };
        Some((
            name,
            OutputRect {
                x,
                y,
                width: width.max(1),
                height: height.max(1),
            },
            scale,
        ))
    }

    fn spawn_for_output(&mut self, output: &wl_output::WlOutput) {
        let Some((name, geom, scale)) = self.output_geom(output) else {
            return;
        };
        if !self.config.wallpaper.matches_output(&name) {
            return;
        }
        if self.surfaces.iter().any(|s| s.output == *output) {
            return;
        }
        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Background,
            Some("vilewall"),
            Some(output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        // -1: cover the whole output, including the bar exclusive zone / floating gap.
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let _ = layer.set_buffer_scale(scale);
        layer.commit();

        let id = self.next_id;
        self.next_id += 1;
        info!(
            "surface {id} output={name} {}x{}+{}+{}",
            geom.width, geom.height, geom.x, geom.y
        );
        self.surfaces.push(WallSurface {
            id,
            output: output.clone(),
            output_name: name,
            geom,
            layer,
            width: 0,
            height: 0,
            scale,
            configured: false,
        });
    }

    fn destroy_output(&mut self, output: &wl_output::WlOutput) {
        self.surfaces.retain(|s| s.output != *output);
    }

    fn apply_configure(&mut self, index: usize, configure: LayerSurfaceConfigure) {
        let surface = &mut self.surfaces[index];
        if configure.new_size.0 > 0 {
            surface.width = configure.new_size.0;
        }
        if configure.new_size.1 > 0 {
            surface.height = configure.new_size.1;
        }
        surface.configured = surface.width > 0 && surface.height > 0;
        let id = surface.id;
        self.draw_id(id);
    }

    fn all_rects(&self) -> Vec<OutputRect> {
        self.surfaces.iter().map(|s| s.geom).collect()
    }

    fn draw_all(&mut self) {
        let ids: Vec<u64> = self.surfaces.iter().map(|s| s.id).collect();
        for id in ids {
            self.draw_id(id);
        }
    }

    fn draw_id(&mut self, id: u64) {
        let Some(index) = self.surfaces.iter().position(|s| s.id == id) else {
            return;
        };
        self.draw_index(index);
    }

    fn draw_index(&mut self, index: usize) {
        let Some(image) = self.image.as_ref() else {
            return;
        };
        let surface = &self.surfaces[index];
        if !surface.configured || surface.width == 0 || surface.height == 0 {
            return;
        }
        let scale = surface.scale.max(1);
        let buf_w = surface.width * scale;
        let buf_h = surface.height * scale;
        let stride = buf_w as i32 * 4;
        let needed = stride as usize * buf_h as usize;
        let all = self.all_rects();
        let geom = surface.geom;
        let mode = self.mode;

        if self.pool.len() < needed {
            if let Err(err) = self
                .pool
                .resize(needed.max(self.pool.len().saturating_mul(2)))
            {
                warn!("shm pool resize failed: {err}");
                return;
            }
        }
        let (buffer, canvas) = match self.pool.create_buffer(
            buf_w as i32,
            buf_h as i32,
            stride,
            wl_shm::Format::Argb8888,
        ) {
            Ok(pair) => pair,
            Err(err) => {
                warn!("create buffer failed: {err}");
                return;
            }
        };
        render_into(image, mode, canvas, buf_w, buf_h, geom, &all, scale);

        let layer = &self.surfaces[index].layer;
        let _ = layer.set_buffer_scale(scale);
        layer
            .wl_surface()
            .damage_buffer(0, 0, buf_w as i32, buf_h as i32);
        if let Err(err) = buffer.attach_to(layer.wl_surface()) {
            warn!("buffer attach failed: {err}");
            return;
        }
        layer.commit();
    }

    fn retry_socket(&mut self) {
        if self.writer.is_some() {
            return;
        }
        let path = socket_path();
        let stream = match UnixStream::connect(&path) {
            Ok(stream) => stream,
            Err(_) => return,
        };
        if let Err(err) = stream.set_nonblocking(true) {
            warn!("socket nonblocking failed: {err}");
            return;
        }
        let writer = match stream.try_clone() {
            Ok(writer) => writer,
            Err(err) => {
                warn!("socket clone failed: {err}");
                return;
            }
        };
        let token = match self.handle.insert_source(
            Generic::new(SocketConn { stream }, Interest::READ, Mode::Level),
            |_, conn, app| app.on_socket_read(conn),
        ) {
            Ok(token) => token,
            Err(err) => {
                warn!("insert socket source failed: {err}");
                return;
            }
        };
        self.writer = Some(writer);
        self.socket_token = Some(token);
        self.read_buf.clear();
        info!("connected to daemon");
        if let Some(writer) = self.writer.as_mut() {
            if let Err(err) = write_json_line(writer, &BarToDaemon::WallHello) {
                warn!("wall hello failed: {err}");
                self.drop_socket();
            }
        }
    }

    fn on_socket_read(&mut self, conn: &impl AsRef<SocketConn>) -> std::io::Result<PostAction> {
        let mut stream = &conn.as_ref().stream;
        let messages: Vec<DaemonToBar> = match read_json_lines(&mut stream, &mut self.read_buf) {
            Ok(messages) => messages,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                self.drop_socket();
                return Ok(PostAction::Remove);
            }
            Err(err) => {
                warn!("socket error: {err}");
                self.drop_socket();
                return Ok(PostAction::Remove);
            }
        };
        for message in messages {
            match message {
                DaemonToBar::SetWallpaper { image, mode } => {
                    info!("set wallpaper mode={} img={image}", mode.as_str());
                    self.mode = mode;
                    self.wanted_path = Some(image.clone());
                    self.request_decode(image);
                }
                DaemonToBar::Ok
                | DaemonToBar::Error { .. }
                | DaemonToBar::Frame { .. }
                | DaemonToBar::Keyboard { .. } => {}
            }
        }
        Ok(PostAction::Continue)
    }

    fn drop_socket(&mut self) {
        self.writer = None;
        if let Some(token) = self.socket_token.take() {
            self.handle.remove(token);
        }
        self.read_buf.clear();
    }

    fn surface_index_for_layer(&self, layer: &LayerSurface) -> Option<usize> {
        self.surfaces.iter().position(|s| s.layer == *layer)
    }

    fn surface_index_for_wl(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|s| s.layer.wl_surface() == surface)
    }
}

impl AsRef<SocketConn> for SocketConn {
    fn as_ref(&self) -> &SocketConn {
        self
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if let Some(index) = self.surface_index_for_wl(surface) {
            self.surfaces[index].scale = new_factor.max(1) as u32;
            let id = self.surfaces[index].id;
            self.draw_id(id);
        }
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
        output: wl_output::WlOutput,
    ) {
        self.spawn_for_output(&output);
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let Some((name, geom, scale)) = self.output_geom(&output) {
            if let Some(surface) = self.surfaces.iter_mut().find(|s| s.output == output) {
                surface.output_name = name;
                surface.geom = geom;
                surface.scale = scale;
                let id = surface.id;
                self.draw_id(id);
                return;
            }
        }
        self.spawn_for_output(&output);
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.destroy_output(&output);
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(index) = self.surface_index_for_layer(layer) {
            self.surfaces.remove(index);
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if let Some(index) = self.surface_index_for_layer(layer) {
            self.apply_configure(index, configure);
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        _capability: Capability,
    ) {
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _capability: Capability,
    ) {
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        _events: &[PointerEvent],
    ) {
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
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
    use super::{parse_args, usage};
    use vile_protocol::WallpaperMode;

    #[test]
    fn parses_img_and_mode() {
        let args = parse_args([
            "--img".into(),
            "a.jpg".into(),
            "--mode".into(),
            "span".into(),
        ])
        .unwrap();
        assert_eq!(args.img.unwrap().as_os_str(), "a.jpg");
        assert_eq!(args.mode, Some(WallpaperMode::Span));
        let args = parse_args(["--img=/tmp/w.png".into(), "--mode=scale".into()]).unwrap();
        assert_eq!(args.mode, Some(WallpaperMode::Fit));
        assert!(parse_args(["--mode".into(), "nope".into()]).is_err());
        assert!(usage().contains("vilewall"));
    }
}
