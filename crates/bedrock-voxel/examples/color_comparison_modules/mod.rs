mod image;
mod obj;
mod obj_source;

use std::path::Path;

pub(super) fn compare_image(input: &Path, output: &Path) -> Result<(), String> {
    image::compare_image(input, output)
}

pub(super) fn compare_saved_images(source: &Path, converted: &Path) -> Result<(), String> {
    image::compare_saved_images(source, converted)
}

pub(super) fn compare_obj(
    input: &Path,
    output: &Path,
    longest_side: u16,
    exclude_snow: bool,
    compare_order: bool,
) -> Result<(), String> {
    obj::compare_obj(input, output, longest_side, exclude_snow, compare_order)
}
