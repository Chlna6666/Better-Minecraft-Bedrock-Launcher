use super::*;

#[test]
fn test_depth_precedence() {
    let bindings = [
        KeyBinding::new("ctrl-a", ActionBeta {}, Some("pane")),
        KeyBinding::new("ctrl-a", ActionGamma {}, Some("editor")),
    ];

    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    let (result, pending) = keymap.bindings_for_input(
        &[Keystroke::parse("ctrl-a").unwrap()],
        &[
            KeyContext::parse("pane").unwrap(),
            KeyContext::parse("editor").unwrap(),
        ],
    );

    assert!(!pending);
    assert_eq!(result.len(), 2);
    assert!(result[0].action.partial_eq(&ActionGamma {}));
    assert!(result[1].action.partial_eq(&ActionBeta {}));
}

#[test]
fn test_pending_match_enabled() {
    let bindings = [
        KeyBinding::new("ctrl-x", ActionBeta, Some("vim_mode == normal")),
        KeyBinding::new("ctrl-x 0", ActionAlpha, Some("Workspace")),
    ];
    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    let matched = keymap.bindings_for_input(
        &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
        &[
            KeyContext::parse("Workspace"),
            KeyContext::parse("Pane"),
            KeyContext::parse("Editor vim_mode=normal"),
        ]
        .map(Result::unwrap),
    );
    assert_eq!(matched.0.len(), 1);
    assert!(matched.0[0].action.partial_eq(&ActionBeta));
    assert!(matched.1);
}

#[test]
fn test_pending_match_enabled_extended() {
    let bindings = [
        KeyBinding::new("ctrl-x", ActionBeta, Some("vim_mode == normal")),
        KeyBinding::new("ctrl-x 0", NoAction, Some("Workspace")),
    ];
    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    let matched = keymap.bindings_for_input(
        &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
        &[
            KeyContext::parse("Workspace"),
            KeyContext::parse("Pane"),
            KeyContext::parse("Editor vim_mode=normal"),
        ]
        .map(Result::unwrap),
    );
    assert_eq!(matched.0.len(), 1);
    assert!(matched.0[0].action.partial_eq(&ActionBeta));
    assert!(!matched.1);
    let bindings = [
        KeyBinding::new("ctrl-x", ActionBeta, Some("Workspace")),
        KeyBinding::new("ctrl-x 0", NoAction, Some("vim_mode == normal")),
    ];
    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    let matched = keymap.bindings_for_input(
        &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
        &[
            KeyContext::parse("Workspace"),
            KeyContext::parse("Pane"),
            KeyContext::parse("Editor vim_mode=normal"),
        ]
        .map(Result::unwrap),
    );
    assert_eq!(matched.0.len(), 1);
    assert!(matched.0[0].action.partial_eq(&ActionBeta));
    assert!(!matched.1);
}

#[test]
fn test_overriding_prefix() {
    let bindings = [
        KeyBinding::new("ctrl-x 0", ActionAlpha, Some("Workspace")),
        KeyBinding::new("ctrl-x", ActionBeta, Some("vim_mode == normal")),
    ];
    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    let matched = keymap.bindings_for_input(
        &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
        &[
            KeyContext::parse("Workspace"),
            KeyContext::parse("Pane"),
            KeyContext::parse("Editor vim_mode=normal"),
        ]
        .map(Result::unwrap),
    );
    assert_eq!(matched.0.len(), 1);
    assert!(matched.0[0].action.partial_eq(&ActionBeta));
    assert!(!matched.1);
}

#[test]
fn test_context_precedence_with_same_source() {
    // Test case: User has both Workspace and Editor bindings for the same key
    // Editor binding should take precedence over Workspace binding
    let bindings = [
        KeyBinding::new("cmd-r", ActionAlpha {}, Some("Workspace")),
        KeyBinding::new("cmd-r", ActionBeta {}, Some("Editor")),
    ];

    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    // Test with context stack: [Workspace, Editor] (Editor is deeper)
    let (result, _) = keymap.bindings_for_input(
        &[Keystroke::parse("cmd-r").unwrap()],
        &[
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Editor").unwrap(),
        ],
    );

    // Both bindings should be returned, but Editor binding should be first (highest precedence)
    assert_eq!(result.len(), 2);
    assert!(result[0].action.partial_eq(&ActionBeta {})); // Editor binding first
    assert!(result[1].action.partial_eq(&ActionAlpha {})); // Workspace binding second
}

#[test]
fn test_bindings_for_action() {
    let bindings = [
        KeyBinding::new("ctrl-a", ActionAlpha {}, Some("pane")),
        KeyBinding::new("ctrl-b", ActionBeta {}, Some("editor && mode == full")),
        KeyBinding::new("ctrl-c", ActionGamma {}, Some("workspace")),
        KeyBinding::new("ctrl-a", NoAction {}, Some("pane && active")),
        KeyBinding::new("ctrl-b", NoAction {}, Some("editor")),
    ];

    let mut keymap = Keymap::default();
    keymap.add_bindings(bindings);

    assert_bindings(&keymap, &ActionAlpha {}, &["ctrl-a"]);
    assert_bindings(&keymap, &ActionBeta {}, &[]);
    assert_bindings(&keymap, &ActionGamma {}, &["ctrl-c"]);

    #[track_caller]
    fn assert_bindings(keymap: &Keymap, action: &dyn Action, expected: &[&str]) {
        let actual = keymap
            .bindings_for_action(action)
            .map(|binding| binding.keystrokes[0].inner().unparse())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "{:?}", action);
    }
}

#[test]
fn test_source_precedence_sorting() {
    // KeybindSource precedence: User (0) > Vim (1) > Base (2) > Default (3)
    // Test that user keymaps take precedence over default keymaps at the same context depth
    let mut keymap = Keymap::default();

    // Add a default keymap binding first
    let mut default_binding = KeyBinding::new("cmd-r", ActionAlpha {}, Some("Editor"));
    default_binding.set_meta(KeyBindingMetaIndex(3)); // Default source
    keymap.add_bindings([default_binding]);

    // Add a user keymap binding
    let mut user_binding = KeyBinding::new("cmd-r", ActionBeta {}, Some("Editor"));
    user_binding.set_meta(KeyBindingMetaIndex(0)); // User source
    keymap.add_bindings([user_binding]);

    // Test with Editor context stack
    let (result, _) = keymap.bindings_for_input(
        &[Keystroke::parse("cmd-r").unwrap()],
        &[KeyContext::parse("Editor").unwrap()],
    );

    // User binding should take precedence over default binding
    assert_eq!(result.len(), 2);
    assert!(result[0].action.partial_eq(&ActionBeta {}));
    assert!(result[1].action.partial_eq(&ActionAlpha {}));
}
