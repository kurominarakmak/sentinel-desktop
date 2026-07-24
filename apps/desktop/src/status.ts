export type AgentEventPayload = { type: string };

export function statusFor(event: AgentEventPayload): string {
  if (event.type === "completed") return "Completed";
  if (event.type === "failed") return "Failed";
  if (event.type === "waiting_for_input") return "Waiting";
  return "Running";
}
