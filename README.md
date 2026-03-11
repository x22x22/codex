# Backup of older version of `main` branch

As explained on https://github.com/openai/codex/issues/14324, the following PR added a number of large binaries to a commit on the `main` branch that should not have gone in:

https://github.com/openai/codex/pull/13953

This PR reverted it:

https://github.com/openai/codex/pull/14232

But keeping this mistake in the history of `main` would forever add a material cost to doing `git clone https://github.com/openai/codex`.

We caught this shortly after more PRs were landed on `main` beyond #14232.

To ameliorate the situation, we went to https://github.com/openai/codex/commit/402ca6cc54141f145c42213e87c4e7b41e40d442 on `main` and pushed it to this branch to ensure all existing commits would be preserved:

https://github.com/openai/codex/tree/main-with-prs-13953-and-14232

We then took `main`, excised these commits from history:

- https://github.com/openai/codex/commit/6baeec68bd1bdc11284885a6d00fa4db4e1327b6
- https://github.com/openai/codex/commit/9b3332e62ffb4c10bfa8c7ac47184d8e6df92c49

and force-pushed the result to `main`, admittedly rewriting history.

If you see any unexpected results when updating your local copy of the Codex repo, this is likely why.
