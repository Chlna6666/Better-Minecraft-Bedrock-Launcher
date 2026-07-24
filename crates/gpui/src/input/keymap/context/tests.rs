use core::slice;

use super::*;
use crate as gpui;
use KeyBindingContextPredicate::*;

#[test]
fn test_actions_definition() {
    {
        actions!(test_only, [A, B, C, D, E, F, G]);
    }

    {
        actions!(
            test_only,
            [
                H, I, J, K, L, M, N, // Don't wrap, test the trailing comma
            ]
        );
    }
}

#[test]
fn test_parse_context() {
    let mut expected = KeyContext::default();
    expected.add("baz");
    expected.set("foo", "bar");
    assert_eq!(KeyContext::parse("baz foo=bar").unwrap(), expected);
    assert_eq!(KeyContext::parse("baz foo = bar").unwrap(), expected);
    assert_eq!(
        KeyContext::parse("  baz foo   =   bar baz").unwrap(),
        expected
    );
    assert_eq!(KeyContext::parse(" baz foo = bar").unwrap(), expected);
}

#[test]
fn test_parse_identifiers() {
    // Identifiers
    assert_eq!(
        KeyBindingContextPredicate::parse("abc12").unwrap(),
        Identifier("abc12".into())
    );
    assert_eq!(
        KeyBindingContextPredicate::parse("_1a").unwrap(),
        Identifier("_1a".into())
    );
}

#[test]
fn test_parse_negations() {
    assert_eq!(
        KeyBindingContextPredicate::parse("!abc").unwrap(),
        Not(Box::new(Identifier("abc".into())))
    );
    assert_eq!(
        KeyBindingContextPredicate::parse(" ! ! abc").unwrap(),
        Not(Box::new(Not(Box::new(Identifier("abc".into())))))
    );
}

#[test]
fn test_parse_equality_operators() {
    assert_eq!(
        KeyBindingContextPredicate::parse("a == b").unwrap(),
        Equal("a".into(), "b".into())
    );
    assert_eq!(
        KeyBindingContextPredicate::parse("c!=d").unwrap(),
        NotEqual("c".into(), "d".into())
    );
    assert_eq!(
        KeyBindingContextPredicate::parse("c == !d")
            .unwrap_err()
            .to_string(),
        "operands of == must be identifiers"
    );
}

#[test]
fn test_parse_boolean_operators() {
    assert_eq!(
        KeyBindingContextPredicate::parse("a || b").unwrap(),
        Or(
            Box::new(Identifier("a".into())),
            Box::new(Identifier("b".into()))
        )
    );
    assert_eq!(
        KeyBindingContextPredicate::parse("a || !b && c").unwrap(),
        Or(
            Box::new(Identifier("a".into())),
            Box::new(And(
                Box::new(Not(Box::new(Identifier("b".into())))),
                Box::new(Identifier("c".into()))
            ))
        )
    );
    assert_eq!(
        KeyBindingContextPredicate::parse("a && b || c&&d").unwrap(),
        Or(
            Box::new(And(
                Box::new(Identifier("a".into())),
                Box::new(Identifier("b".into()))
            )),
            Box::new(And(
                Box::new(Identifier("c".into())),
                Box::new(Identifier("d".into()))
            ))
        )
    );
    assert_eq!(
        KeyBindingContextPredicate::parse("a == b && c || d == e && f").unwrap(),
        Or(
            Box::new(And(
                Box::new(Equal("a".into(), "b".into())),
                Box::new(Identifier("c".into()))
            )),
            Box::new(And(
                Box::new(Equal("d".into(), "e".into())),
                Box::new(Identifier("f".into()))
            ))
        )
    );
    assert_eq!(
        KeyBindingContextPredicate::parse("a && b && c && d").unwrap(),
        And(
            Box::new(And(
                Box::new(And(
                    Box::new(Identifier("a".into())),
                    Box::new(Identifier("b".into()))
                )),
                Box::new(Identifier("c".into())),
            )),
            Box::new(Identifier("d".into()))
        ),
    );
}

#[test]
fn test_parse_parenthesized_expressions() {
    assert_eq!(
        KeyBindingContextPredicate::parse("a && (b == c || d != e)").unwrap(),
        And(
            Box::new(Identifier("a".into())),
            Box::new(Or(
                Box::new(Equal("b".into(), "c".into())),
                Box::new(NotEqual("d".into(), "e".into())),
            )),
        ),
    );
    assert_eq!(
        KeyBindingContextPredicate::parse(" ( a || b ) ").unwrap(),
        Or(
            Box::new(Identifier("a".into())),
            Box::new(Identifier("b".into())),
        )
    );
}

#[test]
fn test_is_superset() {
    assert_is_superset("editor", "editor", true);
    assert_is_superset("editor", "workspace", false);

    assert_is_superset("editor", "editor && vim_mode", true);
    assert_is_superset("editor", "mode == full && editor", true);
    assert_is_superset("editor && mode == full", "editor", false);

    assert_is_superset("editor", "something > editor", true);
    assert_is_superset("editor", "editor > menu", false);

    assert_is_superset("foo || bar || baz", "bar", true);
    assert_is_superset("foo || bar || baz", "quux", false);

    #[track_caller]
    fn assert_is_superset(a: &str, b: &str, result: bool) {
        let a = KeyBindingContextPredicate::parse(a).unwrap();
        let b = KeyBindingContextPredicate::parse(b).unwrap();
        assert_eq!(a.is_superset(&b), result, "({a:?}).is_superset({b:?})");
    }
}

