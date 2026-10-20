use serde_json::Value;

const ROOT: char = '$';
const DOT: char = '.';
const OPEN: char = '[';
const CLOSE: char = ']';
const ESCAPE: char = '\\';
const WILDCARD: &str = "*";
const CURRENT: char = '@';
const FILTER_OPEN: &str = "?(";
const FILTER_CLOSE: char = ')';
const QUOTES: [char; 2] = ['"', '\''];
const EQUAL: &str = "==";
const NOT_EQUAL: &str = "!=";
const BRACES: (char, char) = ('{', '}');

#[derive(Clone, Debug, PartialEq)]
enum Step {
    Field(String),
    Index(i64),
    All,
    Filter(Filter),
}

#[derive(Clone, Debug, PartialEq)]
struct Filter {
    path: Vec<Step>,
    test: Option<(bool, Value)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Path(Vec<Step>);

impl Path {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text
            .strip_prefix(BRACES.0)
            .and_then(|inner| inner.strip_suffix(BRACES.1))
            .unwrap_or(text)
            .trim();
        let text = text.strip_prefix(ROOT).unwrap_or(text);
        if !text.starts_with([DOT, OPEN]) {
            return None;
        }
        steps(text).map(Self)
    }

    pub fn first<'a>(&self, value: &'a Value) -> Option<&'a Value> {
        select(&self.0, value).into_iter().next()
    }

    pub fn all<'a>(&self, value: &'a Value) -> Vec<&'a Value> {
        select(&self.0, value)
    }
}

fn steps(mut text: &str) -> Option<Vec<Step>> {
    let mut parsed = Vec::new();
    while !text.is_empty() {
        let (step, rest) = match text.strip_prefix(DOT) {
            Some(rest) => field(rest)?,
            None => bracket(text.strip_prefix(OPEN)?)?,
        };
        parsed.push(step);
        text = rest;
    }
    Some(parsed)
}

fn field(text: &str) -> Option<(Step, &str)> {
    let mut name = String::new();
    let mut chars = text.char_indices();
    let mut end = text.len();
    while let Some((ix, c)) = chars.next() {
        match c {
            ESCAPE => {
                let (_, escaped) = chars.next()?;
                name.push(escaped);
            }
            DOT => {
                end = ix;
                break;
            }
            OPEN => {
                end = ix;
                break;
            }
            other => name.push(other),
        }
    }
    let rest = text.get(end..)?;
    if name == WILDCARD {
        return Some((Step::All, rest));
    }
    (!name.is_empty()).then_some((Step::Field(name), rest))
}

fn bracket(text: &str) -> Option<(Step, &str)> {
    if let Some(quoted) = text.strip_prefix(QUOTES) {
        let quote = text.chars().next()?;
        let end = quoted.find(quote)?;
        let name = quoted.get(..end)?.to_owned();
        let rest = quoted.get(end + quote.len_utf8()..)?.strip_prefix(CLOSE)?;
        return Some((Step::Field(name), rest));
    }
    if let Some(filter) = text.strip_prefix(FILTER_OPEN) {
        let end = closing(filter)?;
        let body = filter.get(..end)?;
        let rest = filter
            .get(end + FILTER_CLOSE.len_utf8()..)?
            .strip_prefix(CLOSE)?;
        return Some((Step::Filter(parse_filter(body)?), rest));
    }
    let end = text.find(CLOSE)?;
    let inner = text.get(..end)?.trim();
    let rest = text.get(end + CLOSE.len_utf8()..)?;
    if inner == WILDCARD {
        return Some((Step::All, rest));
    }
    let index = inner.parse().ok()?;
    Some((Step::Index(index), rest))
}

fn closing(text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (ix, c) in text.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if QUOTES.contains(&c) => quote = Some(c),
            None if c == FILTER_CLOSE => return Some(ix),
            None => {}
        }
    }
    None
}

fn parse_filter(body: &str) -> Option<Filter> {
    let (left, test) = match (body.find(EQUAL), body.find(NOT_EQUAL)) {
        (Some(ix), _) => (body.get(..ix)?, Some((true, body.get(ix + EQUAL.len()..)?))),
        (None, Some(ix)) => (
            body.get(..ix)?,
            Some((false, body.get(ix + NOT_EQUAL.len()..)?)),
        ),
        (None, None) => (body, None),
    };
    let path = left.trim().strip_prefix(CURRENT)?;
    let path = if path.is_empty() {
        Vec::new()
    } else {
        steps(path)?
    };
    let test = match test {
        Some((equal, literal)) => Some((equal, parse_literal(literal.trim())?)),
        None => None,
    };
    Some(Filter { path, test })
}

