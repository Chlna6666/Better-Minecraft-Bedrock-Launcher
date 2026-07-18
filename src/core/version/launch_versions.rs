use std::cmp::Ordering;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LaunchVersionEntry {
    pub(crate) folder: Arc<str>,
    pub(crate) name: Arc<str>,
    pub(crate) version: Arc<str>,
    pub(crate) manifest_version: Arc<str>,
    pub(crate) path: Arc<str>,
    pub(crate) kind: Arc<str>,
    pub(crate) custom_icon_path: Option<Arc<str>>,
}

fn next_version_number(version: &str, cursor: &mut usize) -> Option<u64> {
    let bytes = version.as_bytes();
    let len = bytes.len();

    while *cursor < len {
        let byte = bytes[*cursor];
        if byte.is_ascii_digit() {
            break;
        }
        *cursor += 1;
    }

    if *cursor >= len {
        return None;
    }

    let start = *cursor;
    while *cursor < len && bytes[*cursor].is_ascii_digit() {
        *cursor += 1;
    }

    version[start..*cursor].parse::<u64>().ok()
}

pub(crate) fn compare_versions_desc(left: &str, right: &str) -> Ordering {
    let mut left_cursor = 0;
    let mut right_cursor = 0;

    loop {
        let left_number = next_version_number(left, &mut left_cursor);
        let right_number = next_version_number(right, &mut right_cursor);

        match (left_number, right_number) {
            (Some(left_number), Some(right_number)) => match right_number.cmp(&left_number) {
                Ordering::Equal => continue,
                non_equal => return non_equal,
            },
            (Some(left_number), None) => {
                return if left_number == 0 {
                    Ordering::Equal
                } else {
                    Ordering::Less
                };
            }
            (None, Some(right_number)) => {
                return if right_number == 0 {
                    Ordering::Equal
                } else {
                    Ordering::Greater
                };
            }
            (None, None) => return Ordering::Equal,
        }
    }
}

pub(crate) fn sort_launch_versions(versions: &mut [LaunchVersionEntry]) {
    versions.sort_by(|left, right| {
        compare_versions_desc(left.version.as_ref(), right.version.as_ref())
    });
}

pub(crate) fn sort_versions_by_launch_counts(
    versions: &mut [LaunchVersionEntry],
    launch_count_of: impl Fn(&str) -> u32,
) {
    versions.sort_by(|left, right| {
        let left_count = launch_count_of(left.folder.as_ref());
        let right_count = launch_count_of(right.folder.as_ref());
        match right_count.cmp(&left_count) {
            Ordering::Equal => compare_versions_desc(left.version.as_ref(), right.version.as_ref()),
            other => other,
        }
    });
}
