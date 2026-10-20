use std::collections::BTreeMap;

use crate::filter::{FilterSpec, TextFilter};
use crate::{ResourceRef, Status};

const MISSING: &str = "—";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listener {
    pub status: Status,
    pub name: String,
    pub hostname: Option<String>,
    pub port: u16,
    pub http3: bool,
    pub routes: Option<u32>,
    pub tls: String,
}

impl Listener {
    pub fn port_label(&self) -> String {
        if self.http3 {
            format!("{} · h3", self.port)
        } else {
            self.port.to_string()
        }
    }

    pub fn routes_label(&self) -> String {
        self.routes
            .map_or_else(|| MISSING.to_owned(), |routes| routes.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub labels: BTreeMap<String, String>,
    pub status: Status,
    pub name: String,
    pub host: Option<String>,
    pub backend: String,
    pub ready: Option<(u32, u32)>,
    pub detail: String,
    pub application: Option<ResourceRef>,
}

impl Route {
    pub const FILTER: FilterSpec<Self> = FilterSpec {
        fields: &[
            ("name", |r, (), out| out.push(r.name.as_str().into())),
            ("host", |r, (), out| {
                out.push(r.host.as_deref().unwrap_or_default().into());
            }),
            ("backend", |r, (), out| out.push(r.backend.as_str().into())),
            ("detail", |r, (), out| out.push(r.detail.as_str().into())),
        ],
        namespace: true,
        labels: true,
    };

    pub fn matches(&self, query: &TextFilter) -> bool {
        let namespace = self
            .application
            .as_ref()
            .and_then(|a| a.namespace.as_deref());
        Self::FILTER.matches(self, query, namespace, &[&self.labels])
    }

    pub fn host_label(&self) -> &str {
        self.host.as_deref().unwrap_or(&self.name)
    }

    pub fn ready_label(&self) -> String {
        self.ready.map_or_else(
            || MISSING.to_owned(),
            |(ready, total)| format!("{ready}/{total}"),
        )
    }

    pub fn no_ready_endpoints(&self) -> bool {
        self.ready.is_some_and(|(ready, _)| ready == 0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RouteFilter {
    #[default]
    All,
    Problems,
}

impl RouteFilter {
    pub const ALL: [Self; 2] = [Self::All, Self::Problems];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Problems => "Problems",
        }
    }

    pub fn keeps(self, route: &Route) -> bool {
        match self {
            Self::All => true,
            Self::Problems => route.status != Status::Ok,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Routes {
    pub gateway: String,
    pub envoy_ready: Option<(u32, u32)>,
    pub http3: bool,
    pub listeners: Vec<Listener>,
    pub total: usize,
    pub routes: Vec<Route>,
}

impl Routes {
    pub fn subtitle(&self) -> String {
        let mut parts = vec![format!("Gateway {}", self.gateway)];
        parts.extend(
            self.envoy_ready
                .map(|(ready, desired)| format!("Envoy {ready}/{desired}")),
        );
        if self.http3 {
            parts.push("HTTP/3 on".to_owned());
        }
        parts.join(" · ")
    }

    pub fn problems(&self) -> usize {
        self.routes
            .iter()
            .filter(|route| route.status != Status::Ok)
            .count()
    }

    pub fn filter_counts(&self, query: &TextFilter) -> Vec<(RouteFilter, usize)> {
        RouteFilter::ALL
            .iter()
            .map(|filter| (*filter, self.ranked(*filter, query).len()))
            .collect()
    }

    pub fn ranked(&self, filter: RouteFilter, query: &TextFilter) -> Vec<Route> {
        let mut routes: Vec<Route> = self
            .routes
            .iter()
            .filter(|route| filter.keeps(route) && route.matches(query))
            .cloned()
            .collect();
        routes.sort_by_key(|route| std::cmp::Reverse(route.status));
        routes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(name: &str, status: Status, ready: Option<(u32, u32)>) -> Route {
        Route {
            labels: BTreeMap::new(),
            status,
            name: name.into(),
            host: None,
            backend: String::new(),
            ready,
            detail: String::new(),
            application: None,
        }
    }

    #[test]
    fn list_label_tokens_match_metadata() {
        let mut route = route("web", Status::Ok, None);
        route.labels.insert("team".into(), "checkout".into());
        assert!(route.matches(&TextFilter::parse("label:team=checkout")));
        assert!(!route.matches(&TextFilter::parse("l:team=platform")));
    }

    #[test]
    fn problems_rank_first_and_keep_their_order() {
        let routes = Routes {
            routes: vec![
                route("a", Status::Ok, Some((1, 1))),
                route("b", Status::Warn, Some((0, 2))),
                route("c", Status::Ok, None),
                route("d", Status::Err, None),
            ],
            ..Routes::default()
        };
        let names = |filter, text: &str| -> Vec<String> {
            routes
                .ranked(filter, &TextFilter::parse(text))
                .into_iter()
                .map(|r| r.name)
                .collect()
        };
        assert_eq!(names(RouteFilter::All, ""), ["d", "b", "a", "c"]);
        assert_eq!(names(RouteFilter::Problems, ""), ["d", "b"]);
        assert_eq!(names(RouteFilter::All, "b"), ["b"]);
        assert_eq!(routes.problems(), 2);
    }

    #[test]
    fn labels_follow_the_design() {
        let mut r = route("checkout-worker", Status::Ok, Some((0, 2)));
        assert_eq!(r.host_label(), "checkout-worker");
        assert!(r.no_ready_endpoints());
        r.host = Some("checkout.onloco.app".into());
        assert_eq!(r.host_label(), "checkout.onloco.app");
        assert_eq!(route("x", Status::Warn, None).ready_label(), "—");
        let listener = Listener {
            status: Status::Ok,
            name: "https-apps-0".into(),
            hostname: None,
            port: 443,
            http3: true,
            routes: Some(41),
            tls: String::new(),
        };
        assert_eq!(listener.port_label(), "443 · h3");
        let routes = Routes {
            gateway: "loco-system/eg".into(),
            envoy_ready: Some((3, 3)),
            http3: true,
            ..Routes::default()
        };
        assert_eq!(
            routes.subtitle(),
            "Gateway loco-system/eg · Envoy 3/3 · HTTP/3 on"
        );
    }
}
