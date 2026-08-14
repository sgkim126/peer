const CHECK_KINDS = [
    "size",
    "intent",
    "quality",
    "security",
    "coherence",
] as const;

const STAGE_KINDS = [
    "review_context",
    "commit_scope",
    "commit_sequence",
    "size",
    "intent",
    "quality",
    "security",
] as const;

const READ_TOOLS = [
    "get_commit_message",
    "get_commit_diff",
    "get_changed_files",
    "get_commits_in_range",
    "get_file_content",
    "get_file_diff",
    "list_tree",
    "grep",
] as const;

const CHECK_TERMINAL_TOOLS = [
    "submit_check_result",
    "request_clarification",
] as const;

const STAGE_TERMINAL_TOOLS = [
    "request_clarification",
    "submit_review_context",
    "submit_commit_scope",
    "submit_commit_sequence",
    "submit_size",
    "submit_intent",
    "submit_quality",
    "submit_security",
] as const;

const REVIEW_CONTEXT_TERMINAL_TOOLS = [
    "submit_review_context_digest",
] as const;


export type CheckKind = (typeof CHECK_KINDS)[number];
export type StageKind = (typeof STAGE_KINDS)[number];
export type ReadTool = (typeof READ_TOOLS)[number];
export type CheckTerminalTool = (typeof CHECK_TERMINAL_TOOLS)[number];
export type StageTerminalTool = (typeof STAGE_TERMINAL_TOOLS)[number];
export type ReviewContextTerminalTool =
    (typeof REVIEW_CONTEXT_TERMINAL_TOOLS)[number];
export type TerminalTool =
    | CheckTerminalTool
    | StageTerminalTool
    | ReviewContextTerminalTool;

const STAGE_SUBMISSION_TOOLS: Record<StageKind, StageTerminalTool> = {
    review_context: "submit_review_context",
    commit_scope: "submit_commit_scope",
    commit_sequence: "submit_commit_sequence",
    size: "submit_size",
    intent: "submit_intent",
    quality: "submit_quality",
    security: "submit_security",
};

export interface ReviewContextOperation {
    type: "review_context";
}

export interface CheckOperation {
    type: "check";
    check: CheckKind;
    target: string;
    expected_commits: string[];
}

export interface StageOperation {
    type: "stage";
    stage: StageKind;
    target: string;
    expected_commits: string[];
}

export type RunConfig =
    | {
        tool_contract_digest: string;
        operation: ReviewContextOperation;
        system_prompt: string;
        read_tools: [];
        terminal_tools: ReviewContextTerminalTool[];
        max_turns: number;
    }
    | {
        tool_contract_digest: string;
        operation: CheckOperation;
        system_prompt: string;
        read_tools: ReadTool[];
        terminal_tools: CheckTerminalTool[];
        max_turns: number;
    }
    | {
        tool_contract_digest: string;
        operation: StageOperation;
        system_prompt: string;
        read_tools: ReadTool[];
        terminal_tools: StageTerminalTool[];
        max_turns: number;
    };

export interface ConfigureEnvelope {
    digest: string;
    config: RunConfig;
}

export function requireConfiguredTerminalTool(
    envelope: ConfigureEnvelope,
    tool: TerminalTool,
): void {
    if (!envelope.config.terminal_tools.some((configured) => configured === tool)) {
        throw new Error(`terminal tool is not configured for the active operation: ${tool}`);
    }
}

function isStringArray(value: unknown): value is string[] {
    if (!Array.isArray(value)) {
        return false;
    }
    return value.every((item) => typeof item === "string");
}

function isCheckKind(value: unknown): value is CheckKind {
    return CHECK_KINDS.some((kind) => kind === value);
}

function isStageKind(value: unknown): value is StageKind {
    return STAGE_KINDS.some((kind) => kind === value);
}

function isArrayOf<T extends string>(
    value: unknown,
    allowList: readonly T[],
): value is T[] {
    if (!Array.isArray(value)) {
        return false;
    }
    return value.every((item) => allowList.some((allowed) => allowed === item));
}

function isReadToolArray(value: unknown): value is ReadTool[] {
    return isArrayOf(value, READ_TOOLS);
}

function isCheckTerminalToolArray(value: unknown): value is CheckTerminalTool[] {
    return isArrayOf(value, CHECK_TERMINAL_TOOLS);
}

function isStageTerminalToolArray(value: unknown): value is StageTerminalTool[] {
    return isArrayOf(value, STAGE_TERMINAL_TOOLS);
}

