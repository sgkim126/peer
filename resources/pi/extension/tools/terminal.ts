import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import type { ConfigureEnvelope } from "../protocol.ts";

type GetConfig = () => ConfigureEnvelope | undefined;

const Finding = Type.Object({
    commit: Type.String({
        minLength: 1,
    }),
    severity: Type.Union([
        Type.Literal("info"),
        Type.Literal("low"),
        Type.Literal("medium"),
        Type.Literal("high"),
        Type.Literal("critical"),
    ]),
    message: Type.String({
        minLength: 1,
    }),
    file: Type.Optional(Type.String()),
    line: Type.Optional(Type.Integer({
        minimum: 1,
    })),
});

const DigestItem = Type.Object({
    kind: Type.Union([
        Type.Literal("requirement"),
        Type.Literal("decision"),
        Type.Literal("constraint"),
        Type.Literal("unresolved"),
        Type.Literal("superseded"),
    ]),
    text: Type.String({
        minLength: 1,
    }),
    sources: Type.Array(Type.String(), {
        minItems: 1,
    }),
});

const MissingContext = Type.Object({
    text: Type.String({
        minLength: 1,
    }),
    sources: Type.Array(Type.String(), {
        minItems: 1,
    }),
});

const SourcedStatement = Type.Object({
    text: Type.String({ minLength: 1 }),
    sources: Type.Array(Type.String({ minLength: 1 }), { minItems: 1 }),
});

const ReviewContextReport = Type.Object({
    summary: Type.String({ minLength: 1 }),
    objectives: Type.Array(SourcedStatement),
    expected_behavior: Type.Array(SourcedStatement),
    scope: Type.Array(SourcedStatement),
    constraints: Type.Array(SourcedStatement),
    implementation: Type.Array(SourcedStatement),
    verification: Type.Array(SourcedStatement),
    unresolved: Type.Array(SourcedStatement),
});

const CommitScopeEntry = Type.Object({
    commit: Type.String({ minLength: 1 }),
    purpose: Type.String({ minLength: 1 }),
    role: Type.Union([
        Type.Literal("primary"),
        Type.Literal("supporting"),
        Type.Literal("prerequisite"),
        Type.Literal("unrelated"),
    ]),
    disposition: Type.Union([
        Type.Literal("keep"),
        Type.Literal("split_pr"),
        Type.Literal("extract_prerequisite"),
    ]),
    rationale: Type.String({ minLength: 1 }),
});

const CommitProgress = Type.Object({
    commit: Type.String({ minLength: 1 }),
    direction: Type.String({ minLength: 1 }),
    change_kind: Type.Union([
        Type.Literal("forward"),
        Type.Literal("fixup"),
        Type.Literal("reversion"),
    ]),
    depends_on: Type.Array(Type.String({ minLength: 1 })),
});

const SequenceIssue = Type.Object({
    kind: Type.Union([
        Type.Literal("reorder"),
        Type.Literal("dependency_direction"),
        Type.Literal("confusing_progression"),
    ]),
    commits: Type.Array(Type.String({ minLength: 1 }), { minItems: 1 }),
    message: Type.String({ minLength: 1 }),
});

const SizeIssue = Type.Object({
    kind: Type.Union([
        Type.Literal("split"),
        Type.Literal("move"),
        Type.Literal("merge_squash"),
    ]),
    message: Type.String({ minLength: 1 }),
    related_commits: Type.Array(Type.String({ minLength: 1 })),
});

const IntentIssue = Type.Intersect([
    Type.Object({
        commit: Type.String({ minLength: 1 }),
        kind: Type.Union([
            Type.Literal("undocumented_change"),
            Type.Literal("missing_claimed_change"),
            Type.Literal("misstated_effect"),
        ]),
        message: Type.String({ minLength: 1 }),
    }),
    Type.Partial(Type.Object({
        file: Type.String(),
        line: Type.Integer({ minimum: 1 }),
    })),
]);

const SecurityFinding = Type.Intersect([
    Finding,
    Type.Object({
        attacker_control: Type.String({ minLength: 1 }),
        sensitive_operation: Type.String({ minLength: 1 }),
        impact: Type.String({ minLength: 1 }),
    }),
]);

function requireOperation(getConfig: GetConfig, type: "check" | "review_context" | "stage") {
    const envelope = getConfig();
    if (envelope?.config.operation.type !== type) {
        throw new Error(`terminal tool is not valid for the active ${type} operation`);
    }
    return envelope;
}

function requireStage(getConfig: GetConfig, stage: string) {
    const envelope = requireOperation(getConfig, "stage");
    const operation = envelope.config.operation;
    if (operation.type !== "stage" || operation.stage !== stage) {
        throw new Error(`terminal tool is not valid for the active ${stage} stage`);
    }
    return envelope;
}

function completed(report: unknown, message: string) {
    return outcome({
        type: "completed",
        report,
    }, message);
}

function outcome(value: unknown, message: string) {
    return {
        content: [{
            type: "text" as const,
            text: message,
        }],
        details: {
            type: "peer.outcome",
            outcome: value,
        },
        terminate: true,
    };
}

