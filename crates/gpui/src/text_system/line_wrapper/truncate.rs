use crate::{Pixels, SharedString, TextRun, px};

use super::LineWrapper;

pub(super) fn truncate_line(
    wrapper: &mut LineWrapper,
    line: SharedString,
    truncate_width: Pixels,
    truncation_suffix: &str,
    runs: &mut Vec<TextRun>,
) -> SharedString {
    let mut width = px(0.);
    let suffix_width = truncation_suffix
        .chars()
        .map(|c| wrapper.width_for_char(c))
        .fold(px(0.0), |a, x| a + x);
    let char_indices = line.char_indices();
    let mut truncate_ix = 0;
    for (ix, c) in char_indices {
        if width + suffix_width < truncate_width {
            truncate_ix = ix;
        }

        let char_width = wrapper.width_for_char(c);
        width += char_width;

        if width.floor() > truncate_width {
            let result =
                SharedString::from(format!("{}{}", &line[..truncate_ix], truncation_suffix));
            update_runs_after_truncation(&result, truncation_suffix, runs);

            return result;
        }
    }

    line
}

pub(super) fn update_runs_after_truncation(result: &str, ellipsis: &str, runs: &mut Vec<TextRun>) {
    let mut truncate_at = result.len() - ellipsis.len();
    for (run_index, run) in runs.iter_mut().enumerate() {
        if run.len <= truncate_at {
            truncate_at -= run.len;
        } else {
            run.len = truncate_at + ellipsis.len();
            runs.truncate(run_index + 1);
            break;
        }
    }
}
