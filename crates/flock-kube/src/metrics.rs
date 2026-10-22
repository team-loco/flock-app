use std::collections::BTreeMap;
use std::fmt::Write as _;

use kube::Api;
use kube::api::{DynamicObject, ListParams};
use kube::core::{ApiResource, GroupVersionKind};
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::manager::ClusterHandle;
use crate::probe::PrometheusTarget;

const METRICS_GROUP: &str = "metrics.k8s.io";
const METRICS_VERSION: &str = "v1beta1";
const NODE_METRICS_KIND: &str = "NodeMetrics";
const NODE_METRICS_PLURAL: &str = "nodes";
const NODE_SUMMARY_SUFFIX: &str = "proxy/stats/summary";
const POD_METRICS_KIND: &str = "PodMetrics";
const POD_METRICS_PLURAL: &str = "pods";
const CPU: &str = "cpu";
const MEMORY: &str = "memory";
const NANO_PER_MILLI: f64 = 1e6;
const MICRO_PER_MILLI: f64 = 1e3;
const MILLI_PER_CORE: f64 = 1e3;
const PROMETHEUS_QUERY_PATH: &str = "api/v1/query_range";
const PROMETHEUS_SUCCESS: &str = "success";
const BINARY_SUFFIXES: [(&str, f64); 6] = [
    ("Ki", 1024.0),
    ("Mi", 1_048_576.0),
    ("Gi", 1_073_741_824.0),
    ("Ti", 1_099_511_627_776.0),
    ("Pi", 1_125_899_906_842_624.0),
    ("Ei", 1_152_921_504_606_846_976.0),
];
const DECIMAL_SUFFIXES: [(&str, f64); 9] = [
    ("n", 1e-9),
    ("u", 1e-6),
    ("m", 1e-3),
    ("k", 1e3),
    ("M", 1e6),
    ("G", 1e9),
    ("T", 1e12),
    ("P", 1e15),
    ("E", 1e18),
];

pub fn parse_quantity(text: &str) -> Option<f64> {
    let text = text.trim();
    if let Some((number, factor)) = BINARY_SUFFIXES
        .iter()
        .find_map(|(suffix, factor)| text.strip_suffix(suffix).map(|number| (number, *factor)))
    {
        return number.parse::<f64>().ok().map(|value| value * factor);
    }
    if let Some((number, factor)) = DECIMAL_SUFFIXES
        .iter()
        .find_map(|(suffix, factor)| text.strip_suffix(suffix).map(|number| (number, *factor)))
    {
        return number.parse::<f64>().ok().map(|value| value * factor);
    }
    text.parse::<f64>().ok()
}

