use super::{KeyBindingContextPredicate, KeyContext};
use anyhow::{Context as _, Result};

impl KeyContext {
    /// Parse a key context from a string.
    /// The key context format is very simple:
    /// - either a single identifier, such as `StatusBar`
    /// - or a key value pair, such as `mode = visible`
    /// - separated by whitespace, such as `StatusBar mode = visible`
    pub fn parse(source: &str) -> Result<Self> {
        let mut context = Self::default();
        let source = skip_whitespace(source);
        Self::parse_expr(source, &mut context)?;
        Ok(context)
    }

    fn parse_expr(mut source: &str, context: &mut Self) -> Result<()> {
        if source.is_empty() {
            return Ok(());
        }

        let key = source
            .chars()
            .take_while(|c| is_identifier_char(*c))
            .collect::<String>();
        source = skip_whitespace(&source[key.len()..]);
        if let Some(suffix) = source.strip_prefix('=') {
            source = skip_whitespace(suffix);
            let value = source
                .chars()
                .take_while(|c| is_identifier_char(*c))
                .collect::<String>();
            source = skip_whitespace(&source[value.len()..]);
            context.set(key, value);
        } else {
            context.add(key);
        }

        Self::parse_expr(source, context)
    }
}

impl KeyBindingContextPredicate {
    /// Parse a string in the same format as the keymap's context field.
    ///
    /// A basic equivalence check against a set of identifiers can performed by
    /// simply writing a string:
    ///
    /// `StatusBar` -> A predicate that will match a context with the identifier `StatusBar`
    ///
    /// You can also specify a key-value pair:
    ///
    /// `mode == visible` -> A predicate that will match a context with the key `mode`
    ///                      with the value `visible`
    ///
    /// And a logical operations combining these two checks:
    ///
    /// `StatusBar && mode == visible` -> A predicate that will match a context with the
    ///                                   identifier `StatusBar` and the key `mode`
    ///                                   with the value `visible`
    ///
    ///
    /// There is also a special child `>` operator that will match a predicate that is
    /// below another predicate:
    ///
    /// `StatusBar > mode == visible` -> A predicate that will match a context identifier `StatusBar`
    ///                                  and a child context that has the key `mode` with the
    ///                                  value `visible`
    ///
    /// This syntax supports `!=`, `||` and `&&` as logical operators.
    /// You can also preface an operation or check with a `!` to negate it.
    pub fn parse(source: &str) -> Result<Self> {
        let source = skip_whitespace(source);
        let (predicate, rest) = Self::parse_expr(source, 0)?;
        if let Some(next) = rest.chars().next() {
            anyhow::bail!("unexpected character '{next:?}'");
        } else {
            Ok(predicate)
        }
    }

    fn parse_expr(mut source: &str, min_precedence: u32) -> anyhow::Result<(Self, &str)> {
        type Op = fn(
            KeyBindingContextPredicate,
            KeyBindingContextPredicate,
        ) -> Result<KeyBindingContextPredicate>;

        let (mut predicate, rest) = Self::parse_primary(source)?;
        source = rest;

        'parse: loop {
            for (operator, precedence, constructor) in [
                (">", PRECEDENCE_CHILD, Self::new_child as Op),
                ("&&", PRECEDENCE_AND, Self::new_and as Op),
                ("||", PRECEDENCE_OR, Self::new_or as Op),
                ("==", PRECEDENCE_EQ, Self::new_eq as Op),
                ("!=", PRECEDENCE_EQ, Self::new_neq as Op),
            ] {
                if source.starts_with(operator) && precedence >= min_precedence {
                    source = skip_whitespace(&source[operator.len()..]);
                    let (right, rest) = Self::parse_expr(source, precedence + 1)?;
                    predicate = constructor(predicate, right)?;
                    source = rest;
                    continue 'parse;
                }
            }
            break;
        }

        Ok((predicate, source))
    }

    fn parse_primary(mut source: &str) -> anyhow::Result<(Self, &str)> {
        let next = source.chars().next().context("unexpected end")?;
        match next {
            '(' => {
                source = skip_whitespace(&source[1..]);
                let (predicate, rest) = Self::parse_expr(source, 0)?;
                let stripped = rest.strip_prefix(')').context("expected a ')'")?;
                source = skip_whitespace(stripped);
                Ok((predicate, source))
            }
            '!' => {
                let source = skip_whitespace(&source[1..]);
                let (predicate, source) = Self::parse_expr(source, PRECEDENCE_NOT)?;
                Ok((KeyBindingContextPredicate::Not(Box::new(predicate)), source))
            }
            _ if is_identifier_char(next) => {
                let len = source
                    .find(|c: char| !is_identifier_char(c) && !is_vim_operator_char(c))
                    .unwrap_or(source.len());
                let (identifier, rest) = source.split_at(len);
                source = skip_whitespace(rest);
                Ok((
                    KeyBindingContextPredicate::Identifier(identifier.to_string().into()),
                    source,
                ))
            }
            _ if is_vim_operator_char(next) => {
                let (operator, rest) = source.split_at(1);
                source = skip_whitespace(rest);
                Ok((
                    KeyBindingContextPredicate::Identifier(operator.to_string().into()),
                    source,
                ))
            }
            _ => anyhow::bail!("unexpected character '{next:?}'"),
        }
    }

    fn new_or(self, other: Self) -> Result<Self> {
        Ok(Self::Or(Box::new(self), Box::new(other)))
    }

    fn new_and(self, other: Self) -> Result<Self> {
        Ok(Self::And(Box::new(self), Box::new(other)))
    }

    fn new_child(self, other: Self) -> Result<Self> {
        Ok(Self::Descendant(Box::new(self), Box::new(other)))
    }

    fn new_eq(self, other: Self) -> Result<Self> {
        if let (Self::Identifier(left), Self::Identifier(right)) = (self, other) {
            Ok(Self::Equal(left, right))
        } else {
            anyhow::bail!("operands of == must be identifiers");
        }
    }

    fn new_neq(self, other: Self) -> Result<Self> {
        if let (Self::Identifier(left), Self::Identifier(right)) = (self, other) {
            Ok(Self::NotEqual(left, right))
        } else {
            anyhow::bail!("operands of != must be identifiers");
        }
    }
}

const PRECEDENCE_CHILD: u32 = 1;
const PRECEDENCE_OR: u32 = 2;
const PRECEDENCE_AND: u32 = 3;
const PRECEDENCE_EQ: u32 = 4;
const PRECEDENCE_NOT: u32 = 5;

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn is_vim_operator_char(c: char) -> bool {
    c == '>' || c == '<' || c == '~' || c == '"' || c == '?'
}

fn skip_whitespace(source: &str) -> &str {
    let len = source
        .find(|c: char| !c.is_whitespace())
        .unwrap_or(source.len());
    &source[len..]
}
