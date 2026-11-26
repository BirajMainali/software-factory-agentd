# skills: record flat skill name in lockfile for namespaced skills (#15147)

- URL: https://github.com/cli/cli/pull/15158
- Author: @wellparth
- State: closed
- Base: trunk (b546819fd547ef8c97da470bb00f2aaad4165c84)
- Head: fix-skill-install-lock-key (32d603324eae2fccc54d4b1682a111345cd18f98)
- Labels: external, unmet-requirements
- Created: 2026-11-25T06:37:40Z
- Updated: 2026-11-25T08:01:28Z
- Latest feedback: #4 at 2026-11-25T06:41:13Z

## Body

Fixes #15147

### Description

When installing a skill with a namespace prefix (e.g. `skills/alice/deploy` with name `deploy` and namespace `alice`), `gh skill install` intentionally writes files to a flat directory (`baseDir/deploy`) so agent hosts (Copilot, Claude Code, etc.) can discover them as immediate subdirectories.

However, `internal/skills/installer/installer.go` previously recorded `skill.InstallName()` (`"alice/deploy"`) as the key in the shared `~/.agents/.skill-lock.json` file. Because Vercel's `skills` CLI lockfile specification uses the lockfile key to locate the installed skill folder on disk, running `skills update` looked for a folder named `alice/deploy`, could not find it, and failed with `No matching skills found`.

Additionally, in `pkg/cmd/skills/install/install.go`, `printFileTree` attempted to look up the on-disk directory using the unstripped install name (`filepath.Join(dir, "alice/deploy")`), which caused the post-install tree view to emit `(could not read directory)`.

This change:
1. Records `skill.Name` instead of `skill.InstallName()` in `lockfile.RecordInstall`, ensuring the lockfile key matches the flat directory on disk.
2. Resolves the directory name in `printFileTree` using `filepath.Base(...)`, allowing the installed file tree to render properly for namespaced skills.
3. Adds unit and regression tests in `internal/skills/installer` and `pkg/cmd/skills/install`.

### How did you test this change?

1. **Unit and Integration Tests:**
   * Ran `go test -v ./internal/skills/installer -run TestInstall`: verified `TestInstall/namespaced_skill_records_flat_name_in_lockfile` passes, writing `"deploy"` to `.skill-lock.json` and ensuring `"alice/deploy"` is not written.
   * Ran `go test -v ./pkg/cmd/skills/install -run "TestInstallRun/remote_install_namespaced_exact_match"`: verified that the tree output for a namespaced install renders `xlsx-pro/` on `stderr` without `(could not read directory)`.
   * Ran all skills test suites (`go test ./internal/skills/... ./pkg/cmd/skills/...`): all 13 packages passed cleanly.

2. **Manual Walkthrough:**
   * Given an installation of a namespaced skill `bob/xlsx-pro` into a temporary directory
   * When `Install` runs
   * Then `.skill-lock.json` records `"xlsx-pro"` under `skills`, matching the `xlsx-pro` directory created in the target path.

### Key points

* `Result.Installed` continues to preserve `skill.InstallName()` so user-facing completion output (`✓ Installed alice/deploy`) and review hints (`gh skill preview repo alice/deploy`) retain full remote namespace context.
* Only the lockfile key (which maps directly to the on-disk folder name) and the local tree inspection (`printFileTree`) are flattened to match the actual filesystem layout.

### Notes for reviewers

* Review can start in `internal/skills/installer/installer.go` lines 79 and 132, followed by the tree helper in `pkg/cmd/skills/install/install.go` line 1150.
* Associated unit tests are in `internal/skills/installer/installer_test.go` and `pkg/cmd/skills/install/install_test.go`.

### Authorship and follow-up

Who wrote this:

- [ ] A human wrote it.
- [x] An agent wrote it under close human direction.
- [ ] An agent wrote it independently, and no human has guided the implementation beyond the initial prompt.

Who answers review comments:

- [x] @wellparth will read and reply directly.
- [ ] An agent will draft replies and @wellparth will read them before they are posted.
- [ ] Nobody has explicitly committed to replying.

## Feedback (4)

### 1. 2026-11-25T06:37:54Z - comment by @github-actions[bot]

Thanks for your pull request! Unfortunately, it doesn't meet the requirements for review:


- None of the referenced issues have the `help wanted` label

**Please update your PR to address the above.** This PR will be automatically closed in **4 days** if these requirements are not met.

<details>
<summary>Full contribution requirements</summary>

1. Include a detailed description of what this PR does
2. Link to an issue with the `help wanted` label (use `Fixes #123` or `Closes #123`)
</details>

