use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::Value;

use crate::LogLevel;

const LEVEL_KEYS: [&str; 4] = ["level", "lvl", "severity", "levelname"];
const TIME_KEYS: [&str; 5] = ["time", "ts", "timestamp", "@timestamp", "t"];
const MESSAGE_KEYS: [&str; 3] = ["msg", "message", "@message"];
const JSON_OPEN: char = '{';
const QUOTE: char = '"';
const ESCAPE: char = '\\';
const EQUALS: char = '=';
const FIELD_SEPARATOR: &str = " ";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedLine {
    pub level: Option<LogLevel>,
    pub time: Option<String>,
    pub message: Option<String>,
    pub fields: Vec<(String, String)>,
}

impl ParsedLine {
    pub fn fields_text(&self) -> String {
        self.fields
            .iter()
            .map(|(key, value)| format!("{key}{EQUALS}{}", logfmt_value(value)))
            .collect::<Vec<_>>()
            .join(FIELD_SEPARATOR)
    }
}

fn logfmt_value(value: &str) -> String {
    let plain = !value.is_empty()
        && !value.contains(|c: char| c.is_whitespace() || c == QUOTE || c == EQUALS);
    if plain {
        value.to_owned()
    } else {
        format!("{value:?}")
    }
}

pub fn parse(line: &str) -> Option<ParsedLine> {
    let trimmed = line.trim();
    if trimmed.starts_with(JSON_OPEN) {
        return parse_json(trimmed);
    }
    parse_logfmt(trimmed)
}

struct Pairs(Vec<(String, Value)>);

struct PairsVisitor;

impl<'de> Visitor<'de> for PairsVisitor {
    type Value = Pairs;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Pairs, A::Error> {
        let mut pairs = Vec::new();
        while let Some(pair) = map.next_entry::<String, Value>()? {
            pairs.push(pair);
        }
        Ok(Pairs(pairs))
    }
}

impl<'de> Deserialize<'de> for Pairs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(PairsVisitor)
    }
}

fn parse_json(line: &str) -> Option<ParsedLine> {
    let Pairs(pairs) = serde_json::from_str(line).ok()?;
    let texts = pairs
        .into_iter()
        .map(|(key, value)| (key, json_text(value)));
    Some(classify(texts))
}

fn json_text(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

fn parse_logfmt(line: &str) -> Option<ParsedLine> {
    let mut pairs = Vec::new();
    let mut rest = line.trim_start();
    while !rest.is_empty() {
        let (key, after) = rest.split_once(EQUALS)?;
        let valid_key = !key.is_empty() && !key.contains(|c: char| c.is_whitespace() || c == QUOTE);
        if !valid_key {
            return None;
        }
        let (value, after) = logfmt_token(after)?;
        pairs.push((key.to_owned(), value));
        rest = after.trim_start();
    }
    (!pairs.is_empty()).then(|| classify(pairs))
}

fn logfmt_token(input: &str) -> Option<(String, &str)> {
    let Some(quoted) = input.strip_prefix(QUOTE) else {
        let end = input.find(char::is_whitespace).unwrap_or(input.len());
        let (value, rest) = input.split_at(end);
        return (!value.contains(QUOTE)).then(|| (value.to_owned(), rest));
    };
    let mut value = String::new();
    let mut chars = quoted.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            QUOTE => {
                let rest = quoted.get(at + c.len_utf8()..)?;
                let separated = rest.is_empty() || rest.starts_with(char::is_whitespace);
                return separated.then_some((value, rest));
            }
            ESCAPE => {
                let (_, escaped) = chars.next()?;
                value.push(unescape(escaped));
            }
            other => value.push(other),
        }
    }
    None
}

fn unescape(escaped: char) -> char {
    match escaped {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        other => other,
    }
}

