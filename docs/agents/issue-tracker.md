# Issue tracker: GitHub

Issues and PRDs for this repo live as GitHub issues. Use the `gh` CLI for all operations.

## Conventions

- **Create an issue**: `gh issue create --title "..." --body "..."`.
- **Read an issue**: `gh issue view <number> --comments`, including its labels.
- **List issues**: `gh issue list --state open --json number,title,body,labels,comments`.
- **Comment on an issue**: `gh issue comment <number> --body "..."`.
- **Apply or remove labels**: `gh issue edit <number> --add-label "..."` or `--remove-label "..."`.
- **Close an issue**: `gh issue close <number> --comment "..."`.

Infer the repository from the local GitHub remote.

## Pull requests as a triage surface

**PRs as a request surface: no.**

## When a skill says “publish to the issue tracker”

Create a GitHub issue.

## When a skill says “fetch the relevant ticket”

Run `gh issue view <number> --comments`.

## Blocking relationships

Use GitHub's native issue dependencies when available. Add a blocker with:

```sh
gh api --method POST repos/darshmahadevia/limitr/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-database-id>
```

The database ID comes from:

```sh
gh api repos/darshmahadevia/limitr/issues/<number> --jq .id
```

If the dependency API is unavailable, include `Blocked by: #<number>` in the issue body.

