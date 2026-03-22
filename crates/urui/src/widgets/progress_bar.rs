use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::theme::Theme;

/// A mini progress bar widget showing completed/total with a visual bar.
///
/// Renders a filled/unfilled bar followed by a "N/M" count label. The filled
/// portion uses `theme.success` and the unfilled portion uses `theme.neutral`.
/// The count text uses the default foreground color.
pub struct MiniProgressBar {
    pub completed: u32,
    pub total: u32,
}

impl MiniProgressBar {
    /// Render just the progress bar (no label) into the given area.
    ///
    /// The bar fills the entire area width using filled '█' and unfilled '░'
    /// characters. The filled portion uses `theme.accent` and the unfilled
    /// portion uses `theme.neutral`.
    pub fn render_bar(
        &self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        bg: ratatui::style::Color,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let bar_width = area.width;
        let fraction = if self.total == 0 {
            0.0
        } else {
            self.completed as f64 / self.total as f64
        };
        let filled = ((fraction * bar_width as f64).round() as u16).min(bar_width);
        let unfilled = bar_width - filled;

        let filled_style = Style::default().fg(theme.accent).bg(bg);
        let unfilled_style = Style::default().fg(theme.neutral).bg(bg);

        let y = area.y;
        let mut x = area.x;

        for _ in 0..filled {
            if x < area.x + area.width {
                buf[(x, y)].set_char('█').set_style(filled_style);
                x += 1;
            }
        }

        for _ in 0..unfilled {
            if x < area.x + area.width {
                buf[(x, y)].set_char('░').set_style(unfilled_style);
                x += 1;
            }
        }
    }

    /// Render the count label ("N/M") into the given area.
    pub fn render_label(
        &self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        bg: ratatui::style::Color,
    ) {
        self.render_label_styled(area, buf, theme.base_content, bg);
    }

    /// Render the count label ("N/M") with explicit fg/bg colors.
    pub fn render_label_styled(
        &self,
        area: Rect,
        buf: &mut Buffer,
        fg: ratatui::style::Color,
        bg: ratatui::style::Color,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let label = format!("{}/{}", self.completed, self.total);
        render_text(area, buf, &label, Style::default().fg(fg).bg(bg));
    }
}

/// Render plain text left-aligned in the area.
fn render_text(area: Rect, buf: &mut Buffer, text: &str, style: Style) {
    let y = area.y;
    let mut x = area.x;
    for ch in text.chars() {
        if x < area.x + area.width {
            buf[(x, y)].set_char(ch).set_style(style);
            x += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn test_theme() -> Theme {
        Theme {
            base_100: Color::Black,
            base_200: Color::DarkGray,
            base_300: Color::Gray,
            base_content: Color::White,
            primary: Color::Blue,
            primary_content: Color::White,
            secondary: Color::Cyan,
            secondary_content: Color::White,
            accent: Color::Magenta,
            accent_content: Color::White,
            neutral: Color::DarkGray,
            neutral_content: Color::White,
            info: Color::Blue,
            info_content: Color::White,
            success: Color::Green,
            success_content: Color::White,
            warning: Color::Yellow,
            warning_content: Color::White,
            error: Color::Red,
            error_content: Color::White,
            border_rounded: false,
        }
    }

    #[test]
    fn renders_full_bar() {
        let theme = test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let bar = MiniProgressBar {
            completed: 3,
            total: 3,
        };
        bar.render_bar(Rect::new(0, 0, 10, 1), &mut buf, &theme, Color::Black);

        let content: String = (0..10)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap())
            .collect();
        assert_eq!(content, "██████████"); // All filled
    }

    #[test]
    fn renders_empty_bar() {
        let theme = test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let bar = MiniProgressBar {
            completed: 0,
            total: 3,
        };
        bar.render_bar(Rect::new(0, 0, 10, 1), &mut buf, &theme, Color::Black);

        let content: String = (0..10)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap())
            .collect();
        assert_eq!(content, "░░░░░░░░░░"); // All unfilled
    }

    #[test]
    fn renders_label() {
        let theme = test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 1));
        let bar = MiniProgressBar {
            completed: 1,
            total: 2,
        };
        bar.render_label(Rect::new(0, 0, 5, 1), &mut buf, &theme, Color::Black);

        let content: String = (0..3)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap())
            .collect();
        assert_eq!(content, "1/2");
    }

    #[test]
    fn handles_zero_width() {
        let theme = test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 0, 0));
        let bar = MiniProgressBar {
            completed: 1,
            total: 2,
        };
        // Should not panic
        bar.render_bar(Rect::new(0, 0, 0, 0), &mut buf, &theme, Color::Black);
        bar.render_label(Rect::new(0, 0, 0, 0), &mut buf, &theme, Color::Black);
    }
}
