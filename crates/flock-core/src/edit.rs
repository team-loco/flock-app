use serde_json::Value;
use similar::{ChangeTag, TextDiff};

use crate::changes::{LineChange, PatchLine};
use crate::{ResourceRef, Sink, Uid};

pub const DIFF_CONTEXT: usize = 3;
const SECRET_KIND: &str = "Secret";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub text: String,
    pub uid: Uid,
    pub resource_version: String,
    pub revealed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub base: Live,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditStep {
    Load { reveal: bool },
    Review(Draft),
    Apply(Draft),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditRequest {
    pub target: ResourceRef,
    pub step: EditStep,
    pub sink: Sink<EditEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rebased {
    pub text: String,
    pub contested: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub live: Live,
    pub changed: Vec<PatchLine>,
    pub rebased: Rebased,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditEvent {
    Loaded(Live),
    Reviewed(Vec<PatchLine>),
    Applied(Live),
    Conflict(Box<Conflict>),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    #[error("{message}")]
    Syntax {
        line: usize,
        column: usize,
        message: String,
    },
    #[error("The document is not a mapping of fields.")]
    NotAMapping,
    #[error("{field} is missing.")]
    Missing { field: &'static str },
    #[error("{field} must stay {expected}; edit cannot rename or move an object.")]
    Changed {
        field: &'static str,
        expected: String,
    },
}

impl Invalid {
    pub const fn line(&self) -> Option<usize> {
        match self {
            Self::Syntax { line, .. } => Some(*line),
            Self::NotAMapping => None,
            Self::Missing { .. } => None,
            Self::Changed { .. } => None,
        }
    }
}

pub fn is_secret(target: &ResourceRef) -> bool {
    target.kind == SECRET_KIND
}

pub fn validate(target: &ResourceRef, text: &str) -> Result<Value, Invalid> {
    let value: Value = serde_norway::from_str(text).map_err(|error| syntax(&error))?;
    if !value.is_object() {
        return Err(Invalid::NotAMapping);
    }
    let text_at = |pointer: &str| value.pointer(pointer).and_then(Value::as_str);
    if text_at("/apiVersion").is_none() {
        return Err(Invalid::Missing {
            field: "apiVersion",
        });
    }
    expect("kind", text_at("/kind"), &target.kind)?;
    expect("metadata.name", text_at("/metadata/name"), &target.name)?;
    if let Some(namespace) = &target.namespace {
        let written = text_at("/metadata/namespace");
        if written.is_some_and(|written| written != namespace) {
            return Err(Invalid::Changed {
                field: "metadata.namespace",
                expected: namespace.clone(),
            });
        }
    }
    Ok(value)
}

fn expect(field: &'static str, written: Option<&str>, expected: &str) -> Result<(), Invalid> {
    match written {
        None => Err(Invalid::Missing { field }),
        Some(written) if written == expected => Ok(()),
        Some(_) => Err(Invalid::Changed {
            field,
            expected: expected.to_owned(),
        }),
    }
}

fn syntax(error: &serde_norway::Error) -> Invalid {
    let location = error.location();
    let message = error.to_string();
    Invalid::Syntax {
        line: location.as_ref().map_or(0, serde_norway::Location::line),
        column: location.as_ref().map_or(0, serde_norway::Location::column),
        message,
    }
}

pub fn diff_lines(old: &str, new: &str) -> Vec<PatchLine> {
    let diff = TextDiff::from_lines(old, new);
    diff.grouped_ops(DIFF_CONTEXT)
        .iter()
        .flatten()
        .flat_map(|op| diff.iter_changes(op))
        .map(|change| {
            let (kind, number) = match change.tag() {
                ChangeTag::Equal => (LineChange::Context, line_number(change.new_index())),
                ChangeTag::Insert => (LineChange::Added, line_number(change.new_index())),
                ChangeTag::Delete => (LineChange::Removed, line_number(change.old_index())),
            };
            PatchLine {
                change: kind,
                number,
                text: change.value().trim_end_matches('\n').to_owned(),
                note: None,
            }
        })
        .collect()
}

fn line_number(index: Option<usize>) -> u32 {
    index
        .and_then(|ix| u32::try_from(ix + 1).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClusterId;

    fn target() -> ResourceRef {
        ResourceRef {
            cluster: ClusterId::new("kind-fleet-a"),
            kind: "ConfigMap".into(),
            namespace: Some("apps".into()),
            name: "settings".into(),
        }
    }

    const VALID: &str =
        "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: settings\n  namespace: apps\n";

    #[test]
    fn a_manifest_for_the_edited_object_validates() {
        assert!(validate(&target(), VALID).is_ok());
        let without_namespace = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: settings\n";
        assert!(validate(&target(), without_namespace).is_ok());
    }

    #[test]
    fn syntax_errors_name_their_line() {
        let broken = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: [settings\n";
        let Err(error) = validate(&target(), broken) else {
            panic!("broken YAML validated");
        };
        assert!(error.line().is_some_and(|line| line >= 3), "{error:?}");
        assert!(matches!(error, Invalid::Syntax { .. }));
    }

    #[test]
    fn renaming_or_moving_the_object_is_refused() {
        let renamed = VALID.replace("name: settings", "name: other");
        assert_eq!(
            validate(&target(), &renamed),
            Err(Invalid::Changed {
                field: "metadata.name",
                expected: "settings".into()
            })
        );
        let moved = VALID.replace("namespace: apps", "namespace: default");
        assert!(matches!(
            validate(&target(), &moved),
            Err(Invalid::Changed {
                field: "metadata.namespace",
                ..
            })
        ));
        let kind = VALID.replace("kind: ConfigMap", "kind: Secret");
        assert!(matches!(
            validate(&target(), &kind),
            Err(Invalid::Changed { field: "kind", .. })
        ));
        assert_eq!(validate(&target(), "- a\n- b\n"), Err(Invalid::NotAMapping));
        assert_eq!(
            validate(&target(), "kind: ConfigMap\n"),
            Err(Invalid::Missing {
                field: "apiVersion"
            })
        );
    }

    #[test]
    fn diff_lines_number_each_side() {
        let lines = diff_lines("a: 1\nb: 2\n", "a: 1\nb: 3\n");
        let changes: Vec<(LineChange, u32, &str)> = lines
            .iter()
            .map(|line| (line.change, line.number, line.text.as_str()))
            .collect();
        assert_eq!(
            changes,
            [
                (LineChange::Context, 1, "a: 1"),
                (LineChange::Removed, 2, "b: 2"),
                (LineChange::Added, 2, "b: 3"),
            ]
        );
        assert_eq!(diff_lines("a: 1\n", "a: 1\n"), Vec::<PatchLine>::new());
    }
}
