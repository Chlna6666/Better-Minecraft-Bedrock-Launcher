# Version Custom Icon Design

## Goal

Allow a managed Minecraft version to use a custom icon. The user chooses a PNG
in Version Settings. On save, BMCBL copies it to `icon.png` in that version's
game installation directory. The icon is shown in the Home and Manage version
lists after the next version refresh.

## Scope

The feature covers only the installed-version scan, Version Settings modal,
Home list, Manage list, localized UI text, and focused tests. It does not
change launcher behavior, version configuration storage, import workflows, or
the GPUI framework.

## Data Flow

1. The version scanner checks each valid game installation directory for an
   existing `icon.png` and attaches its absolute path to the version entry.
2. `use_local_versions` carries that optional path into the Manage page state.
3. A shared icon resolver returns the custom path when present, otherwise the
   existing edition-specific bundled image.
4. Home and Manage both use that resolver when rendering their version cards.

The scanner performs the filesystem check during the existing background load.
Render functions only consume the path already present in the entry.

## Settings Interaction

The Version Settings modal adds a Version Icon card with a native file-picker
button restricted to `.png` files. Selecting a file only records the source
path in modal state. Saving the modal runs the existing background save task;
it copies the selected source to `<game installation directory>/icon.png`.

If no icon is selected, saving retains any existing `icon.png`. A copy failure
keeps the modal open, clears its saving state, and shows the existing error
toast. A successful copy refreshes the local-version state so both lists see
the new icon immediately.

## Error Handling

The copy operation validates that a source path was selected and reports a
contextual error when the source cannot be read or the destination cannot be
written. The existing `icon.png` remains the fallback target when no new file
is chosen. Missing custom icons are not errors; the UI falls back to the
built-in edition icon.

## Tests

Add focused tests for custom-icon path resolution: an existing `icon.png` is
returned, and an absent file results in no custom path. Add a copy test using
an isolated temporary directory to verify that the selected PNG becomes the
destination `icon.png`. The tests must be written and observed failing before
the implementation.

## Localization

Add Version Settings labels for the icon card, its description, the select
button, and the selected state to all five existing locale files.
