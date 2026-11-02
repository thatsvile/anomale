mod app;
mod hex;
mod knobs;
mod recipes;
mod ui;
mod walls;

use std::io::{stdout, Stdout};

use anyhow::{Context, Result};
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use ratatui_image::picker::{Picker, ProtocolType};

use crate::LabOpts;

pub use app::PickResult;

pub fn run_lab(opts: LabOpts) -> Result<Option<PickResult>> {
    let picker = match Picker::from_query_stdio() {
        Ok(picker) => picker,
        Err(_) => {
            let mut picker = Picker::from_fontsize((8, 16));
            picker.set_protocol_type(ProtocolType::Halfblocks);
            picker
        }
    };
    let mut app = app::App::new(opts, picker)?;
    if app.quit {
        return Ok(app.pick.take());
    }
    let mut terminal = setup()?;
    let result = run_loop(&mut terminal, &mut app);
    restore()?;
    result?;
    Ok(app.pick.take())
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut app::App) -> Result<()> {
    while !app.quit {
        app.flush_debounce();
        terminal.draw(|frame| ui::draw(frame, app))?;
        app.pump_thumbs();
        if event::poll(app.poll_timeout()).context("poll input")? {
            match event::read().context("read input")? {
                Event::Key(key) => app.on_key(key),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}

fn setup() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("raw mode")?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen).context("enter alternate screen")?;
    let backend = CrosstermBackend::new(out);
    Terminal::new(backend).context("terminal")
}

fn restore() -> Result<()> {
    disable_raw_mode().ok();
    execute!(stdout(), LeaveAlternateScreen).ok();
    Ok(())
}
