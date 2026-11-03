// The wire between dovecote and main.js. Not part of what a rule author sees.

import type { Json, RuleEvent, RuleResult } from "./contract";

/** The event without its per-rule fields, shared by every rule in the bundle. */
export type CommonEvent = Omit<RuleEvent, "rule" | "state">;

/** One telemetry batch for every enabled rule of the flock: one dispatch per batch. */
export interface LiveInput {
  mode: "live";
  /** One wall budget for the whole dispatch, all rules together. */
  wall_ms: number;
  common: CommonEvent;
  rules: { id: string; version: number; state: RuleEvent["state"] }[];
}

/** A dry run: the same events through each listed rule, state threaded per pigeon. */
export interface ReplayInput {
  mode: "replay";
  /** Budget per event per rule; a timeout stops that rule's replay. */
  wall_ms: number;
  events: CommonEvent[];
  /** `seed`: the live state to start from; absent, the replay starts from `{}`. */
  rules: {
    id: string;
    version: number;
    seed?: { pigeons: Record<string, Record<string, Json>>; flock: Record<string, Json> };
  }[];
}

export type RunStatus = "ok" | "threw" | "timeout" | "skipped" | "version_mismatch" | "bad_output";

export interface RunResult {
  id: string;
  status: RunStatus;
  /**
   * Present when `ok`: the rule's result, serialized by the harness inside the rule's own
   * `try`, at most 48 KiB. dovecote parses and validates it alone, as untrusted input.
   */
  out_json?: string;
  /** Up to 1,024 bytes of the thrown value's stack or message. */
  error?: string;
  /** Up to 20 captured `console.log` lines of 256 bytes. */
  logs?: string[];
}

/** Either output carries `error` instead when the harness itself failed. */
export type HarnessError = { contract: 1; error: "bad_input" | "harness_failed" };

export interface LiveOutput {
  contract: 1;
  results: RunResult[];
}

export interface ReplayOutput {
  contract: 1;
  rules: {
    id: string;
    status: "ok" | "version_mismatch";
    /** Only runs that did something: quiet `ok` runs are left out. `i` indexes `events`. */
    runs: (RunResult & { i: number })[];
    stopped: { at: number; status: "timeout" } | null;
    final: { pigeons: Record<string, Record<string, Json>>; flock: Record<string, Json> };
  }[];
}
