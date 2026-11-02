mod render;

use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::{Context, Result};
use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use log::{debug, info, warn};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
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
    dirty_cell_ranges, load_config, merge_ranges, socket_path, write_json_line, BarConfig,
    BarToDaemon, CellStyle, Config, DaemonToBar, PointerEvent as ProtoPointer, Position,
};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use wayland_client::{Connection, Proxy, QueueHandle};

use crate::render::{cell_damage_rect, FontEngine};

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = load_config()?;
    if config.bars.is_empty() {
        anyhow::bail!("no bars in config");
    }

    let conn = Connection::connect_to_env().context("connecting to Wayland")?;
    let (globals, event_queue) = registry_queue_init(&conn).context("wayland registry")?;
    let qh = event_queue.handle();

    let compositor =
        CompositorState::bind(&globals, &qh).context("wl_compositor is not available")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context("layer shell is not available")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm is not available")?;
    let pool = SlotPool::new(256 * 64 * 4, &shm).context("creating shm pool")?;

    let mut event_loop = EventLoop::<App>::try_new().context("creating event loop")?;
    let handle = event_loop.handle();
    calloop_wayland_source::WaylandSource::new(conn.clone(), event_queue)
        .insert(handle.clone())
        .map_err(|err| anyhow::anyhow!("inserting wayland source: {err}"))?;

    handle
        .insert_source(
            Timer::from_duration(Duration::from_millis(200)),
            |_, _, app| {
                app.retry_sockets();
                TimeoutAction::ToDuration(Duration::from_millis(200))
            },
        )
        .map_err(|err| anyhow::anyhow!("inserting reconnect timer: {err}"))?;

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
        font: FontEngine::new(),
        surfaces: Vec::new(),
        next_id: 1,
        pointer: None,
        keyboard: None,
        keyboard_surface: None,
    };

    info!("vilebar starting");
    event_loop
        .run(None, &mut app, |_| {})
        .context("event loop")?;
    Ok(())
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
    font: FontEngine,
    surfaces: Vec<BarSurface>,
    next_id: u64,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    keyboard_surface: Option<u64>,
}

struct BarSurface {
    id: u64,
    bar: BarConfig,
    output: wl_output::WlOutput,
    output_name: String,
    layer: LayerSurface,
    width: u32,
    height: u32,
    scale: u32,
    cell_w: f32,
    origin_x: f32,
    origin_y: f32,
    cols: u32,
    last_col: Option<u32>,
    text: String,
    drawn_text: String,
    cells: Vec<CellStyle>,
    drawn_cells: Vec<CellStyle>,
    fg: u32,
    bg: u32,
    drawn_fg: Option<u32>,
    drawn_bg: Option<u32>,
    pixels: Vec<u8>,
    pixels_w: u32,
    pixels_h: u32,
    writer: Option<UnixStream>,
    socket_token: Option<RegistrationToken>,
    read_buf: Vec<u8>,
    configured: bool,
    keyboard_exclusive: bool,
}

struct SocketConn {
    surface_id: u64,
    stream: UnixStream,
}

impl AsFd for SocketConn {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.stream.as_fd()
    }
}

impl App {
    fn spawn_for_output(&mut self, output: &wl_output::WlOutput) {
        let Some(info) = self.output_state.info(output) else {
            return;
        };
        let output_name = info
            .name
            .clone()
            .unwrap_or_else(|| format!("output-{}", output.id().protocol_id()));
        let scale = info.scale_factor.max(1) as u32;
        let bars: Vec<BarConfig> = self
            .config
            .bars
            .iter()
            .filter(|bar| bar.matches_output(&output_name))
            .cloned()
            .collect();
        for bar in bars {
            if self
                .surfaces
                .iter()
                .any(|surface| surface.output == *output && surface.bar.name == bar.name)
            {
                continue;
            }
            self.create_surface(output, output_name.clone(), scale, bar);
        }
    }

