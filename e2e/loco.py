#!/usr/bin/env python3
import json
import os
import sys
import uuid

LOCO_NAMESPACE = "loco-system"
BUILD_NAMESPACE = "loco-builds"
API_VERSION = "infra.loco.io/v1alpha1"
EXPECT_LABEL = "flock.dev/expect"
ORGANIZATION_ID = "019a2b10-0000-7000-8000-000000000001"
APP_DOMAIN = "onloco.app"
APP_PORT = 8080
CPU = "100m"
MEMORY = "32Mi"
HEALTHY_REPLICAS = (1, 1, 2, 1)
ROLLOUT_REPLICAS = 4
ROLLOUT_GRACE_SECONDS = 180
HEALTH_INTERVAL_SECONDS = 30
CRASHLOOP_INTERVAL_SECONDS = 10
HEALTH_TIMEOUT_SECONDS = 2
HEALTH_FAIL_THRESHOLD = 2
IDLE_TIMEOUT_SECONDS = 60
MAX_REPLICAS = 6
PLACEMENT_REVISION = 3
MISSING_TAG = "flock-missing-tag"
MISSING_HEALTH_PATH = "/flock-missing-health"
MISSING_DOCKERFILE = "deploy/Dockerfile"
MISSING_SOURCE = "flock-missing-source.tar.gz"
OK_SOURCE = "ok.tar.gz"
SLOW_SOURCE = "slow.tar.gz"

WORKSPACES = (
    "acme", "northwind", "globex", "initech", "umbrella", "hooli", "stark", "wayne",
    "tyrell", "cyberdyne", "soylent", "vandelay", "wonka", "oscorp", "aperture", "massive-dynamic",
    "pied-piper", "dunder-mifflin", "gringotts", "monsters-inc", "nakatomi", "prestige-worldwide",
    "sterling-cooper", "bluth-company", "los-pollos-hermanos", "krusty-krab", "planet-express",
    "black-mesa-research-facility-internal-tooling", "weyland-yutani", "duff-brewing",
)
SERVICES = ("web", "api")
ENVIRONMENTS = ("production", "staging", "preview-checkout-redesign-with-a-really-long-branch-name")

FAILURES = {
    ("acme", "api"): "crashloop",
    ("globex", "web"): "crashloop",
    ("hooli", "api"): "crashloop",
    ("tyrell", "web"): "crashloop",
    ("oscorp", "api"): "crashloop",
    ("nakatomi", "web"): "crashloop",
    ("initech", "web"): "image-pull",
    ("wonka", "api"): "image-pull",
    ("krusty-krab", "web"): "image-pull",
    ("stark", "api"): "root-image",
    ("bluth-company", "api"): "root-image",
    ("umbrella", "web"): "env-pending",
    ("vandelay", "api"): "env-pending",
    ("planet-express", "web"): "rollout",
}
PHASES = {
    "healthy": "Ready",
    "crashloop": "Failed",
    "image-pull": "Failed",
    "root-image": "Failed",
    "env-pending": "Deploying",
    "rollout": "Rollout",
}
ROLLOUT_PHASES = ("Deploying", "Ready")
MIN_PODS = 200
WORKSPACE_PREFIX = "ws-"
CRASHLOOP = "CrashLoopBackOff"
BUILDS = (
    ("acme", "web", "Succeeded"),
    ("northwind", "web", "Succeeded"),
    ("hooli", "web", "Succeeded"),
    ("pied-piper", "api", "Succeeded"),
    ("black-mesa-research-facility-internal-tooling", "web", "Succeeded"),
    ("initech", "web", "Failed"),
    ("wonka", "api", "Failed"),
    ("planet-express", "web", "Running"),
)


def stable_id(*parts):
    digest = uuid.uuid5(uuid.NAMESPACE_URL, "/".join(("flock-loco",) + parts)).hex
    return f"019a{digest[4:8]}-{digest[8:12]}-7{digest[13:16]}-8{digest[17:20]}-{digest[20:32]}"


