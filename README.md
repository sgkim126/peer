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

Instead, `peer` focuses on questions that surface undocumented intent, constraints, rationale, tradeoffs, operational expectations, and verification knowledge. It reports implementation problems as a secondary benefit, but it is not designed to maximize bug-finding coverage.

The security review is intended to identify contextual risks that may not be captured by mechanical rules.
It is not a replacement for SAST, dependency scanning, secret scanning, or other dedicated security tooling.

## Features

Every review uses four stages:

- `review_context` builds a source-backed statement of the documented objective and expected behavior. It blocks only when missing or contradictory information makes a defensible review impossible.

- `knowledge` reviews the whole change for important decisions that are visible in the implementation but not explained in the supplied context. It asks evidence-backed questions that only the author can answer and makes structural recommendations when the evidence is conclusive without additional intent.

- `quality` reviews each commit for non-security correctness, reliability, maintainability, and design problems that require contextual judgment.

- `security` reviews each commit for vulnerabilities with a credible attacker-controlled path, sensitive operation, and impact.

The knowledge stage considers pull-request scope, commit sequence, atomicity, and message-to-diff intent as complementary ways to find missing context. It first searches the supplied discussion, repository documentation, and directly relevant code, and does not ask questions whose answers are already available. There is no fixed question limit; every reported question must independently preserve information that matters to future review, operation, or maintenance.

`peer review` runs `review_context`, then `knowledge`, then the per-commit `quality` and `security` stages. Ordinary knowledge questions do not stop the later bug reviews. Any stage may instead request blocking clarification when it cannot complete defensibly. Blocking clarification, execution failure, or iteration exhaustion prevents successful completion.

`peer` uses the models and providers supported by Pi.
A review can incorporate its title, body, and existing comment threads so that feedback is grounded in the discussion surrounding the change. Results can be rendered for a terminal, as JSON or Markdown, or with GitHub links.
The rendered result also reports stage status, token usage, and estimated model cost.

## Requirements and installation

Running `peer` requires the following tools:

| Tool | Required version |
| --- | --- |
| Git | 2.30.0 or later |
| Node.js | 22.19.0 or later |
| Pi | Exactly 0.83.0 |

You also need network access to the selected model provider and credentials for that provider.

