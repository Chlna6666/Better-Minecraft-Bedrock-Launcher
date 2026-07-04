use super::KeyContext;
use crate::SharedString;
use std::fmt;

/// A datastructure for resolving whether an action should be dispatched
/// Representing a small language for describing which contexts correspond
/// to which actions.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum KeyBindingContextPredicate {
    /// A predicate that will match a given identifier.
    Identifier(SharedString),
    /// A predicate that will match a given key-value pair.
    Equal(SharedString, SharedString),
    /// A predicate that will match a given key-value pair not being present.
    NotEqual(SharedString, SharedString),
    /// A predicate that will match a given predicate appearing below another predicate.
    /// in the element tree
    Descendant(
        Box<KeyBindingContextPredicate>,
        Box<KeyBindingContextPredicate>,
    ),
    /// Predicate that will invert another predicate.
    Not(Box<KeyBindingContextPredicate>),
    /// A predicate that will match if both of its children match.
    And(
        Box<KeyBindingContextPredicate>,
        Box<KeyBindingContextPredicate>,
    ),
    /// A predicate that will match if either of its children match.
    Or(
        Box<KeyBindingContextPredicate>,
        Box<KeyBindingContextPredicate>,
    ),
}

impl fmt::Display for KeyBindingContextPredicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Identifier(name) => write!(f, "{}", name),
            Self::Equal(left, right) => write!(f, "{} == {}", left, right),
            Self::NotEqual(left, right) => write!(f, "{} != {}", left, right),
            Self::Not(pred) => write!(f, "!{}", pred),
            Self::Descendant(parent, child) => write!(f, "{} > {}", parent, child),
            Self::And(left, right) => write!(f, "({} && {})", left, right),
            Self::Or(left, right) => write!(f, "({} || {})", left, right),
        }
    }
}

impl KeyBindingContextPredicate {
    /// Find the deepest depth at which the predicate matches.
    pub fn depth_of(&self, contexts: &[KeyContext]) -> Option<usize> {
        for depth in (0..=contexts.len()).rev() {
            let context_slice = &contexts[0..depth];
            if self.eval_inner(context_slice, contexts) {
                return Some(depth);
            }
        }
        None
    }

    /// Eval a predicate against a set of contexts, arranged from lowest to highest.
    #[allow(unused)]
    pub(crate) fn eval(&self, contexts: &[KeyContext]) -> bool {
        self.eval_inner(contexts, contexts)
    }

    /// Eval a predicate against a set of contexts, arranged from lowest to highest.
    pub fn eval_inner(&self, contexts: &[KeyContext], all_contexts: &[KeyContext]) -> bool {
        let Some(context) = contexts.last() else {
            return false;
        };
        match self {
            Self::Identifier(name) => context.contains(name),
            Self::Equal(left, right) => context
                .get(left)
                .map(|value| value == right)
                .unwrap_or(false),
            Self::NotEqual(left, right) => context
                .get(left)
                .map(|value| value != right)
                .unwrap_or(true),
            Self::Not(pred) => {
                for i in 0..all_contexts.len() {
                    if pred.eval_inner(&all_contexts[..=i], all_contexts) {
                        return false;
                    }
                }
                true
            }
            // Workspace > Pane > Editor
            //
            // Pane > (Pane > Editor) // should match?
            // (Pane > Pane) > Editor // should not match?
            // Pane > !Workspace <-- should match?
            // !Workspace        <-- shouldn't match?
            Self::Descendant(parent, child) => {
                for i in 0..contexts.len() - 1 {
                    // [Workspace >  Pane], [Editor]
                    if parent.eval_inner(&contexts[..=i], all_contexts) {
                        if !child.eval_inner(&contexts[i + 1..], &contexts[i + 1..]) {
                            return false;
                        }
                        return true;
                    }
                }
                false
            }
            Self::And(left, right) => {
                left.eval_inner(contexts, all_contexts) && right.eval_inner(contexts, all_contexts)
            }
            Self::Or(left, right) => {
                left.eval_inner(contexts, all_contexts) || right.eval_inner(contexts, all_contexts)
            }
        }
    }

    /// Returns whether or not this predicate matches all possible contexts matched by
    /// the other predicate.
    pub fn is_superset(&self, other: &Self) -> bool {
        if self == other {
            return true;
        }

        if let KeyBindingContextPredicate::Or(left, right) = self {
            return left.is_superset(other) || right.is_superset(other);
        }

        match other {
            KeyBindingContextPredicate::Descendant(_, child) => self.is_superset(child),
            KeyBindingContextPredicate::And(left, right) => {
                self.is_superset(left) || self.is_superset(right)
            }
            KeyBindingContextPredicate::Identifier(_) => false,
            KeyBindingContextPredicate::Equal(_, _) => false,
            KeyBindingContextPredicate::NotEqual(_, _) => false,
            KeyBindingContextPredicate::Not(_) => false,
            KeyBindingContextPredicate::Or(_, _) => false,
        }
    }
}
