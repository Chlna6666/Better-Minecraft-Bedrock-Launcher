#[path = "palette/import.rs"]
mod import;
mod light;
mod map_color;

pub use import::{PaletteImportReport, RenderPalette, RgbaColor};
pub use map_color::{MapColor, MapColorEntry, MapTintMethod};
