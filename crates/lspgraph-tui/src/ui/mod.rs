pub mod graph;
pub mod overlay;
pub mod search;

use crate::app::{App, Screen};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &App) {
    match app.screen() {
        Screen::Graph => graph::draw_graph(f, f.area(), app),
        Screen::Search => search::draw_search(f, f.area(), app),
        Screen::Starting => {
            use ratatui::widgets::Paragraph;
            f.render_widget(
                Paragraph::new(format!("starting… {}", app.progress())),
                f.area(),
            );
        }
    }
    if let Some(e) = app.error() {
        overlay::draw_error(f, f.area(), e);
    }
}
