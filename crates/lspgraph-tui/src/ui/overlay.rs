//! Errors render over whatever is behind them. Never as an empty state.

use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

pub fn draw_error(f: &mut Frame, area: Rect, msg: &str) {
    let w = area.width.saturating_sub(8).min(70).max(20);
    let h = 7u16.min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(format!("{msg}\n\nesc dismiss · r exit to relaunch · q quit"))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title("error")),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rendered(msg: &str) -> String {
        let mut t = Terminal::new(TestBackend::new(60, 10)).unwrap();
        t.draw(|f| draw_error(f, f.area(), msg)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn shows_the_message() {
        assert!(rendered("server not ready after 300s").contains("not ready"));
    }

    #[test]
    fn mentions_how_to_dismiss() {
        let out = rendered("boom");
        assert!(out.to_lowercase().contains("esc"), "must say how to dismiss:\n{out}");
    }

    #[test]
    fn the_restart_hint_does_not_promise_a_restart() {
        // `r` does not restart anything: the worker answers every Restart
        // with a fresh Fatal and shuts down. The hint must say what actually
        // happens (exit to relaunch), never something that reads as an
        // in-session restart.
        let out = rendered("boom");
        assert!(
            !out.contains("restart server"),
            "must not promise a restart the code cannot deliver:\n{out}"
        );
        assert!(
            out.to_lowercase().contains("relaunch"),
            "must say what actually happens instead:\n{out}"
        );
    }
}
