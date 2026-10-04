pub mod bricks;

use ratatui::{buffer::Buffer, layout::Rect, style::Style};

use super::point::Point;

pub trait Font {
    fn get_char(&self, c: char) -> Option<&[Point]>;
    fn get_char_width(&self) -> u16;
    fn get_char_height(&self) -> u16;

    /// Horizontal advance for a character, including trailing spacing.
    /// Fonts may render some glyphs narrower than the default char width
    /// (e.g. BricksFont renders 'd' at 1/3 size).
    fn char_advance(&self, _c: char) -> u16 {
        self.get_char_width() + 2
    }

    fn draw_char(&self, c: char, x: u16, y: u16, style: Style, buf: &mut Buffer) {
        if let Some(points) = self.get_char(c) {
            for point in points {
                let x = x + point.0;
                let y = y + point.1;
                if x < buf.area.right() && y < buf.area.bottom() {
                    buf.get_mut(x, y).set_style(style);
                }
            }
        }
    }

    fn draw_str(&self, s: &str, area: Rect, style: Style, buf: &mut Buffer) {
        let mut x = area.x;
        let y = area.y;
        for c in s.chars() {
            let advance = self.char_advance(c);
            if x + advance - 2 > area.right() {
                break;
            }
            self.draw_char(c, x, y, style, buf);
            x += advance;
        }
    }
}
