use crate::material::BlockComponents;
use crate::state::BlockStateQuery;

#[derive(Clone, Debug, PartialEq)]
pub struct BlockPermutation {
    pub condition: String,
    pub components: BlockComponents,
}

impl BlockPermutation {
    #[must_use]
    pub fn matches(&self, state: &BlockStateQuery) -> ConditionEvaluation {
        evaluate_condition(&self.condition, state)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConditionEvaluation {
    Matched,
    NotMatched,
    Unsupported(String),
}

#[must_use]
pub fn evaluate_condition(condition: &str, state: &BlockStateQuery) -> ConditionEvaluation {
    let trimmed = strip_outer_parentheses(condition.trim());
    if trimmed.is_empty() {
        return ConditionEvaluation::Matched;
    }

    if let Some(parts) = split_top_level(trimmed, "||") {
        let mut unsupported = Vec::new();
        for part in parts {
            match evaluate_condition(part, state) {
                ConditionEvaluation::Matched => return ConditionEvaluation::Matched,
                ConditionEvaluation::NotMatched => {}
                ConditionEvaluation::Unsupported(reason) => unsupported.push(reason),
            }
        }
        if unsupported.is_empty() {
            ConditionEvaluation::NotMatched
        } else {
            ConditionEvaluation::Unsupported(unsupported.join("; "))
        }
    } else if let Some(parts) = split_top_level(trimmed, "&&") {
        let mut unsupported = Vec::new();
        for part in parts {
            match evaluate_condition(part, state) {
                ConditionEvaluation::Matched => {}
                ConditionEvaluation::NotMatched => return ConditionEvaluation::NotMatched,
                ConditionEvaluation::Unsupported(reason) => unsupported.push(reason),
            }
        }
        if unsupported.is_empty() {
            ConditionEvaluation::Matched
        } else {
            ConditionEvaluation::Unsupported(unsupported.join("; "))
        }
    } else if let Some(rest) = trimmed.strip_prefix('!') {
        match evaluate_condition(rest, state) {
            ConditionEvaluation::Matched => ConditionEvaluation::NotMatched,
            ConditionEvaluation::NotMatched => ConditionEvaluation::Matched,
            unsupported @ ConditionEvaluation::Unsupported(_) => unsupported,
        }
    } else {
        evaluate_atom(trimmed, state)
    }
}

fn evaluate_atom(condition: &str, state: &BlockStateQuery) -> ConditionEvaluation {
    if let Some((left, right)) = split_comparison(condition, "==") {
        return compare_block_state(left, right, state, true);
    }

    if let Some((left, right)) = split_comparison(condition, "!=") {
        return compare_block_state(left, right, state, false);
    }

    if let Some(state_name) = block_state_call_argument(condition) {
        return state
            .state(&state_name)
            .map_or(ConditionEvaluation::NotMatched, |value| {
                if value.is_truthy() {
                    ConditionEvaluation::Matched
                } else {
                    ConditionEvaluation::NotMatched
                }
            });
    }

    ConditionEvaluation::Unsupported(format!("unsupported permutation condition `{condition}`"))
}

fn compare_block_state(
    left: &str,
    right: &str,
    state: &BlockStateQuery,
    equals: bool,
) -> ConditionEvaluation {
    let Some(state_name) = block_state_call_argument(left) else {
        return ConditionEvaluation::Unsupported(format!(
            "left side is not a block_state call `{left}`"
        ));
    };
    let literal = strip_literal(right.trim());
    let matched = state
        .state(&state_name)
        .is_some_and(|value| value.matches_literal(&literal));

    if matched == equals {
        ConditionEvaluation::Matched
    } else {
        ConditionEvaluation::NotMatched
    }
}

fn split_comparison<'a>(condition: &'a str, operator: &str) -> Option<(&'a str, &'a str)> {
    let parts = split_top_level(condition, operator)?;
    if parts.len() == 2 {
        Some((parts[0], parts[1]))
    } else {
        None
    }
}

fn block_state_call_argument(condition: &str) -> Option<String> {
    let trimmed = condition.trim();
    let prefix = trimmed
        .strip_prefix("query.block_state")
        .or_else(|| trimmed.strip_prefix("q.block_state"))?;
    let argument = prefix.trim();
    let argument = argument.strip_prefix('(')?.strip_suffix(')')?.trim();
    Some(strip_literal(argument))
}

fn strip_literal(value: &str) -> String {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|item| item.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|item| item.strip_suffix('\''))
        })
        .unwrap_or(trimmed)
        .to_owned()
}

fn split_top_level<'a>(value: &'a str, delimiter: &str) -> Option<Vec<&'a str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0;
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;
    let mut index = 0;

    while index < value.len() {
        let Some(character) = value[index..].chars().next() else {
            break;
        };
        let character_len = character.len_utf8();

        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == quote {
                in_string = false;
            }
            index += character_len;
            continue;
        }

        match character {
            '"' | '\'' => {
                in_string = true;
                quote = character;
            }
            '(' => depth += 1,
            ')' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {
                if depth == 0 && value[index..].starts_with(delimiter) {
                    parts.push(value[start..index].trim());
                    index += delimiter.len();
                    start = index;
                    continue;
                }
            }
        }

        index += character_len;
    }

    if parts.is_empty() {
        None
    } else {
        parts.push(value[start..].trim());
        Some(parts)
    }
}

fn strip_outer_parentheses(value: &str) -> &str {
    let mut trimmed = value;
    loop {
        let Some(inner) = trimmed
            .strip_prefix('(')
            .and_then(|item| item.strip_suffix(')'))
        else {
            return trimmed;
        };
        if has_balanced_outer_parentheses(trimmed) {
            trimmed = inner.trim();
        } else {
            return trimmed;
        }
    }
}

fn has_balanced_outer_parentheses(value: &str) -> bool {
    let mut depth = 0;
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;

    for (index, character) in value.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == quote {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' | '\'' => {
                in_string = true;
                quote = character;
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 && index + character.len_utf8() < value.len() {
                    return false;
                }
            }
            _ => {}
        }
    }

    depth == 0
}

#[cfg(test)]
mod tests {
    use super::{ConditionEvaluation, evaluate_condition};
    use crate::state::BlockStateQuery;

    #[test]
    fn evaluate_condition_should_match_string_block_state() {
        let state = BlockStateQuery::new("minecraft:oak_trapdoor")
            .with_state("minecraft:cardinal_direction", "north")
            .with_state("open_bit", true);

        let result = evaluate_condition(
            "query.block_state('minecraft:cardinal_direction') == 'north' && q.block_state('open_bit')",
            &state,
        );

        assert_eq!(result, ConditionEvaluation::Matched);
    }
}
