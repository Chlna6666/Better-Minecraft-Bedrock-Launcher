use super::*;

#[test]
fn test_keymap() {
    let bindings = [
        KeyBinding::new("ctrl-a", ActionAlpha {}, None),
        KeyBinding::new("ctrl-a", ActionBeta {}, Some("pane")),
        KeyBinding::new("ctrl-a", ActionGamma {}, Some("editor && mode==full")),
    ];

    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings.clone());

    // global bindings are enabled in all contexts
    assert_eq!(keymap.binding_enabled(&bindings[0], &[]), Some(0));
    assert_eq!(
        keymap.binding_enabled(&bindings[0], &[KeyContext::parse("terminal").unwrap()]),
        Some(1)
    );

    // contextual bindings are enabled in contexts that match their predicate
    assert_eq!(
        keymap.binding_enabled(&bindings[1], &[KeyContext::parse("barf x=y").unwrap()]),
        None
    );
    assert_eq!(
        keymap.binding_enabled(&bindings[1], &[KeyContext::parse("pane x=y").unwrap()]),
        Some(1)
    );

    assert_eq!(
        keymap.binding_enabled(&bindings[2], &[KeyContext::parse("editor").unwrap()]),
        None
    );
    assert_eq!(
        keymap.binding_enabled(
            &bindings[2],
            &[KeyContext::parse("editor mode=full").unwrap()]
        ),
        Some(1)
    );
}

#[test]
fn test_keymap_disabled() {
    let bindings = [
        KeyBinding::new("ctrl-a", ActionAlpha {}, Some("editor")),
        KeyBinding::new("ctrl-b", ActionAlpha {}, Some("editor")),
        KeyBinding::new("ctrl-a", NoAction {}, Some("editor && mode==full")),
        KeyBinding::new("ctrl-b", NoAction {}, None),
    ];

    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    // binding is only enabled in a specific context
    assert!(
        keymap
            .bindings_for_input(
                &[Keystroke::parse("ctrl-a").unwrap()],
                &[KeyContext::parse("barf").unwrap()],
            )
            .0
            .is_empty()
    );
    assert!(
        !keymap
            .bindings_for_input(
                &[Keystroke::parse("ctrl-a").unwrap()],
                &[KeyContext::parse("editor").unwrap()],
            )
            .0
            .is_empty()
    );

    // binding is disabled in a more specific context
    assert!(
        keymap
            .bindings_for_input(
                &[Keystroke::parse("ctrl-a").unwrap()],
                &[KeyContext::parse("editor mode=full").unwrap()],
            )
            .0
            .is_empty()
    );

    // binding is globally disabled
    assert!(
        keymap
            .bindings_for_input(
                &[Keystroke::parse("ctrl-b").unwrap()],
                &[KeyContext::parse("barf").unwrap()],
            )
            .0
            .is_empty()
    );
}

