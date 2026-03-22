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
    /// Render the progress bar into the given area using theme colors.
    ///
    /// Layout: `[████░░░░] N/M`
    ///
    /// The bar portion occupies `area.width - label_width - 1` columns (1 space
    /// separator between bar and label). If the area is too narrow for both,
    /// only the label is shown.
    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme, bg: ratatui::style::Color) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let label = format!("{}/{}", self.completed, self.total);
        let label_width = label.len() as u16;

        // Need at least label_width + 1 (space) + 1 (min bar) to show a bar.
        let bar_width = if area.width > label_width + 1 {
            area.width - label_width - 1
        } else {
            // Only enough room for the label.
            render_text(
                area,
                buf,
                &label,
                Style::default().fg(theme.base_content).bg(bg),
            );
            return;
        };

        let fraction = if self.total == 0 {
            0.0
        } else {
            self.completed as f64 / self.total as f64
        };
        let filled = ((fraction * bar_width as f64).round() as u16).min(bar_width);
        let unfilled = bar_width - filled;

        let filled_style = Style::default().fg(theme.success).bg(bg);
        let unfilled_style = Style::default().fg(theme.neutral).bg(bg);
        let label_style = Style::default().fg(theme.base_content).bg(bg);

        let y = area.y;
        let mut x = area.x;

        // Filled portion (using block char '█')
        for _ in 0..filled {
            if x < area.x + area.width {
                buf[(x, y)].set_char('█').set_style(filled_style);
                x += 1;
            }
        }

        // Unfilled portion (using light shade '░')
        for _ in 0..unfilled {
            if x < area.x + area.width {
                buf[(x, y)].set_char('░').set_style(unfilled_style);
                x += 1;
            }
        }

        // Space separator
        if x < area.x + area.width {
            buf[(x, y)].set_char(' ').set_style(label_style);
            x += 1;
        }

        // Label text
        for ch in label.chars() {
            if x < area.x + area.width {
                buf[(x, y)].set_char(ch).set_style(label_style);
                x += 1;
            }
        }
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
        let mut buf = Buffer::empty(Rect::new(0, 0, 15, 1));
        let bar = MiniProgressBar {
            completed: 3,
            total: 3,
        };
        bar.render(Rect::new(0, 0, 15, 1), &mut buf, &theme, Color::Black);

        // Label is "3/3" (3 chars), space (1 char), bar = 15 - 3 - 1 = 11 chars
        // All 11 should be filled
        let content: String = (0..15)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap())
            .collect();
        assert!(content.starts_with("███████████")); // 11 filled
        assert!(content.ends_with("3/3"));
    }

    #[test]
    fn renders_empty_bar() {
        let theme = test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 15, 1));
        let bar = MiniProgressBar {
            completed: 0,
            total: 3,
        };
        bar.render(Rect::new(0, 0, 15, 1), &mut buf, &theme, Color::Black);

        let content: String = (0..15)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap())
            .collect();
        assert!(content.starts_with("░░░░░░░░░░░")); // 11 unfilled
        assert!(content.ends_with("0/3"));
    }

    #[test]
    fn renders_partial_bar() {
        let theme = test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 15, 1));
        let bar = MiniProgressBar {
            completed: 1,
            total: 2,
        };
        bar.render(Rect::new(0, 0, 15, 1), &mut buf, &theme, Color::Black);

        let content: String = (0..15)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap())
            .collect();
        assert!(content.ends_with("1/2"));
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
        bar.render(Rect::new(0, 0, 0, 0), &mut buf, &theme, Color::Black);
    }

    #[test]
    fn narrow_area_shows_label_only() {
        let theme = test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        let bar = MiniProgressBar {
            completed: 1,
            total: 2,
        };
        bar.render(Rect::new(0, 0, 3, 1), &mut buf, &theme, Color::Black);

        let content: String = (0..3)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap())
            .collect();
        assert_eq!(content, "1/2");
    }
}
