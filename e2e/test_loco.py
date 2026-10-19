import collections
import re
import unittest

import loco

IMAGES = dict(app="example.test/nginx:1.0@sha256:" + "a" * 64, root="example.test/busybox:1.0")
NAME = re.compile(r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$")


def generated(rollout="1"):
    return list(loco.objects(IMAGES, "registry.test:5000", "http://sources.test", rollout))


def of_kind(kind):
    return [item for item in generated() if item["kind"] == kind]


class LocoScenario(unittest.TestCase):
    def test_thirty_workspaces_with_two_applications_each(self):
        applications = of_kind("Application")
        workspaces = {item["spec"]["workspaceId"] for item in applications}
        self.assertEqual(len(workspaces), len(loco.WORKSPACES))
        self.assertEqual(len(applications), len(loco.WORKSPACES) * len(loco.SERVICES))

    def test_every_failure_names_a_real_application(self):
        for workspace, service in list(loco.FAILURES) + [(w, s) for w, s, _ in loco.BUILDS]:
            self.assertIn(workspace, loco.WORKSPACES)
            self.assertIn(service, loco.SERVICES)

    def test_every_expected_phase_is_covered(self):
        phases = collections.Counter(item["metadata"]["labels"][loco.EXPECT_LABEL] for item in of_kind("Application"))
        self.assertEqual(set(phases), set(loco.PHASES.values()))
        builds = {item["metadata"]["labels"][loco.EXPECT_LABEL] for item in of_kind("Build")}
        self.assertEqual(builds, {"Succeeded", "Failed", "Running"})

    def test_ids_and_names_are_stable_unique_and_valid(self):
        self.assertEqual(generated(), generated())
        names = [(item["kind"], item["metadata"]["name"]) for item in generated()]
        self.assertEqual(len(names), len(set(names)))
        for _, name in names:
            self.assertRegex(name, NAME)
            self.assertLessEqual(len(name), 63)

    def test_hostnames_fit_a_dns_label(self):
        for item in of_kind("Application"):
            routing = item["spec"]["serviceSpec"].get("routing")
            if routing:
                label = routing["hostName"].removesuffix("." + loco.APP_DOMAIN)
                self.assertRegex(label, NAME)
                self.assertLessEqual(len(label), 63)

    def test_only_healthy_applications_get_staged_secrets(self):
        secrets = {item["metadata"]["name"] for item in of_kind("Secret")}
        for item in of_kind("Application"):
            reference = item["spec"]["serviceSpec"]["deployment"].get("envSecretRef")
            phase = item["metadata"]["labels"][loco.EXPECT_LABEL]
            if phase == "Ready":
                self.assertIn(reference["name"], secrets)
            if phase == "Deploying":
                self.assertNotIn(reference["name"], secrets)

    def test_only_the_rollout_changes_with_the_trigger(self):
        before, after = generated("1"), generated("2")
        changed = [a["metadata"]["name"] for a, b in zip(before, after) if a != b]
        rollout = [item["metadata"]["name"] for item in of_kind("Application")
                   if item["metadata"]["labels"][loco.EXPECT_LABEL] == loco.PHASES["rollout"]]
        self.assertEqual(changed, rollout)

    def test_check_reports_missing_pods_crashloops_and_a_finished_rollout(self):
        rollout = dict(kind="Application", metadata=dict(name="r", labels={loco.EXPECT_LABEL: loco.PHASES["rollout"]}),
                       status=dict(phase="Failed"))
        found = loco.problems([rollout])
        self.assertEqual(len(found), 3)

    def test_check_passes_a_realistic_cluster(self):
        crashing = dict(kind="Pod", metadata=dict(namespace="ws-a"),
                        status=dict(containerStatuses=[dict(state=dict(waiting=dict(reason=loco.CRASHLOOP)))]))
        pods = [crashing] + [dict(kind="Pod", metadata=dict(namespace="ws-a"), status={})] * loco.MIN_PODS
        rollout = dict(kind="Application", metadata=dict(name="r", labels={loco.EXPECT_LABEL: loco.PHASES["rollout"]}),
                       status=dict(phase="Deploying"))
        self.assertEqual(loco.problems(pods + [rollout]), [])


if __name__ == "__main__":
    unittest.main()
