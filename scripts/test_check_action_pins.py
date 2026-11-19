"""Tests for check_action_pins.py: python -m unittest discover -s scripts -p 'test_check_action_pins.py'"""

from __future__ import annotations

import io
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest import mock

import check_action_pins as pins

ROOT = Path(__file__).resolve().parent.parent
SHA = "11d5960a326750d5838078e36cf38b85af677262"
DIGEST = "0" * 64


class Fixture:
    """A throwaway repository with one workflow file."""

    def __init__(self, body: str):
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name)
        folder = self.root / ".github" / "workflows"
        folder.mkdir(parents=True)
        (folder / "ci.yml").write_text(textwrap.dedent(body), encoding="utf-8")

    def run(self, online: bool = False) -> tuple[int, str]:
        out = io.StringIO()
        code = pins.check(self.root, pins.workflow_files(self.root), online, out)
        return code, out.getvalue()

    def close(self):
        self.dir.cleanup()


def workflow(*uses: str) -> str:
    steps = "\n".join(f"      - uses: {u}" for u in uses)
    return f"jobs:\n  a:\n    runs-on: ubuntu-latest\n    steps:\n{steps}\n"


class CheckTests(unittest.TestCase):
    def check(self, body: str, online: bool = False) -> tuple[int, str]:
        fixture = Fixture(body)
        self.addCleanup(fixture.close)
        return fixture.run(online)

    def test_the_repository_workflows_pass(self):
        out = io.StringIO()
        self.assertEqual(pins.check(ROOT, pins.workflow_files(ROOT), False, out), 0, out.getvalue())
        self.assertIn("0 failing", out.getvalue())

    def test_a_major_version_tag_fails(self):
        code, out = self.check(workflow("actions/checkout@v4"))
        self.assertEqual(code, 1)
        self.assertIn("ci.yml:5 actions/checkout@v4: mutable ref 'v4'", out)

    def test_branch_short_sha_uppercase_sha_and_no_ref_fail(self):
        for ref, reason in [
            ("actions/checkout@main", "mutable ref 'main'"),
            ("actions/checkout@11d5960", "short or uppercase SHA"),
            (f"actions/checkout@{SHA.upper()}", "short or uppercase SHA"),
            ("actions/checkout", "no ref"),
        ]:
            with self.subTest(ref=ref):
                code, out = self.check(workflow(ref))
                self.assertEqual(code, 1)
                self.assertIn(reason, out)

    def test_a_pinned_sha_needs_its_release_comment(self):
        for comment in ["", " # pinned", " #"]:
            with self.subTest(comment=comment):
                code, out = self.check(workflow(f"actions/checkout@{SHA}{comment}"))
                self.assertEqual(code, 1)
                self.assertIn("no `# vX.Y.Z` comment", out)

    def test_pinned_and_labelled_references_pass_in_every_spelling(self):
        body = textwrap.dedent(f"""\
            jobs:
              a:
                runs-on: ubuntu-latest
                steps:
                  - uses: actions/checkout@{SHA} # v4.4.0
                  - name: quoted
                    uses: "actions/upload-artifact@{SHA}" #v4.6.2
                  - uses: 'github/codeql-action/init@{SHA}' # v3
                  - uses: ./.github/actions/local
                  - uses: docker://alpine@sha256:{DIGEST}
              reusable:
                uses: octo/workflows/.github/workflows/build.yml@{SHA} # v1.2
            """)
        code, out = self.check(body)
        self.assertEqual(code, 0, out)
        self.assertIn("6 references, 0 failing", out)

    def test_a_container_tag_fails(self):
        code, out = self.check(workflow("docker://alpine:3.20"))
        self.assertEqual(code, 1)
        self.assertIn("not pinned to a sha256 digest", out)

    def test_an_unrecognized_uses_form_fails_rather_than_passing_unread(self):
        body = "jobs:\n  a:\n    steps:\n      - { uses: actions/checkout@v4 }\n"
        code, out = self.check(body)
        self.assertEqual(code, 1)
        self.assertIn("unrecognized `uses:` form", out)

    def test_comments_and_prose_are_not_references(self):
        body = workflow(f"actions/checkout@{SHA} # v4.4.0") + "      # uses: actions/checkout@v4 in a comment\n"
        body += "      - run: echo 'this step uses no action'\n"
        code, out = self.check(body)
        self.assertEqual(code, 0, out)

    def test_no_references_at_all_fails(self):
        code, out = self.check("jobs:\n  a:\n    steps:\n      - run: true\n")
        self.assertEqual(code, 1)
        self.assertIn("no `uses:` references found", out)

    def test_an_exception_allows_exactly_its_line(self):
        fixture = Fixture(workflow("actions/checkout@v4", "actions/cache@v4"))
        self.addCleanup(fixture.close)
        with mock.patch.dict(pins.EXCEPTIONS, {".github/workflows/ci.yml:5": "test reason"}):
            code, out = fixture.run()
        self.assertEqual(code, 1)
        self.assertIn("allowed .github/workflows/ci.yml:5 actions/checkout@v4: test reason", out)
        self.assertIn("error   .github/workflows/ci.yml:6 actions/cache@v4", out)

    def test_main_reports_a_missing_file_as_a_usage_error(self):
        with mock.patch("sys.stderr", io.StringIO()):
            self.assertEqual(pins.main([str(ROOT / "no-such-workflow.yml")]), 2)


class VerifyTests(unittest.TestCase):
    """--verify, with GitHub's API answers faked."""

    def run_online(self, answers: dict[str, dict]) -> tuple[int, str]:
        fixture = Fixture(workflow(f"actions/checkout@{SHA} # v4.4.0"))
        self.addCleanup(fixture.close)

        def fake(endpoint: str):
            if endpoint not in answers:
                raise RuntimeError("HTTP 404: Not Found")
            return answers[endpoint]

        with mock.patch.object(pins, "gh_json", side_effect=fake):
            return fixture.run(online=True)

    def test_a_lightweight_tag_on_the_pinned_commit_passes(self):
        code, out = self.run_online({"repos/actions/checkout/git/ref/tags/v4.4.0": {"object": {"type": "commit", "sha": SHA}}})
        self.assertEqual(code, 0, out)
        self.assertIn("ok      .github/workflows/ci.yml:5", out)

    def test_an_annotated_tag_is_followed_to_its_commit(self):
        code, out = self.run_online({
            "repos/actions/checkout/git/ref/tags/v4.4.0": {"object": {"type": "tag", "sha": "a" * 40}},
            f"repos/actions/checkout/git/tags/{'a' * 40}": {"object": {"type": "commit", "sha": SHA}},
        })
        self.assertEqual(code, 0, out)

    def test_a_tag_on_another_commit_fails(self):
        code, out = self.run_online({"repos/actions/checkout/git/ref/tags/v4.4.0": {"object": {"type": "commit", "sha": "b" * 40}}})
        self.assertEqual(code, 1)
        self.assertIn(f"actions/checkout v4.4.0 is {'b' * 40}, not the pinned {SHA}", out)

    def test_a_missing_tag_fails(self):
        code, out = self.run_online({})
        self.assertEqual(code, 1)
        self.assertIn("couldn't read actions/checkout v4.4.0", out)


if __name__ == "__main__":
    unittest.main()
