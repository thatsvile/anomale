use std::collections::HashSet;
use std::time::Duration;

use niri_ipc::Workspace;
use vile_protocol::{
    display_width, fit_cells, pack_layout, pack_line, BarConfig, CellStyle, HwMetric, MeterStyle,
    NotificationSide,
};

use crate::apps::{command_remainder, AppsPrompt};
use crate::niri::workspaces_for;
use crate::sys::{Battery, Memory, SysSnapshot, Volume};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeKind {
    InspectClick { col: u32, button: u32 },
    InspectScroll { col: u32, steps: i32 },
    VolumeOverlay,
    BatteryOverlay,
    MemOverlay,
    CpuOverlay,
    GpuOverlay,
}

#[derive(Debug, Clone)]
pub struct ModuleSpan {
    pub name: String,
    pub start: u32,
    pub end: u32,
    pub workspace_id: Option<u64>,
}

impl ModuleSpan {
    pub fn contains(&self, col: u32) -> bool {
        col >= self.start && col < self.end
    }
}

#[derive(Debug, Clone)]
pub struct ComposedLine {
    pub text: String,
    pub spans: Vec<ModuleSpan>,
    pub cells: Vec<CellStyle>,
    pub left_start: usize,
    pub left_w: usize,
    pub right_start: usize,
    pub right_w: usize,
}

impl ComposedLine {
    fn full(text: String, spans: Vec<ModuleSpan>, cells: Vec<CellStyle>, cols: usize) -> Self {
        Self {
            text,
            spans,
            cells,
            left_start: 0,
            left_w: cols,
            right_start: 0,
            right_w: 0,
        }
    }

    pub fn side_region(&self, side: NotificationSide) -> (usize, usize) {
        match side {
            NotificationSide::Left => (self.left_start, self.left_w.max(1)),
            NotificationSide::Right => (self.right_start, self.right_w.max(1)),
        }
    }
}

struct Piece {
    name: String,
    text: String,
    invert: Vec<(usize, usize)>,
    workspaces: Vec<(u64, usize, usize)>,
}

pub fn is_full_overlay(kind: &ModeKind) -> bool {
    matches!(
        kind,
        ModeKind::VolumeOverlay
            | ModeKind::BatteryOverlay
            | ModeKind::MemOverlay
            | ModeKind::CpuOverlay
            | ModeKind::GpuOverlay
    )
}

pub fn should_anim_mode(prev: Option<&ModeKind>, next: Option<&ModeKind>) -> bool {
    prev.is_some_and(is_full_overlay) != next.is_some_and(is_full_overlay)
}

pub fn span_at(spans: &[ModuleSpan], col: u32) -> Option<&ModuleSpan> {
    spans
        .iter()
        .filter(|span| span.contains(col))
        .min_by_key(|span| span.end.saturating_sub(span.start))
}

pub fn workspace_id_at(spans: &[ModuleSpan], col: u32) -> Option<u64> {
    if let Some(id) = span_at(spans, col).and_then(|span| span.workspace_id) {
        return Some(id);
    }
    if !spans
        .iter()
        .any(|span| span.name == "workspaces" && span.contains(col))
    {
        return None;
    }
    spans
        .iter()
        .filter(|span| span.name == "workspaces" && span.workspace_id.is_some())
        .min_by_key(|span| {
            let mid = span.start.saturating_add(span.end) / 2;
            col.abs_diff(mid)
        })
        .and_then(|span| span.workspace_id)
}

pub fn compose_line(
    bar: &BarConfig,
    cols: usize,
    clock: &str,
    hover_col: Option<u32>,
    mode: Option<&ModeKind>,
    apps: Option<&AppsPrompt>,
    sys: &SysSnapshot,
    workspaces: &[Workspace],
    occupied: &HashSet<u64>,
    output: Option<&str>,
    charge_elapsed: Duration,
    inbox: Option<&str>,
    toast: Option<(NotificationSide, &str)>,
    notify_count: usize,
) -> ComposedLine {
    if cols == 0 {
        return ComposedLine::full(String::new(), Vec::new(), Vec::new(), 0);
    }

    if let Some(apps) = apps {
        return apps_line(cols, apps, bar.fg_argb(), bar.bg_argb());
    }

    if let Some(text) = inbox {
        return inbox_line(cols, text);
    }

    if let Some(mode) = mode {
        return match mode {
            ModeKind::InspectClick { col, button } => ComposedLine::full(
                fit_cells(&format!("clicked col {}  button {button}", col + 1), cols),
                Vec::new(),
                Vec::new(),
                cols,
            ),
            ModeKind::InspectScroll { col, steps } => ComposedLine::full(
                fit_cells(&format!("scrolled col {}  steps {steps}", col + 1), cols),
                Vec::new(),
                Vec::new(),
                cols,
            ),
            ModeKind::VolumeOverlay => {
                volume_overlay_line(cols, sys.volume.as_ref(), bar.fg_argb(), bar.bg_argb())
            }
            ModeKind::BatteryOverlay => {
                battery_overlay_line(cols, sys.battery.as_ref(), charge_elapsed)
            }
            ModeKind::MemOverlay => mem_overlay_line(cols, &sys.memory),
            ModeKind::CpuOverlay => {
                hw_overlay_line(cols, "cpu", "CPU", sys.cpu_usage, sys.cpu_temp_c)
            }
            ModeKind::GpuOverlay => match sys.gpu.as_ref() {
                Some(gpu) => hw_overlay_line(cols, "gpu", "GPU", gpu.usage, gpu.temp_c),
                None => hw_overlay_line(cols, "gpu", "GPU", 0, None),
            },
        };
    }

    let left = render_side(
        bar,
        &bar.modules.left,
        clock,
        hover_col,
        cols as u32,
        sys,
        workspaces,
        occupied,
        output,
        charge_elapsed,
        notify_count,
    );
    let center = render_side(
        bar,
        &bar.modules.center,
        clock,
        hover_col,
        cols as u32,
        sys,
        workspaces,
        occupied,
        output,
        charge_elapsed,
        notify_count,
    );
    let right = render_side(
        bar,
        &bar.modules.right,
        clock,
        hover_col,
        cols as u32,
        sys,
        workspaces,
        occupied,
        output,
        charge_elapsed,
        notify_count,
    );

    let left_text = join_text(&left);
    let center_text = join_text(&center);
    let right_text = join_text(&right);
    let packed = match toast {
        Some((NotificationSide::Left, text)) => pack_layout(cols, text, &center_text, &right_text),
        Some((NotificationSide::Right, text)) => pack_layout(cols, &left_text, &center_text, text),
        None => pack_layout(cols, &left_text, &center_text, &right_text),
    };
    let mut spans = Vec::new();
    let mut invert = Vec::new();
    match toast.map(|(side, _)| side) {
        Some(NotificationSide::Left) => {
            place_side(
                &center,
                packed.center_start,
                packed.center_w,
                &mut spans,
                &mut invert,
            );
            place_side(
                &right,
                packed.right_start,
                packed.right_w,
                &mut spans,
                &mut invert,
            );
        }
        Some(NotificationSide::Right) => {
            place_side(&left, 0, packed.left_w, &mut spans, &mut invert);
            place_side(
                &center,
                packed.center_start,
                packed.center_w,
                &mut spans,
                &mut invert,
            );
        }
        None => {
            place_side(&left, 0, packed.left_w, &mut spans, &mut invert);
            place_side(
                &center,
                packed.center_start,
                packed.center_w,
                &mut spans,
                &mut invert,
            );
            place_side(
                &right,
                packed.right_start,
                packed.right_w,
                &mut spans,
                &mut invert,
            );
        }
    }

    let fg = bar.fg_argb();
    let bg = bar.bg_argb();
    let cells = invert
        .into_iter()
        .flat_map(|(start, end)| {
            (start..end).map(move |col| CellStyle {
                col: col as u32,
                fg: bg,
                bg: fg,
            })
        })
        .collect();

    ComposedLine {
        text: packed.text,
        spans,
        cells,
        left_start: 0,
        left_w: packed.left_w,
        right_start: packed.right_start,
        right_w: packed.right_w,
    }
}

