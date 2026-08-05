import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import { executePeerTool } from "../extension/tool-client.ts";

test("forwards a tool request over a Unix socket", async () => {
    const directory = await mkdtemp(path.join(tmpdir(), "peer-tool-client-"));
    const socketPath = path.join(directory, "tools.sock");
    const server = createServer((socket) => {
        socket.setEncoding("utf8");
        socket.once("data", (record) => {
            const request = JSON.parse(record.trim());
            socket.end(`${JSON.stringify({
                id: request.id,
                success: true,
                data: {
                    tool: request.tool,
                    arguments: request.arguments,
                },
            })}\n`);
        });
    });
    await new Promise<void>((resolve, reject) => {
        server.once("error", reject);
        server.listen(socketPath, resolve);
    });
    const previousSocket = process.env.PEER_TOOL_SOCKET;
    process.env.PEER_TOOL_SOCKET = socketPath;

    try {
        const result = await executePeerTool("get_commit_diff", {
            revision: "HEAD",
        });
        assert.deepEqual(result, {
            tool: "get_commit_diff",
            arguments: {
                revision: "HEAD",
            },
        });
    } finally {
        if (previousSocket === undefined) {
            delete process.env.PEER_TOOL_SOCKET;
        } else {
            process.env.PEER_TOOL_SOCKET = previousSocket;
        }
        await new Promise<void>((resolve, reject) => {
            server.close((error) => error ? reject(error) : resolve());
        });
        await rm(directory, {
            recursive: true,
        });
    }
});
