use hz_diff::FileStatus;
use std::borrow::Cow;
use unicode_width::UnicodeWidthChar;

const STATIC_SPACES: &str = concat!(
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
);

pub(crate) fn fit_with_ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let (fitted, _, complete) = fit_with_width(text, width);
    if complete {
        return fitted;
    }
    if width <= 3 {
        return fit("...", width);
    }

    format!("{}...", fit(text, width - 3))
}

pub(crate) fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(digit);
    }

    formatted
}

pub(crate) fn status_code(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Modified => "M",
        FileStatus::Added => "A",
        FileStatus::Deleted => "D",
        FileStatus::Renamed => "R",
        FileStatus::Copied => "C",
        FileStatus::TypeChanged => "T",
        FileStatus::Unknown => "?",
    }
}

pub(crate) fn progress_label(scroll: usize, max_scroll: usize) -> String {
    if max_scroll == 0 {
        return "100%".to_owned();
    }

    format!(
        "{}%",
        scroll.min(max_scroll).saturating_mul(100) / max_scroll
    )
}

pub(crate) fn fit_padded(text: &str, width: usize) -> String {
    fit_padded_from(text, 0, width)
}

pub(crate) fn fit_padded_from(text: &str, horizontal_scroll: usize, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let visible = if horizontal_scroll > 0 {
        skip_display_prefix(text, horizontal_scroll).0
    } else {
        text
    };

    let ascii_prefix = single_width_ascii_prefix_len(visible, width);
    if ascii_prefix == visible.len()
        || (ascii_prefix == width && next_byte_is_single_width_ascii(visible, ascii_prefix))
    {
        let used = ascii_prefix;
        let mut out = String::with_capacity(width);
        out.push_str(&visible[..used]);
        if used < width {
            out.extend(std::iter::repeat_n(' ', width - used));
        }
        return out;
    }

    let (mut out, len, _) = fit_with_width(visible, width);
    if len < width {
        out.reserve(width - len);
        out.extend(std::iter::repeat_n(' ', width - len));
    }
    out
}

pub(crate) fn skip_display_prefix(text: &str, columns: usize) -> (&str, usize) {
    if columns == 0 {
        return (text, 0);
    }

    let ascii_prefix = single_width_ascii_prefix_len(text, columns);
    if ascii_prefix == text.len()
        || (ascii_prefix == columns && next_byte_is_single_width_ascii(text, ascii_prefix))
    {
        let skipped = ascii_prefix;
        return (&text[skipped..], skipped);
    }

    let mut skipped = 0usize;
    let mut byte_index = 0usize;
    for (index, ch) in text.char_indices() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if skipped >= columns {
            if ch_width == 0 {
                byte_index = index + ch.len_utf8();
                continue;
            }
            break;
        }

        skipped = skipped.saturating_add(ch_width);
        byte_index = index + ch.len_utf8();
    }

    (&text[byte_index..], skipped)
}

pub(crate) fn fit(text: &str, width: usize) -> String {
    fit_with_width(text, width).0
}

pub(crate) fn fit_with_width(text: &str, width: usize) -> (String, usize, bool) {
    if width == 0 {
        return (String::new(), 0, text.is_empty());
    }

    let ascii_prefix = single_width_ascii_prefix_len(text, width);
    if ascii_prefix == text.len()
        || (ascii_prefix == width && next_byte_is_single_width_ascii(text, ascii_prefix))
    {
        return (
            text[..ascii_prefix].to_owned(),
            ascii_prefix,
            ascii_prefix == text.len(),
        );
    }

    let mut out = String::with_capacity(text.len().min(width.saturating_mul(4)));
    let mut used = 0;
    let mut byte_end = 0;
    for (index, ch) in text.char_indices() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width {
            break;
        }
        used += ch_width;
        byte_end = index + ch.len_utf8();
        out.push(ch);
    }
    (out, used, byte_end == text.len())
}

pub(crate) fn spaces(width: usize) -> Cow<'static, str> {
    if width <= STATIC_SPACES.len() {
        Cow::Borrowed(&STATIC_SPACES[..width])
    } else {
        Cow::Owned(" ".repeat(width))
    }
}

fn single_width_ascii_prefix_len(text: &str, max_bytes: usize) -> usize {
    text.as_bytes()
        .iter()
        .take(max_bytes)
        .take_while(|&&byte| (b' '..=b'~').contains(&byte))
        .count()
}

fn next_byte_is_single_width_ascii(text: &str, index: usize) -> bool {
    text.as_bytes()
        .get(index)
        .is_some_and(|byte| (b' '..=b'~').contains(byte))
}
