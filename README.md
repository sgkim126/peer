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

Instead, `peer` focuses on questions that require contextual judgment: whether a change matches its stated intent, whether important reasoning remains undocumented, whether the implementation introduces maintainability concerns, and whether a series of commits forms a coherent change.

The security review is intended to identify contextual risks that may not be captured by mechanical rules.
It is not a replacement for SAST, dependency scanning, secret scanning, or other dedicated security tooling.

## Features

`peer` reviews a commit with four complementary checks:

- `size` considers whether the commit is structurally coherent and can be reviewed or reverted as one atomic change.

- `intent` considers whether the implementation matches the stated purpose of the change.

- `quality` considers correctness, maintainability, and design concerns that require contextual judgment.

- `security` considers contextual security risks that deterministic tooling may not capture.

When the review target is a commit range, `peer` also runs a `coherence` check that considers whether the commits form a clear and consistent sequence.

`peer` supports Mistral, OpenAI, Anthropic, and Gemini models.
A review can incorporate its title, body, and existing comment threads so that feedback is grounded in the discussion surrounding the change. Results can be rendered for a terminal, as JSON or Markdown, or with GitHub links.
The rendered result also reports check status, token usage, and estimated model cost.

## Requirements and installation

Running `peer` requires Git, network access to the selected model provider, and an API key for that provider.

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
The command creates `.peer/config.toml` and `.peer/.gitignore`.
Reviews create and use `.peer/cache` as needed.

Set the API key expected by the default provider, then review a commit or a commit range:

```bash
peer init
export MISTRAL_API_KEY="..."
peer review main..HEAD
```

A single revision such as `HEAD` reviews one commit.
A two-dot range such as `main..HEAD` reviews each commit in the range and also considers the coherence of the complete sequence.
Three-dot ranges are not supported.
Review targets must not contain merge commits.
The default configuration accepts at most ten commits in one review.

## Review context

The title, body, and existing discussion explain why a change exists and which constraints shaped it.
Passing that information gives each check access to the requirements, constraints, decisions, and unresolved discussions surrounding the change instead of limiting the review to the diff in isolation.

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

## Review checks

By default, `peer` runs every check that applies to the selected target.
Use `--only-check` to run a subset or `--skip-check` to exclude a subset.

```bash
peer review main..HEAD --only-check quality,security
peer review main..HEAD --skip-check size
```

The two filtering options are mutually exclusive, and a filter must leave at least one applicable check.

## Providers and configuration

`peer init` copies the default configuration to `.peer/config.toml`.
The configuration selects the default provider and model, limits the number of commits and model iterations, and records model prices used to estimate review cost.

| Provider | API key environment variable |
| --- | --- |
| Mistral | `MISTRAL_API_KEY` |
| OpenAI | `OPENAI_API_KEY` |
| Anthropic | `ANTHROPIC_API_KEY` |
| Gemini | `GEMINI_API_KEY` |

Use `--provider` or `--model` to override the configured defaults for one review.

```bash
peer review main..HEAD --provider openai
peer review main..HEAD --provider anthropic --model claude-sonnet-5
```

See [`resources/default_config.toml`](resources/default_config.toml) for every configuration field and the models included in the current release.
Model prices in that file are used only to estimate cost; the provider's billing data remains authoritative.

## Output and exit status

Terminal output is the default.
The other formats are selected with `--format`.

```bash
peer review main..HEAD --format json
peer review main..HEAD --format markdown
peer review main..HEAD --format github --repo owner/repository
```

The GitHub format requires `--repo` so that findings can link to repository files.
Every rendered review includes its findings, individual check statuses, and a summary of the peer version, provider, model, token usage, and estimated cost.

`peer review` exits with status `0` when every planned check completes without an execution error, even if findings are reported.
It exits with a non-zero status when a check fails, exhausts its allowed iterations, or the review cannot be completed or rendered.
CI jobs should use this status to decide whether the review succeeded.

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

    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0

      - id: peer
        uses: sgkim126/peer/.github/actions/peer-review@main
        with:
          version: "0.11.0"
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
Do not review material that the provider is not permitted to receive, and provide API keys through environment variables instead of storing them in `.peer/config.toml`.

The reported cost is an estimate calculated from token usage and the prices in the local configuration.
Actual billing may differ.
Review findings are also nondeterministic and can be incomplete, so they do not replace human judgment or dedicated verification tools.

## Cache management

Review results are stored under `.peer/cache`.
The cache avoids repeating model work when the relevant inputs have not changed.
If a check exhausts its iteration budget or stops because of a transient provider error, `peer`
stores the completed conversation and resumes it the next time the same check runs.
Pass `--no-resume` to `peer review` or `peer check` to ignore resumable checkpoints for that run.

`peer prune` removes cache data belonging to older `peer` versions while preserving data for the current version.
`peer prune --all` removes every cache entry, including entries for the current version.

## Advanced usage

The `check` command runs an individual review check.
The `extract` command reads repository data through the same constrained operations available to the review agents.
The `render` command renders a JSON review result read from standard input.

Use the built-in help for the complete command and option reference.

```bash
peer --help
peer review --help
peer check --help
peer extract --help
peer render --help
```

Use `--verbose` to report model usage while commands run.
Use `--debug` when diagnosing execution errors.

## Development

Run the test suite, lints, and formatting check before submitting a change.

```bash
cargo test
cargo clippy --all-targets
cargo fmt --check
```

## License

`peer` is licensed under the [GNU General Public License v3.0 or later](LICENSE).