    fn create_surface(
        &mut self,
        output: &wl_output::WlOutput,
        output_name: String,
        scale: u32,
        bar: BarConfig,
    ) {
        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Top,
            Some("vilebar"),
            Some(output),
        );
        let anchor = match bar.position {
            Position::Top => Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
            Position::Bottom => Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
        };
        layer.set_anchor(anchor);
        layer.set_size(0, bar.surface_height());
        let (margin_top, margin_right, margin_bottom, margin_left) = bar.layer_margins();
        layer.set_margin(margin_top, margin_right, margin_bottom, margin_left);
        layer.set_exclusive_zone(bar.exclusive_zone());
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let _ = layer.set_buffer_scale(scale);
        if bar.corner_radius > 0 {
            // Punched corners are alpha-0; empty opaque region lets wallpaper show through.
            layer.wl_surface().set_opaque_region(None);
        }
        layer.commit();

        let id = self.next_id;
        self.next_id += 1;
        info!(
            "surface {id} bar={} output={output_name} height={} surface={}",
            bar.name,
            bar.height,
            bar.surface_height()
        );
        self.surfaces.push(BarSurface {
            id,
            fg: bar.fg_argb(),
            bg: bar.bg_argb(),
            bar,
            output: output.clone(),
            output_name,
            layer,
            width: 0,
            height: 0,
            scale,
            cell_w: 8.0,
            origin_x: 0.0,
            origin_y: 0.0,
            cols: 0,
            last_col: None,
            text: String::new(),
            drawn_text: String::new(),
            cells: Vec::new(),
            drawn_cells: Vec::new(),
            drawn_fg: None,
            drawn_bg: None,
            pixels: Vec::new(),
            pixels_w: 0,
            pixels_h: 0,
            writer: None,
            socket_token: None,
            read_buf: Vec::new(),
            configured: false,
            keyboard_exclusive: false,
        });
    }

    fn destroy_output(&mut self, output: &wl_output::WlOutput) {
        let ids: Vec<u64> = self
            .surfaces
            .iter()
            .filter(|surface| surface.output == *output)
            .map(|surface| surface.id)
            .collect();
        for id in ids {
            self.drop_socket(id, true);
            self.surfaces.retain(|surface| surface.id != id);
        }
    }

    fn surface_index_for_layer(&self, layer: &LayerSurface) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|surface| surface.layer == *layer)
    }

    fn surface_index_for_wl(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|bar| bar.layer.wl_surface() == surface)
    }

    fn apply_configure(&mut self, index: usize, configure: LayerSurfaceConfigure) {
        let surface = &mut self.surfaces[index];
        if configure.new_size.0 > 0 {
            surface.width = configure.new_size.0;
        }
        surface.height = if configure.new_size.1 > 0 {
            configure.new_size.1
        } else {
            surface.bar.surface_height()
        };
        self.recompute_cells(index);
        self.surfaces[index].configured = true;
        let id = self.surfaces[index].id;
        if self.surfaces[index].writer.is_some() {
            self.send_configure(id);
        } else {
            self.try_connect(id);
        }
        self.draw_id(id);
    }

    fn recompute_cells(&mut self, index: usize) {
        let surface = &self.surfaces[index];
        let scale = surface.scale.max(1);
        let insets = surface
            .bar
            .text_insets(surface.width, surface.height.max(1));
        let font_size = (insets.inner_height.max(1) * scale) as f32;
        let font = surface.bar.font.clone();
        let cell_w_buffer = self.font.cell_width(&font, font_size, font_size);
        let cell_w = cell_w_buffer / scale as f32;
        let inner_w = insets.inner_width as f32;
        let cols = if cell_w > 0.0 && inner_w > 0.0 {
            (inner_w / cell_w).floor() as u32
        } else {
            0
        };
        let origin_x = insets.origin_x() as f32;
        let origin_y = insets.origin_y() as f32;
        let surface = &mut self.surfaces[index];
        let layout_changed = surface.cell_w != cell_w
            || surface.cols != cols
            || surface.origin_x != origin_x
            || surface.origin_y != origin_y;
        surface.cell_w = cell_w;
        surface.origin_x = origin_x;
        surface.origin_y = origin_y;
        surface.cols = cols;
        if layout_changed {
            surface.drawn_text.clear();
            surface.drawn_cells.clear();
            debug!(
                "surface {} cells: inner={}x{} cell_w={cell_w:.2} cols={cols} margin={}x{}",
                surface.id,
                insets.inner_width,
                insets.inner_height,
                insets.margin_x,
                insets.margin_y
            );
        }
    }

    fn retry_sockets(&mut self) {
        let ids: Vec<u64> = self
            .surfaces
            .iter()
            .filter(|surface| surface.configured && surface.writer.is_none())
            .map(|surface| surface.id)
            .collect();
        for id in ids {
            self.try_connect(id);
        }
    }

    fn try_connect(&mut self, id: u64) {
        let Some(surface) = self.surfaces.iter().find(|surface| surface.id == id) else {
            return;
        };
        if !surface.configured || surface.writer.is_some() {
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
            Generic::new(
                SocketConn {
                    surface_id: id,
                    stream,
                },
                Interest::READ,
                Mode::Level,
            ),
            |_, conn, app| app.on_socket_read(conn),
        ) {
            Ok(token) => token,
            Err(err) => {
                warn!("insert socket source failed: {err}");
                return;
            }
        };
        if let Some(surface) = self.surfaces.iter_mut().find(|surface| surface.id == id) {
            surface.writer = Some(writer);
            surface.socket_token = Some(token);
            surface.read_buf.clear();
            info!("surface {id} connected to daemon");
        }
        self.send_hello(id);
    }

    fn on_socket_read(&mut self, conn: &impl AsRef<SocketConn>) -> std::io::Result<PostAction> {
        let conn = conn.as_ref();
        let id = conn.surface_id;
        let Some(index) = self.surfaces.iter().position(|surface| surface.id == id) else {
            return Ok(PostAction::Remove);
        };
        let mut stream = &conn.stream;
        let messages: Vec<DaemonToBar> =
            match vile_protocol::read_json_lines(&mut stream, &mut self.surfaces[index].read_buf) {
                Ok(messages) => messages,
                Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.drop_socket(id, false);
                    return Ok(PostAction::Remove);
                }
                Err(err) => {
                    warn!("surface {id} socket error: {err}");
                    self.drop_socket(id, false);
                    return Ok(PostAction::Remove);
                }
            };
        for message in messages {
            match message {
                DaemonToBar::Frame {
                    text,
                    fg,
                    bg,
                    cells,
                } => {
                    if let Some(surface) = self.surfaces.iter_mut().find(|surface| surface.id == id)
                    {
                        surface.text = text;
                        surface.fg = fg;
                        surface.bg = bg;
                        surface.cells = cells;
                    }
                    self.draw_id(id);
                }
                DaemonToBar::Keyboard { exclusive } => {
                    self.set_keyboard_exclusive(id, exclusive);
                }
                DaemonToBar::Ok | DaemonToBar::Error { .. } | DaemonToBar::SetWallpaper { .. } => {}
            }
        }
        Ok(PostAction::Continue)
    }

    fn send_hello(&mut self, id: u64) {
        let Some(surface) = self.surfaces.iter_mut().find(|surface| surface.id == id) else {
            return;
        };
        let msg = BarToDaemon::Hello {
            bar: surface.bar.name.clone(),
            output: surface.output_name.clone(),
            width_px: surface.width,
            height_px: surface.height,
            scale: surface.scale,
            cols: surface.cols,
        };
        let failed = surface
            .writer
            .as_mut()
            .and_then(|writer| write_json_line(writer, &msg).err());
        if let Some(err) = failed {
            warn!("hello failed: {err}");
            self.drop_socket(id, true);
        }
    }

    fn send_configure(&mut self, id: u64) {
        let Some(surface) = self.surfaces.iter_mut().find(|surface| surface.id == id) else {
            return;
        };
        let msg = BarToDaemon::Configure {
            width_px: surface.width,
            height_px: surface.height,
            scale: surface.scale,
            cols: surface.cols,
        };
        let failed = surface
            .writer
            .as_mut()
            .and_then(|writer| write_json_line(writer, &msg).err());
        if let Some(err) = failed {
            warn!("configure failed: {err}");
            self.drop_socket(id, true);
        }
    }

    fn set_keyboard_exclusive(&mut self, id: u64, exclusive: bool) {
        let Some(surface) = self.surfaces.iter_mut().find(|surface| surface.id == id) else {
            return;
        };
        if surface.keyboard_exclusive == exclusive {
            return;
        }
        surface.keyboard_exclusive = exclusive;
        let mode = if exclusive {
            KeyboardInteractivity::Exclusive
        } else {
            KeyboardInteractivity::None
        };
        surface.layer.set_keyboard_interactivity(mode);
        surface.layer.commit();
        info!("surface {id} keyboard exclusive={exclusive}");
    }

    fn send_keyboard_lost(&mut self, id: u64) {
        let Some(surface) = self.surfaces.iter_mut().find(|surface| surface.id == id) else {
            return;
        };
        if !surface.keyboard_exclusive {
            return;
        }
        let failed = surface
            .writer
            .as_mut()
            .and_then(|writer| write_json_line(writer, &BarToDaemon::KeyboardLost).err());
        if let Some(err) = failed {
            warn!("keyboard lost failed: {err}");
            self.drop_socket(id, true);
        }
    }

    fn send_key(&mut self, id: u64, name: Option<String>, text: Option<String>) {
        let Some(surface) = self.surfaces.iter_mut().find(|surface| surface.id == id) else {
            return;
        };
        if !surface.keyboard_exclusive {
            return;
        }
        let msg = BarToDaemon::Key { name, text };
        let failed = surface
            .writer
            .as_mut()
            .and_then(|writer| write_json_line(writer, &msg).err());
        if let Some(err) = failed {
            warn!("key failed: {err}");
            self.drop_socket(id, true);
        }
    }

    fn handle_key(&mut self, event: KeyEvent) {
        let Some(id) = self.keyboard_surface else {
            return;
        };
        let name = match event.keysym {
            Keysym::Escape => Some("escape"),
            Keysym::Return | Keysym::KP_Enter => Some("return"),
            Keysym::BackSpace => Some("backspace"),
            Keysym::Tab | Keysym::ISO_Left_Tab => Some("tab"),
            Keysym::Up | Keysym::KP_Up => Some("up"),
            Keysym::Down | Keysym::KP_Down => Some("down"),
            Keysym::Left | Keysym::KP_Left => Some("left"),
            Keysym::Right | Keysym::KP_Right => Some("right"),
            _ => None,
        };
        if let Some(name) = name {
            self.send_key(id, Some(name.into()), None);
            return;
        }
        let Some(text) = event.utf8 else {
            return;
        };
        if text.is_empty() || text.chars().any(|c| c.is_control()) {
            return;
        }
        self.send_key(id, None, Some(text));
    }

    fn send_pointer(&mut self, id: u64, col: u32, event: ProtoPointer) {
        let Some(surface) = self.surfaces.iter_mut().find(|surface| surface.id == id) else {
            return;
        };
        let msg = BarToDaemon::Pointer { col, event };
        let failed = surface
            .writer
            .as_mut()
            .and_then(|writer| write_json_line(writer, &msg).err());
        if let Some(err) = failed {
            warn!("pointer failed: {err}");
            self.drop_socket(id, true);
        }
    }

    fn drop_socket(&mut self, id: u64, remove_source: bool) {
        let token = self
            .surfaces
            .iter_mut()
            .find(|surface| surface.id == id)
            .and_then(|surface| {
                surface.writer = None;
                surface.read_buf.clear();
                surface.socket_token.take()
            });
        if remove_source {
            if let Some(token) = token {
                self.handle.remove(token);
            }
        }
    }

    fn pointer_col(&self, surface: &BarSurface, x: f64) -> u32 {
        if surface.cell_w <= 0.0 {
            return 0;
        }
        let x = (x - f64::from(surface.origin_x)).max(0.0);
        let col = (x / f64::from(surface.cell_w)).floor() as u32;
        if surface.cols == 0 {
            col
        } else {
            col.min(surface.cols.saturating_sub(1))
        }
    }

    fn handle_pointer(&mut self, events: &[PointerEvent]) {
        for event in events {
            let Some(index) = self.surface_index_for_wl(&event.surface) else {
                continue;
            };
            let id = self.surfaces[index].id;
            let col = self.pointer_col(&self.surfaces[index], event.position.0);
            match event.kind {
                PointerEventKind::Enter { .. } => {
                    self.surfaces[index].last_col = Some(col);
                    self.send_pointer(id, col, ProtoPointer::Enter);
                }
                PointerEventKind::Leave { .. } => {
                    self.surfaces[index].last_col = None;
                    self.send_pointer(id, col, ProtoPointer::Leave);
                }
                PointerEventKind::Motion { .. } => {
                    if self.surfaces[index].last_col != Some(col) {
                        self.surfaces[index].last_col = Some(col);
                        self.send_pointer(id, col, ProtoPointer::Motion);
                    }
                }
                PointerEventKind::Press { button, .. } => {
                    self.surfaces[index].last_col = Some(col);
                    self.send_pointer(
                        id,
                        col,
                        ProtoPointer::Button {
                            code: button,
                            pressed: true,
                        },
                    );
                }
                PointerEventKind::Release { button, .. } => {
                    self.send_pointer(
                        id,
                        col,
                        ProtoPointer::Button {
                            code: button,
                            pressed: false,
                        },
                    );
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } => {
                    let steps = axis_steps(&vertical, &horizontal);
                    if steps != 0 {
                        self.send_pointer(id, col, ProtoPointer::Axis { steps });
                    }
                }
            }
        }
    }

    fn draw_id(&mut self, id: u64) {
        let Some(index) = self.surfaces.iter().position(|surface| surface.id == id) else {
            return;
        };
        self.draw_index(index);
    }

    fn draw_index(&mut self, index: usize) {
        let surface = &self.surfaces[index];
        if surface.width == 0 || surface.height == 0 {
            return;
        }
        let scale = surface.scale.max(1);
        let buf_w = surface.width * scale;
        let buf_h = surface.height * scale;
        let stride = buf_w as i32 * 4;
        let needed = (stride as usize) * buf_h as usize;
        let font = surface.bar.font.clone();
        let text = surface.text.clone();
        let drawn_text = surface.drawn_text.clone();
        let cells = surface.cells.clone();
        let drawn_cells = surface.drawn_cells.clone();
        let fg = surface.fg;
        let bg = surface.bg;
        let cell_w = surface.cell_w * scale as f32;
        let origin_x = surface.origin_x * scale as f32;
        let origin_y = surface.origin_y * scale as f32;
        let insets = surface
            .bar
            .text_insets(surface.width, surface.height.max(1));
        let font_size = (insets.inner_height.max(1) * scale) as f32;
        let size_changed = surface.pixels_w != buf_w
            || surface.pixels_h != buf_h
            || surface.pixels.len() != needed;
        let colors_changed = surface.drawn_fg != Some(fg) || surface.drawn_bg != Some(bg);
        let style_changed = drawn_cells != cells;
        let edges = surface.bar.drawn_edges();
        let radius_px = surface.bar.corner_radius.saturating_mul(scale);
        let full = size_changed || colors_changed || style_changed || drawn_text.is_empty();
        let dirty = if full {
            vec![(0, surface.cols.max(1) as usize)]
        } else {
            let mut dirty = dirty_cell_ranges(&drawn_text, &text);
            dirty.extend(dirty_style_ranges(&drawn_cells, &cells));
            merge_ranges(dirty)
        };
        if dirty.is_empty() {
            return;
        }

        let surface = &mut self.surfaces[index];
        if size_changed {
            surface.pixels.resize(needed, 0);
            surface.pixels_w = buf_w;
            surface.pixels_h = buf_h;
        }

        self.font.paint(
            &mut self.surfaces[index].pixels,
            buf_w,
            buf_h,
            &text,
            &font,
            font_size,
            cell_w,
            origin_x,
            origin_y,
            fg,
            bg,
            &cells,
            &dirty,
            full,
            insets.border.saturating_mul(scale),
            radius_px,
            edges,
        );

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
        canvas.copy_from_slice(&self.surfaces[index].pixels);

        let layer = &self.surfaces[index].layer;
        let _ = layer.set_buffer_scale(scale);
        if full {
            layer
                .wl_surface()
                .damage_buffer(0, 0, buf_w as i32, buf_h as i32);
        } else {
            for &(start, end) in &dirty {
                let (x, y, w, h) = cell_damage_rect(start, end, cell_w, origin_x, buf_w, buf_h);
                if w > 0 && h > 0 {
                    layer.wl_surface().damage_buffer(x, y, w, h);
                }
            }
        }
        if let Err(err) = buffer.attach_to(layer.wl_surface()) {
            warn!("buffer attach failed: {err}");
            return;
        }
        layer.commit();
        let surface = &mut self.surfaces[index];
        surface.drawn_text = text;
        surface.drawn_cells = cells;
        surface.drawn_fg = Some(fg);
        surface.drawn_bg = Some(bg);
    }
}

