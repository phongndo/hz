use std::{io, time::Duration};

use crossterm::event::{Event, KeyCode, KeyModifiers};
use hz_core::HzResult;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::Alignment,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{event_reader::TerminalEventReader, run::TerminalCleanup, runtime};

const MAIN_EVENT_POLL: Duration = Duration::from_millis(50);

pub fn run_main() -> HzResult<()> {
    runtime::block_on(run_main_async())?
}

async fn run_main_async() -> HzResult<()> {
    let mut cleanup = TerminalCleanup::install()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut dirty = true;

    {
        let mut events = TerminalEventReader::start("hz-main-events")?;
        loop {
            if dirty {
                terminal.draw(draw_main)?;
                dirty = false;
            }

            let Some(event) = events.read_timeout(MAIN_EVENT_POLL).await? else {
                continue;
            };

            match event {
                Event::Key(key)
                    if key.code == KeyCode::Char('q')
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)) =>
                {
                    break;
                }
                Event::Resize(_, _) => dirty = true,
                _ => {}
            }
        }
    }

    cleanup.cleanup()
}

fn draw_main(frame: &mut ratatui::Frame<'_>) {
    let area = frame.area();
    if area.height == 0 || area.width == 0 {
        return;
    }

    let content = Paragraph::new(vec![
        Line::from(Span::styled(
            "Hello word",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("hz workspace dashboard foundation"),
    ])
    .alignment(Alignment::Center);

    frame.render_widget(content, area);
}
