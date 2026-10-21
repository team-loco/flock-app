use flock_core::crd::CrdKey;
use flock_core::expiry::{Expiring, SupportEnd, SupportTrack, soonest_first};
use flock_core::{ClusterId, ResourceRef};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use k8s_openapi::ByteString;
use k8s_openapi::api::admissionregistration::v1::{
    MutatingWebhookConfiguration, ValidatingWebhookConfiguration,
};
use k8s_openapi::kube_aggregator::pkg::apis::apiregistration::v1::APIService;
use kube::api::{Api, DynamicObject, ListParams};
use kube::core::ApiResource;
use kube::{Client, ResourceExt};
use serde_json::Value;
use x509_parser::pem::Pem;

use crate::manager::ClusterHandle;
use crate::provider::Provider;

const CERT_GROUP: &str = "cert-manager.io";
const CERT_KIND: &str = "Certificate";
const PEM_CERTIFICATE: &str = "CERTIFICATE";
const READY: &str = "Ready";
const ISSUING: &str = "Issuing";
const TRUE: &str = "True";
const UNKNOWN: &str = "Unknown";
const SEPARATOR: &str = " · ";
const CA_BUNDLE: &str = "caBundle";
const SECONDS_PER_DAY: i64 = 86_400;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    label: String,
    detail: String,
    not_after: Option<Timestamp>,
    ready: bool,
    target: Option<ResourceRef>,
}

impl Found {
    fn at(&self, now: Timestamp) -> Expiring {
        Expiring {
            label: self.label.clone(),
            detail: self.detail.clone(),
            days: self.not_after.map(|at| days_until(now, at)),
            ready: self.ready,
            target: self.target.clone(),
        }
    }
}

fn days_until(now: Timestamp, at: Timestamp) -> i64 {
    let seconds = at.as_second() - now.as_second();
    seconds.div_euclid(SECONDS_PER_DAY)
}

pub fn expiring(found: &[Found], now: Timestamp) -> Vec<Expiring> {
    let mut items: Vec<Expiring> = found.iter().map(|item| item.at(now)).collect();
    soonest_first(&mut items);
    items
}

pub fn support(provider: Provider, version: Option<&str>, now: Timestamp) -> Option<SupportEnd> {
    let track = match provider {
        Provider::Eks => SupportTrack::Eks,
        Provider::Gke => SupportTrack::Gke,
        Provider::Aks => SupportTrack::Aks,
        _ => SupportTrack::Upstream,
    };
    let today = now.to_zoned(TimeZone::system()).date();
    SupportEnd::of(track, version?, today)
}

pub fn certificate_resource(handle: &ClusterHandle) -> Option<ApiResource> {
    let key = CrdKey::new(CERT_GROUP, CERT_KIND);
    crate::crd::find(handle, &key).map(|crd| crate::crd::api_resource(&crd))
}

pub fn earliest_not_after(bundle: &[u8]) -> Option<Timestamp> {
    Pem::iter_from_buffer(bundle)
        .filter_map(Result::ok)
        .filter(|pem| pem.label == PEM_CERTIFICATE)
        .filter_map(|pem| {
            let seconds = pem
                .parse_x509()
                .ok()
                .map(|cert| cert.validity().not_after.timestamp());
            seconds.and_then(|seconds| Timestamp::from_second(seconds).ok())
        })
        .min()
}

pub async fn fetch(
    client: Client,
    cluster: ClusterId,
    certificates: Option<ApiResource>,
) -> Vec<Found> {
    let certs = async {
        match certificates {
            Some(resource) => list_certificates(&client, &cluster, &resource).await,
            None => Vec::new(),
        }
    };
    let (certs, validating, mutating, services) = tokio::join!(
        certs,
        list::<ValidatingWebhookConfiguration>(&client),
        list::<MutatingWebhookConfiguration>(&client),
        list::<APIService>(&client),
    );
    let validating = validating.iter().filter_map(|config| {
        let bundles = config
            .webhooks
            .iter()
            .flatten()
            .map(|hook| (hook.name.as_str(), hook.client_config.ca_bundle.as_ref()));
        webhook(&config.name_any(), bundles)
    });
    let mutating = mutating.iter().filter_map(|config| {
        let bundles = config
            .webhooks
            .iter()
            .flatten()
            .map(|hook| (hook.name.as_str(), hook.client_config.ca_bundle.as_ref()));
        webhook(&config.name_any(), bundles)
    });
    let services = services.iter().filter_map(api_service);
    certs
        .into_iter()
        .chain(validating)
        .chain(mutating)
        .chain(services)
        .collect()
}