function hasValidStageSubmissionTools(
    stage: StageKind,
    terminalTools: StageTerminalTool[],
): boolean {
    const submissionTool = STAGE_SUBMISSION_TOOLS[stage];
    return terminalTools.includes(submissionTool)
        && terminalTools.every((tool) =>
            tool === submissionTool || tool === "request_clarification"
        );
}

function isReviewContextTerminalToolArray(value: unknown): value is ReviewContextTerminalTool[] {
    return isArrayOf(value, REVIEW_CONTEXT_TERMINAL_TOOLS);
}

function isOperation(value: unknown): value is RunConfig["operation"] {
    if (typeof value !== "object" || value === null) {
        return false;
    }
    if (!("type" in value)) {
        return false;
    }
    if (value.type === "review_context") {
        return true;
    }
    if (value.type === "check") {
        return "check" in value && isCheckKind(value.check)
            && "target" in value && typeof value.target === "string" && value.target.length > 0
            && "expected_commits" in value
            && isStringArray(value.expected_commits)
            && value.expected_commits.length > 0
            && value.expected_commits.every((commit) => commit.length > 0);
    }
    if (value.type === "stage") {
        return "stage" in value && isStageKind(value.stage)
            && "target" in value && typeof value.target === "string" && value.target.length > 0
            && "expected_commits" in value
            && isStringArray(value.expected_commits)
            && value.expected_commits.length > 0
            && value.expected_commits.every((commit) => commit.length > 0);
    }
    return false;
}

export function decodeConfigureEnvelope(encoded: string): ConfigureEnvelope {
    let value: unknown;
    try {
        value = JSON.parse(Buffer.from(encoded, "base64url").toString("utf8"));
    } catch (error) {
        throw new Error(`invalid peer configuration encoding: ${String(error)}`);
    }
    if (typeof value !== "object" || value === null) {
        throw new Error("invalid peer configuration envelope");
    }

    const DIGEST_PATTERN = /^[0-9a-f]{64}$/;

    if (!("digest" in value) || typeof value.digest !== "string" || !DIGEST_PATTERN.test(value.digest)) {
        throw new Error("invalid peer configuration digest");
    }
    if (!("config" in value)) {
        throw new Error("invalid peer run configuration");
    }
    const config = value.config;
    if (typeof config !== "object" || config === null) {
        throw new Error("invalid peer run configuration");
    }
    if (
        !("tool_contract_digest" in config)
        || typeof config.tool_contract_digest !== "string"
        || !DIGEST_PATTERN.test(config.tool_contract_digest)
    ) {
        throw new Error("invalid peer tool contract digest");
    }
    if (!("operation" in config) || !isOperation(config.operation)) {
        throw new Error("invalid peer operation");
    }
    if (!("system_prompt" in config) || typeof config.system_prompt !== "string") {
        throw new Error("invalid peer system prompt");
    }
    if (!("read_tools" in config)) {
        throw new Error("missing peer read tools");
    }
    if (!("terminal_tools" in config)) {
        throw new Error("missing peer terminal tools");
    }
    if (!("max_turns" in config) || !Number.isSafeInteger(config.max_turns) || config.max_turns < 1) {
        throw new Error("invalid peer maximum turn count");
    }
    if (config.operation.type === "review_context") {
        if (!Array.isArray(config.read_tools) || config.read_tools.length !== 0) {
            throw new Error("invalid read tools for peer review context operation");
        }
        if (!isReviewContextTerminalToolArray(config.terminal_tools)) {
            throw new Error("invalid terminal tools for peer review context operation");
        }
        if (config.terminal_tools.length === 0) {
            throw new Error("peer review context operation requires at least one terminal tool");
        }
    } else if (config.operation.type === "check") {
        if (!isReadToolArray(config.read_tools)) {
            throw new Error("invalid read tools for peer check operation");
        }
        if (!isCheckTerminalToolArray(config.terminal_tools)) {
            throw new Error("invalid terminal tools for peer check operation");
        }
        if (config.terminal_tools.length === 0) {
            throw new Error("peer check operation requires at least one terminal tool");
        }
    } else {
        if (!isReadToolArray(config.read_tools)) {
            throw new Error("invalid read tools for peer stage operation");
        }
        if (!isStageTerminalToolArray(config.terminal_tools)) {
            throw new Error("invalid terminal tools for peer stage operation");
        }
        if (config.terminal_tools.length === 0) {
            throw new Error("peer stage operation requires at least one terminal tool");
        }
        if (!hasValidStageSubmissionTools(config.operation.stage, config.terminal_tools)) {
            throw new Error("invalid terminal tools for peer stage operation");
        }
    }
    return value as ConfigureEnvelope;
}
