import assert from "node:assert/strict";
import test from "node:test";

import { decodeConfigureEnvelope } from "../extension/protocol.ts";

function encode(value: unknown): string {
    return Buffer.from(JSON.stringify(value), "utf8").toString("base64url");
}

test("decodes a supported peer run configuration", () => {
    const envelope = {
        digest: "a".repeat(64),
        config: {
            tool_contract_digest: "b".repeat(64),
            operation: {
                type: "check",
                check: "quality",
                target: "abc1234",
                expected_commits: ["abc1234"],
            },
            system_prompt: "Review code.",
            read_tools: ["get_commit_diff"],
            terminal_tools: ["submit_check_result"],
            max_turns: 4,
        },
    };

    assert.deepEqual(decodeConfigureEnvelope(encode(envelope)), envelope);
});

test("rejects a malformed payload", () => {
    assert.throws(
        () => decodeConfigureEnvelope("not-json"),
        /invalid peer configuration encoding/,
    );
});

test("rejects a short digest", () => {
    const encoded = encode({
        digest: "a".repeat(63),
        config: {
            tool_contract_digest: "b".repeat(64),
            operation: {
                type: "review_context"
            },
            system_prompt: "Compress context.",
            read_tools: [],
            terminal_tools: ["submit_review_context_digest"],
            max_turns: 1,
        },
    });

    assert.throws(
        () => decodeConfigureEnvelope(encoded),
        /invalid peer configuration digest/,
    );
});

test("rejects read tools for a review context operation", () => {
    const encoded = encode({
        digest: "a".repeat(64),
        config: {
            tool_contract_digest: "b".repeat(64),
            operation: {
                type: "review_context"
            },
            system_prompt: "Compress context.",
            read_tools: ["get_commit_diff"],
            terminal_tools: ["submit_review_context_digest"],
            max_turns: 1,
        },
    });

    assert.throws(
        () => decodeConfigureEnvelope(encoded),
        /invalid read tools for peer review context operation/,
    );
});

test("rejects a zero turn count", () => {
    const encoded = encode({
        digest: "a".repeat(64),
        config: {
            tool_contract_digest: "b".repeat(64),
            operation: {
                type: "review_context"
            },
            system_prompt: "Compress context.",
            read_tools: [],
            terminal_tools: ["submit_review_context_digest"],
            max_turns: 0,
        },
    });

    assert.throws(
        () => decodeConfigureEnvelope(encoded),
        /invalid peer maximum turn count/,
    );
});

test("rejects tools for another operation", () => {
    const encoded = encode({
        digest: "a".repeat(64),
        config: {
            tool_contract_digest: "b".repeat(64),
            operation: {
                type: "review_context"
            },
            system_prompt: "Compress context.",
            read_tools: [],
            terminal_tools: ["submit_check_result"],
            max_turns: 1,
        },
    });

    assert.throws(
        () => decodeConfigureEnvelope(encoded),
        /invalid terminal tools for peer review context operation/,
    );
});

test("rejects empty terminal tools for a review context operation", () => {
    const encoded = encode({
        digest: "a".repeat(64),
        config: {
            tool_contract_digest: "b".repeat(64),
            operation: {
                type: "review_context"
            },
            system_prompt: "Compress context.",
            read_tools: [],
            terminal_tools: [],
            max_turns: 1,
        },
    });

    assert.throws(
        () => decodeConfigureEnvelope(encoded),
        /peer review context operation requires at least one terminal tool/,
    );
});

test("rejects empty terminal tools for a check operation", () => {
    const encoded = encode({
        digest: "a".repeat(64),
        config: {
            tool_contract_digest: "b".repeat(64),
            operation: {
                type: "check",
                check: "quality",
                target: "abc1234",
                expected_commits: ["abc1234"],
            },
            system_prompt: "Review code.",
            read_tools: ["get_commit_diff"],
            terminal_tools: [],
            max_turns: 4,
        },
    });

    assert.throws(
        () => decodeConfigureEnvelope(encoded),
        /peer check operation requires at least one terminal tool/,
    );
});

test("rejects a check operation without expected commits", () => {
    const encoded = encode({
        digest: "a".repeat(64),
        config: {
            tool_contract_digest: "b".repeat(64),
            operation: {
                type: "check",
                check: "quality",
                target: "abc1234",
                expected_commits: [],
            },
            system_prompt: "Review code.",
            read_tools: ["get_commit_diff"],
            terminal_tools: ["submit_check_result"],
            max_turns: 4,
        },
    });

    assert.throws(
        () => decodeConfigureEnvelope(encoded),
        /invalid peer operation/,
    );
});

test("rejects a check operation with an empty expected commit", () => {
    const encoded = encode({
        digest: "a".repeat(64),
        config: {
            tool_contract_digest: "b".repeat(64),
            operation: {
                type: "check",
                check: "quality",
                target: "abc1234",
                expected_commits: [""],
            },
            system_prompt: "Review code.",
            read_tools: ["get_commit_diff"],
            terminal_tools: ["submit_check_result"],
            max_turns: 4,
        },
    });

    assert.throws(
        () => decodeConfigureEnvelope(encoded),
        /invalid peer operation/,
    );
});
