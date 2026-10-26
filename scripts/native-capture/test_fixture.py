"""Check the real loopback fixture without opening a window or making native evidence."""

import http.client
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest


class FixtureContract(unittest.TestCase):
    def test_stale_and_unavailable_retain_last_good_data_then_refresh_replaces_it(self):
        with tempfile.TemporaryDirectory(prefix="dashboard-fixture-test-") as folder:
            root = Path(folder)
            state = root / "state.txt"
            state.write_text("normal")
            fixture = Path(__file__).with_name("Start-LoopbackPmd.ps1")
            process = subprocess.Popen(
                [shutil.which("pwsh") or "pwsh", "-NoProfile", "-File", str(fixture), "-Root", folder],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                text=True,
            )
            try:
                deadline = time.monotonic() + 15
                while not (root / "port.txt").exists():
                    self.assertIsNone(process.poll(), "Loopback fixture exited")
                    self.assertLess(time.monotonic(), deadline, "Fixture startup timed out")
                    time.sleep(0.05)
                port = int((root / "port.txt").read_text())

                def get(path, code=200):
                    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
                    try:
                        connection.request("GET", path, headers={"Authorization": "Bearer synthetic-issue31-token"})
                        response = connection.getresponse()
                        self.assertEqual(response.status, code)
                        return json.loads(response.read())
                    finally:
                        connection.close()

                def snapshot():
                    document = get("/v1/projects/synthetic-dashboard/status")
                    board = get("/v1/board?project=synthetic-dashboard")
                    work = get("/v1/work?project=synthetic-dashboard&limit=500")
                    self.assertEqual(work["nextCursor"], "synthetic-next-page")
                    self.assertEqual(
                        get("/v1/work?project=synthetic-dashboard&limit=500&cursor=synthetic-next-page"),
                        {"items": [], "nextCursor": None},
                    )
                    self.assertTrue(board["columns"][0]["truncated"])
                    self.assertEqual(len(board["columns"]), 4)
                    self.assertNotIn("health", document["status"])
                    return document, board, work

                normal = snapshot()
                for good_mode in ("normal", "refreshed", "board"):
                    state.write_text(good_mode)
                    good = normal if good_mode == "normal" else snapshot()
                    label = "SYNTHETIC FIXTURE " + good_mode.upper()
                    self.assertEqual(good[0]["status"]["issues"][0]["title"], label)
                    self.assertEqual(good[1]["columns"][0]["items"][0]["title"], label)
                    self.assertEqual(good[2]["items"][0]["title"], label)
                    self.assertEqual(good[1]["generatedAt"], good[0]["generatedAt"])
                    state.write_text("stale")
                    stale = snapshot()
                    self.assertIn("upstream refresh failed", stale[0]["lastError"]["message"])
                    stale[0]["lastError"] = None
                    self.assertEqual(stale, good, "Stale must preserve the complete last-good status, age, Board and work")
                    state.write_text("unavailable")
                    self.assertEqual(get("/v1/projects/synthetic-dashboard/status", 404)["error"]["code"], "no_status")
                    self.assertEqual(get("/v1/board?project=synthetic-dashboard"), good[1])
                    self.assertEqual(get("/v1/work?project=synthetic-dashboard&limit=500"), good[2])
                self.assertNotEqual(normal[0]["generatedAt"], good[0]["generatedAt"])
                self.assertEqual(normal[1]["columns"][0]["items"][0]["id"], good[1]["columns"][0]["items"][0]["id"])
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