Install [Node.js 22.19.0 or later](https://nodejs.org/en/download), then install the required version of Pi with npm:

```bash
npm install --global --ignore-scripts @earendil-works/pi-coding-agent@0.83.0
```

See the [Pi quickstart](https://github.com/earendil-works/pi/blob/v0.83.0/packages/coding-agent/docs/quickstart.md) for other installation and authentication options.

Building `peer` from source requires Rust 1.96.0 or later.
The release binary and the bundled GitHub Action currently support Linux x86-64.

To install a release binary, download `peer-linux-x86_64-<version>.tar.gz` from [GitHub Releases](https://github.com/sgkim126/peer/releases), extract the archive, and place the `peer` executable somewhere on `PATH`.

To build `peer` from source, run:

```bash
git clone https://github.com/sgkim126/peer.git
cd peer
cargo build --release
```

The resulting executable is available at `target/release/peer`.

## Quick start

Run `peer init` from the root of the Git repository that you want to review.
The command adds a `.peer` directory and its configuration to the repository.
Reviews use subdirectories beneath `.peer` to cache their work as needed.

Set the API key expected by the default provider, then review a commit or a commit range:

```bash
peer init
export MISTRAL_API_KEY="..."
peer review main..HEAD
```

A single revision such as `HEAD` reviews one commit.
A two-dot range such as `main..HEAD` reviews the complete change before reviewing each commit for quality and security problems.
Three-dot ranges are not supported.
Review targets must not contain merge commits.
The default configuration accepts at most ten commits in one review.

## Review context

The title, body, and existing discussion explain why a change exists and which constraints shaped it.
Passing that information lets the first stage establish the documented objective and expected behavior, and lets the knowledge stage avoid asking questions that have already been answered. Missing metadata is not an error by itself. The context stage requests blocking clarification only when the available sources are missing or contradictory enough that the review has no defensible basis.

Use `--title` for the review title, `--body-file` for a file containing the description, and `--comments-file` for a JSON file containing comment threads.

```bash
peer review main..HEAD \
  --title "Add cache pruning support" \
  --body-file /tmp/review-body.md \
  --comments-file /tmp/review-comments.json
```

The comments file contains an array of threads.
Every thread contains comments and may identify a commit and source location.

```json
[
  {
    "commit": "abc1234",
    "location": {
      "path": "src/cache/mod.rs",
      "line": 42
    },
    "comments": [
      {
        "author": "alice",
        "body": "Should old cache versions be removed automatically?"
      },
      {
        "author": "bob",
        "body": "Keep the current version unless --all is specified."
      }
    ]
  }
]
```

The `commit` and `location` fields are optional.
Each comment must contain an `author` and a `body`.

Knowledge questions are normal successful review feedback. Answer one by adding the answer to a human-authored pull-request comment or to the pull-request description, then run the review again. The comment may optionally quote the question for context, but it must contain the answer. The new description and comments become review input, so sufficiently documented decisions are not asked again.

## Providers and configuration

`peer` uses the providers and authentication methods supported by Pi.
The default configuration includes the following common API-key providers as examples:

| Provider | API key environment variable |
| --- | --- |
| Mistral | `MISTRAL_API_KEY` |
| OpenAI | `OPENAI_API_KEY` |
| Anthropic | `ANTHROPIC_API_KEY` |
| Gemini | `GEMINI_API_KEY` |

See [Pi's provider documentation](https://github.com/earendil-works/pi/blob/v0.83.0/packages/coding-agent/docs/providers.md) for the complete list of supported providers and authentication methods.

`peer init` copies the default configuration to `.peer/config.toml`.
The configuration selects the default provider and model and limits the number of commits and model iterations.
The removed `commit_scope`, `commit_sequence`, `size`, and `intent` stage overrides are invalid; use `[stages.knowledge]` instead.

Use `--provider` or `--model` to override the configured defaults for one review.

```bash
peer review main..HEAD --provider openai
peer review main..HEAD --provider anthropic --model claude-sonnet-5
```

See [`resources/default_config.toml`](resources/default_config.toml) for every configuration field.

## Output and exit status

`peer review` always outputs JSON. Pipe that document to `peer render` to produce a human-readable format; terminal output is the renderer's default. `peer render` also accepts a single `KnowledgeQuestion`, `StructuralRecommendation`, or `RenderFinding` JSON object.

```bash
peer review main..HEAD
peer review main..HEAD | peer render --format markdown
peer review main..HEAD | peer render --format github --repo owner/repository
```

The GitHub format requires `--repo` so that feedback can link to repository files.
Questions, structural recommendations, and quality or security findings appear in separate `Review questions`, `Structural recommendations`, and `Review findings` sections. Each entry is tagged with its kind, such as `question/rationale`, `recommendation/split_commit`, or `finding/high`. Stage details report status, summary, token usage, and estimated model cost without repeating those results.

JSON output stores the same result types in separate top-level `questions`, `recommendations`, and `findings` arrays.

`peer review` exits with status `0` when every planned stage completes without an execution error, even when questions, structural recommendations, or bug findings are reported.
It exits with a non-zero status when a stage needs clarification, fails, exhausts its allowed iterations, or the review cannot be completed or rendered.
CI jobs should use this status to decide whether the review succeeded.

The selected model receives the review metadata, commit messages, changed-file summaries, and diffs needed by these stages. During a stage it may also request repository content through the configured read-only tools. Avoid supplying secrets or other material that should not be sent to the model provider.

## GitHub Actions

The bundled composite action downloads a selected `peer` release, initializes the repository when necessary, optionally restores the review cache, and exposes the review's exit code and output paths.
It currently runs only on a Linux x86-64 runner.

The following workflow reviews a pull request without collecting its body and comment threads.

```yaml
name: Peer review

on:
  pull_request:

permissions:
  contents: read

jobs:
  review:
    runs-on: ubuntu-latest
    timeout-minutes: 10

    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0

      - id: peer
        uses: sgkim126/peer/.github/actions/peer-review@main
        with:
          version: "0.12.0"
          provider: mistral
          target: ${{ github.event.pull_request.base.sha }}..${{ github.event.pull_request.head.sha }}
          repo: ${{ github.repository }}
        env:
          MISTRAL_API_KEY: ${{ secrets.MISTRAL_API_KEY }}

      - name: Fail when the review is unsuccessful
        if: steps.peer.outputs.exit-code != '0'
        run: exit 1
```

The action captures the review status instead of failing its own step, which allows a later step to publish the output before deciding whether the job should fail.
It exposes `exit-code`, `stdout-path`, and `stderr-path`.

See [`.github/actions/peer-review/action.yml`](.github/actions/peer-review/action.yml) for the complete input and output reference.
The manually dispatched workflow in [`.github/workflows/peer-review-dispatch.yml`](.github/workflows/peer-review-dispatch.yml) shows how to collect a pull request's title, body, and comments, maintain a placeholder comment, and publish the GitHub-formatted review.

## Privacy and cost

`peer` sends the reviewed code and any supplied review context to the selected model provider.
Do not review material that the provider is not permitted to receive, and make sure commits submitted for review do not contain passwords, API tokens, or other secrets that must not be disclosed.

The reported cost comes from the usage information returned by Pi and remains an estimate.
Actual billing may differ.
Review feedback is nondeterministic and can be incomplete, so it does not replace human judgment or dedicated verification tools.

## Cache management

Review results are stored under `.peer/cache`.
The cache avoids repeating model work when the relevant inputs have not changed.
If a stage exhausts its iteration budget or stops because of a transient provider error, `peer`
stores the completed conversation and resumes it the next time the same stage runs.
Pass `--no-resume` to `peer review` to ignore resumable checkpoints for that run.

`peer prune` removes cache data belonging to older `peer` versions while preserving data for the current version.
`peer prune --all` removes every cache entry, including entries for the current version.

## Development

Development requires Cargo 1.96.0 or later and Node.js 22.19.0 or later.

Run the test suite, lints, and formatting check before submitting a change.

```bash
cargo test
cargo clippy --all-targets
cargo fmt --check
```

## License

`peer` is licensed under the [GNU General Public License v3.0 or later](LICENSE).
