use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Gauge, Paragraph};
use ratatui::Frame;
use ratatui_image::{FilterType, Resize, StatefulImage};

use crate::recipe::ExtractorKind;
use crate::LabIntent;

use super::app::{App, Overlay, Screen};
use super::hex;
use super::knobs::{self, KnobKind};

const ACCENT: Color = Color::Indexed(4);
const MUTED: Color = Color::Indexed(8);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(area);
    if app.screen == Screen::Walls {
        let body = chunks[1];
        app.wall_cols = (body.width / 28).clamp(2, 6) as usize;
        app.wall_rows = (body.height / 12).max(1) as usize;
        app.refresh_wall_status();
    }
    draw_header(frame, chunks[0], app);
    match app.screen {
        Screen::Walls => draw_walls(frame, chunks[1], app),
        Screen::Recipes => draw_recipes(frame, chunks[1], app),
        Screen::Editor => draw_editor(frame, chunks[1], app),
    }
    draw_footer(frame, chunks[2], app);
    match &app.overlay {
        Overlay::SaveName { buf, .. } => draw_save(frame, area, buf),
        Overlay::ConfirmDelete { name } => draw_confirm_delete(frame, area, name),
        Overlay::None => {}
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let img = app
        .image
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("no image");
    let dirty = if app.dirty_recipe { "*" } else { "" };
    let ext = match app.recipe.extractor {
        ExtractorKind::Cut => "cut",
        ExtractorKind::Hue => "hue",
        ExtractorKind::Kmeans => "kmeans",
    };
    let title = format!(
        " vilecolor / lab   {}{dirty}   {img}   [{ext}]   {}",
        app.recipe.name, app.status
    );
    frame.render_widget(
        Paragraph::new(title).style(Style::default().fg(ACCENT)),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let text = match app.screen {
        Screen::Walls => " arrows move   pgup/pgdn page   enter select   q quit",
        Screen::Recipes if app.intent == LabIntent::PickApply => {
            " arrows move   enter apply   e colorlab   d delete   o walls   q quit"
        }
        Screen::Recipes => " arrows move   enter open   d delete   o walls   q quit",
        Screen::Editor => {
            " arrows focus   shift+arrows value   1/2/3 extractor   s save   o walls   l load   a apply   q quit"
        }
    };
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(MUTED)), area);
}

fn draw_walls(frame: &mut Frame, area: Rect, app: &mut App) {
    let cols = (area.width / 28).clamp(2, 6);
    let rows = (area.height / 12).max(1);
    let cell_w = area.width / cols;
    let cell_h = area.height / rows;
    let extra_w = area.width - cell_w * cols;
    let extra_h = area.height - cell_h * rows;
    let start = super::walls::page_start(app.wall_sel, app.wall_cols, app.wall_rows);
    for row in 0..rows {
        for col in 0..cols {
            let idx = start + row as usize * cols as usize + col as usize;
            let Some(path) = app.walls.get(idx) else {
                continue;
            };
            let x = area.x + col * cell_w + extra_w.min(col);
            let y = area.y + row * cell_h + extra_h.min(row);
            let cell = Rect {
                x,
                y,
                width: cell_w + u16::from(col < extra_w),
                height: cell_h + u16::from(row < extra_h),
            };
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("?");
            let selected = idx == app.wall_sel;
            let title_w = cell.width.saturating_sub(2) as usize;
            let title = if selected {
                format!("▸ {}", truncate(name, title_w.saturating_sub(2)))
            } else {
                truncate(name, title_w)
            };
            let title_style = if selected {
                Style::default()
                    .fg(Color::Indexed(0))
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED)
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(if selected {
                    BorderType::Thick
                } else {
                    BorderType::Plain
                })
                .title(Span::styled(title, title_style))
                .border_style(if selected {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(MUTED)
                });
            let inner = block.inner(cell);
            frame.render_widget(block, cell);
            if let Some(proto) = app.thumbs.get_mut(path) {
                let widget = StatefulImage::new().resize(Resize::Scale(Some(FilterType::Triangle)));
                frame.render_stateful_widget(widget, inner, proto);
            }
        }
    }
}