fn render_side(
    bar: &BarConfig,
    names: &[String],
    clock: &str,
    hover_col: Option<u32>,
    cols: u32,
    sys: &SysSnapshot,
    workspaces: &[Workspace],
    occupied: &HashSet<u64>,
    output: Option<&str>,
    charge_elapsed: Duration,
    notify_count: usize,
) -> Vec<Piece> {
    names
        .iter()
        .filter_map(|name| {
            render_module(
                bar,
                name,
                clock,
                hover_col,
                cols,
                sys,
                workspaces,
                occupied,
                output,
                charge_elapsed,
                notify_count,
            )
        })
        .collect()
}

fn render_module(
    bar: &BarConfig,
    name: &str,
    clock: &str,
    hover_col: Option<u32>,
    cols: u32,
    sys: &SysSnapshot,
    workspaces: &[Workspace],
    occupied: &HashSet<u64>,
    output: Option<&str>,
    charge_elapsed: Duration,
    notify_count: usize,
) -> Option<Piece> {
    let piece = match name {
        "clock" => plain("clock", clock),
        "hint" => plain("hint", "click any cell"),
        "hover" => {
            let text = match hover_col {
                Some(col) => format!("h:{} c:{cols}", col + 1),
                None => format!("h:- c:{cols}"),
            };
            plain("hover", &text)
        }
        "workspaces" => render_workspaces(workspaces, occupied, output),
        "volume" => plain(
            "volume",
            &volume_module(sys.volume.as_ref(), bar.volume_style),
        ),
        "battery" => {
            let bat = sys.battery.as_ref()?;
            battery_piece(bat, bar.battery_style, charge_elapsed)
        }
        "cpu" => plain(
            "cpu",
            &hw_module(
                "CPU",
                sys.cpu_usage,
                sys.cpu_temp_c,
                bar.cpu_style,
                bar.cpu_metric,
            ),
        ),
        "gpu" => {
            let gpu = sys.gpu.as_ref()?;
            plain(
                "gpu",
                &hw_module("GPU", gpu.usage, gpu.temp_c, bar.gpu_style, bar.gpu_metric),
            )
        }
        "mem" => plain("mem", &mem_module(&sys.memory, bar.mem_style)),
        "notifications" => notifications_piece(notify_count)?,
        other => plain(other, other),
    };
    if piece.text.is_empty() {
        None
    } else {
        Some(piece)
    }
}

fn render_workspaces(
    workspaces: &[Workspace],
    occupied: &HashSet<u64>,
    output: Option<&str>,
) -> Piece {
    let Some(output) = output else {
        return plain("workspaces", "-");
    };
    let list = workspaces_for(workspaces, output, occupied);
    if list.is_empty() {
        return plain("workspaces", "-");
    }
    let mut text = String::new();
    let mut invert = Vec::new();
    let mut ids = Vec::new();
    for (i, ws) in list.iter().enumerate() {
        if i > 0 {
            text.push(' ');
        }
        let start = display_width(&text);
        let label = ws.idx.to_string();
        let end = start + display_width(&label);
        text.push_str(&label);
        ids.push((ws.id, start, end));
        if ws.is_active {
            invert.push((start, end));
        }
    }
    Piece {
        name: "workspaces".into(),
        text,
        invert,
        workspaces: ids,
    }
}

fn inbox_line(cols: usize, text: &str) -> ComposedLine {
    ComposedLine::full(pack_line(cols, "", text, ""), Vec::new(), Vec::new(), cols)
}

fn apps_line(cols: usize, prompt: &AppsPrompt, fg: u32, bg: u32) -> ComposedLine {
    if command_remainder(&prompt.query).is_some() {
        return centered_prompt_line(cols, &prompt.query, "", 0, 0, fg, bg);
    }
    if prompt.query.is_empty() {
        const PLACEHOLDER: &str = "search apps...";
        return centered_prompt_line(
            cols,
            &format!("> {PLACEHOLDER}"),
            "",
            display_width("> "),
            display_width(PLACEHOLDER),
            fg,
            bg,
        );
    }
    let selected = prompt.selected_match();
    let ghost = selected
        .and_then(|app| prefix_remainder(&app.name, &prompt.query))
        .unwrap_or_default();
    let prompt_text = format!("> {}{ghost}", prompt.query);
    let right = if ghost.is_empty() {
        selected
            .filter(|app| !app.name.eq_ignore_ascii_case(&prompt.query))
            .map(|app| app.name.as_str())
            .unwrap_or("")
    } else {
        ""
    };
    centered_prompt_line(
        cols,
        &prompt_text,
        right,
        display_width(&format!("> {}", prompt.query)),
        display_width(ghost),
        fg,
        bg,
    )
}

fn centered_prompt_line(
    cols: usize,
    prompt: &str,
    right: &str,
    invert_from: usize,
    invert_w: usize,
    fg: u32,
    bg: u32,
) -> ComposedLine {
    let packed = pack_layout(cols, "", prompt, right);
    let limit = packed.center_start + packed.center_w;
    let start = packed.center_start.saturating_add(invert_from);
    let end = start.saturating_add(invert_w).min(limit).min(cols);
    let cells = (start.min(end)..end)
        .map(|col| CellStyle {
            col: col as u32,
            fg: bg,
            bg: fg,
        })
        .collect();
    ComposedLine::full(packed.text, Vec::new(), cells, cols)
}

