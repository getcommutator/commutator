---
description: Bump Commutator's version, tag it and push, so CI publishes the macOS, Android and Linux release
argument-hint: "[patch|minor|major|X.Y.Z]"
---

Release a new Commutator version. Pushing the tag `v<version>` is the whole release: `.github/workflows/release.yml`
then notarizes the macOS app and creates the GitHub release (`make release`: the app tarball and `manifest.json` the
desktop updater reads, the dmg, notes from `--generate-notes`), and its `publish` job attaches the signed APK the phone's
updater looks for and the Linux engine builds `install.sh` downloads. When the iOS app changed since the previous tag (`apps/ios/`, `scripts/ios/` or a crate the iOS core
links; the `ios-changes` job decides), the same run uploads a TestFlight build too (`testflight.yml`). `make release` names the release after `[workspace.package] version` in `Cargo.toml`, not after the
tag, so the two must match.

Argument: `$ARGUMENTS`. Empty means `patch`; `minor` and `major` bump that part; `X.Y.Z` is the exact version.

1. Check the state; stop and say why if any of these fail:
   - the branch is `main` and `git fetch origin --tags` shows it is not behind `origin/main`;
   - `Cargo.toml` and `Cargo.lock` have no uncommitted changes (the bump commit must hold only the version);
   - the new version is greater than the current one (`sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1`, the
     Makefile's `VERSION`) and `v<new>` is not already a tag;
   - the `mobile` run on `HEAD` (it builds the core and the phone apps), if one ran, did not fail: `gh run list
     --workflow mobile.yml --commit $(git rev-parse HEAD) --json databaseId,status,conclusion`. Still running: wait
     with `gh run watch <id> --exit-status` (not piped, or the exit code is lost). Failed: stop with
     `gh run view <id> --log-failed`.
   Other uncommitted files are fine: leave them alone, do not stash or commit them.
2. Show what changed since the last release: `git log --no-merges --format='- %s' $(git describe --tags --abbrev=0)..HEAD`.
   If the list is empty, stop: there is nothing to release.
3. Set `version = "<new>"` under `[workspace.package]` in `Cargo.toml`, then `cargo update --workspace --offline` to
   update `Cargo.lock`. Then `cargo check --workspace --locked`; if it fails, revert the two files and stop with the output.
4. Commit only `Cargo.toml` and `Cargo.lock` as `release: <new>`, tag it `git tag v<new>`, then
   `git push origin main v<new>`.
5. Find the run with `gh run list --workflow release.yml --branch v<new> --limit 1` and `gh run watch <id>
   --exit-status` in the background. When it ends, report the release URL (`gh release view v<new> --json url -q .url`)
   and check its assets hold the macOS tarball, `manifest.json`, the dmg, `commutator-<new>-android-*.apk` and
   `commutator-<new>-linux-{x86_64,aarch64}.tar.gz`. Say
   whether TestFlight ran (the `testflight` job, or skipped: no iOS changes) and its build number from the run summary.
   On a failure, the failing job's log.
