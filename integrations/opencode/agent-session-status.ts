import type { Plugin } from "@opencode-ai/plugin"

const relevantEvents = new Set([
  "session.created",
  "session.updated",
  "session.deleted",
  "session.status",
  "session.idle",
  "permission.asked",
  "permission.replied",
  "question.asked",
  "question.replied",
  "question.rejected",
])

export const AgentSessionStatus: Plugin = async ({ directory }) => ({
  event: async ({ event }) => {
    if (!relevantEvents.has(event.type)) return

    const child = Bun.spawn(["agent-session-status", "event", "opencode"], {
      stdin: new Blob([JSON.stringify({ ...event, instanceDirectory: directory })]),
      stdout: "ignore",
      stderr: "ignore",
    })
    await child.exited
  },
})
