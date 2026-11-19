# Workflow action pins

Every `uses:` in `.github/workflows/` names a full commit SHA and, after it, the release that commit is:

```yaml
- uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0
```

A tag such as `@v4` can be moved upstream, which changes the code a job runs (including the release job, which has
`contents: write`) without a change in this repository (#103). A commit can't move.

## The check

`scripts/check_action_pins.py` fails on any reference that isn't a full lowercase 40-character SHA with a `# vX.Y.Z`
comment. Local actions (`./...`) and containers pinned by `sha256` digest pass. A `uses:` the line reader doesn't
recognize, such as the flow form `{ uses: ... }`, fails rather than going unread.

```sh
python scripts/check_action_pins.py            # offline; CI runs this in the required advisories job
python scripts/check_action_pins.py --verify   # also asks GitHub (gh CLI) that each comment's tag is the pinned commit
python -m unittest discover -s scripts -p 'test_check_action_pins.py' -v
```

There are no exceptions. If an action really can't be pinned, add its `path:line` and the reason to `EXCEPTIONS` in
the script, so the reason shows in the review.

## Pinned actions

Pinned 2026-11-18 to what each workflow's tag resolved to then, keeping each release line:

| Action | Was | Release | Commit |
| --- | --- | --- | --- |
| `actions/checkout` | `@v4` | v4.4.0 | `11d5960a326750d5838078e36cf38b85af677262` |
| `Swatinem/rust-cache` | `@v2` | v2.9.3 | `d1527c8f67ef9af357290475d4af4b0fab8f12e0` |
| `actions/upload-artifact` | `@v4` | v4.6.2 | `ea165f8d65b6e75b540449e92b4886f43607fa02` |
| `actions/download-artifact` | `@v4` | v4.3.0 | `d3f86a106a0bac45b974a628896c90dbdf5c8093` |
| `EmbarkStudios/cargo-deny-action` | already pinned | v2.1.1 | `3c6349835b2b7b196a839186cb8b78e02f7b5f25` |

## Updating a pin

1. Pick the release within the same major line, for example the newest `v4.x.y` of `actions/checkout`. Moving to a new
   major (`v5`) is a separate change: read its release notes, then say so in the pull request title.
2. Resolve its commit from the official repository, following an annotated tag to its commit:

   ```sh
   gh api repos/actions/checkout/git/ref/tags/v4.4.0 --jq '.object.type + " " + .object.sha'
   # if the type is "tag":
   gh api repos/actions/checkout/git/tags/<sha> --jq .object.sha
   ```

3. Replace the SHA and the comment together in every workflow that uses the action, then update the table above.
4. Run `python scripts/check_action_pins.py --verify`. It fails if any comment's tag isn't the pinned commit.
5. If the action is `EmbarkStudios/cargo-deny-action`, also check the cargo-deny version its Dockerfile installs
   against the version `AGENTS.md` names.
