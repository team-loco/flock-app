use std::borrow::Cow;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::filter::{FilterSpec, Labels, Searchable};
use crate::flux::HelmRevision;
use crate::{ClusterId, FluxKey, Status};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HelmKey {
    pub cluster: ClusterId,
    pub namespace: String,
    pub name: String,
}

impl HelmKey {
    pub fn new(cluster: ClusterId, namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            cluster,
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    #[must_use]
    pub fn on(&self, cluster: &ClusterId) -> Self {
        Self {
            cluster: cluster.clone(),
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelmRelease {
    pub key: HelmKey,
    pub chart: String,
    pub chart_version: String,
    pub app_version: String,
    pub revision: u32,
    pub status: Status,
    pub state: String,
    pub updated: Option<Duration>,
    pub flux: Option<FluxKey>,
}

impl HelmRelease {
    pub fn chart_label(&self) -> String {
        format!("{}-{}", self.chart, self.chart_version)
    }
}

impl Searchable for HelmRelease {
    const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("name", |r, (), out| out.push(r.key.name.as_str().into())),
            ("namespace", |r, (), out| {
                out.push(r.key.namespace.as_str().into());
            }),
            ("chart", |r, (), out| out.push(r.chart_label().into())),
            ("version", |r, (), out| {
                out.push(r.app_version.as_str().into());
            }),
            ("status", |r, (), out| out.push(r.state.as_str().into())),
        ],
        facets: &[
            ("status", |r, (), out| out.push(r.state.as_str().into())),
            ("chart", |r, (), out| out.push(r.chart_label().into())),
        ],
        namespace: true,
        labels: false,
    };

    fn namespace(&self) -> Option<&str> {
        Some(&self.key.namespace)
    }

    fn labels(&self) -> Option<Cow<'_, Labels>> {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelmReleaseDetail {
    pub key: HelmKey,
    pub history: Vec<HelmRevision>,
    pub user_values: String,
    pub computed_values: String,
    pub manifest: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::TextFilter;

    fn release(namespace: &str, name: &str, chart: &str) -> HelmRelease {
        HelmRelease {
            key: HelmKey::new(ClusterId::new("a"), namespace, name),
            chart: chart.into(),
            chart_version: "1.2.0".into(),
            app_version: "2.0".into(),
            revision: 3,
            status: Status::Ok,
            state: "deployed".into(),
            updated: None,
            flux: None,
        }
    }

    #[test]
    fn releases_filter_on_name_namespace_chart_and_state() {
        let releases = [
            release("payments", "checkout-api", "podinfo"),
            release("monitoring", "grafana", "grafana"),
        ];
        let names = |query: &str| -> Vec<String> {
            let query = TextFilter::parse(query);
            releases
                .iter()
                .filter(|r| r.matches(&query))
                .map(|r| r.key.name.clone())
                .collect()
        };
        assert_eq!(names("podinfo-1.2"), ["checkout-api"]);
        assert_eq!(names("ns:monitoring"), ["grafana"]);
        assert_eq!(names("deployed").len(), 2);
        assert_eq!(names("nothing"), Vec::<String>::new());
        assert_eq!(releases[0].chart_label(), "podinfo-1.2.0");
        let moved = releases[0].key.on(&ClusterId::new("b"));
        assert_eq!(moved.cluster, ClusterId::new("b"));
        assert_eq!(moved.name, "checkout-api");
    }
}
