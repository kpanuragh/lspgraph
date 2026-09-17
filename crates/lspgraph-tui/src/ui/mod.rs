pub mod graph;

use crate::app::{App, Screen};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &App) {
    match app.screen() {
        Screen::Graph => graph::draw_graph(f, f.area(), app),
        _ => {}
    }
}