fn notifications_piece(count: usize) -> Option<Piece> {
    if count == 0 {
        return None;
    }
    let n = count.min(99);
    let digits = n.to_string();
    let text = format!("NTF {digits}");
    let start = display_width("NTF ");
    let end = start + display_width(&digits);
    Some(Piece {
        name: "notifications".into(),
        text,
        invert: vec![(start, end)],
        workspaces: Vec::new(),
    })
}

fn prefix_remainder<'a>(name: &'a str, query: &str) -> Option<&'a str> {
    let mut name_chars = name.char_indices();
    for qc in query.chars() {
        let (_, nc) = name_chars.next()?;
        if !nc.eq_ignore_ascii_case(&qc) {
            return None;
        }
    }
    let idx = name_chars.next().map(|(i, _)| i).unwrap_or(name.len());
    if idx >= name.len() {
        Some("")
    } else {
        Some(&name[idx..])
    }
}

const METER_CELLS: usize = 3;
const SHADES: [char; 4] = ['░', '▒', '▓', '█'];

fn volume_text(volume: Option<&Volume>) -> String {
    match volume {
        Some(vol) if vol.muted => "VOL XX".into(),
        Some(vol) => format!("VOL {}", vol.percent),
        None => "VOL --".into(),
    }
}

fn volume_module(volume: Option<&Volume>, style: MeterStyle) -> String {
    let numbers = volume_text(volume);
    let pct = volume
        .filter(|vol| !vol.muted)
        .map(|vol| vol.percent)
        .unwrap_or(0);
    let meter = shade_bar(pct, METER_CELLS);
    match style {
        MeterStyle::Numbers => numbers,
        MeterStyle::Meter => format!("VOL {meter}"),
    }
}

fn hw_module(
    tag: &str,
    usage: u32,
    temp: Option<u32>,
    style: MeterStyle,
    metric: HwMetric,
) -> String {
    match (style, metric) {
        (MeterStyle::Numbers, HwMetric::Load) => format!("{tag} {usage}"),
        (MeterStyle::Numbers, HwMetric::Temp) => match temp {
            Some(t) => format!("{tag} {t}C"),
            None => format!("{tag} --"),
        },
        (MeterStyle::Meter, HwMetric::Load) => {
            format!("{tag} {}", shade_bar(usage.min(100), METER_CELLS))
        }
        (MeterStyle::Meter, HwMetric::Temp) => {
            format!(
                "{tag} {}",
                shade_bar(temp.unwrap_or(0).min(100), METER_CELLS)
            )
        }
    }
}

fn mem_module(mem: &Memory, style: MeterStyle) -> String {
    match style {
        MeterStyle::Numbers => format!("MEM {}", mem.percent),
        MeterStyle::Meter => format!("MEM {}", shade_bar(mem.percent, METER_CELLS)),
    }
}

fn battery_piece(bat: &Battery, style: MeterStyle, charge_elapsed: Duration) -> Piece {
    match style {
        MeterStyle::Numbers => {
            let plus = if bat.charging { "+" } else { "" };
            plain("battery", &format!("BAT {}{plus}", bat.percent))
        }
        MeterStyle::Meter if bat.percent >= 100 => {
            let text = "BAT MAX";
            let start = display_width("BAT ");
            let end = start + display_width("MAX");
            Piece {
                name: "battery".into(),
                text: text.into(),
                invert: vec![(start, end)],
                workspaces: Vec::new(),
            }
        }
        MeterStyle::Meter => {
            let meter = shade_bar(battery_fill_percent(bat, charge_elapsed), METER_CELLS);
            plain("battery", &format!("BAT {meter}"))
        }
    }
}

pub(crate) const CHARGE_CYCLE: Duration = Duration::from_millis(2500);

fn battery_fill_percent(bat: &Battery, elapsed: Duration) -> u32 {
    charging_meter_percent(bat.percent, battery_is_charging(bat), elapsed)
}

fn battery_is_charging(bat: &Battery) -> bool {
    bat.charging && bat.percent < 100 && !bat.status.eq_ignore_ascii_case("Full")
}

pub(crate) fn charging_meter_percent(actual: u32, charging: bool, elapsed: Duration) -> u32 {
    let actual = actual.min(100);
    if !charging || actual >= 100 {
        return actual;
    }
    let cycle = CHARGE_CYCLE.as_millis();
    let t = (elapsed.as_millis() % cycle) as f32 / cycle as f32;
    let span = 100 - actual;
    actual + (span as f32 * t).round() as u32
}

/// Three fill steps per cell (`▒▓█`); empty stays `░` so it isn't the bar background.
fn shade_bar(percent: u32, cells: usize) -> String {
    if cells == 0 {
        return String::new();
    }
    let steps = cells * 3;
    let filled = (percent.min(100) as usize * steps + 50) / 100;
    let mut out = String::new();
    for i in 0..cells {
        let level = filled.saturating_sub(i * 3).min(3);
        out.push(SHADES[level]);
    }
    out
}

fn volume_overlay_line(cols: usize, volume: Option<&Volume>, fg: u32, bg: u32) -> ComposedLine {
    const GAP: usize = 2;
    const BTN: usize = 3;
    // [ x ]  meter  [ - ]  [ + ]
    let stepper_w = BTN + GAP + GAP + BTN + GAP + BTN;
    let meter_w = if cols > stepper_w {
        (cols / 2).clamp(1, cols - stepper_w)
    } else {
        cols.max(1)
    };
    let show_steppers = cols >= meter_w + stepper_w;
    let cluster_w = if show_steppers {
        meter_w + stepper_w
    } else {
        meter_w
    };
    let start = cols.saturating_sub(cluster_w) / 2;
    let mut text = " ".repeat(start);
    let mut spans = Vec::new();
    let mut cells = Vec::new();
    let mut cursor = start;
    if show_steppers {
        push_meter_button(
            &mut text,
            &mut spans,
            &mut cells,
            &mut cursor,
            "volume-mute",
            " x ",
            fg,
            bg,
        );
        text.push_str(&" ".repeat(GAP));
        cursor += GAP;
    }
    let block = fit_cells(&volume_overlay_block(meter_w, volume), meter_w);
    spans.push(ModuleSpan {
        name: "volume".into(),
        start: cursor as u32,
        end: (cursor + meter_w) as u32,
        workspace_id: None,
    });
    text.push_str(&block);
    cursor += meter_w;
    if show_steppers {
        text.push_str(&" ".repeat(GAP));
        cursor += GAP;
        push_meter_button(
            &mut text,
            &mut spans,
            &mut cells,
            &mut cursor,
            "volume-down",
            " - ",
            fg,
            bg,
        );
        text.push_str(&" ".repeat(GAP));
        cursor += GAP;
        push_meter_button(
            &mut text,
            &mut spans,
            &mut cells,
            &mut cursor,
            "volume-up",
            " + ",
            fg,
            bg,
        );
    }
    ComposedLine::full(fit_cells(&text, cols), spans, cells, cols)
}