fn dirty_style_ranges(old: &[CellStyle], new: &[CellStyle]) -> Vec<(usize, usize)> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut prev = BTreeMap::new();
    for cell in old {
        prev.insert(cell.col, (cell.fg, cell.bg));
    }
    let mut next = BTreeMap::new();
    for cell in new {
        next.insert(cell.col, (cell.fg, cell.bg));
    }
    let cols: BTreeSet<u32> = prev.keys().chain(next.keys()).copied().collect();
    let mut ranges = Vec::new();
    for col in cols {
        if prev.get(&col) != next.get(&col) {
            ranges.push((col as usize, col as usize + 1));
        }
    }
    ranges
}

fn axis_steps(
    vertical: &smithay_client_toolkit::seat::pointer::AxisScroll,
    horizontal: &smithay_client_toolkit::seat::pointer::AxisScroll,
) -> i32 {
    if vertical.discrete != 0 {
        return vertical.discrete;
    }
    if horizontal.discrete != 0 {
        return horizontal.discrete;
    }
    let from_120 = |value: i32| {
        if value.abs() >= 120 {
            value / 120
        } else {
            value.signum()
        }
    };
    let vertical_steps = from_120(vertical.value120);
    if vertical_steps != 0 {
        return vertical_steps;
    }
    let horizontal_steps = from_120(horizontal.value120);
    if horizontal_steps != 0 {
        return horizontal_steps;
    }
    if vertical.absolute.abs() > 0.01 {
        return if vertical.absolute > 0.0 { 1 } else { -1 };
    }
    if horizontal.absolute.abs() > 0.01 {
        return if horizontal.absolute > 0.0 { 1 } else { -1 };
    }
    0
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
            self.recompute_cells(index);
            let id = self.surfaces[index].id;
            if self.surfaces[index].configured {
                if self.surfaces[index].writer.is_some() {
                    self.send_configure(id);
                }
                self.draw_id(id);
            }
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
            let id = self.surfaces[index].id;
            self.drop_socket(id, true);
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
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(err) => warn!("failed to bind pointer: {err}"),
            }
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seat_state.get_keyboard_with_repeat(
                qh,
                &seat,
                None,
                self.handle.clone(),
                Box::new(|app, _, event| app.handle_key(event)),
            ) {
                Ok(keyboard) => self.keyboard = Some(keyboard),
                Err(err) => warn!("failed to bind keyboard: {err}"),
            }
        }
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
                pointer.release();
            }
        }
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
            self.keyboard_surface = None;
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
        surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        self.keyboard_surface = self
            .surface_index_for_wl(surface)
            .map(|index| self.surfaces[index].id);
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        let id = self
            .surface_index_for_wl(surface)
            .map(|index| self.surfaces[index].id);
        self.keyboard_surface = None;
        if let Some(id) = id {
            self.send_keyboard_lost(id);
        }
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event);
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
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        self.handle_pointer(events);
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