fn classify(pairs: impl IntoIterator<Item = (String, String)>) -> ParsedLine {
    let mut parsed = ParsedLine::default();
    for (key, value) in pairs {
        let lower = key.to_ascii_lowercase();
        let lower = lower.as_str();
        let level = LEVEL_KEYS
            .contains(&lower)
            .then(|| LogLevel::from_word(&value))
            .flatten();
        if parsed.level.is_none() && level.is_some() {
            parsed.level = level;
        } else if parsed.time.is_none() && TIME_KEYS.contains(&lower) {
            parsed.time = Some(value);
        } else if parsed.message.is_none() && MESSAGE_KEYS.contains(&lower) {
            parsed.message = Some(value);
        } else {
            parsed.fields.push((key, value));
        }
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn logfmt_splits_level_time_message_and_fields() {
        let parsed = parse(
            r#"ts=2026-10-20T09:14:52.310Z level=warn msg="cache unavailable, serving without cache" redis=redis:6379 err="dial tcp: i/o timeout""#,
        );
        assert_eq!(
            parsed,
            Some(ParsedLine {
                level: Some(LogLevel::Warn),
                time: Some("2026-10-20T09:14:52.310Z".into()),
                message: Some("cache unavailable, serving without cache".into()),
                fields: fields(&[("redis", "redis:6379"), ("err", "dial tcp: i/o timeout")]),
            })
        );
    }

    #[test]
    fn logfmt_unescapes_quoted_values_and_keeps_equals_in_values() {
        let parsed = parse(r#"msg="say \"hi\"\n" query=a=b empty="""#);
        assert_eq!(
            parsed,
            Some(ParsedLine {
                message: Some("say \"hi\"\n".into()),
                fields: fields(&[("query", "a=b"), ("empty", "")]),
                ..ParsedLine::default()
            })
        );
    }

    #[test]
    fn json_keeps_key_order_and_renders_non_strings_compactly() {
        let parsed = parse(
            r#"{"level":"error","ts":"2026-10-20T02:37:04.902Z","msg":"failed to load config","path":"/data/config.yaml","code":1,"ok":false,"tags":["a","b"],"ctx":{"id":7}}"#,
        );
        assert_eq!(
            parsed,
            Some(ParsedLine {
                level: Some(LogLevel::Error),
                time: Some("2026-10-20T02:37:04.902Z".into()),
                message: Some("failed to load config".into()),
                fields: fields(&[
                    ("path", "/data/config.yaml"),
                    ("code", "1"),
                    ("ok", "false"),
                    ("tags", r#"["a","b"]"#),
                    ("ctx", r#"{"id":7}"#),
                ]),
            })
        );
    }

    #[test]
    fn unknown_levels_stay_fields_and_level_keys_ignore_case() {
        let parsed = parse(r#"{"Severity":"WARNING","level":"chatty","message":"slow"}"#);
        assert_eq!(
            parsed,
            Some(ParsedLine {
                level: Some(LogLevel::Warn),
                message: Some("slow".into()),
                fields: fields(&[("level", "chatty")]),
                ..ParsedLine::default()
            })
        );
    }

    #[test]
    fn mixed_streams_parse_structured_lines_and_leave_the_rest() {
        let lines = [
            r#"{"level":"info","msg":"Starting podinfo"}"#,
            "GET /healthz 200 0.4ms",
            "level=info msg=ready addr=:9898",
            "cache hit sku:44718 ttl=287s",
            "I1013 21:12:20.114 main.go:12] started",
        ];
        let parsed: Vec<bool> = lines.iter().map(|line| parse(line).is_some()).collect();
        assert_eq!(parsed, [true, false, true, false, false]);
    }

    #[test]
    fn malformed_lines_are_not_parsed() {
        let malformed = [
            "",
            "   ",
            r#"{"level":"info","msg":"cut off"#,
            r#"["level","info"]"#,
            "{not json}",
            r#"msg="unterminated"#,
            r#"msg="closed"tail"#,
            "=value",
            r#"a"b=c"#,
            r#"key=abc"def"#,
            "key=value trailing",
        ];
        for line in malformed {
            assert_eq!(parse(line), None, "{line}");
        }
    }

    #[test]
    fn fields_text_quotes_values_that_need_it() {
        let parsed = ParsedLine {
            fields: fields(&[("addr", ":9898"), ("err", "i/o timeout"), ("empty", "")]),
            ..ParsedLine::default()
        };
        assert_eq!(
            parsed.fields_text(),
            r#"addr=:9898 err="i/o timeout" empty="""#
        );
    }
}
