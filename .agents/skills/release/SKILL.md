---
name: release
description: "Run the Faz macOS release flow end-to-end: preflight, build, notarize, tag, upload to GitHub Releases, or publish the rolling Nightly preview package. Source of truth is scripts/release.sh and scripts/nightly.sh; this skill is a runbook with one-time setup commands and recovery hints."
when_to_use: "发布, 出版本, 打 tag, release Faz, ship Faz, V0.x.x, release, prepare release, dry-run release, resume release, notarize, nightly, Nightly 包, 更新 nightly, 预览包"
---

# Faz Release Runbook

`scripts/release.sh` is authoritative. This skill captures the local-machine setup and the order Tang and the agent rely on each release.

## TL;DR

```bash
./scripts/release.sh --dry-run                              # preflight only
FAZ_ASC_API_KEY_PATH=/dev/null ./scripts/release.sh        # full release on slow network (recommended)
./scripts/release.sh                                        # full release on fast direct AWS path
./scripts/nightly.sh                                        # rolling Nightly DMG for fixes already on main
```

Both must run on `main`, with a clean tree, in sync with `origin/main`. The script auto-detects the signing identity and reads notarization creds from the login keychain.

The `FAZ_ASC_API_KEY_PATH=/dev/null` prefix skips rcodesign and forces the notarytool fallback. Use it whenever direct connectivity to `notary-submissions-prod.s3.amazonaws.com` is slow or proxied (see "rcodesign S3 connect timeout" under Common blockers). Both keychain entries should still be configured; the env var only redirects the path inside `notarize.sh`.

## Nightly preview package

`scripts/nightly.sh` is authoritative for the rolling `nightly` GitHub prerelease. It builds a release-grade universal app, signs, notarizes, staples, copies `dist/Faz.dmg` to `dist/Faz-nightly.dmg`, and recreates the `nightly` release. It does not bump versions, create a `V*` tag, or refresh any stable release surface.

Rules:

1. Only publish Nightly from the commit users should test. The script checks that HEAD exists on `tw93/Faz` before deleting the old nightly release.
2. Do not tell reporters to try Nightly until `./scripts/nightly.sh` has completed and `gh release view nightly -R tw93/Faz --json tagName,targetCommitish,publishedAt,assets,url` points at the intended commit with `Faz-nightly.dmg`.
3. Do not use `--upload-only` unless the existing `dist/Faz.dmg` is already known to be a signed, notarized, stapled build from the intended commit.
4. Keep public wording precise: "available in the latest Nightly" only after the check above; otherwise say "fixed on main and will be in the next Nightly or release."

## One-time machine setup

Tang's credentials live at `~/save/AppleCertificates/`. The release script checks for either `faz-asc-api-key-path` (rcodesign route, preferred) or `faz-notarytool-profile` (notarytool fallback) in the login keychain.

### Preferred: rcodesign + ASC API key

`rcodesign` is required on macOS 26+ because notarytool can SIGBUS. It is not in homebrew core.

```bash
cargo install apple-codesign         # binary: rcodesign, lands in ~/.cargo/bin
security add-generic-password -s 'faz-asc-api-key-path' -a "faz" \
  -w "$HOME/save/AppleCertificates/asc_api_key.json"
```

The JSON already has `issuer_id`, `key_id`, and `private_key` baked in (rcodesign-native format).

### Fallback: notarytool profile

```bash
xcrun notarytool store-credentials faz-notarytool \
  --apple-id  "$(cat ~/save/AppleCertificates/notary_apple_id.txt)" \
  --team-id   "$(cat ~/save/AppleCertificates/team_id.txt)"  \
  --password  "$(cat ~/save/AppleCertificates/notary_password.txt)"
security add-generic-password -s 'faz-notarytool-profile' -a "faz" -w 'faz-notarytool'
```

### Verify

```bash
which rcodesign
security find-generic-password -s 'faz-asc-api-key-path' -w
security find-identity -v -p codesigning | grep 'Developer ID Application'
gh auth status
```

## Pre-release content checklist

Before invoking `release.sh`, confirm in this order:

1. `faz-cli/Cargo.toml` and `faz-gui/Cargo.toml` versions match (e.g. `0.10.0`).
2. `assets/shell-integration/config_version.txt` is the intended schema version, with highlight rows in the matching docs.
3. `.github/RELEASE_NOTES.md` first heading is `# V<version> <suffix>` (uppercase V, used as the GitHub Release title).
4. Both English `Changelog` and Chinese `更新日志` sections in `RELEASE_NOTES.md` cover the same items.
5. Any pending fixes are committed and pushed to `origin/main`.