pub fn cpu_millicores(text: &str) -> Option<f64> {
    let text = text.trim();
    if let Some(nanos) = text.strip_suffix('n') {
        return nanos
            .parse::<f64>()
            .ok()
            .map(|value| value / NANO_PER_MILLI);
    }
    if let Some(micros) = text.strip_suffix('u') {
        return micros
            .parse::<f64>()
            .ok()
            .map(|value| value / MICRO_PER_MILLI);
    }
    if let Some(millis) = text.strip_suffix('m') {
        return millis.parse::<f64>().ok();
    }
    parse_quantity(text).map(|cores| cores * MILLI_PER_CORE)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub cpu_millicores: f64,
    pub memory_bytes: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeUsage {
    pub name: String,
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeDisk {
    pub name: String,
    pub used_bytes: f64,
    pub capacity_bytes: f64,
}

#[derive(Deserialize)]
struct Summary {
    node: SummaryNode,
}

#[derive(Deserialize)]
struct SummaryNode {
    fs: SummaryFs,
}

#[derive(Deserialize)]
struct SummaryFs {
    #[serde(rename = "usedBytes")]
    used_bytes: f64,
    #[serde(rename = "capacityBytes")]
    capacity_bytes: f64,
}

pub fn parse_node_disk(name: &str, body: &str) -> Option<NodeDisk> {
    let summary: Summary = serde_json::from_str(body).ok()?;
    let fs = summary.node.fs;
    Some(NodeDisk {
        name: name.to_owned(),
        used_bytes: fs.used_bytes,
        capacity_bytes: fs.capacity_bytes,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct PodUsage {
    pub namespace: String,
    pub name: String,
    pub usage: Usage,
}

fn usage_of(value: &serde_json::Value) -> Usage {
    let cpu = value
        .get(CPU)
        .and_then(serde_json::Value::as_str)
        .and_then(cpu_millicores)
        .unwrap_or_default();
    let memory = value
        .get(MEMORY)
        .and_then(serde_json::Value::as_str)
        .and_then(parse_quantity)
        .unwrap_or_default();
    Usage {
        cpu_millicores: cpu,
        memory_bytes: memory,
    }
}

pub fn node_usage_of(object: &DynamicObject) -> Option<NodeUsage> {
    let name = object.metadata.name.clone()?;
    let usage = usage_of(object.data.get("usage")?);
    Some(NodeUsage { name, usage })
}

pub fn pod_usage_of(object: &DynamicObject) -> Option<PodUsage> {
    let name = object.metadata.name.clone()?;
    let namespace = object.metadata.namespace.clone()?;
    let containers = object.data.get("containers")?.as_array()?;
    let usage = containers
        .iter()
        .filter_map(|container| container.get("usage"))
        .map(usage_of)
        .fold(
            Usage {
                cpu_millicores: 0.0,
                memory_bytes: 0.0,
            },
            |total, next| Usage {
                cpu_millicores: total.cpu_millicores + next.cpu_millicores,
                memory_bytes: total.memory_bytes + next.memory_bytes,
            },
        );
    Some(PodUsage {
        namespace,
        name,
        usage,
    })
}

fn metrics_resource(kind: &str, plural: &str) -> ApiResource {
    let gvk = GroupVersionKind::gvk(METRICS_GROUP, METRICS_VERSION, kind);
    ApiResource::from_gvk_with_plural(&gvk, plural)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub labels: BTreeMap<String, String>,
    pub points: Vec<(f64, f64)>,
}

#[derive(Deserialize)]
struct PromResponse {
    status: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    data: Option<PromData>,
}

#[derive(Deserialize)]
struct PromData {
    #[serde(default)]
    result: Vec<PromSeries>,
}

#[derive(Deserialize)]
struct PromSeries {
    #[serde(default)]
    metric: BTreeMap<String, String>,
    #[serde(default)]
    values: Vec<(f64, String)>,
}

pub fn parse_prometheus(body: &str) -> Result<Vec<Series>> {
    let response: PromResponse = serde_json::from_str(body)?;
    if response.status != PROMETHEUS_SUCCESS {
        return Err(Error::Prometheus(response.error.unwrap_or(response.status)));
    }
    let series = response
        .data
        .map(|data| data.result)
        .unwrap_or_default()
        .into_iter()
        .map(|raw| Series {
            labels: raw.metric,
            points: raw
                .values
                .into_iter()
                .filter_map(|(at, value)| {
                    value
                        .parse::<f64>()
                        .ok()
                        .filter(|value| value.is_finite() && at.is_finite())
                        .map(|value| (at, value))
                })
                .collect(),
        })
        .collect();
    Ok(series)
}

fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[derive(Debug, Clone, PartialEq)]
pub struct RangeQuery {
    pub query: String,
    pub start: jiff::Timestamp,
    pub end: jiff::Timestamp,
    pub step_seconds: u32,
}

pub fn range_query_path(target: &PrometheusTarget, query: &RangeQuery) -> String {
    format!(
        "/api/v1/namespaces/{}/services/{}:{}:{}/proxy/{PROMETHEUS_QUERY_PATH}?query={}&start={}&end={}&step={}",
        target.namespace,
        target.scheme,
        target.service,
        target.port,
        encode_query(&query.query),
        query.start.as_second(),
        query.end.as_second(),
        query.step_seconds,
    )
}

impl ClusterHandle {
    pub async fn node_usage(&self) -> Result<Vec<NodeUsage>> {
        let client = self.client()?;
        let resource = metrics_resource(NODE_METRICS_KIND, NODE_METRICS_PLURAL);
        let api: Api<DynamicObject> = Api::all_with(client, &resource);
        let list = api.list(&ListParams::default()).await?;
        Ok(list.items.iter().filter_map(node_usage_of).collect())
    }

    pub async fn node_disk(&self, names: &[String]) -> Vec<NodeDisk> {
        let Ok(client) = self.client() else {
            return Vec::new();
        };
        let fetches = names.iter().map(|name| {
            let client = client.clone();
            async move {
                let path = format!("/api/v1/nodes/{name}/{NODE_SUMMARY_SUFFIX}");
                let request = http::Request::get(path).body(Vec::new()).ok()?;
                let body = client.request_text(request).await.ok()?;
                parse_node_disk(name, &body)
            }
        });
        futures::future::join_all(fetches)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    pub async fn pod_usage(&self, namespace: Option<&str>) -> Result<Vec<PodUsage>> {
        let client = self.client()?;
        let resource = metrics_resource(POD_METRICS_KIND, POD_METRICS_PLURAL);
        let api: Api<DynamicObject> = match namespace {
            Some(namespace) => Api::namespaced_with(client, namespace, &resource),
            None => Api::all_with(client, &resource),
        };
        let list = api.list(&ListParams::default()).await?;
        Ok(list.items.iter().filter_map(pod_usage_of).collect())
    }

    pub async fn prometheus_range(
        &self,
        target: &PrometheusTarget,
        query: &RangeQuery,
    ) -> Result<Vec<Series>> {
        let client = self.client()?;
        let path = range_query_path(target, query);
        let request = http::Request::get(path)
            .body(Vec::new())
            .map_err(|err| Error::Prometheus(err.to_string()))?;
        let body = client.request_text(request).await?;
        parse_prometheus(&body)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn quantities() {
        assert_eq!(parse_quantity("128Mi"), Some(134_217_728.0));
        assert_eq!(parse_quantity("1Gi"), Some(1_073_741_824.0));
        assert_eq!(parse_quantity("1500k"), Some(1_500_000.0));
        assert_eq!(parse_quantity("42"), Some(42.0));
        assert_eq!(parse_quantity("abc"), None);
        assert_eq!(cpu_millicores("250m"), Some(250.0));
        assert_eq!(cpu_millicores("2"), Some(2000.0));
        assert_eq!(cpu_millicores("12345678n"), Some(12.345_678));
        assert_eq!(cpu_millicores("500u"), Some(0.5));
    }

    #[test]
    fn pod_usage_sums_containers() {
        let Ok(object) = serde_json::from_value::<DynamicObject>(json!({
            "apiVersion": "metrics.k8s.io/v1beta1",
            "kind": "PodMetrics",
            "metadata": { "name": "web", "namespace": "apps" },
            "containers": [
                { "name": "a", "usage": { "cpu": "100m", "memory": "64Mi" } },
                { "name": "b", "usage": { "cpu": "50000000n", "memory": "64Mi" } }
            ]
        })) else {
            panic!("fixture");
        };
        let Some(usage) = pod_usage_of(&object) else {
            panic!("usage");
        };
        assert_eq!(usage.usage.cpu_millicores, 150.0);
        assert_eq!(usage.usage.memory_bytes, 134_217_728.0);
    }

    #[test]
    fn node_disk_from_kubelet_summary() {
        let body = r#"{"node":{"nodeName":"w1","fs":{"availableBytes":1,"capacityBytes":53687091200,"usedBytes":49505374208,"inodes":5}}}"#;
        let disk = parse_node_disk("w1", body);
        assert_eq!(
            disk,
            Some(NodeDisk {
                name: "w1".to_owned(),
                used_bytes: 49_505_374_208.0,
                capacity_bytes: 53_687_091_200.0,
            })
        );
        assert_eq!(parse_node_disk("w1", r#"{"node":{}}"#), None);
        assert_eq!(parse_node_disk("w1", "Forbidden"), None);
    }

    #[test]
    fn prometheus_parsing() {
        let body = r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{"pod":"web"},"values":[[1700000000,"1.5"],[1700000060,"NaN"],[1700000120,"x"]]}]}}"#;
        let Ok(series) = parse_prometheus(body) else {
            panic!("parse");
        };
        assert_eq!(series.len(), 1);
        let points = series.first().map(|s| s.points.len());
        assert_eq!(points, Some(1));
        let error = parse_prometheus(r#"{"status":"error","error":"bad query"}"#);
        assert!(matches!(error, Err(Error::Prometheus(ref m)) if m == "bad query"));
    }

    #[test]
    fn proxy_path() {
        let target = PrometheusTarget {
            namespace: "monitoring".to_owned(),
            service: "prometheus-k8s".to_owned(),
            port: 9090,
            scheme: "http".to_owned(),
        };
        let query = RangeQuery {
            query: "sum(rate(x[5m]))".to_owned(),
            start: jiff::Timestamp::from_second(100).unwrap_or_default(),
            end: jiff::Timestamp::from_second(200).unwrap_or_default(),
            step_seconds: 60,
        };
        assert_eq!(
            range_query_path(&target, &query),
            "/api/v1/namespaces/monitoring/services/http:prometheus-k8s:9090/proxy/api/v1/query_range?query=sum%28rate%28x%5B5m%5D%29%29&start=100&end=200&step=60"
        );
    }
}
