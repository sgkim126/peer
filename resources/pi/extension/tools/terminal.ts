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

function requireOperation(getConfig: GetConfig, type: "check" | "review_context") {
    const envelope = getConfig();
    if (envelope?.config.operation.type !== type) {
        throw new Error(`terminal tool is not valid for the active ${type} operation`);
    }
    return envelope;
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
        description: "Request facts necessary to assess a concrete potential finding.",
        parameters: Type.Object({
            questions: Type.Array(Type.String({
                minLength: 1
            }), {
                minItems: 1
            }),
        }),
        async execute(_id, params) {
            requireOperation(getConfig, "check");
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