def workspace_id(workspace):
    return stable_id(workspace)


def resource_id(workspace, service):
    return stable_id(workspace, service)


def scenario(workspace, service):
    return FAILURES.get((workspace, service), "healthy")


def environment(index):
    return ENVIRONMENTS[index % len(ENVIRONMENTS)]


def application_name(workspace, service):
    return f"resource-{resource_id(workspace, service)}"


def placement_id(workspace, service):
    return stable_id(workspace, service, "placement")


def env_secret_name(workspace, service):
    return f"env-{placement_id(workspace, service)}"


def hostname(workspace, service, env):
    label = f"{workspace}-{service}" if env == ENVIRONMENTS[0] else f"{workspace}-{service}-{env}"
    return f"{label[:63].rstrip('-')}.{APP_DOMAIN}"


def image(kind, images):
    if kind == "image-pull":
        return f"{images['app'].split('@')[0].rsplit(':', 1)[0]}:{MISSING_TAG}"
    if kind == "root-image":
        return images["root"]
    return images["app"]


def health_check(kind):
    path = MISSING_HEALTH_PATH if kind == "crashloop" else "/"
    interval = CRASHLOOP_INTERVAL_SECONDS if kind == "crashloop" else HEALTH_INTERVAL_SECONDS
    check = dict(path=path, interval=interval, timeout=HEALTH_TIMEOUT_SECONDS,
                 failThreshold=HEALTH_FAIL_THRESHOLD)
    if kind == "rollout":
        check["startupGracePeriod"] = ROLLOUT_GRACE_SECONDS
    return check


def replicas(kind, index):
    if kind == "rollout":
        return ROLLOUT_REPLICAS
    if kind == "healthy":
        return HEALTHY_REPLICAS[index % len(HEALTHY_REPLICAS)]
    return 1


def deployment(workspace, service, kind, images, rollout):
    spec = dict(image=image(kind, images), port=APP_PORT, healthCheck=health_check(kind),
                env={"NGINX_ENTRYPOINT_WORKER_PROCESSES_AUTOTUNE": "1", "LOCO_WORKSPACE": workspace})
    if kind == "rollout":
        spec["env"]["FLOCK_ROLLOUT"] = rollout
    if kind in ("healthy", "env-pending"):
        spec["envSecretRef"] = dict(name=env_secret_name(workspace, service), revision=PLACEMENT_REVISION)
    return spec


def application(index, workspace, service, images, rollout):
    kind = scenario(workspace, service)
    env = environment(index)
    count = replicas(kind, index)
    service_spec = dict(
        deployment=deployment(workspace, service, kind, images, rollout),
        resources=dict(cpu=CPU, memory=MEMORY,
                       replicas=dict(min=count, max=max(count, MAX_REPLICAS))),
    )
    if service == "web":
        service_spec["routing"] = dict(hostName=hostname(workspace, service, env), pathPrefix="/",
                                       idleTimeout=IDLE_TIMEOUT_SECONDS)
    return dict(
        apiVersion=API_VERSION,
        kind="Application",
        metadata=dict(
            name=application_name(workspace, service),
            namespace=LOCO_NAMESPACE,
            labels={EXPECT_LABEL: PHASES[kind]},
            annotations={"loco.io/placement-id": placement_id(workspace, service),
                         "loco.io/placement-revision": str(PLACEMENT_REVISION)},
        ),
        spec=dict(
            type="SERVICE",
            resourceId=resource_id(workspace, service),
            organizationId=ORGANIZATION_ID,
            workspaceId=workspace_id(workspace),
            region="local",
            environmentId=stable_id(workspace, env),
            environmentName=env,
            deploymentId=stable_id(workspace, service, "deployment", str(PLACEMENT_REVISION)),
            serviceSpec=service_spec,
        ),
    )


