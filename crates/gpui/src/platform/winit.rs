#![allow(dead_code)]

use crate::{CursorStyle, Pixels, Point, ResizeEdge, Size, point, px, size};

pub(crate) fn request_window_inner_size(window: &winit::window::Window, size: Size<Pixels>) {
    let _ = window.request_inner_size(winit::dpi::Size::Logical(logical_size_to_winit(size)));
}

pub(crate) fn start_window_move(
    window: &winit::window::Window,
) -> Result<(), winit::error::ExternalError> {
    window.drag_window()
}

pub(crate) fn start_window_resize(
    window: &winit::window::Window,
    edge: ResizeEdge,
) -> Result<(), winit::error::ExternalError> {
    window.drag_resize_window(resize_edge_to_winit(edge))
}

pub(crate) fn maximize_window(window: &winit::window::Window) {
    window.set_maximized(true);
}

pub(crate) fn minimize_window(window: &winit::window::Window) {
    window.set_minimized(true);
}

pub(crate) fn restore_window(window: &winit::window::Window) {
    window.set_minimized(false);
    window.set_maximized(false);
}

pub(crate) fn toggle_window_maximized(window: &winit::window::Window) {
    window.set_maximized(!window.is_maximized());
}

pub(crate) fn toggle_window_fullscreen(window: &winit::window::Window) {
    if window.fullscreen().is_some() {
        window.set_fullscreen(None);
    } else {
        window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
    }
}

pub(crate) fn logical_position_from_winit(position: dpi::LogicalPosition<f64>) -> Point<Pixels> {
    point(px(position.x as f32), px(position.y as f32))
}

pub(crate) fn logical_size_from_winit(dimensions: dpi::LogicalSize<f64>) -> Size<Pixels> {
    size(px(dimensions.width as f32), px(dimensions.height as f32))
}

pub(crate) fn logical_position_to_winit(position: Point<Pixels>) -> dpi::LogicalPosition<f64> {
    dpi::LogicalPosition::new(
        f64::from(position.x / px(1.0)),
        f64::from(position.y / px(1.0)),
    )
}

pub(crate) fn logical_size_to_winit(size: Size<Pixels>) -> dpi::LogicalSize<f64> {
    dpi::LogicalSize::new(
        f64::from(size.width / px(1.0)),
        f64::from(size.height / px(1.0)),
    )
}

pub(crate) fn physical_position_from_winit(
    position: dpi::PhysicalPosition<i32>,
    scale_factor: f64,
) -> Point<Pixels> {
    logical_position_from_winit(position.to_logical(scale_factor))
}

pub(crate) fn physical_size_from_winit(
    size: dpi::PhysicalSize<u32>,
    scale_factor: f64,
) -> Size<Pixels> {
    logical_size_from_winit(size.to_logical(scale_factor))
}

pub(crate) fn physical_position_to_winit(
    position: Point<Pixels>,
    scale_factor: f64,
) -> dpi::PhysicalPosition<i32> {
    logical_position_to_winit(position).to_physical(scale_factor)
}

pub(crate) fn physical_size_to_winit(
    size: Size<Pixels>,
    scale_factor: f64,
) -> dpi::PhysicalSize<u32> {
    logical_size_to_winit(size).to_physical(scale_factor)
}

pub(crate) fn cursor_style_to_icon(style: CursorStyle) -> Option<cursor_icon::CursorIcon> {
    match style {
        CursorStyle::Arrow => Some(cursor_icon::CursorIcon::Default),
        CursorStyle::IBeam => Some(cursor_icon::CursorIcon::Text),
        CursorStyle::Crosshair => Some(cursor_icon::CursorIcon::Crosshair),
        CursorStyle::ClosedHand => Some(cursor_icon::CursorIcon::Grabbing),
        CursorStyle::OpenHand => Some(cursor_icon::CursorIcon::Grab),
        CursorStyle::PointingHand => Some(cursor_icon::CursorIcon::Pointer),
        CursorStyle::ResizeLeft => Some(cursor_icon::CursorIcon::WResize),
        CursorStyle::ResizeRight => Some(cursor_icon::CursorIcon::EResize),
        CursorStyle::ResizeLeftRight => Some(cursor_icon::CursorIcon::EwResize),
        CursorStyle::ResizeUp => Some(cursor_icon::CursorIcon::NResize),
        CursorStyle::ResizeDown => Some(cursor_icon::CursorIcon::SResize),
        CursorStyle::ResizeUpDown => Some(cursor_icon::CursorIcon::NsResize),
        CursorStyle::ResizeUpLeftDownRight => Some(cursor_icon::CursorIcon::NwseResize),
        CursorStyle::ResizeUpRightDownLeft => Some(cursor_icon::CursorIcon::NeswResize),
        CursorStyle::ResizeColumn => Some(cursor_icon::CursorIcon::ColResize),
        CursorStyle::ResizeRow => Some(cursor_icon::CursorIcon::RowResize),
        CursorStyle::IBeamCursorForVerticalLayout => Some(cursor_icon::CursorIcon::VerticalText),
        CursorStyle::OperationNotAllowed => Some(cursor_icon::CursorIcon::NotAllowed),
        CursorStyle::DragLink => Some(cursor_icon::CursorIcon::Alias),
        CursorStyle::DragCopy => Some(cursor_icon::CursorIcon::Copy),
        CursorStyle::ContextualMenu => Some(cursor_icon::CursorIcon::ContextMenu),
        CursorStyle::None => None,
    }
}

