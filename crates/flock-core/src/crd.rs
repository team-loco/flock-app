use serde::{Deserialize, Serialize};

use crate::emptiness::Noun;
use crate::filter::{FilterSpec, TextFilter};
use crate::flux::FluxKind;
use crate::resources::Resource;
use crate::{ClusterId, Command, Page, ResourceRef};

const GROUP_SEPARATOR: char = '.';
const NAMESPACED: &str = "Namespaced";
const CLUSTER_SCOPED: &str = "Cluster";
#[cfg(feature = "loco")]
const LOCO_PAGES: [Page; 2] = [Page::LocoApplications, Page::LocoBuilds];

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

    pub fn flux_kind(&self) -> Option<FluxKind> {
        FluxKind::ALL
            .into_iter()
            .find(|kind| kind.group() == self.group && kind.label() == self.kind)
    }

    pub fn page(&self) -> Option<Page> {
        self.flux_kind()
            .map(FluxKind::page)
            .or_else(|| self.loco_page())
    }

    #[cfg(feature = "loco")]
    fn loco_page(&self) -> Option<Page> {
        let loco = self.group == crate::loco::GROUP;
        LOCO_PAGES
            .into_iter()
            .find(|page| loco && page.singular() == self.kind)
    }

    #[cfg(not(feature = "loco"))]
    #[allow(clippy::unused_self)]
    const fn loco_page(&self) -> Option<Page> {
        None
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

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
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

    pub fn noun(&self) -> Noun {
        let kind = &self.key.kind;
        let plural = match self
            .plural
            .to_lowercase()
            .strip_prefix(&kind.to_lowercase())
        {
            Some(suffix) => format!("{kind}{suffix}"),
            None => capitalized(&self.plural),
        };
        Noun::named(kind.clone(), plural)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub key: CrdKey,
    pub plural: String,
    pub version: String,
    pub namespaced: bool,
    pub objects: Option<usize>,
    pub page: Option<Page>,
}

impl Definition {
    pub fn of(crd: &Crd, objects: Option<usize>) -> Self {
        Self {
            key: crd.key.clone(),
            plural: crd.plural.clone(),
            version: crd.version.clone(),
            namespaced: crd.namespaced,
            objects,
            page: crd.key.page(),
        }
    }

    pub fn name(&self) -> String {
        format!("{}{GROUP_SEPARATOR}{}", self.plural, self.key.group)
    }

    pub const fn scope(&self) -> &'static str {
        if self.namespaced {
            NAMESPACED
        } else {
            CLUSTER_SCOPED
        }
    }

    pub fn tag(&self) -> Option<&'static str> {
        self.page?.section()
    }

    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("kind", |d, (), out| out.push(d.key.kind.as_str().into())),
            ("plural", |d, (), out| out.push(d.plural.as_str().into())),
            ("group", |d, (), out| out.push(d.key.group.as_str().into())),
        ],
        namespace: false,
        labels: false,
    };

    pub fn matches(&self, filter: &TextFilter) -> bool {
        Self::FILTER.matches(self, filter, None, &[])
    }

    pub fn command(&self, cluster: ClusterId) -> Command {
        match self.page {
            Some(page) => Command::SelectPage(page),
            None => Command::OpenCrd {
                cluster,
                crd: self.key.clone(),
            },
        }
    }
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
    fn a_crd_names_its_kind_in_singular_and_plural() {
        let certificate = crd("cert-manager.io", "Certificate", "certificates").noun();
        assert_eq!(certificate.plural, "Certificates");
        assert_eq!(certificate.singular, "Certificate");
        let request = crd(
            "cert-manager.io",
            "CertificateRequest",
            "certificaterequests",
        )
        .noun();
        assert_eq!(request.plural, "CertificateRequests");
        let odd = crd("example.com", "Mouse", "mice").noun();
        assert_eq!(odd.plural, "Mice");
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
    fn definitions_filter_on_kind_plural_or_group() {
        let crds = [
            crd(
                "source.toolkit.fluxcd.io",
                "GitRepository",
                "gitrepositories",
            ),
            crd("source.toolkit.fluxcd.io", "Bucket", "buckets"),
            crd("cert-manager.io", "Certificate", "certificates"),
        ];
        let kinds = |query: &str| -> Vec<String> {
            let filter = TextFilter::parse(query);
            crds.iter()
                .map(|crd| Definition::of(crd, None))
                .filter(|definition| definition.matches(&filter))
                .map(|definition| definition.key.kind)
                .collect()
        };
        assert_eq!(kinds("GITREPO"), ["GitRepository"]);
        assert_eq!(kinds("cert-manager"), ["Certificate"]);
        assert_eq!(kinds("buckets flux"), ["Bucket"]);
        assert_eq!(kinds("nothing"), Vec::<String>::new());
        assert_eq!(crds[0].name(), "gitrepositories.source.toolkit.fluxcd.io");
    }

    #[test]
    fn flux_kinds_open_their_flux_page_and_others_their_object_list() {
        let cluster = ClusterId::new("a");
        let repository = crd(
            "source.toolkit.fluxcd.io",
            "GitRepository",
            "gitrepositories",
        );
        let repository = Definition::of(&repository, None);
        assert_eq!(repository.page, Some(Page::Sources));
        assert_eq!(repository.tag(), Some("Flux"));
        assert_eq!(
            repository.command(cluster.clone()),
            Command::SelectPage(Page::Sources)
        );
        let release = CrdKey::new("helm.toolkit.fluxcd.io", "HelmRelease");
        assert_eq!(release.page(), Some(Page::HelmReleases));
        let bucket = Definition::of(&crd("source.toolkit.fluxcd.io", "Bucket", "buckets"), None);
        assert_eq!(bucket.page, None);
        assert_eq!(bucket.tag(), None);
        assert_eq!(
            bucket.command(cluster.clone()),
            Command::OpenCrd {
                cluster,
                crd: bucket.key.clone()
            }
        );
        let lookalike = CrdKey::new("example.com", "Kustomization");
        assert_eq!(lookalike.page(), None);
        assert_eq!(bucket.scope(), "Namespaced");
    }

    #[cfg(feature = "loco")]
    #[test]
    fn loco_kinds_open_their_loco_page() {
        let application = CrdKey::new(crate::loco::GROUP, "Application");
        assert_eq!(application.page(), Some(Page::LocoApplications));
        let build = Definition::of(&crd(crate::loco::GROUP, "Build", "builds"), None);
        assert_eq!(build.page, Some(Page::LocoBuilds));
        assert_eq!(build.tag(), Some("Loco"));
        assert_eq!(CrdKey::new("argoproj.io", "Application").page(), None);
    }
}
