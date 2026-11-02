use super::ApplicationStatusPhase;

#[allow(unused_imports)]
mod prelude {
    pub use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
    pub use kube::CustomResource;
    pub use serde::{Deserialize, Serialize};
    pub use std::collections::BTreeMap;
}

use self::prelude::*;

#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[kube(
    group = "infra.loco.io",
    version = "v1alpha1",
    kind = "Application",
    plural = "applications"
)]
#[kube(namespaced)]
#[kube(status = "ApplicationStatus")]
#[kube(schema = "disabled")]
#[kube(derive = "PartialEq")]
pub struct ApplicationSpec {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "blobSpec")]
    pub blob_spec: Option<ApplicationBlobSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "cacheSpec")]
    pub cache_spec: Option<ApplicationCacheSpec>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "databaseSpec"
    )]
    pub database_spec: Option<ApplicationDatabaseSpec>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "deploymentId"
    )]
    pub deployment_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "environmentId"
    )]
    pub environment_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "environmentName"
    )]
    pub environment_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "organizationId"
    )]
    pub organization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "queueSpec")]
    pub queue_spec: Option<ApplicationQueueSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "resourceId"
    )]
    pub resource_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "resourceName"
    )]
    pub resource_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "serviceSpec"
    )]
    pub service_spec: Option<ApplicationServiceSpec>,
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "workspaceId"
    )]
    pub workspace_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationBlobSpec {}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationCacheSpec {}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationDatabaseSpec {}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationQueueSpec {}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationServiceSpec {
    pub deployment: ApplicationServiceSpecDeployment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obs: Option<ApplicationServiceSpecObs>,
    pub resources: ApplicationServiceSpecResources,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<ApplicationServiceSpecRouting>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationServiceSpecDeployment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "envSecretRef"
    )]
    pub env_secret_ref: Option<ApplicationServiceSpecDeploymentEnvSecretRef>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "healthCheck"
    )]
    pub health_check: Option<ApplicationServiceSpecDeploymentHealthCheck>,
    pub image: String,
    pub port: i32,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "runAsGroup"
    )]
    pub run_as_group: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "runAsUser")]
    pub run_as_user: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationServiceSpecDeploymentEnvSecretRef {
    pub name: String,
    pub revision: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationServiceSpecDeploymentHealthCheck {
    #[serde(rename = "failThreshold")]
    pub fail_threshold: i32,
    pub interval: i32,
    pub path: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "startupGracePeriod"
    )]
    pub startup_grace_period: Option<i32>,
    pub timeout: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationServiceSpecObs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<ApplicationServiceSpecObsLogging>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<ApplicationServiceSpecObsMetrics>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationServiceSpecObsLogging {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "retentionPeriod"
    )]
    pub retention_period: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationServiceSpecObsMetrics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationServiceSpecResources {
    pub cpu: String,
    pub memory: String,
    pub replicas: ApplicationServiceSpecResourcesReplicas,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scalers: Option<ApplicationServiceSpecResourcesScalers>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationServiceSpecResourcesReplicas {
    pub max: i32,
    pub min: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationServiceSpecResourcesScalers {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "cpuTarget")]
    pub cpu_target: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "memoryTarget"
    )]
    pub memory_target: Option<i32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationServiceSpecRouting {
    #[serde(rename = "hostName")]
    pub host_name: String,
    #[serde(rename = "idleTimeout")]
    pub idle_timeout: i32,
    #[serde(rename = "pathPrefix")]
    pub path_prefix: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplicationStatus {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "activeDeploymentRef"
    )]
    pub active_deployment_ref: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "completedAt"
    )]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<Condition>>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "deployedGeneration"
    )]
    pub deployed_generation: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "desiredReplicas"
    )]
    pub desired_replicas: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "observedPlacementRevision"
    )]
    pub observed_placement_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<ApplicationStatusPhase>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "readyReplicas"
    )]
    pub ready_replicas: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "startedAt")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "updatedAt")]
    pub updated_at: Option<String>,
}