#[test]
fn test_child_operator() {
    let predicate = KeyBindingContextPredicate::parse("parent > child").unwrap();

    let parent_context = KeyContext::try_from("parent").unwrap();
    let child_context = KeyContext::try_from("child").unwrap();

    let contexts = vec![parent_context.clone(), child_context.clone()];
    assert!(predicate.eval(&contexts));

    let grandparent_context = KeyContext::try_from("grandparent").unwrap();

    let contexts = vec![
        grandparent_context,
        parent_context.clone(),
        child_context.clone(),
    ];
    assert!(predicate.eval(&contexts));

    let other_context = KeyContext::try_from("other").unwrap();

    let contexts = vec![other_context.clone(), child_context.clone()];
    assert!(!predicate.eval(&contexts));

    let contexts = vec![parent_context.clone(), other_context, child_context.clone()];
    assert!(predicate.eval(&contexts));

    assert!(!predicate.eval(&[]));
    assert!(!predicate.eval(slice::from_ref(&child_context)));
    assert!(!predicate.eval(&[parent_context]));

    let zany_predicate = KeyBindingContextPredicate::parse("child > child").unwrap();
    assert!(!zany_predicate.eval(slice::from_ref(&child_context)));
    assert!(zany_predicate.eval(&[child_context.clone(), child_context]));
}

#[test]
fn test_not_operator() {
    let not_predicate = KeyBindingContextPredicate::parse("!editor").unwrap();
    let editor_context = KeyContext::try_from("editor").unwrap();
    let workspace_context = KeyContext::try_from("workspace").unwrap();
    let parent_context = KeyContext::try_from("parent").unwrap();
    let child_context = KeyContext::try_from("child").unwrap();

    assert!(not_predicate.eval(slice::from_ref(&workspace_context)));
    assert!(!not_predicate.eval(slice::from_ref(&editor_context)));
    assert!(!not_predicate.eval(&[editor_context.clone(), workspace_context.clone()]));
    assert!(!not_predicate.eval(&[workspace_context.clone(), editor_context.clone()]));

    let complex_not = KeyBindingContextPredicate::parse("!editor && workspace").unwrap();
    assert!(complex_not.eval(slice::from_ref(&workspace_context)));
    assert!(!complex_not.eval(&[editor_context.clone(), workspace_context.clone()]));

    let not_mode_predicate = KeyBindingContextPredicate::parse("!(mode == full)").unwrap();
    let mut mode_context = KeyContext::default();
    mode_context.set("mode", "full");
    assert!(!not_mode_predicate.eval(&[mode_context.clone()]));

    let mut other_mode_context = KeyContext::default();
    other_mode_context.set("mode", "partial");
    assert!(not_mode_predicate.eval(&[other_mode_context]));

    let not_descendant = KeyBindingContextPredicate::parse("!(parent > child)").unwrap();
    assert!(not_descendant.eval(slice::from_ref(&parent_context)));
    assert!(not_descendant.eval(slice::from_ref(&child_context)));
    assert!(!not_descendant.eval(&[parent_context.clone(), child_context.clone()]));

    let not_descendant = KeyBindingContextPredicate::parse("parent > !child").unwrap();
    assert!(!not_descendant.eval(slice::from_ref(&parent_context)));
    assert!(!not_descendant.eval(slice::from_ref(&child_context)));
    assert!(!not_descendant.eval(&[parent_context, child_context]));

    let double_not = KeyBindingContextPredicate::parse("!!editor").unwrap();
    assert!(double_not.eval(slice::from_ref(&editor_context)));
    assert!(!double_not.eval(slice::from_ref(&workspace_context)));

    // Test complex descendant cases
    let workspace_context = KeyContext::try_from("Workspace").unwrap();
    let pane_context = KeyContext::try_from("Pane").unwrap();
    let editor_context = KeyContext::try_from("Editor").unwrap();

    // Workspace > Pane > Editor
    let workspace_pane_editor = vec![
        workspace_context.clone(),
        pane_context.clone(),
        editor_context.clone(),
    ];

    // Pane > (Pane > Editor) - should not match
    let pane_pane_editor = KeyBindingContextPredicate::parse("Pane > (Pane > Editor)").unwrap();
    assert!(!pane_pane_editor.eval(&workspace_pane_editor));

    let workspace_pane_editor_predicate =
        KeyBindingContextPredicate::parse("Workspace > Pane > Editor").unwrap();
    assert!(workspace_pane_editor_predicate.eval(&workspace_pane_editor));

    // (Pane > Pane) > Editor - should not match
    let pane_pane_then_editor =
        KeyBindingContextPredicate::parse("(Pane > Pane) > Editor").unwrap();
    assert!(!pane_pane_then_editor.eval(&workspace_pane_editor));

    // Pane > !Workspace - should match
    let pane_not_workspace = KeyBindingContextPredicate::parse("Pane > !Workspace").unwrap();
    assert!(pane_not_workspace.eval(&[pane_context.clone(), editor_context.clone()]));
    assert!(!pane_not_workspace.eval(&[pane_context.clone(), workspace_context.clone()]));

    // !Workspace - shouldn't match when Workspace is in the context
    let not_workspace = KeyBindingContextPredicate::parse("!Workspace").unwrap();
    assert!(!not_workspace.eval(slice::from_ref(&workspace_context)));
    assert!(not_workspace.eval(slice::from_ref(&pane_context)));
    assert!(not_workspace.eval(slice::from_ref(&editor_context)));
    assert!(!not_workspace.eval(&workspace_pane_editor));
}