export function registerTerminalTools(pi: ExtensionAPI, getConfig: GetConfig) {
    pi.registerTool({
        name: "submit_check_result",
        label: "Submit Check Result",
        description: "Submit the final structured check result.",
        parameters: Type.Object({
            summary: Type.Optional(Type.String()),
            findings: Type.Array(Finding),
        }),
        async execute(_id, params) {
            const envelope = requireOperation(getConfig, "check");
            const operation = envelope.config.operation;
            if (operation.type !== "check") {
                throw new Error("check operation changed unexpectedly");
            }
            const expected = new Set(operation.expected_commits);
            const unexpected = params.findings
                .map((finding) => finding.commit)
                .filter((commit) => !expected.has(commit));
            if (unexpected.length > 0) {
                throw new Error(
                    `finding commits are outside the configured check target: ${unexpected.join(", ")};`
                    + ` expected one of: ${operation.expected_commits.join(", ")}`,
                );
            }
            return outcome(
                {
                    type: "check_result",
                    summary: params.summary ?? "",
                    findings: params.findings,
                },
                "Submitted check result.",
            );
        },
    });

    pi.registerTool({
        name: "request_clarification",
        label: "Request Clarification",
        description: "Request facts necessary to complete the active review stage.",
        parameters: Type.Object({
            questions: Type.Array(Type.Object({
                question: Type.String({ minLength: 1 }),
                reason: Type.String({ minLength: 1 }),
            }), {
                minItems: 1
            }),
        }),
        async execute(_id, params) {
            const type = getConfig()?.config.operation.type;
            if (type !== "check" && type !== "stage") {
                throw new Error("terminal tool is not valid for the active operation");
            }
            return outcome(
                {
                    type: "clarification",
                    questions: params.questions,
                },
                "Requested clarification.",
            );
        },
    });

    pi.registerTool({
        name: "submit_review_context",
        label: "Submit Review Context",
        description: "Submit the sufficient, compressed review context.",
        parameters: ReviewContextReport,
        async execute(_id, params) {
            requireStage(getConfig, "review_context");
            return completed(params, "Submitted review context.");
        },
    });

    pi.registerTool({
        name: "submit_commit_scope",
        label: "Submit Commit Scope",
        description: "Submit pull-request scope classifications for every commit.",
        parameters: Type.Object({
            summary: Type.String({ minLength: 1 }),
            commits: Type.Array(CommitScopeEntry),
        }),
        async execute(_id, params) {
            requireStage(getConfig, "commit_scope");
            return completed(params, "Submitted commit scope.");
        },
    });

    pi.registerTool({
        name: "submit_commit_sequence",
        label: "Submit Commit Sequence",
        description: "Submit the ordered commit progression and sequence issues.",
        parameters: Type.Object({
            summary: Type.String({ minLength: 1 }),
            progression: Type.Array(CommitProgress),
            issues: Type.Array(SequenceIssue),
        }),
        async execute(_id, params) {
            requireStage(getConfig, "commit_sequence");
            return completed(params, "Submitted commit sequence.");
        },
    });

    pi.registerTool({
        name: "submit_size",
        label: "Submit Size Stage",
        description: "Submit atomicity issues for the target commit.",
        parameters: Type.Object({
            summary: Type.String({ minLength: 1 }),
            issues: Type.Array(SizeIssue),
        }),
        async execute(_id, params) {
            requireStage(getConfig, "size");
            return completed(params, "Submitted size stage.");
        },
    });

    pi.registerTool({
        name: "submit_intent",
        label: "Submit Intent Stage",
        description: "Submit message-to-diff intent mismatches.",
        parameters: Type.Object({
            summary: Type.String({ minLength: 1 }),
            issues: Type.Array(IntentIssue),
        }),
        async execute(_id, params) {
            requireStage(getConfig, "intent");
            return completed(params, "Submitted intent stage.");
        },
    });

    pi.registerTool({
        name: "submit_quality",
        label: "Submit Quality Stage",
        description: "Submit non-security quality findings.",
        parameters: Type.Object({
            summary: Type.String({ minLength: 1 }),
            findings: Type.Array(Finding),
        }),
        async execute(_id, params) {
            requireStage(getConfig, "quality");
            return completed(params, "Submitted quality stage.");
        },
    });

    pi.registerTool({
        name: "submit_security",
        label: "Submit Security Stage",
        description: "Submit findings with a complete credible exploit chain.",
        parameters: Type.Object({
            summary: Type.String({ minLength: 1 }),
            findings: Type.Array(SecurityFinding),
        }),
        async execute(_id, params) {
            requireStage(getConfig, "security");
            return completed(params, "Submitted security stage.");
        },
    });

    pi.registerTool({
        name: "submit_review_context_digest",
        label: "Submit Review Context Digest",
        description: "Submit the faithful compressed review context.",
        parameters: Type.Object({
            overview: Type.String({
                minLength: 1
            }),
            items: Type.Array(DigestItem),
            missing_context: Type.Array(MissingContext),
        }),
        async execute(_id, params) {
            requireOperation(getConfig, "review_context");
            return outcome({
                type: "review_context",
                digest: params,
            }, "Submitted review context digest.");
        },
    });
}
