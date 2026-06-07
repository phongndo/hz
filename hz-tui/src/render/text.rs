use hz_diff::FileStatus;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) fn fit_with_ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.width() <= width {
        return fit(text, width);
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
    if is_single_width_ascii(visible) {
        let used = visible.len().min(width);
        let mut out = String::with_capacity(width);
        out.push_str(&visible[..used]);
        if used < width {
            out.extend(std::iter::repeat_n(' ', width - used));
        }
        return out;
    }

    let mut out = fit(visible, width);
    let len = UnicodeWidthStr::width(out.as_str());
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
    if is_single_width_ascii(text) {
        let skipped = columns.min(text.len());
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
    if width == 0 {
        return String::new();
    }
    if is_single_width_ascii(text) {
        return text[..text.len().min(width)].to_owned();
    }

    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width {
            break;
        }
        used += ch_width;
        out.push(ch);
    }
    out
}

pub(crate) fn is_single_width_ascii(text: &str) -> bool {
    text.bytes().all(|byte| (b' '..=b'~').contains(&byte))
}
