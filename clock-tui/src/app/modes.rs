mod clock;
mod countdown;
mod pause;
mod stopwatch;
mod timer;

use std::cmp::min;
use std::fmt::Write as _;

use crate::clock_text::ClockText;
use chrono::Duration;
pub(crate) use clock::Clock;
pub(crate) use countdown::Countdown;
pub(crate) use pause::Pause;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::Span,
    widgets::{Paragraph, Widget},
};
pub(crate) use stopwatch::Stopwatch;
pub(crate) use timer::Timer;

#[derive(Copy, Clone)]
pub(crate) enum DurationFormat {
    /// Hours, minutes, seconds, deciseconds
    HourMinSecDeci,
    /// Hours, minutes, seconds
    HourMinSec,
}

fn format_duration(duration: Duration, format: DurationFormat) -> String {
    let is_neg = duration < Duration::zero();
    let duration = if is_neg { -duration } else { duration };

    let millis = duration.num_milliseconds();
    let seconds = millis / 1000;
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    let mut result = String::new();

    fn append_number(s: &mut String, num: i64) {
        if s.is_empty() {
            let _ = write!(s, "{}", num);
        } else {
            let _ = write!(s, "{:02}", num);
        }
    }

    if days > 0 {
        // eg. "18d 23:59:59" — the 'd' suffix is rendered smaller by BricksFont
        let _ = write!(result, "{}d ", days);
    }
    if hours > 0 {
        append_number(&mut result, hours % 24);
        result.push(':');
    }
    append_number(&mut result, minutes % 60);
    result.push(':');

    if is_neg {
        result.insert(0, '-');
    }
    match format {
        DurationFormat::HourMinSecDeci => {
            let _ = write!(result, "{:02}.{}", seconds % 60, (millis % 1000) / 100);
        }
        DurationFormat::HourMinSec => {
            let _ = write!(result, "{:02}", seconds % 60);
        }
    }

    result
}

fn render_centered(
    area: Rect,
    buf: &mut Buffer,
    text: &ClockText,
    header: Option<String>,
    footer: Option<String>,
) {
    let text_size = text.size();
    let text_area = Rect {
        x: area.x + (area.width.saturating_sub(text_size.0)) / 2,
        y: area.y + (area.height.saturating_sub(text_size.1)) / 2,
        width: min(text_size.0, area.width),
        height: min(text_size.1, area.height),
    };
    text.clone().render(text_area, buf);

    let render_text_center = |text: &str, top: u16, buf: &mut Buffer| {
        let text_len = text.len() as u16;
        let paragrahp = Paragraph::new(Span::from(text)).style(Style::default());

        let para_area = Rect {
            x: area.left() + (area.width.saturating_sub(text_len)) / 2,
            y: top,
            width: min(text_len, area.width),
            height: min(1, area.height),
        };
        paragrahp.render(para_area, buf);
    };

    if let Some(text) = header {
        if area.top() + 2 <= text_area.top() {
            render_text_center(text.as_str(), text_area.top() - 2, buf);
        }
    }

    if let Some(text) = footer {
        if area.bottom() >= text_area.bottom() + 2 {
            render_text_center(text.as_str(), text_area.bottom() + 1, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock_text::font::bricks::BricksFont;
    use crate::clock_text::font::Font;

    #[test]
    fn format_duration_with_days_uses_d_suffix() {
        let d = Duration::days(18) + Duration::hours(23) + Duration::minutes(59)
            + Duration::seconds(59);
        assert_eq!(format_duration(d, DurationFormat::HourMinSec), "18d 23:59:59");
        assert_eq!(
            format_duration(d, DurationFormat::HourMinSecDeci),
            "18d 23:59:59.0"
        );
    }

    #[test]
    fn format_duration_without_days_unchanged() {
        let d = Duration::hours(23) + Duration::minutes(59) + Duration::seconds(59);
        assert_eq!(format_duration(d, DurationFormat::HourMinSec), "23:59:59");
        let d = Duration::minutes(5) + Duration::seconds(1);
        assert_eq!(format_duration(d, DurationFormat::HourMinSec), "5:01");
    }

    #[test]
    fn day_suffix_renders_narrower() {
        let font = BricksFont::new(3);
        // 'd' and ' ' advance: 2 * 3 + 2 = 8, full char advance: 6 * 3 + 2 = 20
        assert_eq!(font.char_advance('d'), 8);
        assert_eq!(font.char_advance(' '), 8);
        assert_eq!(font.char_advance('1'), 20);

        // Text width with 'd' and ' ' is smaller than if they were full-width chars
        let text = ClockText::new("1d 2:03".to_string(), &font, Style::default());
        let uniform = 7 * 20 - 2;
        assert_eq!(text.size().0, uniform - 2 * (20 - 8));
    }

    #[test]
    fn day_suffix_is_small_even_at_size_one() {
        use ratatui::{buffer::Buffer, layout::Rect};
        let font = BricksFont::new(1);
        assert_eq!(font.char_advance('d'), 4); // 2 * 1 + 2

        // Draw "1d": digit '1' occupies cols 0..6 rows 0..5;
        // 'd' starts at col 8 and must only occupy 2 rows, bottom-aligned (rows 3..5)
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        font.draw_str("1d", area, Style::default(), &mut buf);
        let filled_rows: Vec<u16> = (0..10)
            .filter(|&y| (8..10).any(|x| buf.get(x, y).symbol() == "█"))
            .collect();
        assert_eq!(filled_rows, vec![3, 4]);
    }
}