/// Tests for https://github.com/zed-industries/zed/issues/30259
#[test]
fn test_multiple_keystroke_binding_disabled() {
    let bindings = [
        KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
        KeyBinding::new("space w w", NoAction {}, Some("editor")),
    ];

    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    let space = || Keystroke::parse("space").unwrap();
    let w = || Keystroke::parse("w").unwrap();

    let space_w = [space(), w()];
    let space_w_w = [space(), w(), w()];

    let workspace_context = || [KeyContext::parse("workspace").unwrap()];

    let editor_workspace_context = || {
        [
            KeyContext::parse("workspace").unwrap(),
            KeyContext::parse("editor").unwrap(),
        ]
    };

    // Ensure `space` results in pending input on the workspace, but not editor
    let space_workspace = keymap.bindings_for_input(&[space()], &workspace_context());
    assert!(space_workspace.0.is_empty());
    assert!(space_workspace.1);

    let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
    assert!(space_editor.0.is_empty());
    assert!(!space_editor.1);

    // Ensure `space w` results in pending input on the workspace, but not editor
    let space_w_workspace = keymap.bindings_for_input(&space_w, &workspace_context());
    assert!(space_w_workspace.0.is_empty());
    assert!(space_w_workspace.1);

    let space_w_editor = keymap.bindings_for_input(&space_w, &editor_workspace_context());
    assert!(space_w_editor.0.is_empty());
    assert!(!space_w_editor.1);

    // Ensure `space w w` results in the binding in the workspace, but not in the editor
    let space_w_w_workspace = keymap.bindings_for_input(&space_w_w, &workspace_context());
    assert!(!space_w_w_workspace.0.is_empty());
    assert!(!space_w_w_workspace.1);

    let space_w_w_editor = keymap.bindings_for_input(&space_w_w, &editor_workspace_context());
    assert!(space_w_w_editor.0.is_empty());
    assert!(!space_w_w_editor.1);

    // Now test what happens if we have another binding defined AFTER the NoAction
    // that should result in pending
    let bindings = [
        KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
        KeyBinding::new("space w w", NoAction {}, Some("editor")),
        KeyBinding::new("space w x", ActionAlpha {}, Some("editor")),
    ];
    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
    assert!(space_editor.0.is_empty());
    assert!(space_editor.1);

    // Now test what happens if we have another binding defined BEFORE the NoAction
    // that should result in pending
    let bindings = [
        KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
        KeyBinding::new("space w x", ActionAlpha {}, Some("editor")),
        KeyBinding::new("space w w", NoAction {}, Some("editor")),
    ];
    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
    assert!(space_editor.0.is_empty());
    assert!(space_editor.1);

    // Now test what happens if we have another binding defined at a higher context
    // that should result in pending
    let bindings = [
        KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
        KeyBinding::new("space w x", ActionAlpha {}, Some("workspace")),
        KeyBinding::new("space w w", NoAction {}, Some("editor")),
    ];
    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
    assert!(space_editor.0.is_empty());
    assert!(space_editor.1);
}

#[test]
fn test_override_multikey() {
    let bindings = [
        KeyBinding::new("ctrl-w left", ActionAlpha {}, Some("editor")),
        KeyBinding::new("ctrl-w", NoAction {}, Some("editor")),
    ];

    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    // Ensure `space` results in pending input on the workspace, but not editor
    let (result, pending) = keymap.bindings_for_input(
        &[Keystroke::parse("ctrl-w").unwrap()],
        &[KeyContext::parse("editor").unwrap()],
    );
    assert!(result.is_empty());
    assert!(pending);

    let bindings = [
        KeyBinding::new("ctrl-w left", ActionAlpha {}, Some("editor")),
        KeyBinding::new("ctrl-w", ActionBeta {}, Some("editor")),
    ];

    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    // Ensure `space` results in pending input on the workspace, but not editor
    let (result, pending) = keymap.bindings_for_input(
        &[Keystroke::parse("ctrl-w").unwrap()],
        &[KeyContext::parse("editor").unwrap()],
    );
    assert_eq!(result.len(), 1);
    assert!(!pending);
}

#[test]
fn test_simple_disable() {
    let bindings = [
        KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")),
        KeyBinding::new("ctrl-x", NoAction {}, Some("editor")),
    ];

    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    // Ensure `space` results in pending input on the workspace, but not editor
    let (result, pending) = keymap.bindings_for_input(
        &[Keystroke::parse("ctrl-x").unwrap()],
        &[KeyContext::parse("editor").unwrap()],
    );
    assert!(result.is_empty());
    assert!(!pending);
}

#[test]
fn test_fail_to_disable() {
    // disabled at the wrong level
    let bindings = [
        KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")),
        KeyBinding::new("ctrl-x", NoAction {}, Some("workspace")),
    ];

    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    // Ensure `space` results in pending input on the workspace, but not editor
    let (result, pending) = keymap.bindings_for_input(
        &[Keystroke::parse("ctrl-x").unwrap()],
        &[
            KeyContext::parse("workspace").unwrap(),
            KeyContext::parse("editor").unwrap(),
        ],
    );
    assert_eq!(result.len(), 1);
    assert!(!pending);
}

#[test]
fn test_disable_deeper() {
    let bindings = [
        KeyBinding::new("ctrl-x", ActionAlpha {}, Some("workspace")),
        KeyBinding::new("ctrl-x", NoAction {}, Some("editor")),
    ];

    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    // Ensure `space` results in pending input on the workspace, but not editor
    let (result, pending) = keymap.bindings_for_input(
        &[Keystroke::parse("ctrl-x").unwrap()],
        &[
            KeyContext::parse("workspace").unwrap(),
            KeyContext::parse("editor").unwrap(),
        ],
    );
    assert_eq!(result.len(), 0);
    assert!(!pending);
}
