# Main gate qualification

The [issue 68 contract](https://github.com/hemsoft-dev/project-dashboard/issues/68) tracks acceptance.
The qualification PR linked there contains head-specific pending and completed check snapshots. Results belong
in PR comments so recording them does not move the qualified source head.

## Source and baseline

Inspected main: d6a23f9765ed6c6d4edd5615f35b1225203c9900.
The existing Mini checkout was read with git show only. The branch, tree and commit were created through
GitHub APIs, without modifying a checkout.

Source SHA-256:
- .github/workflows/ci.yml: 78ff19ffb4cb446af06e71ac5cabdadb37ee08ece3da40b8c50c83a6d980b877
- .github/workflows/release.yml: a2fbc377e00ab2e427c00aad8677b1e05a52d3f5200744d13389e7f5da1859ad

Before enforcement on November 3, 2026: main protection returned Branch not protected, inherited/repository
rulesets and effective main rules were empty. Secret scanning, push protection and Dependabot security updates
were already enabled; the alerts endpoint succeeded. No security toggle was repeated.

## Applied configuration

[Ruleset 26477626](https://github.com/hemsoft-dev/project-dashboard/rules/26477626), main-required-gates:
- Active branch rule, include refs/heads/main, no exclusions.
- Pull request required; formal approving review count 0; code-owner and last-push approval false;
  stale-review dismissal false; review-thread resolution true.
- No bypass actors. Authenticated administrator read-back: current_user_can_bypass=never.
- Deletion and non-fast-forward blocked.
- Strict required checks; do_not_enforce_on_create=false.
- Required app 15368 for each existing context:
  - advisories
  - core (ubuntu-latest)
  - core (windows-latest)
  - rust (ubuntu-latest)
  - rust (windows-latest)

All five existed and passed on the inspected main revision. This changes no CI jobs.
The required AI-review policy is separate from GitHub's zero formal approval count.

## Reproduce settings evidence

Run these read-only commands with the existing authorized GitHub identity on Mini:
```sh
gh api repos/hemsoft-dev/project-dashboard/rulesets/26477626
gh api repos/hemsoft-dev/project-dashboard/rules/branches/main
gh api 'repos/hemsoft-dev/project-dashboard/rulesets?includes_parents=true&targets=tag'
gh api repos/hemsoft-dev/project-dashboard --jq .security_and_analysis
gh api --include repos/hemsoft-dev/project-dashboard/vulnerability-alerts
```

Expect the four rule types above, empty bypass_actors, current_user_can_bypass=never, the five exact checks
pinned to 15368, strict freshness, no tag-targeting rules, enabled security settings and HTTP 204 alerts.
The classic branch-protection endpoint may still return 404 because this configuration uses a ruleset.

## PR qualification

For the qualification PR, query GraphQL fields headRefOid, baseRefOid, mergeable, mergeStateStatus,
reviewDecision, reviewThreads with isResolved, and statusCheckRollup contexts with each CheckRun's name,
status, conclusion and isRequired(pullRequestNumber: NUMBER). Also read the head and test-merge check-run APIs.

A pending-check blocking snapshot requires all five contexts, at least one pending required check,
mergeStateStatus=BLOCKED, mergeable=MERGEABLE and zero unresolved review threads. Preserve the timestamp,
full head SHA, check URLs and API output in the PR. After those same-head checks pass, record the resulting
merge state without merging. A new head requires fresh evidence. An empty rollup or BLOCKED caused by review
threads is insufficient. Do not create synthetic statuses or approvals to qualify this rule.

## Limits and unexecuted cases

- No direct-main push, force-push or deletion probe was run. Their rejection remains NOT EXECUTED.
  Configuration reads and PR merge blocking do not prove direct-push rejection.
- No intentional failing check or behind-base PR was created. Those runtime rejection cases remain unexecuted.
- No tag, release, deployment or release workflow dispatch was created. Runtime release compatibility remains
  NOT EXECUTED. Static evidence: the new rule targets only main, tag rules remain empty and release.yml is
  unchanged. Existing v* tag and manual triggers, packaging checks and tag-only publication remain intact.
- Release tag ancestry on main and successful CI on the tag commit are conventions, not enforced by this rule.
  The path-filtered release windows job remains outside required checks; requiring it would strand unrelated PRs.
- GitHub treats skipped and neutral check conclusions as acceptable. These five canonical jobs have no job-level
  skip or continue-on-error. PR authors can still edit their workflow under the same names; integration pinning
  does not authenticate workflow contents. Administrators can edit settings, separate from a bypass grant.
- No rule, formal review requirement or other repository configuration was weakened.

Council: GPT 6.1 Sol HIGH + Claude Opus 5.5 HIGH consulted; missing: none; adopted: 5; declined: 0.
The issue records the read-only adviser's corrections and their disposition. No recursive Council was convened.
