use crate::{Context, Empty, IntoElement, Pixels, Point, Render, Window, seal::Sealed};
use smallvec::SmallVec;
use std::{fmt::Debug, path::PathBuf};

use super::{InputEvent, MouseEvent, PlatformInput};

/// A collection of paths from the platform, such as from a file drop.
#[derive(Debug, Clone, Default)]
pub struct ExternalPaths(pub(crate) SmallVec<[PathBuf; 2]>);

impl ExternalPaths {
    /// Convert this collection of paths into a slice.
    pub fn paths(&self) -> &[PathBuf] {
        &self.0
    }
}

impl Render for ExternalPaths {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // the platform will render icons for the dragged files
        Empty
    }
}

/// A file drop event from the platform, generated when files are dragged and dropped onto the window.
#[derive(Debug, Clone)]
pub enum FileDropEvent {
    /// The files have entered the window.
    Entered {
        /// The position of the mouse relative to the window.
        position: Point<Pixels>,
        /// The paths of the files that are being dragged.
        paths: ExternalPaths,
    },
    /// The files are being dragged over the window
    Pending {
        /// The position of the mouse relative to the window.
        position: Point<Pixels>,
    },
    /// The files have been dropped onto the window.
    Submit {
        /// The position of the mouse relative to the window.
        position: Point<Pixels>,
    },
    /// The user has stopped dragging the files over the window.
    Exited,
}

impl Sealed for FileDropEvent {}
impl InputEvent for FileDropEvent {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::FileDrop(self)
    }
}
impl MouseEvent for FileDropEvent {}
