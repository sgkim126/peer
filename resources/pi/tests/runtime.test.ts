import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import test, { type TestContext } from "node:test";

// Opt in: PEER_PI_TEST_EXECUTABLE=pi node --test resources/pi/tests/runtime.test.ts
const executable = process.env.PEER_PI_TEST_EXECUTABLE;
const options = {
    skip: !executable,
    timeout: 30000,
};
const digest = "a".repeat(64);
type Record = {
    [key: string]: any;
};

function workspace(t: TestContext) {
    const pin = readFileSync(new URL("../../../src/pi/dependency.rs", import.meta.url), "utf8")
        .match(/SUPPORTED_PI_VERSION: &str = "([^"]+)"/)![1];
    const version = execFileSync(executable!, ["--version"], {
        encoding: "utf8",
        timeout: 10000,
    }).split("\n")[0].trim().split(/\s+/).at(-1)!.replace(/^v/, "");
    assert.equal(version, pin);
    const root = realpathSync(mkdtempSync(join(tmpdir(), "peer-pi-runtime-")));
    mkdirSync(join(root, "agent"));
    mkdirSync(join(root, "sessions"));
    const stops: (() => Promise<void>)[] = [];
    t.after(async () => {
        await Promise.all(stops.map(stop => stop()));
        rmSync(root, { recursive: true, force: true });
    });
    return {
        root,
        stops,
    };
}

function start(t: TestContext, work: ReturnType<typeof workspace>) {
    const { root } = work;
    const child = spawn(executable!, [
        "--mode", "rpc", "--session-dir", join(root, "sessions"), "--no-builtin-tools",
        "--no-extensions", "-e", "builtin:llama.cpp",
        "-e", fileURLToPath(new URL("../extension/index.ts", import.meta.url)),
        "--no-skills", "--no-prompt-templates", "--no-themes", "--no-context-files", "--no-approve",
        "-e", fileURLToPath(new URL("./fixtures/runtime-provider.ts", import.meta.url)),
    ], {
        cwd: root,
        env: {
            PATH: process.env.PATH,
            TMPDIR: tmpdir(),
            PI_CODING_AGENT_DIR: join(root, "agent"),
            PI_SKIP_VERSION_CHECK: "1",
            PI_TELEMETRY: "0",
        },
        stdio: ["pipe", "pipe", "pipe"]
    });
    let stderr = "";
    child.stderr.on("data", chunk => { stderr += chunk; });
    const closed = once(child, "close");
    const kill = () => { child.kill(); };
    t.signal.addEventListener("abort", kill, { once: true });
    const stop = async () => {
        kill();
        await closed;
        t.signal.removeEventListener("abort", kill);
    };
    work.stops.push(stop);
    const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
    const events: Record[] = [];
    let nextId = 0;
    const read = async (): Promise<Record> => {
        const line = await lines.next();
        assert.equal(line.done, false, `Pi exited before returning a record: ${stderr}`);
        const record = JSON.parse(line.value!);
        assert.notEqual(record.type, "extension_error", JSON.stringify(record));
        return record;
    };
    const request = async (command: Record): Promise<Record> => {
        const id = `test-${++nextId}`;
        child.stdin.write(JSON.stringify({
            id,
            ...command,
        }) + "\n");
        while (true) {
            const record = await read();
            if (record.type !== "response" || record.id !== id) {
                events.push(record);
                continue;
            }
            assert.ok(Object.keys(record).every(key => ["id", "type", "command", "success", "data", "error"].includes(key)));
            assert.equal(record.success, true, JSON.stringify(record));
            assert.notEqual(record.data?.cancelled, true);
            return record.data;
        }
    };
    return {
        request,
        stop,
        event: async () => events.shift() ?? await read(),
    };
}

async function configure(client: ReturnType<typeof start>, maxTurns: number) {
    const encoded = Buffer.from(JSON.stringify({
        digest,
        config: {
            tool_contract_digest: "b".repeat(64),
            operation: {
                type: "stage",
                stage: "quality",
                target: "test",
                expected_commits: ["HEAD"],
            },
            system_prompt: "Peer runtime test system prompt.",
            read_tools: ["get_file_content"],
            terminal_tools: ["submit_quality"],
            max_turns: maxTurns,
        },
    })).toString("base64url");
    await client.request({
        type: "prompt",
        message: `/peer-configure-v1 ${encoded}`,
    });
    while (true) {
        const event = await client.event();
        if (event.type === "extension_ui_request" && event.method === "notify" && event.message === `peer.configured:${digest}`) {
            break;
        }
    }
    await client.request({
        type: "set_model",
        provider: "peer-runtime-test",
        modelId: "deterministic",
    });
}

async function turn(client: ReturnType<typeof start>, continuation: boolean) {
    await client.request({
        type: "prompt",
        message: continuation ? `/peer-continue-v1 ${digest}` : "Review the change.",
    });
    let turns = 0;
    let outcome: Record | undefined;
    while (true) {
        const event = await client.event();
        if (event.type === "message_end" && event.message?.role === "assistant") {
            assert.notEqual(event.message.stopReason, "error", JSON.stringify(event.message));
        }
        if (event.type === "turn_end") turns++;
        if (event.type === "tool_execution_end") {
            assert.equal(event.isError, false, JSON.stringify(event));
            assert.equal(event.result.details.type, "peer.outcome");
            assert.equal(event.result.terminate, true);
            outcome = event.result.details.outcome;
            await client.request({
                type: "abort",
            });
        }
        if (event.type === "agent_settled") {
            assert.equal(turns, 1);
            return outcome;
        }
    }
}

async function assertOutcomeAndUsage(client: ReturnType<typeof start>, outcome: Record | undefined) {
    assert.deepEqual(outcome, {
        type: "completed",
        report: {
            summary: "Runtime test passed",
            findings: [],
        },
    });
    const entries = await client.request({
        type: "get_entries",
    });
    const messages = entries.entries.filter((entry: Record) => entry.type === "message" && entry.message?.role === "assistant");
    assert.equal(messages.length, 2);
    for (const { message } of messages) {
        assert.equal(message.provider, "peer-runtime-test");
        assert.equal(message.model, "deterministic");
        assert.equal(message.usage.input, 11);
    }
}

test("Pi preserves the configured prompt and final-turn tools on continuation", options, async t => {
    const client = start(t, workspace(t));
    await client.request({
        type: "new_session",
    });
    await configure(client, 2);
    assert.equal(await turn(client, false), undefined);
    await assertOutcomeAndUsage(client, await turn(client, true));
});

test("Pi reapplies the configured prompt when another process resumes an exhausted session", options, async t => {
    const work = workspace(t);
    const { root } = work;
    const original = start(t, work);
    await original.request({
        type: "new_session",
    });
    const state = await original.request({
        type: "get_state",
    });
    assert.ok(state.sessionFile.startsWith(join(root, "sessions") + "/"));
    await configure(original, 1);
    assert.equal(await turn(original, false), undefined);
    await original.stop();
    const resumed = start(t, work);
    await resumed.request({
        type: "switch_session",
        sessionPath: state.sessionFile,
    });
    const actual = await resumed.request({
        type: "get_state",
    });
    assert.equal(actual.sessionId, state.sessionId);
    assert.equal(actual.sessionFile, state.sessionFile);
    await configure(resumed, 1);
    await assertOutcomeAndUsage(resumed, await turn(resumed, true));
});
