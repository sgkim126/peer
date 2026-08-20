import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import type { ConfigureEnvelope } from "../protocol.ts";
import { executePeerTool } from "../tool-client.ts";

type GetConfig = () => ConfigureEnvelope | undefined;

function requireConfig(getConfig: GetConfig) {
    if (!getConfig()) {
        throw new Error("peer session is not configured");
    }
}

async function execute(
    getConfig: GetConfig,
    tool: string,
    params: unknown,
    signal?: AbortSignal,
) {
    requireConfig(getConfig);
    const value = await executePeerTool(tool, params, signal);
    const text = typeof value === "string" ? value : JSON.stringify(value);
    if (text === undefined) {
        throw new Error("peer tool returned an unsupported value");
    }
    return {
        content: [
            {
                type: "text" as const,
                text,
            }
        ], details: {
        }
    };
}

export function registerReadTools(pi: ExtensionAPI, getConfig: GetConfig) {
    pi.registerTool({
        name: "get_commit_message",
        label: "Get Commit Message",
        description: "Read the full message for one Git commit.",
        parameters: Type.Object({
            revision: Type.String(),
        }),
        async execute(_id, params, signal) {
            return await execute(getConfig, "get_commit_message", params, signal);
        },
    });

    pi.registerTool({
        name: "get_commit_diff",
        label: "Get Commit Diff",
        description: "Read the patch introduced by one Git commit.",
        parameters: Type.Object({
            revision: Type.String(),
        }),
        async execute(_id, params, signal) {
            return await execute(getConfig, "get_commit_diff", params, signal);
        },
    });

    pi.registerTool({
        name: "get_changed_files",
        label: "Get Changed Files",
        description: "List files changed by one Git commit with their status.",
        parameters: Type.Object({
            revision: Type.String(),
        }),
        async execute(_id, params, signal) {
            return await execute(getConfig, "get_changed_files", params, signal);
        },
    });

    pi.registerTool({
        name: "get_commits_in_range",
        label: "Get Commits In Range",
        description: "List commit hashes in a two-dot Git range from oldest to newest.",
        parameters: Type.Object({
            from_revision: Type.String(),
            to_revision: Type.String(),
        }),
        async execute(_id, params, signal) {
            return await execute(getConfig, "get_commits_in_range", params, signal);
        },
    });

    pi.registerTool({
        name: "get_file_content",
        label: "Get File Content",
        description: "Read a repository file at a Git revision.",
        parameters: Type.Object({
            revision: Type.String(),
            path: Type.String(),
            range: Type.Optional(Type.Object({
                start_line: Type.Integer({ minimum: 1 }),
                end_line: Type.Integer({ minimum: 1 }),
            })),
        }),
        async execute(_id, params, signal) {
            return await execute(getConfig, "get_file_content", params, signal);
        },
    });

    pi.registerTool({
        name: "get_file_diff",
        label: "Get File Diff",
        description: "Read the diff for one file between two Git revisions.",
        parameters: Type.Object({
            from_revision: Type.String(),
            to_revision: Type.String(),
            path: Type.String(),
        }),
        async execute(_id, params, signal) {
            return await execute(getConfig, "get_file_diff", params, signal);
        },
    });

    pi.registerTool({
        name: "list_tree",
        label: "List Tree",
        description: "List repository paths at a Git revision.",
        parameters: Type.Object({
            revision: Type.String(),
            path: Type.Optional(Type.String()),
            recursive: Type.Optional(Type.Boolean()),
        }),
        async execute(_id, params, signal) {
            return await execute(getConfig, "list_tree", params, signal);
        },
    });

    pi.registerTool({
        name: "grep",
        label: "Grep",
        description: "Search tracked text at a Git revision using a fixed string.",
        parameters: Type.Object({
            revision: Type.String(),
            query: Type.String({
                minLength: 1
            }),
            path: Type.Optional(Type.String()),
            context_lines: Type.Optional(Type.Integer({
                minimum: 1,
                maximum: 10,
            })),
        }),
        async execute(_id, params, signal) {
            return await execute(getConfig, "grep", params, signal);
        },
    });
}
