# Issue tracker: GitHub

Issues and specs for this repo live as GitHub issues on `alacritree/alacritree`. Use the `gh` CLI for all operations.

## Repository

Pass `-R alacritree/alacritree` to every `gh issue` and `gh pr` command. This checkout has two GitHub remotes, `origin` for the project and `fork` for `AbysmalBiscuit/alacritree`, so `gh` can resolve the wrong one when it infers the repository.

Issues closed before the move to the org stay on `AbysmalBiscuit/alacritree`. Read those with `-R AbysmalBiscuit/alacritree`. Never create new issues there.

Open issues also sit on the org project `https://github.com/orgs/alacritree/projects/1`.

## Conventions

- Create an issue with `gh issue create -R alacritree/alacritree --title "..." --body "..."`. Use a heredoc for a multi-line body.
- Read an issue with `gh issue view <number> -R alacritree/alacritree --json number,title,body,labels,comments`.
- List issues with `gh issue list -R alacritree/alacritree --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'`. Add `--label` and `--state` filters as needed.
- Make an issue a sub-issue with `gh issue create -R alacritree/alacritree --parent <parent> ...`, or with `gh issue edit <parent> -R alacritree/alacritree --add-sub-issue <child>` afterwards (`gh` 2.94+). On an older `gh`, use `gh api --method POST repos/alacritree/alacritree/issues/<parent>/sub_issues -F sub_issue_id=<child-db-id>`, with the database id as in the blocking rule below. Without sub-issues, put `Part of #<parent>` at the top of the child body.
- Comment with `gh issue comment <number> -R alacritree/alacritree --body "..."`.
- Apply or remove a label with `gh issue edit <number> -R alacritree/alacritree --add-label "..."` or `--remove-label "..."`.
- Close with `gh issue close <number> -R alacritree/alacritree --comment "..."`.

## Pull requests as a triage surface

**PRs as a request surface: no.** _(Set to `yes` if this repo treats external PRs as feature requests; `/triage` reads this flag.)_

When the flag is `yes`, PRs run through the same labels and states as issues, with the `gh pr` equivalents:

- Read a PR with `gh pr view <number> -R alacritree/alacritree --comments`, and its diff with `gh pr diff <number> -R alacritree/alacritree`.
- List external PRs for triage with `gh api --paginate 'repos/alacritree/alacritree/pulls?state=open' --jq '.[] | select(.author_association | IN("OWNER","MEMBER","COLLABORATOR") | not) | {number, title, author: .user.login, author_association, labels: [.labels[].name]}'`.
- Comment, label or close with `gh pr comment`, `gh pr edit --add-label` or `--remove-label`, and `gh pr close`, each with `-R alacritree/alacritree`.

GitHub shares one number space across issues and PRs, so a bare `#42` can be either. Resolve it with `gh pr view 42 -R alacritree/alacritree` and fall back to `gh issue view 42 -R alacritree/alacritree`.

## When a skill says "publish to the issue tracker"

Create a GitHub issue on `alacritree/alacritree`.

## When a skill says "fetch the relevant ticket"

Read it as in the conventions above.

## Wayfinding operations

`/wayfinder` uses these. The map is a single issue, and its child issues are the tickets.

- The map is one issue labelled `wayfinder:map` that holds the Notes, Decisions-so-far and Fog body. Create it with `gh issue create -R alacritree/alacritree --label wayfinder:map`.
- A child ticket is an issue linked to the map as a GitHub sub-issue. Where sub-issues are not enabled, add the child to a task list in the map body and put `Part of #<map>` at the top of the child body. Label it `wayfinder:<type>`, where the type is `research`, `prototype`, `grilling` or `task`. A claimed ticket is assigned to the driving dev.
- Blocking uses GitHub's native issue dependencies. Add an edge with `gh api --method POST repos/alacritree/alacritree/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`. The blocker id is the numeric database id from `gh api repos/alacritree/alacritree/issues/<n> --jq .id`, not the `#number` or the `node_id`. GitHub reports open blockers in `issue_dependencies_summary.blocked_by`. Where dependencies are not available, put a `Blocked by: #<n>, #<n>` line at the top of the child body. A ticket is unblocked when every blocker is closed.
- The frontier query lists the open children of the map, then drops each child with an open blocker or an assignee. The first remaining child in map order wins.
- Claim a ticket with `gh issue edit <n> -R alacritree/alacritree --add-assignee @me`. That is the first write of the session.
- Resolve a ticket with `gh issue comment <n> -R alacritree/alacritree --body "<answer>"`, then `gh issue close <n> -R alacritree/alacritree`. Then append a gist and a link to the Decisions-so-far section of the map.
