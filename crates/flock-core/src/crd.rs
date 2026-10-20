use serde::{Deserialize, Serialize};

use crate::resources::Resource;
use crate::{ClusterId, ResourceRef};

const GROUP_SEPARATOR: char = '.';

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CrdKey {
    pub group: String,
    pub kind: String,
}

impl CrdKey {
    pub fn new(group: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            group: group.into(),
            kind: kind.into(),
        }
    }

    pub fn qualified(&self) -> String {
        format!("{}{GROUP_SEPARATOR}{}", self.kind, self.group)
    }

    pub fn parse(kind: &str) -> Option<Self> {
        let (kind, group) = kind.split_once(GROUP_SEPARATOR)?;
        let valid = !kind.is_empty() && group.contains(GROUP_SEPARATOR);
        valid.then(|| Self::new(group, kind))
    }

    pub fn object(
        &self,
        cluster: ClusterId,
        namespace: Option<String>,
        name: String,
    ) -> ResourceRef {
        ResourceRef {
            cluster,
            kind: self.qualified(),
            namespace,
            name,
        }
    }
}

pub fn display_kind(kind: &str) -> &str {
    match kind.split_once(GROUP_SEPARATOR) {
        Some((bare, group)) if group.contains(GROUP_SEPARATOR) => bare,
        _ => kind,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrinterColumn {
    pub name: String,
    pub json_path: String,
    pub date: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crd {
    pub key: CrdKey,
    pub plural: String,
    pub version: String,
    pub namespaced: bool,
    pub columns: Vec<PrinterColumn>,
}

impl Crd {
    pub fn name(&self) -> String {
        format!("{}{GROUP_SEPARATOR}{}", self.plural, self.key.group)
    }

    fn matches(&self, words: &[String]) -> bool {
        let fields = [
            self.key.kind.to_lowercase(),
            self.plural.to_lowercase(),
            self.key.group.to_lowercase(),
        ];
        words
            .iter()
            .all(|word| fields.iter().any(|field| field.contains(word.as_str())))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrdGroup<'a> {
    pub group: &'a str,
    pub crds: Vec<&'a Crd>,
}

pub fn grouped<'a>(crds: &'a [Crd], query: &str) -> Vec<CrdGroup<'a>> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut shown: Vec<&Crd> = crds.iter().filter(|crd| crd.matches(&words)).collect();
    shown.sort_by(|a, b| {
        a.key
            .group
            .cmp(&b.key.group)
            .then_with(|| a.key.kind.cmp(&b.key.kind))
    });
    let mut groups: Vec<CrdGroup<'a>> = Vec::new();
    for crd in shown {
        match groups.last_mut() {
            Some(last) if last.group == crd.key.group => last.crds.push(crd),
            _ => groups.push(CrdGroup {
                group: &crd.key.group,
                crds: vec![crd],
            }),
        }
    }
    groups
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomObjects {
    pub cluster: ClusterId,
    pub key: CrdKey,
    pub rows: Vec<Resource>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crd(group: &str, kind: &str, plural: &str) -> Crd {
        Crd {
            key: CrdKey::new(group, kind),
            plural: plural.into(),
            version: "v1".into(),
            namespaced: true,
            columns: Vec::new(),
        }
    }

    #[test]
    fn qualified_kinds_round_trip_and_builtins_stay_bare() {
        let key = CrdKey::new("cert-manager.io", "Certificate");
        assert_eq!(key.qualified(), "Certificate.cert-manager.io");
        assert_eq!(CrdKey::parse(&key.qualified()), Some(key));
        assert_eq!(CrdKey::parse("Deployment"), None);
        assert_eq!(CrdKey::parse(".example.com"), None);
        assert_eq!(CrdKey::parse("Widget.local"), None);
        assert_eq!(display_kind("Certificate.cert-manager.io"), "Certificate");
        assert_eq!(display_kind("Deployment"), "Deployment");
    }

    #[test]
    fn crds_group_by_api_group_and_filter_on_kind_plural_or_group() {
        let crds = [
            crd(
                "source.toolkit.fluxcd.io",
                "GitRepository",
                "gitrepositories",
            ),
            crd("infra.loco.io", "Build", "builds"),
            crd("source.toolkit.fluxcd.io", "Bucket", "buckets"),
            crd("infra.loco.io", "Application", "applications"),
        ];
        let all: Vec<(&str, Vec<&str>)> = grouped(&crds, "")
            .into_iter()
            .map(|g| {
                (
                    g.group,
                    g.crds.iter().map(|c| c.key.kind.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            all,
            [
                ("infra.loco.io", vec!["Application", "Build"]),
                ("source.toolkit.fluxcd.io", vec!["Bucket", "GitRepository"]),
            ]
        );
        let kinds = |query: &str| -> Vec<String> {
            grouped(&crds, query)
                .into_iter()
                .flat_map(|g| g.crds.into_iter().map(|c| c.key.kind.clone()))
                .collect()
        };
        assert_eq!(kinds("GITREPO"), ["GitRepository"]);
        assert_eq!(kinds("loco"), ["Application", "Build"]);
        assert_eq!(kinds("buckets flux"), ["Bucket"]);
        assert_eq!(kinds("nothing"), Vec::<String>::new());
        assert_eq!(crds[0].name(), "gitrepositories.source.toolkit.fluxcd.io");
    }
}
