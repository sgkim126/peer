# peer

`peer` is an LLM-based code review CLI built primarily for solo development.
When working alone, small concerns are easy to overlook, and knowledge about design decisions often remains implicit instead of being recorded.
`peer` uses code review as a checkpoint for surfacing those concerns and preserving the context behind each change.
Its goal is not to find every possible bug, but to encourage developers to record the context needed for future development and maintenance.

## Who peer is for

`peer` is intended for developers working alone or in environments where regular peer review is not available.
It provides an additional review checkpoint for examining the intent, structure, and consequences of a change before that context is forgotten.

`peer` may not be useful for teams that already maintain a healthy review culture and consistently share development context through human review.
If your primary goal is to detect as many implementation bugs as possible, consider a dedicated AI review product such as:

- [CodeRabbit](https://docs.coderabbit.ai/guides/code-review-overview)
- [Qodo](https://docs.qodo.ai/code-review)
- [CodeSherlock](https://www.codesherlock.ai/)
- [Greptile](https://www.greptile.com/docs/introduction)

## Non-goals

`peer` is not a replacement for static analysis.
Problems that can be detected by compilers, linters, type checkers, formatters, or dedicated security scanners should primarily be handled by those deterministic tools.
Using an LLM as the primary mechanism for finding the same problems is unnecessarily expensive, while relying on nondeterministic output for repeatable enforcement is inherently unreliable.

Instead, `peer` focuses on questions that surface undocumented intent, constraints, rationale, tradeoffs, operational expectations, and verification knowledge.
It reports implementation problems as a secondary benefit, but it is not designed to maximize bug-finding coverage.

The security review is intended to identify contextual risks that may not be captured by mechanical rules.
It is not a replacement for SAST, dependency scanning, secret scanning, or other dedicated security tooling.

## Features

Every review uses four stages:

- `review_context` establishes the documented objective and expected behavior.
- `knowledge` surfaces undocumented decisions and structural recommendations.
- `quality` reviews each commit for problems that require contextual judgment.
- `security` reviews each commit for contextual vulnerabilities.

`peer` uses the models and providers supported by Pi.
A review can incorporate its title, body, and existing comment threads so that feedback is grounded in the discussion surrounding the change.
Results can be rendered for a terminal, as JSON or Markdown, or with GitHub or GitLab links.
The rendered result also reports stage status, token usage, and estimated model cost.

See [Reviewing changes](https://github.com/sgkim126/peer/wiki/Reviewing-Changes) for the review stages and their completion rules.

## Requirements and installation

Running `peer` requires the following tools:

| Tool | Required version |
| --- | --- |
| Git | 2.30.0 or later |
| Node.js | 22.19.0 or later |
| Pi | Exactly 0.85.1 |

You also need network access to the selected model provider and credentials for that provider.

Install [Node.js 22.19.0 or later](https://nodejs.org/en/download), then install the required version of Pi with npm:

```bash
npm install --global --ignore-scripts @earendil-works/pi-coding-agent@0.85.1
```

See the [Pi quickstart](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/quickstart.md) for other installation and authentication options.

The release binary and the bundled GitHub Action currently support Linux x86-64.

To install a release binary, download `peer-linux-x86_64-<version>.tar.gz` from [GitHub Releases](https://github.com/sgkim126/peer/releases), extract the archive, and place the `peer` executable somewhere on `PATH`.

For source builds and development requirements, see [Development](https://github.com/sgkim126/peer/wiki/Development).

## Quick start

Run `peer init` from the root of the Git repository that you want to review.
The command adds a `.peer` directory and its configuration to the repository.

Set the API key expected by the default provider, then review a commit or a commit range:

```bash
peer init
export MISTRAL_API_KEY="..."
peer review main..HEAD
```

A single revision such as `HEAD` reviews one commit.
A two-dot range such as `main..HEAD` reviews the complete change before reviewing each commit for quality and security problems.

See [Reviewing changes](https://github.com/sgkim126/peer/wiki/Reviewing-Changes) for supported targets, commit limits, and review context.

`peer review` outputs JSON.
Pipe it to `peer render` for terminal output:

```bash
peer review main..HEAD | peer render
```

To review a GitLab.com merge request, set `[gitlab].repo` to its target
project, such as `group/subgroup/project`, and provide `GITLAB_TOKEN` with
the `read_api` scope:

```bash
peer review --gitlab 123 > review.json
peer render --format gitlab < review.json
```

`--repo group/subgroup/project` overrides that setting for a review.
The MR's commits and base must already be available in the local Git
repository. `peer init --repo` continues to configure GitHub only.
GitLab rendering also accepts `--repo` and works without a token or network access.

## Documentation

See the [wiki home](https://github.com/sgkim126/peer/wiki) for the full documentation.

| Guide | Contents |
| --- | --- |
| [Reviewing changes](https://github.com/sgkim126/peer/wiki/Reviewing-Changes) | Review stages, supported targets, review context, and answering questions. |
| [Configuration](https://github.com/sgkim126/peer/wiki/Configuration) | Providers, authentication, defaults, and per-review overrides. |
| [GitHub pull requests](https://github.com/sgkim126/peer/wiki/GitHub-Pull-Requests) | Load PR context and publish review comments. |
| [Output and exit status](https://github.com/sgkim126/peer/wiki/Output-and-Exit-Status) | Render JSON results and interpret review exit codes. |
| [GitHub Actions](https://github.com/sgkim126/peer/wiki/GitHub-Actions) | Automate reviews with the bundled action. |
| [Privacy and cost](https://github.com/sgkim126/peer/wiki/Privacy-and-Cost) | Understand model inputs, usage records, and estimated costs. |
| [Cache management](https://github.com/sgkim126/peer/wiki/Cache-Management) | Reuse results, resume stages, and prune cached data. |
| [Development](https://github.com/sgkim126/peer/wiki/Development) | Build from source and run development checks. |

## Privacy and cost

`peer` sends the reviewed code and any supplied review context to the selected model provider.
Do not review material that the provider is not permitted to receive, and make sure commits submitted for review do not contain passwords, API tokens, or other secrets that must not be disclosed.

See [Privacy and cost](https://github.com/sgkim126/peer/wiki/Privacy-and-Cost) for details about model inputs, usage, and estimated costs.

## License

`peer` is licensed under the [GNU General Public License v3.0 or later](LICENSE).
