import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { type ConfigureEnvelope, decodeConfigureEnvelope } from "./protocol.ts";

export default function peerExtension(pi: ExtensionAPI) {
    let envelope: ConfigureEnvelope | undefined;
    let completedTurns = 0;

    const activateConfiguredTools = (
        configuredEnvelope: ConfigureEnvelope,
        terminalOnly: boolean,
    ) => {
        const available = new Set(pi.getAllTools().map((tool) => tool.name));
        const configuredTools = terminalOnly
            ? configuredEnvelope.config.terminal_tools
            : [
                ...configuredEnvelope.config.read_tools,
                ...configuredEnvelope.config.terminal_tools,
            ];
        const unavailableTools = configuredTools.filter((name) => !available.has(name));
        if (unavailableTools.length > 0) {
            throw new Error(`configured peer tools are unavailable: ${unavailableTools.join(", ")}`);
        }
        pi.setActiveTools(configuredTools);
    };

    pi.on("session_start", () => {
        envelope = undefined;
        completedTurns = 0;
        pi.setActiveTools([]);
    });

    pi.registerCommand("peer-configure-v1", {
        description: "Configure a versioned peer agent run",
        handler: async (args, ctx) => {
            const configuredEnvelope = decodeConfigureEnvelope(args.trim());
            if (envelope) {
                if (envelope.digest !== configuredEnvelope.digest) {
                    throw new Error("peer session is already configured differently");
                }
                ctx.ui.notify(`peer.configured:${envelope.digest}`, "info");
                return;
            }
            activateConfiguredTools(
                configuredEnvelope,
                configuredEnvelope.config.max_turns === 1,
            );
            envelope = configuredEnvelope;
            completedTurns = 0;
            ctx.ui.notify(`peer.configured:${envelope.digest}`, "info");
        },
    });

    pi.registerCommand("peer-continue-v1", {
        description: "Continue a configured peer agent run",
        handler: async (args) => {
            const digest = args.trim();
            if (!envelope || digest !== envelope.digest) {
                throw new Error("peer continuation does not match the active configuration");
            }
            if (completedTurns >= envelope.config.max_turns) {
                throw new Error("peer run has reached its configured turn limit");
            }
            pi.sendMessage(
                {
                    customType: "peer.continue",
                    content: "Continue the current task. Use a terminal tool to submit the final outcome.",
                    display: false,
                    details: {
                        digest,
                    },
                },
                {
                    triggerTurn: true,
                    deliverAs: "steer",
                },
            );
        },
    });

    pi.on("before_agent_start", (_event) => {
        if (!envelope) {
            throw new Error("peer session was prompted before configuration");
        }
        return {
            systemPrompt: envelope.config.system_prompt,
        };
    });

    pi.on("turn_end", (_event, ctx) => {
        if (!envelope) {
            return;
        }
        completedTurns += 1;
        if (completedTurns >= envelope.config.max_turns) {
            ctx.abort();
            return;
        }
        if (completedTurns >= envelope.config.max_turns - 1) {
            activateConfiguredTools(envelope, true);
        }
    });
}
