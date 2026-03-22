use std::collections::HashMap;

use codex_protocol::user_input::ByteRange;
use codex_protocol::user_input::TextElement;
use shlex::Shlex;
use shlex::try_join;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlashCommandUsageErrorKind {
    UnexpectedInlineArgs,
    InvalidInlineArgs,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SlashCommandParseInput<'a> {
    pub(crate) args: &'a str,
    pub(crate) text_elements: &'a [TextElement],
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SlashSerializedText {
    pub(crate) text: String,
    pub(crate) text_elements: Vec<TextElement>,
}

impl SlashSerializedText {
    pub(crate) fn empty() -> Self {
        Self {
            text: String::new(),
            text_elements: Vec::new(),
        }
    }

    pub(crate) fn with_prefix(&self, prefix: &str) -> Self {
        if self.text.is_empty() {
            return Self {
                text: prefix.to_string(),
                text_elements: Vec::new(),
            };
        }

        let offset = prefix.len() + 1;
        Self {
            text: format!("{prefix} {}", self.text),
            text_elements: shift_text_elements_right(&self.text_elements, offset),
        }
    }

    #[allow(dead_code)]
    fn prepend_inline(&self, prefix: &str) -> Self {
        if prefix.is_empty() {
            return self.clone();
        }

        Self {
            text: format!("{prefix}{}", self.text),
            text_elements: shift_text_elements_right(&self.text_elements, prefix.len()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SlashTokenArg {
    pub(crate) text: String,
    pub(crate) text_elements: Vec<TextElement>,
}

impl SlashTokenArg {
    pub(crate) fn new(text: String, text_elements: Vec<TextElement>) -> Self {
        Self {
            text,
            text_elements,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SlashTextArg {
    pub(crate) text: String,
    pub(crate) text_elements: Vec<TextElement>,
}

impl SlashTextArg {
    pub(crate) fn new(text: String, text_elements: Vec<TextElement>) -> Self {
        Self {
            text,
            text_elements,
        }
    }
}

pub(crate) trait SlashTokenValue: Sized {
    fn parse_token(token: SlashTokenArg) -> Result<Self, SlashCommandUsageErrorKind>;
    fn serialize_token(&self) -> SlashTokenArg;
}

impl SlashTokenValue for SlashTokenArg {
    fn parse_token(token: SlashTokenArg) -> Result<Self, SlashCommandUsageErrorKind> {
        Ok(token)
    }

    fn serialize_token(&self) -> SlashTokenArg {
        self.clone()
    }
}

impl SlashTokenValue for String {
    fn parse_token(token: SlashTokenArg) -> Result<Self, SlashCommandUsageErrorKind> {
        Ok(token.text)
    }

    fn serialize_token(&self) -> SlashTokenArg {
        SlashTokenArg::new(self.clone(), Vec::new())
    }
}

pub(crate) trait SlashCommandArgs: Sized {
    fn parse(input: SlashCommandParseInput<'_>) -> Result<Self, SlashCommandUsageErrorKind>;
    fn serialize(&self) -> SlashSerializedText;
}

#[derive(Debug)]
pub(crate) struct SlashArgsParser<'a> {
    input: SlashCommandParseInput<'a>,
    positionals: Vec<SlashTokenArg>,
    next_positional: usize,
    named: HashMap<String, SlashTokenArg>,
    duplicates: HashMap<String, usize>,
}

impl<'a> SlashArgsParser<'a> {
    pub(crate) fn new(
        input: SlashCommandParseInput<'a>,
    ) -> Result<Self, SlashCommandUsageErrorKind> {
        let mut positionals = Vec::new();
        let mut named = HashMap::new();
        let mut duplicates = HashMap::new();

        for token in tokenize_with_elements(input.args, input.text_elements)? {
            if let Some((key, value)) = split_named_arg(&token) {
                if named.insert(key.clone(), value).is_some() {
                    *duplicates.entry(key).or_default() += 1;
                }
            } else if token.text.starts_with("--") {
                return Err(SlashCommandUsageErrorKind::InvalidInlineArgs);
            } else {
                positionals.push(token);
            }
        }

        Ok(Self {
            input,
            positionals,
            next_positional: 0,
            named,
            duplicates,
        })
    }

    pub(crate) fn positional<T>(&mut self) -> Result<T, SlashCommandUsageErrorKind>
    where
        T: SlashTokenValue,
    {
        let Some(token) = self.positionals.get(self.next_positional).cloned() else {
            return Err(SlashCommandUsageErrorKind::InvalidInlineArgs);
        };
        self.next_positional += 1;
        T::parse_token(token)
    }

    #[allow(dead_code)]
    pub(crate) fn optional_positional<T>(&mut self) -> Result<Option<T>, SlashCommandUsageErrorKind>
    where
        T: SlashTokenValue,
    {
        if self.next_positional >= self.positionals.len() {
            Ok(None)
        } else {
            self.positional().map(Some)
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn positional_list<T>(&mut self) -> Result<Vec<T>, SlashCommandUsageErrorKind>
    where
        T: SlashTokenValue,
    {
        let mut values = Vec::new();
        while self.next_positional < self.positionals.len() {
            values.push(self.positional()?);
        }
        Ok(values)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn named<T>(
        &mut self,
        key: &'static str,
    ) -> Result<Option<T>, SlashCommandUsageErrorKind>
    where
        T: SlashTokenValue,
    {
        if self.duplicates.contains_key(key) {
            return Err(SlashCommandUsageErrorKind::InvalidInlineArgs);
        }
        let Some(value) = self.named.remove(key) else {
            return Ok(None);
        };
        T::parse_token(value).map(Some)
    }

    pub(crate) fn remainder(&self) -> Option<SlashTextArg> {
        trim_text_arg(self.input.args, self.input.text_elements)
    }

    pub(crate) fn required_remainder(&self) -> Result<SlashTextArg, SlashCommandUsageErrorKind> {
        self.remainder()
            .ok_or(SlashCommandUsageErrorKind::InvalidInlineArgs)
    }

    pub(crate) fn finish(self) -> Result<(), SlashCommandUsageErrorKind> {
        if self.next_positional != self.positionals.len() {
            return Err(SlashCommandUsageErrorKind::InvalidInlineArgs);
        }
        if !self.named.is_empty() || !self.duplicates.is_empty() {
            return Err(SlashCommandUsageErrorKind::InvalidInlineArgs);
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub(crate) struct SlashArgsSerializer {
    fragments: Vec<SlashSerializedText>,
}

impl SlashArgsSerializer {
    pub(crate) fn positional<T>(&mut self, value: &T)
    where
        T: SlashTokenValue,
    {
        self.fragments
            .push(serialize_token(&value.serialize_token()));
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn list<T, I>(&mut self, values: I)
    where
        T: SlashTokenValue,
        I: IntoIterator<Item = T>,
    {
        for value in values {
            self.positional(&value);
        }
    }

    #[allow(dead_code)]
    pub(crate) fn named<T>(&mut self, key: &'static str, value: &T)
    where
        T: SlashTokenValue,
    {
        let serialized_value = serialize_token(&value.serialize_token());
        self.fragments
            .push(serialized_value.prepend_inline(&format!("--{key}=")));
    }

    pub(crate) fn remainder(&mut self, value: &SlashTextArg) {
        self.fragments.push(SlashSerializedText {
            text: value.text.clone(),
            text_elements: value.text_elements.clone(),
        });
    }

    pub(crate) fn finish(self) -> SlashSerializedText {
        join_serialized_fragments(self.fragments)
    }
}

fn trim_text_arg(text: &str, text_elements: &[TextElement]) -> Option<SlashTextArg> {
    let trimmed_start = text.len() - text.trim_start().len();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let trimmed_end = trimmed_start + trimmed.len();
    let mut elements = Vec::new();
    for element in text_elements {
        let start = element.byte_range.start.max(trimmed_start);
        let end = element.byte_range.end.min(trimmed_end);
        if start < end {
            elements.push(element.map_range(|_| ByteRange {
                start: start - trimmed_start,
                end: end - trimmed_start,
            }));
        }
    }

    Some(SlashTextArg::new(trimmed.to_string(), elements))
}

fn split_named_arg(token: &SlashTokenArg) -> Option<(String, SlashTokenArg)> {
    let rest = token.text.strip_prefix("--")?;
    let (key, value) = rest.split_once('=')?;
    if key.is_empty() {
        return None;
    }
    let value_offset = 2 + key.len() + 1;
    let value_elements = token
        .text_elements
        .iter()
        .filter_map(|element| shift_text_element_left(element, value_offset))
        .collect();
    Some((
        key.to_string(),
        SlashTokenArg::new(value.to_string(), value_elements),
    ))
}

fn tokenize_with_elements(
    text: &str,
    text_elements: &[TextElement],
) -> Result<Vec<SlashTokenArg>, SlashCommandUsageErrorKind> {
    let mut elements = text_elements.to_vec();
    elements.sort_by_key(|element| element.byte_range.start);
    let (text_for_shlex, replacements) = replace_text_elements_with_sentinels(text, &elements);
    let mut lexer = Shlex::new(&text_for_shlex);
    let tokens: Vec<String> = lexer.by_ref().collect();
    if lexer.had_error {
        return Err(SlashCommandUsageErrorKind::InvalidInlineArgs);
    }
    Ok(tokens
        .into_iter()
        .map(|token| {
            let restored = restore_sentinels_in_fragment(token, &replacements);
            SlashTokenArg::new(restored.text, restored.text_elements)
        })
        .collect())
}

fn serialize_token(token: &SlashTokenArg) -> SlashSerializedText {
    if token.text.is_empty() {
        return SlashSerializedText::empty();
    }

    let (token_for_shlex, replacements) =
        replace_text_elements_with_sentinels(&token.text, &token.text_elements);
    let quoted = try_join([token_for_shlex.as_str()]).unwrap_or_else(|_| token_for_shlex.clone());
    restore_sentinels_in_fragment(quoted, &replacements)
}

fn join_serialized_fragments(fragments: Vec<SlashSerializedText>) -> SlashSerializedText {
    let mut text = String::new();
    let mut text_elements = Vec::new();

    for fragment in fragments
        .into_iter()
        .filter(|fragment| !fragment.text.is_empty())
    {
        let offset = if text.is_empty() { 0 } else { 1 };
        if offset == 1 {
            text.push(' ');
        }
        let fragment_offset = text.len();
        text.push_str(&fragment.text);
        text_elements.extend(shift_text_elements_right(
            &fragment.text_elements,
            fragment_offset,
        ));
    }

    SlashSerializedText {
        text,
        text_elements,
    }
}

fn shift_text_element_left(element: &TextElement, offset: usize) -> Option<TextElement> {
    if element.byte_range.end <= offset {
        return None;
    }
    let start = element.byte_range.start.saturating_sub(offset);
    let end = element.byte_range.end.saturating_sub(offset);
    (start < end).then(|| element.map_range(|_| ByteRange { start, end }))
}

fn shift_text_elements_right(elements: &[TextElement], offset: usize) -> Vec<TextElement> {
    elements
        .iter()
        .map(|element| {
            element.map_range(|byte_range| ByteRange {
                start: byte_range.start + offset,
                end: byte_range.end + offset,
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
struct ElementReplacement {
    sentinel: String,
    text: String,
    placeholder: Option<String>,
}

fn replace_text_elements_with_sentinels(
    text: &str,
    text_elements: &[TextElement],
) -> (String, Vec<ElementReplacement>) {
    let mut out = String::with_capacity(text.len());
    let mut replacements = Vec::new();
    let mut cursor = 0;
    let text_len = text.len();

    for (idx, element) in text_elements.iter().enumerate() {
        let start = element.byte_range.start.clamp(cursor, text_len);
        let end = element.byte_range.end.clamp(start, text_len);
        out.push_str(&text[cursor..start]);
        let mut sentinel = format!("__CODEX_ELEM_{idx}__");
        while text.contains(&sentinel) {
            sentinel.push('_');
        }
        out.push_str(&sentinel);
        let replacement_text = text
            .get(start..end)
            .or_else(|| element.placeholder(text))
            .unwrap_or_default()
            .to_string();
        replacements.push(ElementReplacement {
            sentinel,
            text: replacement_text,
            placeholder: element.placeholder(text).map(str::to_string),
        });
        cursor = end;
    }

    out.push_str(&text[cursor..]);
    (out, replacements)
}

fn restore_sentinels_in_fragment(
    fragment: String,
    replacements: &[ElementReplacement],
) -> SlashSerializedText {
    if replacements.is_empty() {
        return SlashSerializedText {
            text: fragment,
            text_elements: Vec::new(),
        };
    }

    let mut out = String::with_capacity(fragment.len());
    let mut out_elements = Vec::new();
    let mut cursor = 0;

    while cursor < fragment.len() {
        let Some((offset, replacement)) = next_replacement(&fragment, cursor, replacements) else {
            out.push_str(&fragment[cursor..]);
            break;
        };
        let start_in_fragment = cursor + offset;
        out.push_str(&fragment[cursor..start_in_fragment]);
        let start = out.len();
        out.push_str(&replacement.text);
        let end = out.len();
        if start < end {
            out_elements.push(TextElement::new(
                ByteRange { start, end },
                replacement.placeholder.clone(),
            ));
        }
        cursor = start_in_fragment + replacement.sentinel.len();
    }

    SlashSerializedText {
        text: out,
        text_elements: out_elements,
    }
}

fn next_replacement<'a>(
    text: &str,
    cursor: usize,
    replacements: &'a [ElementReplacement],
) -> Option<(usize, &'a ElementReplacement)> {
    replacements
        .iter()
        .filter_map(|replacement| {
            text[cursor..]
                .find(&replacement.sentinel)
                .map(|offset| (offset, replacement))
        })
        .min_by_key(|(offset, _)| *offset)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Switch {
        On,
        Off,
    }

    impl SlashTokenValue for Switch {
        fn parse_token(token: SlashTokenArg) -> Result<Self, SlashCommandUsageErrorKind> {
            match token.text.as_str() {
                "on" => Ok(Self::On),
                "off" => Ok(Self::Off),
                _ => Err(SlashCommandUsageErrorKind::InvalidInlineArgs),
            }
        }

        fn serialize_token(&self) -> SlashTokenArg {
            let text = match self {
                Self::On => "on",
                Self::Off => "off",
            };
            SlashTokenArg::new(text.to_string(), Vec::new())
        }
    }

    #[test]
    fn parser_supports_positional_list_and_named_args() {
        let mut parser = SlashArgsParser::new(SlashCommandParseInput {
            args: "on first second --path=\"some dir\"",
            text_elements: &[],
        })
        .unwrap();

        assert_eq!(parser.positional::<Switch>(), Ok(Switch::On));
        assert_eq!(
            parser.positional_list::<String>(),
            Ok(vec!["first".to_string(), "second".to_string()])
        );
        assert_eq!(
            parser.named::<String>("path"),
            Ok(Some("some dir".to_string()))
        );
        assert_eq!(parser.finish(), Ok(()));
    }

    #[test]
    fn parser_supports_optional_positional_args() {
        let mut parser = SlashArgsParser::new(SlashCommandParseInput {
            args: "on",
            text_elements: &[],
        })
        .unwrap();

        assert_eq!(parser.positional::<Switch>(), Ok(Switch::On));
        assert_eq!(parser.optional_positional::<String>(), Ok(None));
        assert_eq!(parser.finish(), Ok(()));
    }

    #[test]
    fn serializer_stably_formats_named_args_after_positionals() {
        let mut serializer = SlashArgsSerializer::default();
        serializer.positional(&Switch::On);
        serializer.list::<String, _>(["first".to_string(), "second".to_string()]);
        serializer.named("path", &"some dir".to_string());

        assert_eq!(
            serializer.finish(),
            SlashSerializedText {
                text: "on first second --path='some dir'".to_string(),
                text_elements: Vec::new(),
            }
        );
    }

    #[test]
    fn remainder_preserves_placeholder_ranges() {
        let placeholder = "[Image #1]".to_string();
        let prompt = SlashTextArg::new(
            format!("review {placeholder}"),
            vec![TextElement::new((7..18).into(), Some(placeholder.clone()))],
        );
        let mut serializer = SlashArgsSerializer::default();
        serializer.remainder(&prompt);

        assert_eq!(
            serializer.finish(),
            SlashSerializedText {
                text: format!("review {placeholder}"),
                text_elements: vec![TextElement::new((7..18).into(), Some(placeholder))],
            }
        );
    }
}