fn draw_recipes(frame: &mut Frame, area: Rect, app: &App) {
    if app.recipe_rows.is_empty() {
        frame.render_widget(
            Paragraph::new("No recipes in ~/.config/vile/colors/palettes/")
                .style(Style::default().fg(MUTED)),
            area,
        );
        return;
    }
    let row_h = 3u16;
    let visible = (area.height / row_h).max(1) as usize;
    let start = app
        .recipe_sel
        .saturating_sub(visible.saturating_sub(1) / 2)
        .min(app.recipe_rows.len().saturating_sub(visible));
    for (i, row) in app.recipe_rows.iter().enumerate().skip(start).take(visible) {
        let y = area.y + ((i - start) as u16) * row_h;
        let rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height: row_h,
        };
        let selected = i == app.recipe_sel;
        let title_style = if selected {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let extra = match &row.palette {
            Ok(_) => String::new(),
            Err(err) => format!("  {err}"),
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {}", row.name), title_style),
                Span::styled(extra, Style::default().fg(Color::Indexed(1))),
            ])),
            Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: 1,
            },
        );
        if let Ok(pal) = &row.palette {
            let strip = Rect {
                x: rect.x + 1,
                y: rect.y + 1,
                width: rect.width.saturating_sub(2),
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(palette_line(&pal.colors, strip.width)),
                strip,
            );
        }
    }
}

fn draw_editor(frame: &mut Frame, area: Rect, app: &mut App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
        .split(area);
    draw_preview_column(frame, cols[0], app);
    draw_knob_column(frame, cols[1], app);
}

fn draw_preview_column(frame: &mut Frame, area: Rect, app: &mut App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(7),
        ])
        .split(area);
    let img_block = Block::default()
        .borders(Borders::ALL)
        .title(" image ")
        .border_style(Style::default().fg(MUTED));
    let inner = img_block.inner(rows[0]);
    frame.render_widget(img_block, rows[0]);
    if let Some(proto) = app.preview.as_mut() {
        let widget = StatefulImage::new().resize(Resize::Scale(Some(FilterType::Triangle)));
        frame.render_stateful_widget(widget, inner, proto);
    }

    let pal = app.palettes[app.active_idx()].as_ref();
    draw_palette_block(frame, rows[1], " palette 0–15 ", pal.map(|p| &p.colors[..]));
    draw_compare(frame, rows[2], app);
    draw_term(frame, rows[3], pal);
}

fn draw_palette_block(frame: &mut Frame, area: Rect, title: &str, colors: Option<&[String]>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(MUTED));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(colors) = colors {
        let top = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        let bot = Rect {
            x: inner.x,
            y: inner.y.saturating_add(1),
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(palette_line(&colors[..8.min(colors.len())], top.width)),
            top,
        );
        if colors.len() > 8 {
            frame.render_widget(Paragraph::new(palette_line(&colors[8..], bot.width)), bot);
        }
        if inner.height > 2 {
            let hexes: String = colors
                .iter()
                .enumerate()
                .take(8)
                .map(|(i, c)| format!("{i:02}{c} "))
                .collect();
            frame.render_widget(
                Paragraph::new(hexes).style(Style::default().fg(MUTED)),
                Rect {
                    x: inner.x,
                    y: inner.y + 2,
                    width: inner.width,
                    height: inner.height.saturating_sub(2),
                },
            );
        }
    }
}

fn draw_compare(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" A cut  B hue  C kmeans ")
        .border_style(Style::default().fg(MUTED));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    for (i, label) in ["A", "B", "C"].into_iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let row = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };
        let mut spans = vec![Span::styled(
            format!("{label} "),
            if i == app.active_idx() {
                Style::default().fg(ACCENT)
            } else {
                Style::default().fg(MUTED)
            },
        )];
        if let Some(pal) = &app.palettes[i] {
            spans.extend(palette_spans(&pal.colors, row.width.saturating_sub(2)));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), row);
    }
}

fn draw_term(frame: &mut Frame, area: Rect, pal: Option<&crate::Palette>) {
    let Some(pal) = pal else {
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title(" terminal ")
                .border_style(Style::default().fg(MUTED)),
            area,
        );
        return;
    };
    let bg = hex::rgb(&pal.background);
    let fg = hex::rgb(&pal.foreground);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" terminal ")
        .style(Style::default().bg(bg).fg(fg))
        .border_style(Style::default().fg(MUTED).bg(bg));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = vec![
        Line::from(vec![
            Span::styled("➜ ", Style::default().fg(hex::rgb(&pal.colors[2]))),
            Span::styled("~ ", Style::default().fg(hex::rgb(&pal.colors[4]))),
            Span::raw(format!(
                "vile --colors {} --img wall.png",
                pal.wallpaper.rsplit('/').next().unwrap_or("wall")
            )),
        ]),
        Line::from(vec![
            Span::styled("success  ", Style::default().fg(hex::rgb(&pal.colors[2]))),
            Span::styled("error  ", Style::default().fg(hex::rgb(&pal.colors[1]))),
            Span::styled("warning  ", Style::default().fg(hex::rgb(&pal.colors[3]))),
            Span::styled("link", Style::default().fg(hex::rgb(&pal.colors[4]))),
        ]),
        Line::from("The quick brown fox jumps over the lazy dog."),
        palette_line(&pal.colors, inner.width),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_knob_column(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Min(6),
        ])
        .split(area);
    let panes = [
        knobs::KnobPane::Sample,
        knobs::KnobPane::Extract,
        knobs::KnobPane::Map,
    ];
    let tabs: Vec<Span> = panes
        .iter()
        .flat_map(|pane| {
            let style = if *pane == app.pane {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED)
            };
            [
                Span::styled(format!(" {} ", pane.label()), style),
                Span::raw(" "),
            ]
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(tabs)), rows[0]);
    draw_knobs(frame, rows[1], app);
    draw_ranked(frame, rows[2], app);
}

