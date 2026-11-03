// Rule contract 1: what a rule receives and what it may return. dovecote enforces every cap
// named here; anything it cannot validate is refused, never guessed at.

/** Any value that survives JSON. State documents hold only these. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** One reading of one pigeon. */
export interface Reading {
  /** Unix seconds, clamped by the server into [received_at - 24 h, received_at]. */
  at: number;
  /** Key to value, as the device sent it. Values are always strings. */
  metrics: Record<string, string>;
  /** Per key, the stored entry this reading displaced. `{}` when `degraded`. */
  previous: Record<string, { value: string; at: number }>;
}

/** What a rule receives: one telemetry batch of one pigeon in the rule's flock. */
export interface RuleEvent {
  contract: 1;
  /** The trigger. Only "telemetry" exists in contract 1. */
  hook: "telemetry";
  /**
   * 64 lowercase hex. Equal on a redelivery of the same batch. A device that resends a report
   * sends a new batch: a new id, and reading times a few seconds later.
   */
  batch_id: string;
  /** When dovecote built the event, unix seconds; in a dry run, the batch's last reading. */
  received_at: number;
  /** True when the platform lost this batch's `previous` values; `previous` is then `{}`. */
  degraded: boolean;
  rule: { id: string; version: number };
  flock: { id: string; name: string };
  pigeon: { id: string; name: string };
  /** 1 to 64 readings, oldest first. Batches arrive in no guaranteed order. */
  readings: Reading[];
  /** What this rule has stored for this pigeon and for its flock. `{}` when nothing. */
  state: { pigeon: Record<string, Json>; flock: Record<string, Json> };
}

/** A mail decision. The rule picks the words; the rule's settings pick the recipients. */
export interface RuleAlert {
  /** 1 to 64 bytes of [a-z0-9._:-]. Names the condition, per pigeon or per flock. */
  key: string;
  /**
   * "fire": mail once when the condition starts holding. "clear": mail once when it stops,
   * only after a fire. "notify": mail once per key within 7 days. Restating a fire or a clear
   * every batch is the intended way to write a held condition.
   */
  kind: "fire" | "clear" | "notify";
  severity: "info" | "warning" | "critical";
  /** "flock": one state for the whole flock, so the fleet gets one mail. */
  scope: "pigeon" | "flock";
  /** 1 to 120 bytes, plain text, no control characters. */
  subject: string;
  /** Up to 1,024 bytes, plain text; newline is the only control character allowed. */
  body?: string;
}

/** A derived value, kept beside the device's telemetry and never mixed into it. */
export interface RuleValue {
  /** 1 to 128 bytes. */
  key: string;
  /** Up to 1,024 bytes. Graphs read it when it parses as a number. */
  value: string;
  /** Unix seconds, clamped into the batch's first..last reading. Default: the last. */
  at?: number;
}

/**
 * What a rule may return. Every field is optional; returning nothing does nothing. The whole
 * result must serialize to JSON of at most 48 KiB. Every byte cap counts UTF-8 bytes.
 */
export interface RuleResult {
  /** Up to 8. */
  alerts?: RuleAlert[];
  /** Up to 16. */
  values?: RuleValue[];
  /**
   * A patch per document: each top-level key is set, `null` deletes it, an absent key is
   * kept. The stored pigeon document stays under 4 KiB and the flock document under 32 KiB.
   */
  state?: { pigeon?: Record<string, Json | null>; flock?: Record<string, Json | null> };
}

/** The default export of a rule module. */
export type Rule = (event: RuleEvent) => RuleResult | void | Promise<RuleResult | void>;
