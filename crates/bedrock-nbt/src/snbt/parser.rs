use crate::{Error, NbtTag, Result, constants::*, tag::discriminant};
use indexmap::IndexMap;

/// Parses one complete SNBT value, including scalar and array roots.
///
/// # Errors
/// Rejects malformed syntax, heterogeneous lists, mismatched typed arrays,
/// trailing input, strings over 65535 bytes, input over 32 MiB and depth over 128.
pub fn parse(input: &str) -> Result<NbtTag> {
    let mut parser = Parser { input, offset: 0 };
    if input.len() > MAX_NBT_BYTE_LENGTH {
        return Err(parser.error("SNBT input exceeds byte limit"));
    }
    let value = parser.value(0)?;
    parser.whitespace();
    if parser.offset != input.len() {
        return Err(parser.error("unexpected trailing input"));
    }
    Ok(value)
}

struct Parser<'a> {
    input: &'a str,
    offset: usize,
}

impl Parser<'_> {
    fn value(&mut self, depth: usize) -> Result<NbtTag> {
        if depth > MAX_NBT_DEPTH {
            return Err(self.error("SNBT nesting exceeds depth limit"));
        }
        self.whitespace();
        match self.peek() {
            Some(b'{') => self.compound(depth),
            Some(b'[') => self.list(depth),
            Some(b'\'' | b'"') => self.quoted().map(NbtTag::String),
            _ => {
                let token = self.token()?;
                Ok(number(token).unwrap_or_else(|| NbtTag::String(token.to_string())))
            }
        }
    }

    fn compound(&mut self, depth: usize) -> Result<NbtTag> {
        self.offset += 1;
        let mut values = IndexMap::new();
        if self.consume(b'}') {
            return Ok(NbtTag::Compound(values));
        }
        loop {
            self.whitespace();
            let name = match self.peek() {
                Some(b'\'' | b'"') => self.quoted()?,
                _ => self.token()?.to_string(),
            };
            self.expect(b':')?;
            let value = self.value(depth + 1)?;
            values.insert(name, value);
            if self.consume(b'}') {
                break;
            }
            self.expect(b',')?;
            if self.consume(b'}') {
                break;
            }
        }
        Ok(NbtTag::Compound(values))
    }

    fn list(&mut self, depth: usize) -> Result<NbtTag> {
        self.offset += 1;
        self.whitespace();
        let kind = self
            .peek()
            .filter(|byte| matches!(byte, b'B' | b'I' | b'L'));
        if let Some(kind) =
            kind.filter(|_| self.input.as_bytes().get(self.offset + 1) == Some(&b';'))
        {
            self.offset += 2;
            return self.typed_array(kind, depth);
        }
        let mut values = Vec::new();
        if self.consume(b']') {
            return Ok(NbtTag::List(values));
        }
        loop {
            let value = self.value(depth + 1)?;
            if values
                .first()
                .is_some_and(|first| discriminant(first) != discriminant(&value))
            {
                return Err(self.error("List elements must have the same NBT type"));
            }
            values.push(value);
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
            if self.consume(b']') {
                break;
            }
        }
        Ok(NbtTag::List(values))
    }

    fn typed_array(&mut self, kind: u8, depth: usize) -> Result<NbtTag> {
        match kind {
            b'B' => self
                .array_values(depth, |tag| match tag {
                    NbtTag::Byte(value) => Some(value),
                    _ => None,
                })
                .map(NbtTag::ByteArray),
            b'I' => self
                .array_values(depth, |tag| match tag {
                    NbtTag::Int(value) => Some(value),
                    _ => None,
                })
                .map(NbtTag::IntArray),
            _ => self
                .array_values(depth, |tag| match tag {
                    NbtTag::Long(value) => Some(value),
                    _ => None,
                })
                .map(NbtTag::LongArray),
        }
    }

    fn array_values<T>(
        &mut self,
        depth: usize,
        convert: fn(NbtTag) -> Option<T>,
    ) -> Result<Vec<T>> {
        let mut values = Vec::new();
        if self.consume(b']') {
            return Ok(values);
        }
        loop {
            let value = convert(self.value(depth)?)
                .ok_or_else(|| self.error("typed array element has the wrong NBT type"))?;
            values.push(value);
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
            if self.consume(b']') {
                break;
            }
        }
        Ok(values)
    }

    fn quoted(&mut self) -> Result<String> {
        let quote = self
            .peek()
            .ok_or_else(|| self.error("expected quoted string"))?;
        self.offset += 1;
        let mut output = String::new();
        loop {
            let start = self.offset;
            self.offset += super::scan::string_run(&self.input.as_bytes()[start..], quote);
            output.push_str(&self.input[start..self.offset]);
            if output.len() > MAX_NBT_STRING_BYTES {
                return Err(self.error("string exceeds byte limit"));
            }
            match self.peek() {
                Some(byte) if byte == quote => {
                    self.offset += 1;
                    return Ok(output);
                }
                Some(b'\\') => {
                    self.offset += 1;
                    let byte = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated string escape"))?;
                    if byte != quote && byte != b'\\' {
                        return Err(self.error("invalid string escape"));
                    }
                    output.push(byte as char);
                    self.offset += 1;
                }
                _ => return Err(self.error("unterminated quoted string")),
            }
        }
    }

    fn token(&mut self) -> Result<&str> {
        let start = self.offset;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
        {
            self.offset += 1;
        }
        if start == self.offset {
            return Err(self.error("expected unquoted token or quoted string"));
        }
        if self.offset - start > MAX_NBT_STRING_BYTES {
            return Err(self.error("token exceeds byte limit"));
        }
        Ok(&self.input[start..self.offset])
    }

    fn whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.offset += 1;
        }
    }

    fn consume(&mut self, byte: u8) -> bool {
        self.whitespace();
        if self.peek() != Some(byte) {
            return false;
        }
        self.offset += 1;
        true
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        if self.consume(byte) {
            Ok(())
        } else {
            Err(self.error(&format!("expected '{}'", byte as char)))
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.offset).copied()
    }

    fn error(&self, message: &str) -> Error {
        Error::Snbt {
            offset: self.offset,
            message: message.into(),
        }
    }
}

fn number(token: &str) -> Option<NbtTag> {
    if token.eq_ignore_ascii_case("true") {
        return Some(NbtTag::Byte(1));
    }
    if token.eq_ignore_ascii_case("false") {
        return Some(NbtTag::Byte(0));
    }
    let (&last, body) = token.as_bytes().split_last()?;
    let body = std::str::from_utf8(body).ok()?;
    match last.to_ascii_lowercase() {
        b'b' => body.parse().ok().map(NbtTag::Byte),
        b's' => body.parse().ok().map(NbtTag::Short),
        b'l' => body.parse().ok().map(NbtTag::Long),
        b'f' => body.parse().ok().map(NbtTag::Float),
        b'd' => body.parse().ok().map(NbtTag::Double),
        _ => token.parse().ok().map(NbtTag::Int).or_else(|| {
            // Unsuffixed non-integer numbers default to Double; overflowing
            // integers remain strings instead of being rounded through f64.
            token
                .contains(['.', 'e', 'E'])
                .then(|| token.parse().ok().map(NbtTag::Double))
                .flatten()
        }),
    }
}