fn push_meter_button(
    text: &mut String,
    spans: &mut Vec<ModuleSpan>,
    cells: &mut Vec<CellStyle>,
    cursor: &mut usize,
    name: &str,
    label: &str,
    fg: u32,
    bg: u32,
) {
    let start = *cursor;
    let width = display_width(label);
    let end = start + width;
    text.push_str(label);
    spans.push(ModuleSpan {
        name: name.into(),
        start: start as u32,
        end: end as u32,
        workspace_id: None,
    });
    for col in start..end {
        cells.push(CellStyle {
            col: col as u32,
            fg: bg,
            bg: fg,
        });
    }
    *cursor = end;
}

fn volume_overlay_block(cols: usize, volume: Option<&Volume>) -> String {
    let (pct, muted) = volume
        .map(|vol| (vol.percent.min(100), vol.muted))
        .unwrap_or((0, true));
    let label = if muted {
        "XX".to_string()
    } else {
        format!("{pct}%")
    };
    let prefix = "VOL [";
    let suffix = format!("] {label}");
    let inner = cols.saturating_sub(display_width(prefix) + display_width(&suffix));
    let fill_pct = if muted { 0 } else { pct };
    format!("{prefix}{}{suffix}", shade_bar(fill_pct, inner))
}

fn battery_overlay_line(
    cols: usize,
    bat: Option<&Battery>,
    charge_elapsed: Duration,
) -> ComposedLine {
    let meter_w = (cols / 2).clamp(1, cols.max(1));
    let block = fit_cells(
        &battery_overlay_block(meter_w, bat, charge_elapsed),
        meter_w,
    );
    let extra = match bat {
        Some(bat) => fit_parts(battery_overlay_parts(bat), cols.saturating_sub(meter_w + 2)),
        None => String::new(),
    };
    let extra_w = display_width(&extra);
    let cluster_w = if extra_w == 0 {
        meter_w
    } else {
        meter_w + 2 + extra_w
    };
    let start = cols.saturating_sub(cluster_w) / 2;
    let mut text = " ".repeat(start);
    text.push_str(&block);
    if extra_w > 0 {
        text.push_str("  ");
        text.push_str(&extra);
    }
    ComposedLine::full(
        fit_cells(&text, cols),
        vec![ModuleSpan {
            name: "battery".into(),
            start: start as u32,
            end: (start + cluster_w) as u32,
            workspace_id: None,
        }],
        Vec::new(),
        cols,
    )
}

fn battery_overlay_block(cols: usize, bat: Option<&Battery>, charge_elapsed: Duration) -> String {
    let Some(bat) = bat else {
        return fit_cells("BAT --", cols);
    };
    let prefix = "BAT [";
    let suffix = format!("] {}%", bat.percent.min(100));
    let inner = cols.saturating_sub(display_width(prefix) + display_width(&suffix));
    format!(
        "{prefix}{}{suffix}",
        shade_bar(battery_fill_percent(bat, charge_elapsed), inner)
    )
}

fn battery_overlay_parts(bat: &Battery) -> Vec<String> {
    let mut parts = Vec::new();
    let status = bat.status.to_ascii_lowercase();
    if matches!(
        status.as_str(),
        "charging" | "discharging" | "full" | "not charging"
    ) {
        parts.push(status);
    }
    if let Some(secs) = bat.seconds {
        parts.push(crate::sys::format_secs(secs));
    }
    if let Some(watts) = bat.watts.filter(|w| *w >= 0.05) {
        parts.push(format!("{watts:.1}W"));
    }
    if let Some((now, full)) = bat.energy_wh {
        parts.push(format!("{now:.0}/{full:.0}Wh"));
    }
    parts
}

fn mem_overlay_line(cols: usize, mem: &Memory) -> ComposedLine {
    let meter_w = (cols / 2).clamp(1, cols.max(1));
    let block = fit_cells(&mem_overlay_block(meter_w, mem), meter_w);
    let extra = fit_parts(mem_overlay_parts(mem), cols.saturating_sub(meter_w + 2));
    let extra_w = display_width(&extra);
    let cluster_w = if extra_w == 0 {
        meter_w
    } else {
        meter_w + 2 + extra_w
    };
    let start = cols.saturating_sub(cluster_w) / 2;
    let mut text = " ".repeat(start);
    text.push_str(&block);
    if extra_w > 0 {
        text.push_str("  ");
        text.push_str(&extra);
    }
    ComposedLine::full(
        fit_cells(&text, cols),
        vec![ModuleSpan {
            name: "mem".into(),
            start: start as u32,
            end: (start + cluster_w) as u32,
            workspace_id: None,
        }],
        Vec::new(),
        cols,
    )
}

fn mem_overlay_block(cols: usize, mem: &Memory) -> String {
    if mem.total_kib == 0 {
        return fit_cells("MEM --", cols);
    }
    let prefix = "MEM [";
    let suffix = format!("] {}%", mem.percent.min(100));
    let inner = cols.saturating_sub(display_width(prefix) + display_width(&suffix));
    format!("{prefix}{}{suffix}", shade_bar(mem.percent.min(100), inner))
}