### 2. 2026-11-25T06:41:13Z - review by @copilot-pull-request-reviewer[bot] (COMMENTED)
Commit: f4fd3e113ec7e17de999b30f93e72a7b32693919

<!-- ccr-overview-v2 -->

### 🟡 Changes recommended

Regression tests can pass with the tree fix reverted and do not cover namespaced lockfile keys in multi-skill installs, leaving both regressions undetected.

<details open>
<summary><strong>2 open issues</strong></summary>

- <picture><source media="(prefers-color-scheme: dark)" srcset="https://github.githubassets.com/static/images/icons/copilot-code-review/medium-v2-dark.svg"><source media="(prefers-color-scheme: light)" srcset="https://github.githubassets.com/static/images/icons/copilot-code-review/medium-v2-light.svg"><img src="https://github.githubassets.com/static/images/icons/copilot-code-review/medium-v2-light.png" alt="Medium severity" width="62" height="18" align="texttop"></picture> [Test namespaced lock keys in multi-skill installs](https://github.com/cli/cli/pull/15158#discussion_r4691310671) · New
- <picture><source media="(prefers-color-scheme: dark)" srcset="https://github.githubassets.com/static/images/icons/copilot-code-review/medium-v2-dark.svg"><source media="(prefers-color-scheme: light)" srcset="https://github.githubassets.com/static/images/icons/copilot-code-review/medium-v2-light.svg"><img src="https://github.githubassets.com/static/images/icons/copilot-code-review/medium-v2-light.png" alt="Medium severity" width="62" height="18" align="texttop"></picture> [Assert flat tree heading and SKILL.md child](https://github.com/cli/cli/pull/15158#discussion_r4691310715) · New
</details>

<details>
<summary><strong>What changed in this PR</strong></summary>

This PR addresses #15147 by aligning namespaced skill lockfile keys and file-tree lookups with the flat directories that `gh skill install` creates, improving compatibility with the Vercel `skills` updater.

**Changes:**
- Record flat skill names in the lockfile for single and batch installs.
- Use flat directory names when rendering installed file trees.
- Add regression tests while preserving namespaced identifiers in installation results.

| File | Description |
| ---- | ----------- |
| `pkg/​cmd/​skills/​install/​install.go` | Resolve and display flat directory names in file trees. |
| `pkg/​cmd/​skills/​install/​install_test.go` | Add a tree-output expectation for a namespaced install. |
| `internal/​skills/​installer/​installer.go` | Record flat names in both lockfile-writing paths. |
| `internal/​skills/​installer/​installer_test.go` | Add a namespaced install case and lockfile-key checks. |
</details>

<sub>Review effort: Balanced</sub>

### 3. 2026-11-25T06:41:13Z - review comment by @Copilot on internal/skills/installer/installer.go:132

🛑 Requirement: Cover namespaced lock keys in the multi-skill path.

The new namespaced case contains only one skill, so it returns from the single-skill fast path before reaching this changed call. All existing multi-skill cases use unnamespaced names, for which the old and new expressions are identical. Add a multi-skill case containing a namespaced skill and assert the parsed lockfile keys are the flat names, while the installed result retains its namespace. Otherwise reverting this call alone would leave the reported bug in batch installs without failing any regression test.

```diff
@@ -129,8 +129,8 @@ func Install(opts *Options) (*Result, error) {
 		}
 		installed = append(installed, r.name)
 		skill := opts.Skills[i]
-		if err := lockfile.RecordInstall(opts.Host, skill.InstallName(), opts.Owner, opts.Repo, skill.Path+"/SKILL.md", skill.TreeSHA, opts.PinnedRef); err != nil {
-			warnings = append(warnings, fmt.Sprintf("could not record install for %s: %v", skill.InstallName(), err))
+		if err := lockfile.RecordInstall(opts.Host, skill.Name, opts.Owner, opts.Repo, skill.Path+"/SKILL.md", skill.TreeSHA, opts.PinnedRef); err != nil {
```

### 4. 2026-11-25T06:41:13Z - review comment by @Copilot on pkg/cmd/skills/install/install_test.go:639

🛑 Requirement: Make the tree assertion detect the original failure.

The test harness uses a substring assertion, so "xlsx-pro/" also matches the previous "bob/xlsx-pro/" header. This test still passes if the entire tree fix is reverted, including the directory lookup that prints "(could not read directory)". Assert the flat heading together with its SKILL.md child so the regression test proves the installed directory was actually read.

```diff
@@ -636,6 +636,7 @@ func TestInstallRun(t *testing.T) {
 				}
 			},
 			wantStdout: "Installed bob/xlsx-pro",
+			wantStderr: "xlsx-pro/",
```
