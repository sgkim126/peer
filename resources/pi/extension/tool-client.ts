import { createConnection } from "node:net";

let nextRequestId = 1;

interface ToolResponse {
    id: string;
    success: boolean;
    data?: unknown;
    error?: string;
}

function decodeResponse(record: string, expectedId: string): ToolResponse {
    let value: unknown;
    try {
        value = JSON.parse(record);
    } catch (error) {
        throw new Error(`invalid peer tool response: ${String(error)}`);
    }
    if (typeof value !== "object" || value === null) {
        throw new Error("invalid peer tool response");
    }
    if (!("id" in value) || value.id !== expectedId) {
        throw new Error("peer tool response ID does not match its request");
    }
    if (!("success" in value) || typeof value.success !== "boolean") {
        throw new Error("invalid peer tool response status");
    }
    if (value.success) {
        if (!("data" in value)) {
            throw new Error("successful peer tool response omitted data");
        }
    } else if (!("error" in value) || typeof value.error !== "string") {
        throw new Error("failed peer tool response omitted its error");
    }
    return value as ToolResponse;
}

export function executePeerTool(
    tool: string,
    args: unknown,
    signal?: AbortSignal,
): Promise<unknown> {
    const socketPath = process.env.PEER_TOOL_SOCKET;
    if (!socketPath) {
        throw new Error("peer tool socket is not configured");
    }
    const id = `extension-${nextRequestId}`;
    nextRequestId += 1;

    return new Promise((resolve, reject) => {
        const socket = createConnection({ path: socketPath });
        socket.setEncoding("utf8");
        let buffer = "";
        let settled = false;

        const finish = (error?: unknown, data?: unknown) => {
            if (settled) {
                return;
            }
            settled = true;
            signal?.removeEventListener("abort", abort);
            socket.destroy();
            if (error !== undefined) {
                reject(error);
            } else {
                resolve(data);
            }
        };
        const abort = () => finish(signal?.reason ?? new Error("peer tool execution aborted"));

        if (signal?.aborted) {
            abort();
            return;
        }
        signal?.addEventListener("abort", abort, { once: true });
        socket.on("connect", () => {
            socket.write(`${JSON.stringify({ id, tool, arguments: args })}\n`);
        });
        socket.on("data", (chunk) => {
            buffer += chunk;
            const newline = buffer.indexOf("\n");
            if (newline === -1) {
                return;
            }
            try {
                const response = decodeResponse(buffer.slice(0, newline), id);
                if (response.success) {
                    finish(undefined, response.data);
                } else {
                    finish(new Error(response.error));
                }
            } catch (error) {
                finish(error);
            }
        });
        socket.on("error", finish);
        socket.on("end", () => {
            finish(new Error("peer tool socket ended before returning a response"));
        });
    });
}
