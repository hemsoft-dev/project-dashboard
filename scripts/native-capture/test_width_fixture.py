"""Exercise the production HTTP fixture without creating native evidence."""

import http.client
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest


class WidthFixture(unittest.TestCase):
    def test_both_binaries_receive_identical_long_and_short_activity_and_counts(self):
        with tempfile.TemporaryDirectory(prefix="dashboard-width-fixture-") as folder:
            root = Path(folder)
            (root / "session.txt").write_text("baseline-default")
            process = subprocess.Popen(
                [shutil.which("pwsh") or "pwsh", "-NoProfile", "-File",
                 str(Path(__file__).with_name("Start-WidthPmd.ps1")), "-Root", folder],
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True,
            )
            try:
                deadline = time.monotonic() + 15
                while not (root / "port.txt").exists():
                    self.assertIsNone(process.poll(), "Width fixture exited")
                    self.assertLess(time.monotonic(), deadline, "Width fixture startup timed out")
                    time.sleep(0.05)
                port = int((root / "port.txt").read_text())

                def get(path, code=200, token="synthetic-issue27-token"):
                    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
                    try:
                        connection.request("GET", path, headers={"Authorization": f"Bearer {token}"})
                        response = connection.getresponse()
                        self.assertEqual(response.status, code)
                        return json.loads(response.read())
                    finally:
                        connection.close()

                projects = get("/v1/projects")["projects"]
                self.assertEqual([p["repo"] for p in projects], ["fixture/long", "fixture/short"])
                self.assertTrue(all(p["tracked"] and p["priority"] == 1 for p in projects))
                snapshot = {}
                for project in projects:
                    kind = project["slug"]
                    path = f"/v1/projects/{kind}/status"
                    document = get(path)
                    status = document["status"]
                    self.assertEqual(status["schemaVersion"], 1)
                    self.assertEqual(status["repository"]["nameWithOwner"], project["repo"])
                    self.assertNotIn("health", status)
                    self.assertNotIn("verdict", status)
                    self.assertIsNone(document["lastError"])
                    self.assertEqual(len(status["issues"]), 1)
                    self.assertEqual(len(status["pullRequests"]["open"]), 1)
                    self.assertEqual(status["pullRequests"]["merged"], [])
                    self.assertEqual(len(status["workflowRuns"]), 1)
                    run = status["workflowRuns"][0]
                    self.assertGreater(run["startedAt"], status["issues"][0]["updatedAt"])
                    self.assertGreater(run["startedAt"], status["pullRequests"]["open"][0]["updatedAt"])
                    self.assertEqual(run["startedAt"], document["generatedAt"])
                    if kind == "short":
                        self.assertEqual(run["workflow"], "CI")
                    else:
                        self.assertTrue(run["workflow"].startswith("SYNTHETIC LONG"))
                        self.assertGreater(len(run["workflow"]), 150)
                    self.assertTrue(run["title"].startswith("DESCRIPTION" if kind == "long" else "SYNTHETIC SHORT"))
                    self.assertEqual(len(get(f"/v1/board?project={kind}")["columns"]), 4)
                    work_path = f"/v1/work?project={kind}&limit=500"
                    self.assertEqual(get(work_path), {"items": [], "nextCursor": None})
                    snapshot[path] = document
                (root / "session.txt").write_text("head-saved")
                for path, original in snapshot.items():
                    self.assertEqual(get(path), original, "Head/baseline must receive the same complete status")
                self.assertEqual(get("/v1/projects", 401, "wrong")["error"]["code"], "unauthorized")
                self.assertEqual(get("/unknown", 404)["error"]["code"], "not_found")
                saved = json.loads((root / "fixture-data.json").read_text(encoding="utf-8-sig"))
                for path, original in snapshot.items():
                    self.assertEqual(saved[path], original)
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
                _, stderr = process.communicate()
                self.assertFalse(stderr, stderr)


if __name__ == "__main__":
    unittest.main()