fn parse_literal(text: &str) -> Option<Value> {
    if let Some(quoted) = text.strip_prefix(QUOTES) {
        let quote = text.chars().next()?;
        return quoted
            .strip_suffix(quote)
            .map(|inner| Value::String(inner.to_owned()));
    }
    serde_json::from_str(text).ok()
}

fn select<'a>(steps: &[Step], value: &'a Value) -> Vec<&'a Value> {
    let mut current = vec![value];
    for step in steps {
        current = current
            .into_iter()
            .flat_map(|value| apply(step, value))
            .collect();
    }
    current
}

fn apply<'a>(step: &Step, value: &'a Value) -> Vec<&'a Value> {
    match step {
        Step::Field(name) => value.get(name).into_iter().collect(),
        Step::Index(index) => {
            let Some(items) = value.as_array() else {
                return Vec::new();
            };
            let len = i64::try_from(items.len()).unwrap_or(i64::MAX);
            let position = if *index < 0 { len + index } else { *index };
            usize::try_from(position)
                .ok()
                .and_then(|ix| items.get(ix))
                .into_iter()
                .collect()
        }
        Step::All => match value {
            Value::Array(items) => items.iter().collect(),
            Value::Object(fields) => fields.values().collect(),
            Value::Null => Vec::new(),
            Value::Bool(_) => Vec::new(),
            Value::Number(_) => Vec::new(),
            Value::String(_) => Vec::new(),
        },
        Step::Filter(filter) => {
            let Some(items) = value.as_array() else {
                return Vec::new();
            };
            items.iter().filter(|item| keeps(filter, item)).collect()
        }
    }
}

fn keeps(filter: &Filter, item: &Value) -> bool {
    let found = select(&filter.path, item);
    match &filter.test {
        None => !found.is_empty(),
        Some((equal, literal)) => {
            let matched = found.contains(&literal);
            matched == *equal
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn object() -> Value {
        json!({
            "metadata": {
                "name": "edge",
                "annotations": { "example.com/owner": "platform" }
            },
            "spec": { "gatewayClassName": "eg", "replicas": 3, "paused": false },
            "status": {
                "addresses": [{ "value": "10.0.0.1" }, { "value": "10.0.0.2" }],
                "conditions": [
                    { "type": "Accepted", "status": "True" },
                    { "type": "Ready", "status": "False", "message": "waiting for listeners" }
                ],
                "ancestors": [{ "conditions": [{ "type": "Programmed", "status": "True" }] }]
            }
        })
    }

    fn first(path: &str) -> Option<Value> {
        let value = object();
        Path::parse(path)?.first(&value).cloned()
    }

    #[test]
    fn printer_column_paths_select_what_kubectl_shows() {
        assert_eq!(first(".spec.gatewayClassName"), Some(json!("eg")));
        assert_eq!(first("{.spec.replicas}"), Some(json!(3)));
        assert_eq!(first("$.spec.paused"), Some(json!(false)));
        assert_eq!(first(".status.addresses[*].value"), Some(json!("10.0.0.1")));
        assert_eq!(
            first(".status.addresses[-1].value"),
            Some(json!("10.0.0.2"))
        );
        assert_eq!(first(".status.addresses[1].value"), Some(json!("10.0.0.2")));
        assert_eq!(
            first(r#".status.conditions[?(@.type=="Ready")].status"#),
            Some(json!("False"))
        );
        assert_eq!(
            first(".status.conditions[?(@.type == 'Ready')].message"),
            Some(json!("waiting for listeners"))
        );
        assert_eq!(
            first(r#".status.conditions[?(@.status!="True")].type"#),
            Some(json!("Ready"))
        );
        assert_eq!(
            first(".status.conditions[?(@.message)].type"),
            Some(json!("Ready"))
        );
        assert_eq!(
            first(r#".status.ancestors[0].conditions[?(@.type=="Programmed")].status"#),
            Some(json!("True"))
        );
        assert_eq!(
            first(r".metadata.annotations.example\.com/owner"),
            Some(json!("platform"))
        );
        assert_eq!(
            first(".metadata.annotations['example.com/owner']"),
            Some(json!("platform"))
        );
    }

    #[test]
    fn missing_fields_and_bad_paths_select_nothing() {
        assert_eq!(first(".status.missing"), None);
        assert_eq!(first(".status.addresses[9].value"), None);
        assert_eq!(
            first(r#".status.conditions[?(@.type=="Gone")].status"#),
            None
        );
        assert_eq!(first(".spec.gatewayClassName[0]"), None);
        assert!(Path::parse("spec.replicas").is_none());
        assert!(Path::parse(".status.conditions[?(@.type==").is_none());
        assert!(Path::parse(".status[").is_none());
        assert!(Path::parse(".status.").is_none());
        let value = object();
        let all = Path::parse(".status.addresses[*].value").map(|path| path.all(&value).len());
        assert_eq!(all, Some(2));
    }
}