fn draw_knobs(frame: &mut Frame, area: Rect, app: &App) {
    let knobs = app.knobs();
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let row_h = 3u16;
    for (i, knob) in knobs.iter().enumerate() {
        let col = i % 2;
        let row = (i / 2) as u16;
        let y = cols[col].y + row * row_h;
        if y + 2 >= cols[col].y + cols[col].height {
            continue;
        }
        let cell = Rect {
            x: cols[col].x,
            y,
            width: cols[col].width,
            height: row_h.min(cols[col].y + cols[col].height - y),
        };
        let focused = i == app.knob_index;
        let value = knobs::format_value(&app.recipe, *knob);
        let label_style = if focused {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {}", knob.label), label_style),
                Span::raw("  "),
                Span::styled(value, Style::default().fg(Color::Indexed(3))),
            ])),
            Rect {
                x: cell.x,
                y: cell.y,
                width: cell.width,
                height: 1,
            },
        );
        let ratio = knobs::ratio(&app.recipe, *knob);
        let gauge_style = if focused {
            Style::default().fg(ACCENT).bg(Color::Indexed(0))
        } else {
            Style::default().fg(MUTED).bg(Color::Indexed(0))
        };
        let label = match knob.kind {
            KnobKind::Enum(_) => knobs::format_value(&app.recipe, *knob),
            _ => String::new(),
        };
        frame.render_widget(
            Gauge::default()
                .gauge_style(gauge_style)
                .ratio(ratio)
                .label(label),
            Rect {
                x: cell.x + 1,
                y: cell.y + 1,
                width: cell.width.saturating_sub(2),
                height: 1,
            },
        );
    }
}

fn draw_ranked(frame: &mut Frame, area: Rect, app: &App) {
    let report = &app.reports[app.active_idx()];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " ranked {} / {} raw ",
            report.ranked.len(),
            report.all.len()
        ))
        .border_style(Style::default().fg(MUTED));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = Vec::new();
    for c in report.ranked.iter().take(inner.height as usize) {
        let color = hex::rgb(&c.hex);
        let bar_w = ((c.share * 12.0).round() as usize).clamp(1, 12);
        let fill = Style::default().fg(color).bg(color);
        lines.push(Line::from(vec![
            Span::styled("██", fill),
            Span::raw(" "),
            Span::styled("█".repeat(bar_w), fill),
            Span::styled("░".repeat(12 - bar_w), Style::default().fg(MUTED)),
            Span::raw(format!("  {}  {:.1}%", c.hex, c.share * 100.0)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_confirm_delete(frame: &mut Frame, area: Rect, name: &str) {
    let width = 48.min(area.width.saturating_sub(4)).max(20);
    let height = 5u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" delete recipe · y confirm · n cancel ")
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(Color::Indexed(0)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(format!("delete '{name}'?  y / n")).style(Style::default()),
        inner,
    );
}

fn draw_save(frame: &mut Frame, area: Rect, buf: &str) {
    let width = 48.min(area.width.saturating_sub(4)).max(20);
    let height = 5u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" recipe name · enter save · esc cancel ")
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(Color::Indexed(0)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(format!("{buf}█")).style(Style::default()),
        inner,
    );
}

fn palette_line(colors: &[String], width: u16) -> Line<'static> {
    Line::from(palette_spans(colors, width))
}

fn palette_spans(colors: &[String], width: u16) -> Vec<Span<'static>> {
    if colors.is_empty() || width == 0 {
        return Vec::new();
    }
    let cell = (width as usize / colors.len()).max(1);
    colors
        .iter()
        .map(|hex| {
            let fg = hex::contrast_fg(hex);
            Span::styled(" ".repeat(cell), Style::default().bg(hex::rgb(hex)).fg(fg))
        })
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let take = max.saturating_sub(1);
        format!("{}…", s.chars().take(take).collect::<String>())
    }
}
