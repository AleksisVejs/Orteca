export function weeklyHeadroomStop(limits, providers, floor = 15, reserve = 5) {
  const required = floor + reserve;
  for (const provider of providers) {
    const entry = Object.entries(limits).find(([key]) => key === `${provider} week` || key.startsWith(`${provider} week `));
    if (!entry) continue;
    const [label, used] = entry;
    const remaining = 100 - used;
    if (remaining < required) return `STOP: ${label} has ${remaining}% remaining; need ${required}% (${floor}% floor + ${reserve}% arm reserve)`;
  }
  return null;
}
