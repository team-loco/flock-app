use std::borrow::Cow;

use crate::Page;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Noun {
    pub singular: Cow<'static, str>,
    pub plural: Cow<'static, str>,
}

impl Noun {
    pub const fn of(page: Page) -> Self {
        Self {
            singular: Cow::Borrowed(page.singular()),
            plural: Cow::Borrowed(page.label()),
        }
    }

    pub fn named(singular: impl Into<String>, plural: impl Into<String>) -> Self {
        Self {
            singular: Cow::Owned(singular.into()),
            plural: Cow::Owned(plural.into()),
        }
    }

    pub fn counted(&self, count: usize) -> String {
        let word = if count == 1 {
            &self.singular
        } else {
            &self.plural
        };
        format!("{count} {word}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Emptiness {
    Cluster,
    Namespace { namespace: String, elsewhere: usize },
    Filter { text: String },
}

impl Emptiness {
    pub fn classify(total: usize, namespace: Option<(&str, usize)>, text: &str) -> Self {
        if total == 0 {
            return Self::Cluster;
        }
        match namespace {
            Some((namespace, 0)) => Self::Namespace {
                namespace: namespace.to_owned(),
                elsewhere: total,
            },
            _ => Self::Filter {
                text: text.trim().to_owned(),
            },
        }
    }

    pub fn sentence(&self, noun: &Noun, cluster: &str) -> String {
        let plural = &noun.plural;
        match self {
            Self::Cluster => format!("No {plural} in {cluster}"),
            Self::Namespace { namespace, .. } => format!("No {plural} in namespace {namespace}"),
            Self::Filter { text } if text.is_empty() => format!("No {plural} match the filter"),
            Self::Filter { text } => format!("No {plural} match \"{text}\""),
        }
    }

    pub fn detail(&self, noun: &Noun) -> Option<String> {
        match self {
            Self::Namespace { elsewhere, .. } => {
                let place = if *elsewhere == 1 {
                    "another namespace"
                } else {
                    "other namespaces"
                };
                Some(format!("{} in {place}", noun.counted(*elsewhere)))
            }
            Self::Cluster | Self::Filter { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEPLOYMENTS: Noun = Noun::of(Page::Deployments);

    #[test]
    fn a_cluster_without_objects_is_told_from_a_hiding_namespace_and_a_hiding_filter() {
        assert_eq!(
            Emptiness::classify(0, Some(("default", 0)), "x"),
            Emptiness::Cluster
        );
        assert_eq!(
            Emptiness::classify(6, Some(("default", 0)), "x"),
            Emptiness::Namespace {
                namespace: "default".into(),
                elsewhere: 6
            }
        );
        assert_eq!(
            Emptiness::classify(6, Some(("default", 2)), " web "),
            Emptiness::Filter { text: "web".into() }
        );
        assert_eq!(
            Emptiness::classify(6, None, ""),
            Emptiness::Filter {
                text: String::new()
            }
        );
    }

    #[test]
    fn sentences_name_what_hides_the_rows() {
        let cluster = Emptiness::Cluster;
        assert_eq!(
            cluster.sentence(&DEPLOYMENTS, "kind-fleet-a"),
            "No Deployments in kind-fleet-a"
        );
        let namespace = Emptiness::classify(6, Some(("default", 0)), "");
        assert_eq!(
            namespace.sentence(&DEPLOYMENTS, "kind-fleet-a"),
            "No Deployments in namespace default"
        );
        assert_eq!(
            namespace.detail(&DEPLOYMENTS).as_deref(),
            Some("6 Deployments in other namespaces")
        );
        let text = Emptiness::classify(6, None, "web");
        assert_eq!(
            text.sentence(&DEPLOYMENTS, "kind-fleet-a"),
            "No Deployments match \"web\""
        );
        let chip = Emptiness::classify(6, None, "");
        assert_eq!(
            chip.sentence(&DEPLOYMENTS, "kind-fleet-a"),
            "No Deployments match the filter"
        );
        assert_eq!(text.detail(&DEPLOYMENTS), None);
    }

    #[test]
    fn one_object_elsewhere_is_singular() {
        let namespace = Emptiness::classify(1, Some(("default", 0)), "");
        assert_eq!(
            namespace.detail(&DEPLOYMENTS).as_deref(),
            Some("1 Deployment in another namespace")
        );
        assert_eq!(Noun::of(Page::Ingresses).counted(1), "1 Ingress");
        assert_eq!(Noun::of(Page::Pvcs).counted(2), "2 PVCs");
    }
}