pub(crate) fn cursor_icon_to_style(icon: cursor_icon::CursorIcon) -> CursorStyle {
    match icon {
        cursor_icon::CursorIcon::Default => CursorStyle::Arrow,
        cursor_icon::CursorIcon::Text => CursorStyle::IBeam,
        cursor_icon::CursorIcon::Crosshair => CursorStyle::Crosshair,
        cursor_icon::CursorIcon::Grabbing => CursorStyle::ClosedHand,
        cursor_icon::CursorIcon::Grab => CursorStyle::OpenHand,
        cursor_icon::CursorIcon::Pointer => CursorStyle::PointingHand,
        cursor_icon::CursorIcon::WResize => CursorStyle::ResizeLeft,
        cursor_icon::CursorIcon::EResize => CursorStyle::ResizeRight,
        cursor_icon::CursorIcon::EwResize => CursorStyle::ResizeLeftRight,
        cursor_icon::CursorIcon::NResize => CursorStyle::ResizeUp,
        cursor_icon::CursorIcon::SResize => CursorStyle::ResizeDown,
        cursor_icon::CursorIcon::NsResize => CursorStyle::ResizeUpDown,
        cursor_icon::CursorIcon::NwseResize => CursorStyle::ResizeUpLeftDownRight,
        cursor_icon::CursorIcon::NeswResize => CursorStyle::ResizeUpRightDownLeft,
        cursor_icon::CursorIcon::ColResize => CursorStyle::ResizeColumn,
        cursor_icon::CursorIcon::RowResize => CursorStyle::ResizeRow,
        cursor_icon::CursorIcon::VerticalText => CursorStyle::IBeamCursorForVerticalLayout,
        cursor_icon::CursorIcon::NotAllowed => CursorStyle::OperationNotAllowed,
        cursor_icon::CursorIcon::Alias => CursorStyle::DragLink,
        cursor_icon::CursorIcon::Copy => CursorStyle::DragCopy,
        cursor_icon::CursorIcon::ContextMenu => CursorStyle::ContextualMenu,
        _ => CursorStyle::Arrow,
    }
}

pub(crate) fn resize_edge_to_winit(edge: ResizeEdge) -> winit::window::ResizeDirection {
    match edge {
        ResizeEdge::Top => winit::window::ResizeDirection::North,
        ResizeEdge::TopRight => winit::window::ResizeDirection::NorthEast,
        ResizeEdge::Right => winit::window::ResizeDirection::East,
        ResizeEdge::BottomRight => winit::window::ResizeDirection::SouthEast,
        ResizeEdge::Bottom => winit::window::ResizeDirection::South,
        ResizeEdge::BottomLeft => winit::window::ResizeDirection::SouthWest,
        ResizeEdge::Left => winit::window::ResizeDirection::West,
        ResizeEdge::TopLeft => winit::window::ResizeDirection::NorthWest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_logical_geometry() {
        let position = logical_position_from_winit(dpi::LogicalPosition::new(12.5, 24.0));
        assert_eq!(position, point(px(12.5), px(24.0)));

        let dimensions = logical_size_from_winit(dpi::LogicalSize::new(640.0, 480.0));
        assert_eq!(dimensions, size(px(640.0), px(480.0)));

        assert_eq!(
            logical_position_to_winit(point(px(4.0), px(8.0))),
            dpi::LogicalPosition::new(4.0, 8.0)
        );
        assert_eq!(
            logical_position_to_winit(point(px(128.0), px(256.0))),
            dpi::LogicalPosition::new(128.0, 256.0)
        );
        assert_eq!(
            logical_size_to_winit(size(px(32.0), px(48.0))),
            dpi::LogicalSize::new(32.0, 48.0)
        );
    }

    #[test]
    fn converts_physical_geometry_with_scale_factor() {
        let position = physical_position_from_winit(dpi::PhysicalPosition::new(30, 45), 1.5);
        assert_eq!(position, point(px(20.0), px(30.0)));

        let dimensions = physical_size_from_winit(dpi::PhysicalSize::new(300, 150), 1.5);
        assert_eq!(dimensions, size(px(200.0), px(100.0)));

        assert_eq!(
            physical_position_to_winit(point(px(20.0), px(30.0)), 1.5),
            dpi::PhysicalPosition::new(30, 45)
        );
        assert_eq!(
            physical_size_to_winit(size(px(200.0), px(100.0)), 1.5),
            dpi::PhysicalSize::new(300, 150)
        );
    }

    #[test]
    fn maps_cursor_icons() {
        assert_eq!(
            cursor_style_to_icon(CursorStyle::PointingHand),
            Some(cursor_icon::CursorIcon::Pointer)
        );
        assert_eq!(cursor_style_to_icon(CursorStyle::None), None);
        assert_eq!(
            cursor_icon_to_style(cursor_icon::CursorIcon::NwseResize),
            CursorStyle::ResizeUpLeftDownRight
        );
    }

    #[test]
    fn converts_resize_edges() {
        assert_eq!(
            resize_edge_to_winit(ResizeEdge::TopLeft),
            winit::window::ResizeDirection::NorthWest
        );
        assert_eq!(
            resize_edge_to_winit(ResizeEdge::Right),
            winit::window::ResizeDirection::East
        );
    }
}
