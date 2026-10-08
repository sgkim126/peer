import assert from "node:assert/strict";
import {
    createAssistantMessageEventStream,
    getCurrentSystemPrompt,
    getCurrentTools,
} from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function runtimeProvider(pi: ExtensionAPI) {
    pi.registerProvider("peer-runtime-test", {
        api: "peer-runtime-test",
        apiKey: "unused-test-key",
        baseUrl: "http://127.0.0.1:1/unused",
        models: [{
            id: "deterministic",
            name: "Deterministic runtime test",
            reasoning: false,
            input: ["text"],
            contextWindow: 200000,
            maxTokens: 1024,
            cost: {
                input: 0,
                output: 0,
                cacheRead: 0,
                cacheWrite: 0,
            },
        }],
        streamSimple(model, context) {
            const stream = createAssistantMessageEventStream();
            queueMicrotask(() => {
                const message = {
                    role: "assistant" as const,
                    content: [],
                    api: model.api,
                    provider: model.provider,
                    model: model.id,
                    timestamp: Date.now(),
                    stopReason: "stop" as const,
                    usage: {
                        input: 11,
                        output: 7,
                        cacheRead: 3,
                        cacheWrite: 2,
                        totalTokens: 23,
                        cost: {
                            input: 0,
                            output: 0,
                            cacheRead: 0,
                            cacheWrite: 0,
                            total: 0,
                        },
                    },
                };
                try {
                    assert.equal(getCurrentSystemPrompt(context.messages), "Peer runtime test system prompt.");
                    const tools = getCurrentTools(context.messages).map(tool => tool.name);
                    assert.ok(tools.includes("submit_quality"));
                    const continuing = context.messages.some(entry => entry.role === "assistant");
                    if (continuing) assert.deepEqual(tools, ["submit_quality"]);
                    const content = continuing
                        ? [{
                            type: "toolCall" as const,
                            id: "submit-test",
                            name: "submit_quality",
                            arguments: {
                                summary: "Runtime test passed",
                                findings: [],
                            }
                        }]
                        : [{
                            type: "text" as const,
                            text: "Continue the review.",
                        }];
                    const completed = {
                        ...message,
                        content,
                        stopReason: continuing ? "toolUse" as const : "stop" as const,
                    };
                    stream.push({
                        type: "start",
                        partial: completed,
                    });
                    stream.push({
                        type: "done",
                        reason: completed.stopReason,
                        message: completed,
                    });
                } catch (error) {
                    stream.push({
                        type: "error",
                        reason: "error",
                        error: {
                            ...message,
                            stopReason: "error",
                            errorMessage: String(error),
                        },
                    });
                }
                stream.end();
            });
            return stream;
        },
    });
}
