mod color_comparison_modules;

use std::{env, fs, path::PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let mode = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(usage)?;
    let input = PathBuf::from(arguments.next().ok_or_else(usage)?);
    let output = PathBuf::from(arguments.next().ok_or_else(usage)?);
    if mode == "metrics" {
        if arguments.next().is_some() {
            return Err(usage());
        }
        return color_comparison_modules::compare_saved_images(&input, &output);
    }
    fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    match mode.as_str() {
        "image" if arguments.next().is_none() => {
            color_comparison_modules::compare_image(&input, &output)
        }
        "obj" => {
            let mut longest_side = 64;
            let mut side_was_set = false;
            let mut exclude_snow = false;
            let mut compare_order = false;
            for argument in arguments {
                let value = argument.to_str().ok_or_else(usage)?;
                if value == "--without-snow" {
                    exclude_snow = true;
                } else if value == "--compare-order" {
                    compare_order = true;
                } else if !side_was_set {
                    longest_side = value.parse::<u16>().map_err(|_| usage())?;
                    side_was_set = true;
                } else {
                    return Err(usage());
                }
            }
            if !(1..=384).contains(&longest_side) {
                return Err(usage());
            }
            color_comparison_modules::compare_obj(
                &input,
                &output,
                longest_side,
                exclude_snow,
                compare_order,
            )
        }
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: color_comparison image <jpg> <output-dir> | metrics <source.png> <converted.png> | obj <obj-or-directory> <output-dir> [longest-side-blocks] [--without-snow] [--compare-order]".to_owned()
}
