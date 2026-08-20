use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::error::{BlockModelError, Result};

pub fn read_json_file(path: &Path) -> Result<Value> {
    let content = fs::read_to_string(path).map_err(|source| BlockModelError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let relaxed_content = strip_json_comments_and_trailing_commas(&content);
    serde_json::from_str(&relaxed_content).map_err(|source| BlockModelError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn strip_json_comments_and_trailing_commas(content: &str) -> String {
    let mut without_comments = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(character) = chars.next() {
        if in_string {
            without_comments.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        if character == '"' {
            in_string = true;
            without_comments.push(character);
            continue;
        }

        if character == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    for next_character in chars.by_ref() {
                        if next_character == '\n' {
                            without_comments.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for next_character in chars.by_ref() {
                        if previous == '*' && next_character == '/' {
                            break;
                        }
                        if next_character == '\n' {
                            without_comments.push('\n');
                        }
                        previous = next_character;
                    }
                    continue;
                }
                _ => {}
            }
        }

        without_comments.push(character);
    }

    remove_trailing_commas(&without_comments)
}

fn remove_trailing_commas(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(character) = chars.next() {
        if in_string {
            output.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        if character == '"' {
            in_string = true;
            output.push(character);
            continue;
        }

        if character == ',' {
            let mut lookahead = chars.clone();
            while matches!(lookahead.peek(), Some(next) if next.is_whitespace()) {
                lookahead.next();
            }
            if matches!(lookahead.peek(), Some('}' | ']')) {
                continue;
            }
        }

        output.push(character);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::strip_json_comments_and_trailing_commas;

    #[test]
    fn relaxed_json_should_keep_comment_like_text_inside_strings() {
        let json = r#"{ "path": "textures/blocks", // comment
            "items": [1, 2,],
        }"#;

        let stripped = strip_json_comments_and_trailing_commas(json);

        assert!(stripped.contains("\"textures/blocks\""));
        assert!(!stripped.contains("// comment"));
        assert!(stripped.contains("[1, 2]"));
    }
}