fn hw_overlay_line(
    cols: usize,
    span: &str,
    tag: &str,
    usage: u32,
    temp: Option<u32>,
) -> ComposedLine {
    const GAP: usize = 2;
    let usage = usage.min(100);
    let meter_w = (cols / 4).clamp(1, cols.max(1));
    let (text, cluster_w, start) = if let Some(temp_c) = temp {
        let left_w = meter_w;
        let right_w = meter_w;
        let left = fit_cells(
            &overlay_meter_block(tag, &format!("{usage}%"), usage, left_w),
            left_w,
        );
        let right = fit_cells(
            &overlay_meter_block("TEMP", &format!("{temp_c}C"), temp_c.min(100), right_w),
            right_w,
        );
        let cluster_w = left_w + GAP + right_w;
        let start = cols.saturating_sub(cluster_w) / 2;
        let mut text = " ".repeat(start);
        text.push_str(&left);
        text.push_str(&" ".repeat(GAP));
        text.push_str(&right);
        (text, cluster_w, start)
    } else {
        let block = fit_cells(
            &overlay_meter_block(tag, &format!("{usage}%"), usage, meter_w),
            meter_w,
        );
        let start = cols.saturating_sub(meter_w) / 2;
        let mut text = " ".repeat(start);
        text.push_str(&block);
        (text, meter_w, start)
    };
    ComposedLine::full(
        fit_cells(&text, cols),
        vec![ModuleSpan {
            name: span.into(),
            start: start as u32,
            end: (start + cluster_w) as u32,
            workspace_id: None,
        }],
        Vec::new(),
        cols,
    )
}

fn overlay_meter_block(tag: &str, value: &str, fill: u32, cols: usize) -> String {
    let prefix = format!("{tag} [");
    let suffix = format!("] {value}");
    let inner = cols.saturating_sub(display_width(&prefix) + display_width(&suffix));
    format!("{prefix}{}{suffix}", shade_bar(fill.min(100), inner))
}

fn mem_overlay_parts(mem: &Memory) -> Vec<String> {
    let mut parts = Vec::new();
    if mem.total_kib > 0 {
        parts.push(format!(
            "{}/{}",
            crate::sys::format_kib(mem.used_kib),
            crate::sys::format_kib(mem.total_kib)
        ));
    }
    if let Some(swap) = &mem.swap {
        parts.push(format!(
            "SWAP {} {}%",
            shade_bar(swap.percent, METER_CELLS),
            swap.percent
        ));
        parts.push(format!(
            "{}/{}",
            crate::sys::format_kib(swap.used_kib),
            crate::sys::format_kib(swap.total_kib)
        ));
    }
    parts
}

fn fit_parts(parts: Vec<String>, max_w: usize) -> String {
    let mut parts = parts;
    while !parts.is_empty() {
        let text = parts.join("  ");
        if display_width(&text) <= max_w {
            return text;
        }
        parts.pop();
    }
    String::new()
}

fn plain(name: &str, text: &str) -> Piece {
    Piece {
        name: name.into(),
        text: text.to_string(),
        invert: Vec::new(),
        workspaces: Vec::new(),
    }
}

fn join_text(pieces: &[Piece]) -> String {
    pieces
        .iter()
        .map(|piece| piece.text.as_str())
        .collect::<Vec<_>>()
        .join("  ")
}