Use `./scripts/prep_release.sh <bump>` to draft the version bump and notes when starting from an older tag. Tang typically edits the resulting `.github/RELEASE_NOTES.md` by hand to apply the announcement-writing style (community first, 2 to 4 highlights, user-experience framing).

## Pre-release smoke checklist (runtime-only hotspots)

These are the areas that produce the most post-release bug reports and that CI cannot see (visual layout, native AppKit, shell-in-user-env). Run them by hand in the built `dist/Faz.app` before tagging. Automated coverage already exists for the testable slices: tab width-budget + hover hit-testing have unit tests in `faz-gui/src/tabbar.rs`, and `local`-outside-function / shell syntax is gated by shellcheck + the setup_zsh smoke. The list below is what still needs a human.

1. **macOS window** (#408, #414): on first launch, a single click on the title bar / top inset must not maximize the window; drag the window while it fills the desktop; flip system light/dark and confirm all windows refresh; enter and exit fullscreen cleanly.
2. **Tab bar** (#409, #435, #439, #443, #445, #447): open enough tabs to overflow a narrow window and confirm titles truncate but every tab stays clickable; rename a tab, then click another to switch and confirm no position scramble; confirm a tmux/shell status prompt renders in the bar without clipping.
3. **Shell setup** (#420, #432, #441, #450): from a clean `HOME`, run `faz init` and confirm z / syntax-highlight / autosuggestions are active in a fresh shell; run `faz init --update-only` and confirm it exits clean; open a new shell and confirm `~/.config/faz/zsh/faz.zsh` sources with no error.
4. **AI chat** (#418, #431): run `faz chat`, quit, then run it again in the same window and confirm it reopens.
5. **Render timing / stale drawable** (#452, #458): on the bundled WebGpu backend, sleep the Mac then wake it and confirm the window repaints instead of freezing on the old frame while keystrokes still reach the shell; connect or disconnect an external display and confirm no frozen frame or geometry jump; open a new window straight into fullscreen and resize it, confirming it fills without a stale first frame.
6. **Hyperlinks** (#547): Cmd-hover a URL that the terminal soft-wraps and confirm both rows target the full URL. Then print a URL exactly as wide as the terminal, followed by `\r\n` and unindented unrelated text; the target must exclude that text. Terminal and surface tests cover the grouping; check the built app hover target too.

When a release fixes a bug outside this list, add the reproduction here so the next release re-checks it.

## Verification commands

| Scope | Command |
|---|---|
| Format | `make fmt-check` |
| Compile | `make check` |
| Tests | `make test` |
| Release notes version match | `./scripts/check_release_notes.sh` |
| Config schema versioning | `./scripts/check_release_config.sh` |
| Config release readiness | `./scripts/check_config_release_readiness.sh` |
| Full preflight | `./scripts/release.sh --dry-run` |

`release.sh` runs fmt-check, check, and test again as `stage:checks` after preflight, so it is fine to skip them locally if the dry-run passes.

## Stage map

`./scripts/release.sh` runs these stages in order. Each is timed and labeled `[stage:<name>]`.

1. **preflight** — clean git, version consistency, gh auth, release notes, config, profile, signing identity, notarization creds.
2. **stage:checks** — `make fmt-check && make check && make test`. Set `RUN_CLIPPY=1` to add clippy. Set `SKIP_TESTS=1` to skip tests (avoid).
3. **stage:build** — `./scripts/build.sh`, `PROFILE=release-opt`, `BUILD_ARCH=arm64`. Output: `dist/Faz.app`, `dist/Faz.dmg`, `dist/faz_for_update.zip`, `dist/faz_for_update.zip.sha256`.
4. **stage:notarize** — `./scripts/notarize.sh`. Tries rcodesign first; falls back to notarytool if rcodesign fails and a notarytool profile exists.
5. **stage:tag** — `git tag -a V<version> -m 'Release V<version>'` then `git push origin V<version>`. Idempotent: reuses an existing tag at HEAD instead of dying.
6. **stage:upload** — `gh release create V<version>` (or `gh release edit` if it already exists) with the dmg, zip, and sha256. Title is taken from the first `# ` line of `RELEASE_NOTES.md`.

## Resume after failure

`release.sh` accepts these flags to skip already-completed stages:

| Flag | Skips | Use when |
|---|---|---|
| `--notarize-only` | build | Build succeeded; notarization failed or was interrupted. |
| `--upload-only` | build, notarize | Notarized `dist/Faz.app` and `Faz.dmg` exist on disk; only need to tag + upload. |

Resume flags require the corresponding artifacts in `dist/` to still be present.

## Common blockers

- **`Local main is not synchronized with origin/main`**: push the pending commit. `release.sh` requires the tag to point at a commit that exists on origin.
- **`rcodesign` not found**: `cargo install apple-codesign`, then verify with `which rcodesign`. Do not try `brew install rcodesign`; it is not a core formula.
- **`No Developer ID Application certificate found`**: re-import the Developer ID Application certificate into the login keychain before rerunning the build.
- **`Notarization credentials not found`**: re-run the keychain commands in the setup section above. The script will otherwise prompt interactively, which fails in non-interactive shells.
- **rcodesign S3 connect timeout (3.1s)**: rcodesign uploads the dmg to `notary-submissions-prod.s3.amazonaws.com` via the AWS SDK for Rust, which has a hardcoded 3.1s connect timeout and does not honor `*_proxy` env vars. On networks where direct connect to AWS S3 is slow or proxied (typical for mainland China), it fails consistently with `s3 upload error: HTTP connect timeout occurred after 3.1s`. The error is independent of credentials. Fix: prefix the run with `FAZ_ASC_API_KEY_PATH=/dev/null` to make `notarize.sh` skip rcodesign and use notarytool directly. Both keychain entries should remain set.
- **`rcodesign notarization failed` right after the log shows `"status": "Accepted"`**: Apple accepted; the step died stapling. rcodesign cannot staple an unsigned DMG, which is what `build.sh` produces, and the script used to pass `--staple` (removed 2026-09-10). Do not touch credentials. Recover with `./scripts/notarize.sh --staple-only` then `./scripts/nightly.sh --upload-only`.
- **`codesign: A timestamp was expected but was not found`** (or `timestamp service is not available`): the Apple timestamp round trip failed through the proxy while signing; the certificate is fine. `build.sh` retries three times since 2026-09-10 and now aborts the build if signing still fails, so an unsigned app can no longer reach the DMG. Rerun the build.
- **`stapler` fails with CloudKit timeout, exit code 68**: the log reads `The staple and validate action failed! Error 68` right after `Notarization accepted`. Notarization already succeeded; only ticket retrieval from `api.apple-cloudkit.com` timed out. Same family as the rcodesign S3 timeout, but intermittent rather than systematic, so do NOT re-notarize. Check reachability (`curl -s -o /dev/null -w '%{http_code}' --max-time 20 https://api.apple-cloudkit.com/`; a 400 means reachable), then run `./scripts/notarize.sh --staple-only` and resume with `./scripts/release.sh --upload-only`. The staple-only path regenerates `faz_for_update.zip` and its sha256 on purpose: attaching the ticket changes the app, so the Sparkle archive must be rebuilt after stapling, never before.
- **Tag already exists on origin at a different SHA**: do not force-push tags. Pick the next patch number, bump versions, and start over.

## Environment variable overrides

| Variable | Default | Purpose |
|---|---|---|
| `FAZ_SIGNING_IDENTITY` | auto-detect | Override the Developer ID Application identity. |
| `FAZ_ASC_API_KEY_PATH` | keychain item `faz-asc-api-key-path` | Path to rcodesign ASC API key JSON. |
| `FAZ_NOTARYTOOL_PROFILE` | keychain item `faz-notarytool-profile` | notarytool keychain profile name. |

| `RUN_CLIPPY` | `0` | Set to `1` to run `cargo clippy` during stage:checks. |
| `SKIP_TESTS` | `0` | Set to `1` to skip `make test` during stage:checks. |
| `OUT_DIR` | `<repo>/dist` | Override artifact output directory. |
| `PROFILE` | `release-opt` | Cargo profile. Only `release` and `release-opt` are accepted. |
| `BUILD_ARCH` | `universal` | Passed through to `build.sh`. |

## After release

- GitHub Release URL: `https://github.com/tw93/Faz/releases/tag/V<version>`.
- Sparkle in-app updates are served from the GitHub Release assets (`faz_for_update.zip` + `.sha256`).

For the announcement post (X / WeChat): community first, 2-4 highlights, user-experience framing, one opinionated sentence. The release notes file is a different artifact; do not paste it as the announcement.
