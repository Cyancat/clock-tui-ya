
use ratatui::{buffer::Buffer, style::Style};

use crate::clock_text::point::Point;

use super::Font;

pub struct BricksFont {
    pub size: u16,
}

impl BricksFont {
    pub fn new(size: u16) -> Self {
        Self { size }
    }

    /// each row is represented with a vector of numbers:
    ///   the odd indexed items represent the lenght of "off",
    ///   the even indexed items represent the lenght of "on".
    /// For exmaple:
    ///   vec![0, 6] is  "██████"
    ///   vec![2, 2] is  "  ██"
    ///   vec![0, 2, 2, 2] is  "██  ██"
    fn draw_row(
        start: Point,
        row: Vec<u16>,
        size: u16,
        style: Style,
        area_right: u16,
        area_bottom: u16,
        buf: &mut Buffer,
    ) {
        let mut p = start;
        let mut on = false;
        for len in row {
            let len = len * size;
            if p.0 > area_right {
                break;
            }

            if on {
                let s = std::cmp::min(len, area_right - p.0 + 1);
                let line = "█".repeat(s as usize);
                for r in 0..size {
                    if p.1 > area_bottom {
                        break;
                    }
                    buf.set_string(p.0, p.1 + r, line.as_str(), style);
                }
            }

            p.0 += len;
            on = !on;
        }
    }

    fn get_char_matrix(c: char) -> Option<[Vec<u16>; 5]> {
        match c {
            '0' => Some([
                vec![0, 6],
                vec![0, 2, 2, 2],
                vec![0, 2, 2, 2],
                vec![0, 2, 2, 2],
                vec![0, 6],
            ]),
            '1' => Some([vec![0, 4], vec![2, 2], vec![2, 2], vec![2, 2], vec![0, 6]]),
            '2' => Some([vec![0, 6], vec![4, 2], vec![0, 6], vec![0, 2], vec![0, 6]]),
            '3' => Some([vec![0, 6], vec![4, 2], vec![0, 6], vec![4, 2], vec![0, 6]]),
            '4' => Some([
                vec![0, 2, 2, 2],
                vec![0, 2, 2, 2],
                vec![0, 6],
                vec![4, 2],
                vec![4, 2],
            ]),
            '5' => Some([vec![0, 6], vec![0, 2], vec![0, 6], vec![4, 2], vec![0, 6]]),
            '6' => Some([
                vec![0, 6],
                vec![0, 2],
                vec![0, 6],
                vec![0, 2, 2, 2],
                vec![0, 6],
            ]),
            '7' => Some([vec![0, 6], vec![4, 2], vec![4, 2], vec![4, 2], vec![4, 2]]),
            '8' => Some([
                vec![0, 6],
                vec![0, 2, 2, 2],
                vec![0, 6],
                vec![0, 2, 2, 2],
                vec![0, 6],
            ]),
            '9' => Some([
                vec![0, 6],
                vec![0, 2, 2, 2],
                vec![0, 6],
                vec![4, 2],
                vec![0, 6],
            ]),
            ':' => Some([vec![], vec![2, 2], vec![], vec![2, 2], vec![]]),
            '.' => Some([vec![], vec![], vec![], vec![], vec![2, 2]]),
            '-' => Some([vec![], vec![], vec![0, 6], vec![], vec![]]),
            _ => None,
        }
    }

    fn draw_char_sized(
        &self,
        c: char,
        x: u16,
        y: u16,
        size: u16,
        style: Style,
        buf: &mut Buffer,
    ) {
        if let Some(matrix) = Self::get_char_matrix(c) {
            let mut start = Point(x, y);
            let area_right = buf.area.right();
            let area_bottom = buf.area.bottom();
            for row in matrix {
                Self::draw_row(start, row, size, style, area_right, area_bottom, buf);
                start.1 += size;
            }
        }
    }

    /// Draw the 'd' (days) suffix as a 2x2 mini glyph scaled by `size`:
    /// exactly 1/3 of the digit width (2s vs 6s) and 2/5 of the digit height
    /// (2s vs 5s — the closest discrete approximation of 1/3), bottom-aligned
    /// with the digit baseline.
    fn draw_day_suffix(&self, x: u16, y: u16, style: Style, buf: &mut Buffer) {
        let s = self.size;
        let area_right = buf.area.right();
        let area_bottom = buf.area.bottom();
        let y0 = y + self.get_char_height().saturating_sub(2 * s);
        // crude lowercase 'd': ascender on the right, bowl at the bottom
        const PATTERN: [[bool; 2]; 2] = [[false, true], [true, true]];
        for (r, row) in PATTERN.iter().enumerate() {
            for (c, on) in row.iter().enumerate() {
                if !on {
                    continue;
                }
                let px = x + c as u16 * s;
                let py = y0 + r as u16 * s;
                if px >= area_right {
                    continue;
                }
                let line = "█".repeat(s.min(area_right - px) as usize);
                for dy in 0..s {
                    if py + dy >= area_bottom {
                        break;
                    }
                    buf.set_string(px, py + dy, line.as_str(), style);
                }
            }
        }
    }
}

impl Font for BricksFont {
    fn get_char(&self, _c: char) -> Option<&[Point]> {
        None // We don't use points for BricksFont
    }

    fn get_char_width(&self) -> u16 {
        6 * self.size
    }

    fn get_char_height(&self) -> u16 {
        5 * self.size
    }

    fn char_advance(&self, c: char) -> u16 {
        if c == 'd' || c == ' ' {
            // Mini day suffix and the space after it are 2*size wide
            // (1/3 of the digit width); the space draws nothing, only advances.
            2 * self.size + 2
        } else {
            self.get_char_width() + 2
        }
    }

    fn draw_char(&self, c: char, x: u16, y: u16, style: Style, buf: &mut Buffer) {
        if c == 'd' {
            self.draw_day_suffix(x, y, style, buf);
        } else {
            self.draw_char_sized(c, x, y, self.size, style, buf);
        }
    }
}