fn place_side(
    pieces: &[Piece],
    region_start: usize,
    region_w: usize,
    spans: &mut Vec<ModuleSpan>,
    invert: &mut Vec<(usize, usize)>,
) {
    if region_w == 0 || pieces.is_empty() {
        return;
    }
    let region_end = region_start + region_w;
    let mut cursor = region_start;
    for (i, piece) in pieces.iter().enumerate() {
        if i > 0 {
            cursor += 2;
        }
        let width = display_width(&piece.text);
        let start = cursor;
        let end = cursor + width;
        cursor = end;
        if start >= region_end {
            break;
        }
        let vis_end = end.min(region_end);
        if vis_end > start {
            spans.push(ModuleSpan {
                name: piece.name.clone(),
                start: start as u32,
                end: vis_end as u32,
                workspace_id: None,
            });
        }
        for &(id, rel_start, rel_end) in &piece.workspaces {
            let abs_start = start + rel_start;
            let abs_end = (start + rel_end).min(region_end);
            if abs_end > abs_start && abs_start < region_end {
                spans.push(ModuleSpan {
                    name: piece.name.clone(),
                    start: abs_start as u32,
                    end: abs_end as u32,
                    workspace_id: Some(id),
                });
            }
        }
        for &(rel_start, rel_end) in &piece.invert {
            let abs_start = start + rel_start;
            let abs_end = (start + rel_end).min(region_end);
            if abs_end > abs_start && abs_start < region_end {
                invert.push((abs_start, abs_end));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{compose_line, span_at, workspace_id_at, ModeKind};
    use crate::apps::AppsPrompt;
    use crate::sys::{Battery, Gpu, Memory, Swap, SysSnapshot, Volume};
    use niri_ipc::Workspace;
    use std::collections::HashSet;
    use std::time::Duration;
    use vile_protocol::{display_width, BarConfig};

    fn slice_cols(text: &str, start: u32, end: u32) -> String {
        let mut col = 0u32;
        let mut out = String::new();
        for ch in text.chars() {
            let width = display_width(&ch.to_string()) as u32;
            if col >= end {
                break;
            }
            if col >= start {
                out.push(ch);
            }
            col += width;
        }
        out
    }

    fn ws(id: u64, idx: u8, output: &str, active: bool) -> Workspace {
        Workspace {
            id,
            idx,
            name: None,
            output: Some(output.into()),
            is_urgent: false,
            is_active: active,
            is_focused: active,
            active_window_id: None,
        }
    }

    fn snapshot() -> SysSnapshot {
        SysSnapshot {
            volume: Some(Volume {
                percent: 40,
                muted: false,
            }),
            battery: Some(Battery {
                percent: 87,
                charging: true,
                status: "Charging".into(),
                watts: Some(12.4),
                seconds: Some(2 * 3600 + 14 * 60),
                energy_wh: Some((32.0, 39.0)),
            }),
            memory: Memory {
                used_kib: 3_500_000,
                total_kib: 8_000_000,
                percent: 43,
                swap: Some(Swap {
                    used_kib: 512_000,
                    total_kib: 4_000_000,
                    percent: 12,
                }),
            },
            cpu_usage: 12,
            cpu_temp_c: Some(64),
            gpu: Some(Gpu {
                usage: 40,
                temp_c: Some(71),
            }),
        }
    }

    fn compose(
        bar: &BarConfig,
        cols: usize,
        clock: &str,
        hover_col: Option<u32>,
        mode: Option<&ModeKind>,
        apps: Option<&AppsPrompt>,
        sys: &SysSnapshot,
        workspaces: &[Workspace],
        occupied: &HashSet<u64>,
        output: Option<&str>,
        charge_elapsed: Duration,
    ) -> super::ComposedLine {
        compose_line(
            bar,
            cols,
            clock,
            hover_col,
            mode,
            apps,
            sys,
            workspaces,
            occupied,
            output,
            charge_elapsed,
            None,
            None,
            0,
        )
    }

    #[test]
    fn packs_modules_and_inverts_active_workspace() {
        let mut bar = BarConfig::default();
        bar.modules.left = vec!["workspaces".into(), "clock".into()];
        bar.modules.center.clear();
        bar.modules.right = vec!["cpu".into(), "volume".into(), "battery".into()];
        let composed = compose(
            &bar,
            80,
            "12:00:00",
            None,
            None,
            None,
            &snapshot(),
            &[
                ws(1, 1, "eDP-1", true),
                ws(2, 2, "eDP-1", false),
                ws(3, 1, "HDMI-A-1", true),
            ],
            &HashSet::from([1, 2, 3]),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert_eq!(display_width(&composed.text), 80);
        assert!(composed.text.contains("1 2"));
        assert!(composed.text.contains("12:00:00"));
        assert!(composed.text.contains("CPU 12"));
        assert!(!composed.text.contains("64C"));
        assert!(composed.text.contains("VOL 40"));
        assert!(composed.text.contains("BAT 87+"));
        assert!(!composed.text.contains("HDMI"));
        assert_eq!(composed.cells.len(), 1);
        assert_eq!(composed.cells[0].col, 0);
        assert_eq!(composed.cells[0].fg, bar.bg_argb());
        assert_eq!(composed.cells[0].bg, bar.fg_argb());
        let volume = span_at(
            &composed.spans,
            composed
                .spans
                .iter()
                .find(|s| s.name == "volume")
                .unwrap()
                .start,
        );
        assert_eq!(volume.unwrap().name, "volume");
    }

    #[test]
    fn omits_missing_battery_and_gpu() {
        let mut bar = BarConfig::default();
        bar.modules.left = vec!["clock".into()];
        bar.modules.center.clear();
        bar.modules.right = vec!["gpu".into(), "battery".into(), "mem".into()];
        let mut sys = snapshot();
        sys.battery = None;
        sys.gpu = None;
        let composed = compose(
            &bar,
            40,
            "12:00:00",
            None,
            None,
            None,
            &sys,
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("12:00:00"));
        assert!(composed.text.contains("MEM 43"));
        assert!(!composed.text.contains("BAT"));
        assert!(!composed.text.contains("GPU"));
    }

    #[test]
    fn volume_meter_is_half_width_with_steppers() {
        let bar = BarConfig::default();
        let composed = compose(
            &bar,
            80,
            "12:00:00",
            None,
            Some(&ModeKind::VolumeOverlay),
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert_eq!(display_width(&composed.text), 80);
        assert!(composed.text.contains("VOL ["));
        assert!(composed.text.contains("█") || composed.text.contains("▓"));
        assert!(!composed.text.contains('#'));
        assert!(composed.text.contains("40%"));
        assert!(composed.text.contains(" - "));
        assert!(composed.text.contains(" + "));
        assert!(composed.text.contains(" x "));
        let volume = composed
            .spans
            .iter()
            .find(|span| span.name == "volume")
            .unwrap();
        assert_eq!(volume.end - volume.start, 40);
        let down = composed
            .spans
            .iter()
            .find(|span| span.name == "volume-down")
            .unwrap();
        let up = composed
            .spans
            .iter()
            .find(|span| span.name == "volume-up")
            .unwrap();
        let mute = composed
            .spans
            .iter()
            .find(|span| span.name == "volume-mute")
            .unwrap();
        assert_eq!(mute.end + 2, volume.start);
        assert_eq!(volume.end + 2, down.start);
        assert_eq!(down.end + 2, up.start);
        assert_eq!(down.end - down.start, 3);
        assert_eq!(up.end - up.start, 3);
        assert_eq!(mute.end - mute.start, 3);
        let cluster_start = mute.start;
        let cluster_end = up.end;
        let left_pad = cluster_start;
        let right_pad = 80 - cluster_end;
        assert!((left_pad as i32 - right_pad as i32).abs() <= 1);
        assert_eq!(slice_cols(&composed.text, down.start, down.end), " - ");
        assert_eq!(slice_cols(&composed.text, up.start, up.end), " + ");
        assert_eq!(slice_cols(&composed.text, mute.start, mute.end), " x ");
        assert_eq!(composed.cells.len(), 9);
        let mut muted = snapshot();
        muted.volume.as_mut().unwrap().muted = true;
        let composed = compose(
            &bar,
            80,
            "12:00:00",
            None,
            Some(&ModeKind::VolumeOverlay),
            None,
            &muted,
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("] XX"));
        assert!(composed.text.contains('░'));
        assert!(!composed.text.contains("MUTE"));
    }

    #[test]
    fn compact_volume_when_not_in_meter_mode() {
        let mut bar = BarConfig::default();
        bar.modules.left = vec!["volume".into()];
        bar.modules.center.clear();
        bar.modules.right.clear();
        let composed = compose(
            &bar,
            20,
            "12:00:00",
            None,
            None,
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("VOL 40"));
        assert!(!composed.text.contains(" + "));
        assert!(!composed.text.contains("VOL ["));
        let mut muted = snapshot();
        muted.volume.as_mut().unwrap().muted = true;
        let composed = compose(
            &bar,
            20,
            "12:00:00",
            None,
            None,
            None,
            &muted,
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("VOL XX"));
        assert!(!composed.text.contains("MUTE"));
    }

    #[test]
    fn hides_empty_inactive_workspaces() {
        let mut bar = BarConfig::default();
        bar.modules.left = vec!["workspaces".into()];
        bar.modules.center.clear();
        bar.modules.right.clear();
        let composed = compose(
            &bar,
            20,
            "12:00:00",
            None,
            None,
            None,
            &snapshot(),
            &[ws(1, 1, "eDP-1", true), ws(2, 2, "eDP-1", false)],
            &HashSet::from([1]),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.starts_with("1 "));
        assert!(!composed.text.contains("2"));
        assert_eq!(workspace_id_at(&composed.spans, 0), Some(1));
    }

    #[test]
    fn apps_prompt_empty_command_and_ghost() {
        use crate::apps::{AppsPrompt, DesktopApp};
        let bar = BarConfig::default();
        let firefox = DesktopApp {
            id: "firefox".into(),
            name: "Firefox".into(),
            generic_name: None,
            exec: "firefox".into(),
            keywords: Vec::new(),
            terminal: false,
        };
        let empty = AppsPrompt::from_entries("", 0, vec![firefox.clone()]);
        let composed = compose(
            &bar,
            20,
            "12:00:00",
            None,
            None,
            Some(&empty),
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("search apps..."));
        assert!(!composed.text.contains("Firefox"));
        assert!(!composed.text.starts_with("> "));
        assert!(!composed.cells.is_empty());
        let start = composed.text.find("> search apps...").expect("placeholder");
        let end = 20 - (start + display_width("> search apps..."));
        assert!((start as i32 - end as i32).abs() <= 1);
        assert_eq!(composed.cells[0].col, (start + display_width("> ")) as u32);

        let cmd = AppsPrompt::from_entries(">>cmatrix", 0, vec![firefox.clone()]);
        let composed = compose(
            &bar,
            20,
            "12:00:00",
            None,
            None,
            Some(&cmd),
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains(">>cmatrix"));
        assert!(!composed.text.starts_with(">>cmatrix"));
        assert!(composed.cells.is_empty());
        let start = composed.text.find(">>cmatrix").expect("command");
        let end = 20 - (start + display_width(">>cmatrix"));
        assert!((start as i32 - end as i32).abs() <= 1);

        let typed = AppsPrompt::from_entries("Fire", 0, vec![firefox.clone()]);
        let composed = compose(
            &bar,
            24,
            "12:00:00",
            None,
            None,
            Some(&typed),
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("> Firefox"));
        assert!(!composed.text.starts_with("> Firefox"));
        let start = composed.text.find("> Firefox").expect("ghost prompt");
        let end = 24 - (start + display_width("> Firefox"));
        assert!((start as i32 - end as i32).abs() <= 1);
        assert!(!composed.cells.is_empty());
        assert_eq!(
            composed.cells[0].col,
            (start + display_width("> Fire")) as u32
        );

        let typed = AppsPrompt::from_entries("fox", 0, vec![firefox]);
        let composed = compose(
            &bar,
            24,
            "12:00:00",
            None,
            None,
            Some(&typed),
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        let start = composed.text.find("> fox").expect("query");
        assert!(!composed.text.starts_with("> fox"));
        assert!(composed.text.ends_with("Firefox") || composed.text.trim_end().ends_with("Firefox"));
        let query_end = start + display_width("> fox");
        let match_at = composed.text.rfind("Firefox").expect("match name");
        assert!(match_at >= query_end);
    }

    #[test]
    fn shade_bar_uses_block_shades() {
        use super::shade_bar;
        assert_eq!(shade_bar(0, 3), "░░░");
        assert_eq!(shade_bar(11, 3), "▒░░");
        assert_eq!(shade_bar(22, 3), "▓░░");
        assert_eq!(shade_bar(33, 3), "█░░");
        assert_eq!(shade_bar(50, 3), "█▓░");
        assert_eq!(shade_bar(100, 3), "███");
        assert_eq!(display_width(&shade_bar(40, 3)), 3);
    }

    #[test]
    fn compact_volume_and_battery_honor_meter_style() {
        use vile_protocol::MeterStyle;
        let mut bar = BarConfig::default();
        bar.modules.left = vec![
            "volume".into(),
            "battery".into(),
            "mem".into(),
            "cpu".into(),
        ];
        bar.modules.center.clear();
        bar.modules.right.clear();
        bar.volume_style = MeterStyle::Meter;
        bar.battery_style = MeterStyle::Meter;
        bar.mem_style = MeterStyle::Meter;
        bar.cpu_style = MeterStyle::Meter;
        bar.cpu_metric = vile_protocol::HwMetric::Temp;
        let composed = compose(
            &bar,
            40,
            "12:00:00",
            None,
            None,
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("VOL █▒░"));
        assert!(composed.text.contains("BAT ██▓"));
        assert!(composed.text.contains("MEM █▒░"));
        assert!(composed.text.contains("CPU ██░"));
        assert!(!composed.text.contains("VOL 40"));
        assert!(!composed.text.contains("87"));
        assert!(!composed.text.contains("MEM 43"));
        assert!(!composed.text.contains("64C"));
    }

    #[test]
    fn full_battery_meter_inverts_max() {
        use vile_protocol::MeterStyle;
        let mut bar = BarConfig::default();
        bar.modules.left = vec!["battery".into()];
        bar.modules.center.clear();
        bar.modules.right.clear();
        bar.battery_style = MeterStyle::Meter;
        let mut sys = snapshot();
        sys.battery.as_mut().unwrap().percent = 100;
        sys.battery.as_mut().unwrap().status = "Full".into();
        let composed = compose(
            &bar,
            20,
            "12:00:00",
            None,
            None,
            None,
            &sys,
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("BAT MAX"));
        assert!(!composed.text.contains('█'));
        let battery = composed
            .spans
            .iter()
            .find(|span| span.name == "battery")
            .unwrap();
        assert_eq!(battery.end - battery.start, 7);
        let cols: Vec<u32> = composed.cells.iter().map(|cell| cell.col).collect();
        assert_eq!(
            cols,
            vec![battery.start + 4, battery.start + 5, battery.start + 6]
        );
        assert_eq!(composed.cells[0].fg, bar.bg_argb());
        assert_eq!(composed.cells[0].bg, bar.fg_argb());
    }

    #[test]
    fn charging_meter_sweeps_from_level_to_full() {
        use super::{charging_meter_percent, shade_bar, CHARGE_CYCLE};
        assert_eq!(
            charging_meter_percent(40, false, Duration::from_millis(1000)),
            40
        );
        assert_eq!(
            charging_meter_percent(100, true, Duration::from_millis(1000)),
            100
        );
        assert_eq!(charging_meter_percent(40, true, Duration::ZERO), 40);
        assert_eq!(charging_meter_percent(40, true, CHARGE_CYCLE / 2), 70);
        assert_eq!(charging_meter_percent(40, true, CHARGE_CYCLE), 40);
        assert_ne!(shade_bar(40, 3), shade_bar(70, 3));
        assert_eq!(shade_bar(100, 3), "███");
    }

    #[test]
    fn battery_overlay_is_centered_with_details() {
        let bar = BarConfig::default();
        let composed = compose(
            &bar,
            80,
            "12:00:00",
            None,
            Some(&ModeKind::BatteryOverlay),
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert_eq!(display_width(&composed.text), 80);
        assert!(composed.text.contains("BAT ["));
        assert!(composed.text.contains("87%"));
        assert!(composed.text.contains("charging"));
        assert!(composed.text.contains("2h14m"));
        assert!(composed.text.contains("12.4W"));
        assert!(composed.text.contains("32/39Wh"));
        assert!(composed.text.contains("█") || composed.text.contains("▓"));
        let battery = composed
            .spans
            .iter()
            .find(|span| span.name == "battery")
            .unwrap();
        let left = battery.start;
        let right = 80 - battery.end;
        assert!((left as i32 - right as i32).abs() <= 1);
        let mut empty = snapshot();
        empty.battery = None;
        let composed = compose(
            &bar,
            40,
            "12:00:00",
            None,
            Some(&ModeKind::BatteryOverlay),
            None,
            &empty,
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("BAT --"));
    }

    #[test]
    fn mem_overlay_shows_swap_when_present() {
        let bar = BarConfig::default();
        let composed = compose(
            &bar,
            80,
            "12:00:00",
            None,
            Some(&ModeKind::MemOverlay),
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert_eq!(display_width(&composed.text), 80);
        assert!(composed.text.contains("MEM ["));
        assert!(composed.text.contains("43%"));
        assert!(composed.text.contains("3.3G/7.6G"));
        assert!(composed.text.contains("SWAP"));
        assert!(composed.text.contains("12%"));
        assert!(composed.text.contains("500M/3.8G"));
        let mem = composed
            .spans
            .iter()
            .find(|span| span.name == "mem")
            .unwrap();
        let left = mem.start;
        let right = 80 - mem.end;
        assert!((left as i32 - right as i32).abs() <= 1);

        let mut noswap = snapshot();
        noswap.memory.swap = None;
        let composed = compose(
            &bar,
            80,
            "12:00:00",
            None,
            Some(&ModeKind::MemOverlay),
            None,
            &noswap,
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("MEM ["));
        assert!(!composed.text.contains("SWAP"));
    }

    #[test]
    fn cpu_and_gpu_overlays_show_load_and_temp_meters() {
        let bar = BarConfig::default();
        let composed = compose(
            &bar,
            80,
            "12:00:00",
            None,
            Some(&ModeKind::CpuOverlay),
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert_eq!(display_width(&composed.text), 80);
        assert!(composed.text.contains("CPU ["));
        assert!(composed.text.contains("12%"));
        assert!(composed.text.contains("TEMP ["));
        assert!(composed.text.contains("64C"));
        let cpu = composed
            .spans
            .iter()
            .find(|span| span.name == "cpu")
            .unwrap();
        assert_eq!(cpu.end - cpu.start, 42);
        let left = cpu.start;
        let right = 80 - cpu.end;
        assert!((left as i32 - right as i32).abs() <= 1);

        let composed = compose(
            &bar,
            80,
            "12:00:00",
            None,
            Some(&ModeKind::GpuOverlay),
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("GPU ["));
        assert!(composed.text.contains("40%"));
        assert!(composed.text.contains("TEMP ["));
        assert!(composed.text.contains("71C"));

        let mut notemp = snapshot();
        notemp.cpu_temp_c = None;
        let composed = compose(
            &bar,
            80,
            "12:00:00",
            None,
            Some(&ModeKind::CpuOverlay),
            None,
            &notemp,
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
        );
        assert!(composed.text.contains("CPU ["));
        assert!(composed.text.contains("12%"));
        assert!(!composed.text.contains("TEMP ["));
    }

    #[test]
    fn overlay_timeout_refresh_does_not_count_as_mode_change() {
        use super::should_anim_mode;
        assert!(!should_anim_mode(
            Some(&ModeKind::VolumeOverlay),
            Some(&ModeKind::VolumeOverlay)
        ));
        assert!(should_anim_mode(None, Some(&ModeKind::VolumeOverlay)));
        assert!(should_anim_mode(Some(&ModeKind::VolumeOverlay), None));
        assert!(!should_anim_mode(
            Some(&ModeKind::VolumeOverlay),
            Some(&ModeKind::BatteryOverlay)
        ));
    }

    #[test]
    fn inbox_overlay_is_centered() {
        let bar = BarConfig::default();
        let composed = compose_line(
            &bar,
            40,
            "12:00:00",
            None,
            None,
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
            Some("shutdown  1/3"),
            None,
            0,
        );
        assert_eq!(display_width(&composed.text), 40);
        assert_eq!(composed.text.trim(), "shutdown  1/3");
        assert!(!composed.text.starts_with("shutdown"));
        let start = composed.text.find("shutdown").expect("label");
        let end = 40 - (start + display_width("shutdown  1/3"));
        assert!((start as i32 - end as i32).abs() <= 1);
    }

    #[test]
    fn toast_replaces_right_side_only() {
        use vile_protocol::NotificationSide;
        let mut bar = BarConfig::default();
        bar.modules.left = vec!["clock".into()];
        bar.modules.center.clear();
        bar.modules.right = vec!["volume".into()];
        let composed = compose_line(
            &bar,
            40,
            "12:00:00",
            None,
            None,
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
            None,
            Some((NotificationSide::Right, "term  hello")),
            0,
        );
        assert!(composed.text.contains("12:00:00"));
        assert!(composed.text.contains("hello"));
        assert!(!composed.text.contains("VOL"));
        assert!(composed.spans.iter().all(|s| s.name != "volume"));
    }

    #[test]
    fn notifications_module_omits_zero_and_inverts_digits() {
        let mut bar = BarConfig::default();
        assert!(!bar.modules.right.contains(&"notifications".into()));
        bar.modules.left.clear();
        bar.modules.center.clear();
        bar.modules.right = vec!["notifications".into()];

        let empty = compose_line(
            &bar,
            20,
            "12:00:00",
            None,
            None,
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
            None,
            None,
            0,
        );
        assert!(!empty.text.contains("NTF"));
        assert!(empty.spans.iter().all(|span| span.name != "notifications"));
        assert!(empty.cells.is_empty());

        let one = compose_line(
            &bar,
            20,
            "12:00:00",
            None,
            None,
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
            None,
            None,
            1,
        );
        assert!(one.text.contains("NTF 1"));
        assert!(!one.text.contains("NTF["));
        let span = one
            .spans
            .iter()
            .find(|span| span.name == "notifications")
            .unwrap();
        assert_eq!(one.cells.len(), 1);
        assert_eq!(one.cells[0].col, span.start + display_width("NTF ") as u32);

        let ten = compose_line(
            &bar,
            20,
            "12:00:00",
            None,
            None,
            None,
            &snapshot(),
            &[],
            &HashSet::new(),
            Some("eDP-1"),
            Duration::ZERO,
            None,
            None,
            10,
        );
        assert!(ten.text.contains("NTF 10"));
        let span = ten
            .spans
            .iter()
            .find(|span| span.name == "notifications")
            .unwrap();
        let digit_start = span.start + display_width("NTF ") as u32;
        assert_eq!(ten.cells.len(), 2);
        assert!(ten.cells.iter().all(|cell| cell.col >= digit_start));
        assert_eq!(
            ten.cells.iter().map(|cell| cell.col).collect::<Vec<_>>(),
            vec![digit_start, digit_start + 1]
        );
    }
}