def env_secret(workspace, service):
    return dict(
        apiVersion="v1",
        kind="Secret",
        type="Opaque",
        metadata=dict(name=env_secret_name(workspace, service), namespace=LOCO_NAMESPACE,
                      labels={"loco.io/placement-revision": str(PLACEMENT_REVISION)}),
        stringData={"DATABASE_URL": f"postgres://{workspace}@db.internal/{service}"},
    )


def build_source(phase, index, sources):
    if phase == "Running":
        return f"{sources}/{SLOW_SOURCE}", "Dockerfile"
    if phase == "Failed" and index % 2 == 0:
        return f"{sources}/{OK_SOURCE}", MISSING_DOCKERFILE
    if phase == "Failed":
        return f"{sources}/{MISSING_SOURCE}", "Dockerfile"
    return f"{sources}/{OK_SOURCE}", "Dockerfile"


def build_id(workspace, service):
    return stable_id(workspace, service, "build")


def build(index, workspace, service, phase, registry, sources):
    url, dockerfile = build_source(phase, index, sources)
    identifier = build_id(workspace, service)
    return dict(
        apiVersion=API_VERSION,
        kind="Build",
        metadata=dict(name=f"build-{identifier}", namespace=BUILD_NAMESPACE, labels={EXPECT_LABEL: phase}),
        spec=dict(
            buildId=identifier,
            organizationId=ORGANIZATION_ID,
            workspaceId=workspace_id(workspace),
            resourceId=resource_id(workspace, service),
            sourceURL=url,
            dockerfilePath=dockerfile,
            context=".",
            imageRepository=f"{registry}/ws-{workspace_id(workspace)}/{application_name(workspace, service)}",
        ),
    )


def pairs():
    return [(workspace, service) for workspace in WORKSPACES for service in SERVICES]


def objects(images, registry, sources, rollout):
    for index, (workspace, service) in enumerate(pairs()):
        if scenario(workspace, service) == "healthy":
            yield env_secret(workspace, service)
        yield application(index, workspace, service, images, rollout)
    for index, (workspace, service, phase) in enumerate(BUILDS):
        yield build(index, workspace, service, phase, registry, sources)


def waiting_reasons(pod):
    statuses = pod.get("status", {}).get("containerStatuses") or []
    return {status.get("state", {}).get("waiting", {}).get("reason") for status in statuses}


def problems(items):
    pods = [item for item in items if item["kind"] == "Pod"]
    found = []
    if len(pods) < MIN_PODS:
        found.append(f"{len(pods)} pods, expected at least {MIN_PODS}")
    crashing = [pod for pod in pods if pod["metadata"]["namespace"].startswith(WORKSPACE_PREFIX)
                and CRASHLOOP in waiting_reasons(pod)]
    if not crashing:
        found.append("no workspace pod is in CrashLoopBackOff")
    rollouts = [item for item in items if item["kind"] == "Application"
                and item["metadata"].get("labels", {}).get(EXPECT_LABEL) == PHASES["rollout"]]
    for item in rollouts:
        phase = item.get("status", {}).get("phase")
        if phase not in ROLLOUT_PHASES:
            found.append(f"rollout {item['metadata']['name']} is {phase}")
    if not rollouts:
        found.append("no rollout Application")
    return found


def check(path):
    with open(path) as stream:
        found = problems(json.load(stream)["items"])
    for problem in found:
        print(problem, file=sys.stderr)
    sys.exit(1 if found else 0)


if __name__ == "__main__" and sys.argv[1:2] == ["--check"]:
    check(sys.argv[2])
elif __name__ == "__main__":
    images = dict(app=os.environ["FLOCK_LOCO_APP_IMAGE"], root=os.environ["FLOCK_LOCO_ROOT_IMAGE"])
    items = list(objects(images, os.environ["FLOCK_LOCO_REGISTRY"], os.environ["FLOCK_LOCO_SOURCES"],
                         os.environ["FLOCK_LOCO_ROLLOUT"]))
    print(json.dumps(dict(apiVersion="v1", kind="List", items=items)))
