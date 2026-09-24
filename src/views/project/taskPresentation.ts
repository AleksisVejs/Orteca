import type { TaskEvent } from "../../types";

/** Only validated artifacts can establish a passing check. Prose cannot. */
export function verificationSummary(stages: Array<{ stage: string; artifact: unknown }>) {
  const last = stages.filter((s) => s.stage === "verify" || s.stage === "fix").at(-1);
  if (!last) return "No verification recorded";
  const artifact = last.artifact as { verdict?: string; checks?: Array<{ passed?: boolean }> } | null;
  if (!artifact || !Array.isArray(artifact.checks) || !artifact.checks.length) return "Verification result unavailable";
  const passed = artifact.checks.filter((check) => check?.passed === true).length;
  return artifact.verdict === "pass" && passed === artifact.checks.length
    ? `${passed} recorded ${passed === 1 ? "check" : "checks"} passed`
    : `${passed} of ${artifact.checks.length} recorded checks passed`;
}

export function savedArtifacts(events: TaskEvent[]) {
  return events.filter((event) => event.kind === "artifact").map((event) => {
    const payload = event.payload as { data?: { stage?: string; valid?: boolean; artifact?: unknown } } | null;
    return { stage: payload?.data?.stage ?? event.stage ?? "", artifact: payload?.data?.valid ? payload.data.artifact : null };
  });
}

export function unfinishedSummary(status: string) {
  switch (status) {
    case "done": return "Task finished";
    case "cancelled": return "Stopped early · changes kept";
    case "budgetReached": return "Limit reached · work remains";
    case "verifyFailed": return "Checks need attention";
    case "reviewRejected": return "Review needs attention";
    case "failed": return "Task failed · review the details";
    case "unchanged": return "Nothing needed changing · see why";
    default: return "Work is still in progress";
  }
}
