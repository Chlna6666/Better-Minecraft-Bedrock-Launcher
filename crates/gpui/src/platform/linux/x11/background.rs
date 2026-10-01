use x11rb::{
    connection::Connection,
    protocol::{
        xfixes,
        xproto::{self, ConnectionExt as _},
    },
    wrapper::ConnectionExt as _,
    xcb_ffi::XCBConnection,
};

use crate::{WindowBackgroundAppearance, WindowBackgroundCapabilities};

const BLUR_PROPERTY: &[u8] = b"_KDE_NET_WM_BLUR_BEHIND_REGION";

pub(super) fn is_background_announcement(
    connection: &XCBConnection,
    window: xproto::Window,
    atom: xproto::Atom,
) -> bool {
    connection
        .setup()
        .roots
        .iter()
        .any(|screen| screen.root == window)
        && connection
            .intern_atom(true, BLUR_PROPERTY)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|reply| reply.atom == atom)
}

pub(super) fn watch_background_capabilities(connection: &XCBConnection) -> anyhow::Result<()> {
    let supports_selection_events = xfixes::query_version(connection, 2, 0)
        .ok()
        .is_some_and(|cookie| cookie.reply().is_ok());
    for (index, screen) in connection.setup().roots.iter().enumerate() {
        let attributes = connection.get_window_attributes(screen.root)?.reply()?;
        connection
            .change_window_attributes(
                screen.root,
                &xproto::ChangeWindowAttributesAux::new()
                    .event_mask(attributes.your_event_mask | xproto::EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        let selection = connection
            .intern_atom(false, format!("_NET_WM_CM_S{index}").as_bytes())?
            .reply()?
            .atom;
        if supports_selection_events {
            xfixes::select_selection_input(
                connection,
                screen.root,
                selection,
                xfixes::SelectionEventMask::SET_SELECTION_OWNER
                    | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )?
            .check()?;
        }
    }
    connection.flush()?;
    Ok(())
}

fn native_blur_supported(connection: &XCBConnection, screen_index: usize) -> anyhow::Result<bool> {
    let screen = &connection.setup().roots[screen_index];
    let selection = connection
        .intern_atom(true, format!("_NET_WM_CM_S{screen_index}").as_bytes())?
        .reply()?
        .atom;
    if selection == x11rb::NONE
        || connection.get_selection_owner(selection)?.reply()?.owner == x11rb::NONE
    {
        return Ok(false);
    }
    let atom = connection.intern_atom(true, BLUR_PROPERTY)?.reply()?.atom;
    if atom == x11rb::NONE {
        return Ok(false);
    }
    // KWin announces support using a root property with its own atom as the type, format 8,
    // and a dummy byte. It does not currently add this extension to _NET_SUPPORTED.
    let support = connection
        .get_property(false, screen.root, atom, xproto::AtomEnum::ANY, 0, 1)?
        .reply()?;
    Ok(support.type_ == atom && support.format == 8 && !support.value.is_empty())
}

pub(super) fn apply_background(
    connection: &XCBConnection,
    screen_index: usize,
    window: xproto::Window,
    requested: WindowBackgroundAppearance,
) -> anyhow::Result<(WindowBackgroundCapabilities, WindowBackgroundAppearance)> {
    let capabilities = WindowBackgroundCapabilities {
        blurred: native_blur_supported(connection, screen_index)?,
        ..WindowBackgroundCapabilities::default()
    };
    let effective = capabilities.resolve(requested);
    let atom = connection.intern_atom(false, BLUR_PROPERTY)?.reply()?.atom;
    if effective == WindowBackgroundAppearance::Blurred {
        // An empty CARDINAL region denotes the whole window in the KDE X11 extension.
        connection
            .change_property32(
                xproto::PropMode::REPLACE,
                window,
                atom,
                xproto::AtomEnum::CARDINAL,
                &[],
            )?
            .check()?;
    } else {
        connection.delete_property(window, atom)?.check()?;
    }
    connection.flush()?;
    Ok((capabilities, effective))
}