async fn list<K>(client: &Client) -> Vec<K>
where
    K: kube::Resource<Scope = k8s_openapi::ClusterResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
    K::DynamicType: Default,
{
    let api: Api<K> = Api::all(client.clone());
    match api.list(&ListParams::default()).await {
        Ok(list) => list.items,
        Err(error) => {
            tracing::debug!(%error, "expiry list skipped");
            Vec::new()
        }
    }
}

async fn list_certificates(
    client: &Client,
    cluster: &ClusterId,
    resource: &ApiResource,
) -> Vec<Found> {
    let api: Api<DynamicObject> = Api::all_with(client.clone(), resource);
    match api.list(&ListParams::default()).await {
        Ok(list) => list
            .items
            .iter()
            .map(|object| certificate(cluster, object))
            .collect(),
        Err(error) => {
            tracing::debug!(%error, "cert-manager certificates skipped");
            Vec::new()
        }
    }
}

fn webhook<'a>(
    config: &str,
    bundles: impl Iterator<Item = (&'a str, Option<&'a ByteString>)>,
) -> Option<Found> {
    let (hook, not_after) = bundles
        .filter_map(|(hook, bundle)| Some((hook, earliest_not_after(&bundle?.0)?)))
        .min_by_key(|(_, not_after)| *not_after)?;
    Some(Found {
        label: format!("Webhook {config}"),
        detail: format!("{hook}{SEPARATOR}{CA_BUNDLE}"),
        not_after: Some(not_after),
        ready: true,
        target: None,
    })
}

fn api_service(service: &APIService) -> Option<Found> {
    let bundle = service.spec.as_ref()?.ca_bundle.as_ref()?;
    let not_after = earliest_not_after(&bundle.0)?;
    Some(Found {
        label: format!("APIService {}", service.name_any()),
        detail: CA_BUNDLE.to_owned(),
        not_after: Some(not_after),
        ready: true,
        target: None,
    })
}

fn condition<'a>(conditions: &'a [Value], kind: &str) -> Option<&'a Value> {
    conditions
        .iter()
        .find(|condition| condition.get("type").and_then(Value::as_str) == Some(kind))
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn ready_line(conditions: &[Value]) -> (bool, String) {
    let Some(ready) = condition(conditions, READY) else {
        return (false, format!("{READY}={UNKNOWN}"));
    };
    let status = field(ready, "status");
    if status == TRUE {
        let issuing = condition(conditions, ISSUING)
            .filter(|issuing| field(issuing, "status") == TRUE)
            .map(|_| format!("{SEPARATOR}{ISSUING}={TRUE}"))
            .unwrap_or_default();
        return (true, format!("{READY}={TRUE}{issuing}"));
    }
    let head = [
        format!("{READY}={status}"),
        field(ready, "reason").to_owned(),
    ];
    let head: Vec<String> = head.into_iter().filter(|part| !part.is_empty()).collect();
    let message = field(ready, "message");
    let line = if message.is_empty() {
        head.join(" ")
    } else {
        format!("{}{SEPARATOR}{message}", head.join(" "))
    };
    (false, line)
}

