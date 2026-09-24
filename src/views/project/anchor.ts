// Window anchoring: one tiny call on the cheapest model starts a plan's
// 5-hour window at a chosen time, so it resets when the user needs it to.
// Off by default. The schedule lives here because only the UI has local time.
import { anchorPing, appSettings, providerLimits } from "../../api";
import type { Limits, ProviderId } from "../../types";

export type AnchorMode = "off" | "morning" | "continuous";
export interface Anchor {
  mode: AnchorMode;
  /** Work starts and ends, `HH:MM` local time. */
  start: string;
  end: string;
  /** Morning: hours into the workday the limit is usually hit. */
  hours: number;
  providers: ProviderId[];
}
export const ANCHOR_OFF: Anchor = { mode: "off", start: "09:00", end: "18:00", hours: 3, providers: ["claude"] };

const HOUR = 3_600_000;
/** How late a ping may still go: a machine that slept through it skips it. */
const LATE_MS = 10 * 60_000;
/** A run this recent already started the window. */
const RECENT_MS = 30 * 60_000;

function at(day: Date, hhmm: string): number {
  const [h, m] = hhmm.split(":").map(Number);
  const d = new Date(day);
  d.setHours(h ?? 0, m ?? 0, 0, 0);
  return d.getTime();
}

/** Morning: work start minus (5 - hours), so the window resets `hours` into
 *  the day. Today's until it is too late to send, then tomorrow's. */
export function morningPing(a: Anchor, now: number): number {
  const lead = (5 - Math.min(5, Math.max(0, a.hours))) * HOUR;
  const today = at(new Date(now), a.start) - lead;
  return today + LATE_MS >= now ? today : at(new Date(now + 24 * HOUR), a.start) - lead;
}

/** Continuous: a minute after the 5-hour window resets, inside work hours. */
export function continuousPing(a: Anchor, now: number, resetsAt: number | null): number | null {
  if (resetsAt === null) return null;
  const ping = resetsAt + 60_000;
  const day = new Date(ping);
  return ping >= at(day, a.start) && ping <= at(day, a.end) && ping + LATE_MS >= now ? ping : null;
}

/** When the 5-hour window resets, from a reading. Unix ms, or null. */
export function sessionReset(reading: Limits | undefined): number | null {
  const w = reading?.windows.find((w) => w.label === "session" || w.label === "5-hour");
  return w?.resetsAt ? w.resetsAt * 1000 : null;
}

export function nextPing(a: Anchor, now: number, reading: Limits | undefined): number | null {
  if (a.mode === "morning") return morningPing(a, now);
  if (a.mode === "continuous") return continuousPing(a, now, sessionReset(reading));
  return null;
}

/** Send now: due, not too late, not already sent, and no run going or just done. */
export function due(ping: number | null, now: number, lastPing: number, work: { running: number; lastEnd: number }): boolean {
  return ping !== null && now >= ping && now - ping <= LATE_MS && lastPing < ping
    && work.running === 0 && now - work.lastEnd > RECENT_MS;
}

/** What runs are doing, across every open project. */
export const work = { running: 0, lastEnd: 0 };

let started = false;
const lastPing: Partial<Record<ProviderId, number>> = {};

export function readAnchor(settings: Record<string, string>): Anchor {
  try {
    return { ...ANCHOR_OFF, ...JSON.parse(settings.anchor ?? "{}") };
  } catch {
    return ANCHOR_OFF;
  }
}

/** One ticker for the whole app, however many projects are open. */
export function startAnchor() {
  if (started) return;
  started = true;
  setInterval(() => void tick(), 60_000);
}

async function tick() {
  const a = readAnchor(await appSettings().catch(() => ({})));
  if (a.mode === "off") return;
  const readings = a.mode === "continuous" ? await providerLimits().catch(() => []) : [];
  for (const id of a.providers) {
    const now = Date.now();
    const ping = nextPing(a, now, readings.find((l) => l.id === id));
    if (!due(ping, now, lastPing[id] ?? 0, work)) continue;
    lastPing[id] = now;
    await anchorPing(id).catch(() => null);
  }
}