fn certificate(cluster: &ClusterId, object: &DynamicObject) -> Found {
    let namespace = object.namespace().unwrap_or_default();
    let name = object.name_any();
    let status = object.data.get("status");
    let not_after = status
        .and_then(|status| status.get("notAfter"))
        .and_then(Value::as_str)
        .and_then(|text| text.parse::<Timestamp>().ok());
    let conditions = status
        .and_then(|status| status.get("conditions"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let (ready, detail) = ready_line(conditions);
    let target = ResourceRef {
        cluster: cluster.clone(),
        kind: CrdKey::new(CERT_GROUP, CERT_KIND).qualified(),
        namespace: Some(namespace.clone()),
        name: name.clone(),
    };
    Found {
        label: format!("{CERT_KIND} {namespace}/{name}"),
        detail,
        not_after,
        ready,
        target: Some(target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use serde_json::json;

    const BUNDLE: &str = include_str!("fixtures/ca-bundle.pem");
    const EARLIEST: i64 = 1_795_131_933;

    fn now() -> Timestamp {
        Timestamp::from_second(EARLIEST - 9 * SECONDS_PER_DAY - 60).unwrap_or(Timestamp::UNIX_EPOCH)
    }

    fn cert(status: Value) -> DynamicObject {
        let metadata = ObjectMeta {
            name: Some("wildcard-onloco".into()),
            namespace: Some("ingress".into()),
            ..ObjectMeta::default()
        };
        DynamicObject {
            types: None,
            metadata,
            data: Value::Object(serde_json::Map::from_iter([("status".to_owned(), status)])),
        }
    }

    #[test]
    fn bundle_reports_its_earliest_certificate() {
        let earliest = earliest_not_after(BUNDLE.as_bytes()).map(Timestamp::as_second);
        assert_eq!(earliest, Some(EARLIEST));
    }

    #[test]
    fn bundles_without_certificates_have_no_expiry() {
        assert_eq!(earliest_not_after(b""), None);
        assert_eq!(earliest_not_after(b"not a pem"), None);
        let broken = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
        assert_eq!(earliest_not_after(broken.as_bytes()), None);
    }

    #[test]
    fn webhook_picks_the_hook_that_expires_first() {
        let soon = ByteString(BUNDLE.as_bytes().to_vec());
        let hooks = [("a.kyverno.svc", None), ("b.kyverno.svc", Some(&soon))];
        let found = webhook("kyverno-resource-validating", hooks.into_iter());
        let item = found.map(|found| found.at(now()));
        assert_eq!(
            item.as_ref()
                .map(|item| (item.label.as_str(), item.detail.as_str(), item.days)),
            Some((
                "Webhook kyverno-resource-validating",
                "b.kyverno.svc · caBundle",
                Some(9)
            ))
        );
        assert_eq!(webhook("empty", [("a", None)].into_iter()), None);
    }

    #[test]
    fn failing_certificate_reads_like_the_design() {
        let object = cert(json!({
            "notAfter": "2026-10-29T00:00:00Z",
            "conditions": [{
                "type": "Ready",
                "status": "False",
                "reason": "Failed",
                "message": "ACME order failed: 429 rateLimited",
            }],
        }));
        let cluster = ClusterId::new("prod");
        let item = certificate(&cluster, &object).at("2026-10-20T00:00:00Z"
            .parse()
            .unwrap_or(Timestamp::UNIX_EPOCH));
        assert_eq!(item.label, "Certificate ingress/wildcard-onloco");
        assert_eq!(
            item.detail,
            "Ready=False Failed · ACME order failed: 429 rateLimited"
        );
        assert_eq!(item.days, Some(9));
        assert!(!item.ready);
        assert_eq!(
            item.target.map(|target| target.kind),
            Some("Certificate.cert-manager.io".to_owned())
        );
    }

    #[test]
    fn ready_certificate_mentions_reissue() {
        let issuing = cert(json!({"conditions": [
            {"type": "Ready", "status": "True"},
            {"type": "Issuing", "status": "True"},
        ]}));
        let found = certificate(&ClusterId::new("prod"), &issuing);
        assert_eq!(
            (found.ready, found.detail.as_str()),
            (true, "Ready=True · Issuing=True")
        );
        assert_eq!(found.not_after, None);
        let fresh = certificate(&ClusterId::new("prod"), &cert(json!({})));
        assert_eq!(
            (fresh.ready, fresh.detail.as_str()),
            (false, "Ready=Unknown")
        );
    }

    #[test]
    fn expired_certificates_count_negative_days() {
        let at = Timestamp::from_second(1_000_000).unwrap_or(Timestamp::UNIX_EPOCH);
        let later = Timestamp::from_second(1_000_001).unwrap_or(Timestamp::UNIX_EPOCH);
        assert_eq!(days_until(later, at), -1);
        assert_eq!(days_until(at, at), 0);
    }

    #[test]
    fn providers_pick_their_support_track() {
        let now: Timestamp = "2026-10-20T12:00:00Z"
            .parse()
            .unwrap_or(Timestamp::UNIX_EPOCH);
        let eks = support(Provider::Eks, Some("v1.31.2-eks-1"), now);
        assert_eq!(
            eks.map(|end| end.label),
            Some("EKS v1.31 extended support ends".into())
        );
        let kind = support(Provider::Kind, Some("v1.34.0"), now);
        assert_eq!(
            kind.map(|end| end.label),
            Some("Kubernetes v1.34 support ends".into())
        );
        assert_eq!(support(Provider::Kind, None, now), None);
    }
}
