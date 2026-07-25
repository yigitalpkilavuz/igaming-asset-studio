/** Global background-job tracker. Long operations (generations, refines, upscales,
 *  process, AI sheets, batches) run HERE — module scope — so they survive component
 *  unmounts and view switches. Components derive busy state from `activeJob`, and
 *  completed `AssetRecord`s flow back through `recordUpdates` so the ledger and bench
 *  stay current no matter where the user is when a job lands. `JobToasts` renders the
 *  running/finished stack. */

import type { AssetRecord } from "$lib/ipc";

export type JobKind =
  | "generate"
  | "refine"
  | "upscale"
  | "process"
  | "sheet"
  | "anchor"
  | "batch";

export type Job = {
  id: number;
  gameId: string;
  assetKey: string | null;
  kind: JobKind;
  /** Fine-grained op tag for busy-state derivation ("proc" vs "upai" vs "upfree"…). */
  tag: string;
  label: string;
  status: "running" | "done" | "error";
  /** Live progress line (batch loops update it) or the error text. */
  message: string;
  startedAt: number;
};

export const jobsState = $state<{
  jobs: Job[];
  /** Append-only stream of records written by finished work; `rev` marks each entry. */
  recordUpdates: { rev: number; record: AssetRecord }[];
  rev: number;
  /** Bumped when a style anchor finishes generating (no record to stream). */
  anchorRev: number;
}>({ jobs: [], recordUpdates: [], rev: 0, anchorRev: 0 });

let nextId = 1;

/** Publish a freshly-written record so open views can merge it. */
export function pushRecord(record: AssetRecord) {
  jobsState.rev++;
  jobsState.recordUpdates.push({ rev: jobsState.rev, record });
  if (jobsState.recordUpdates.length > 200) {
    jobsState.recordUpdates.splice(0, jobsState.recordUpdates.length - 200);
  }
}

/** The running job for an asset (optionally narrowed by kind), if any. */
export function activeJob(assetKey: string, kind?: JobKind): Job | undefined {
  return jobsState.jobs.find(
    (j) => j.status === "running" && j.assetKey === assetKey && (!kind || j.kind === kind),
  );
}

export function anyRunning(): boolean {
  return jobsState.jobs.some((j) => j.status === "running");
}

export function dismissJob(id: number) {
  jobsState.jobs = jobsState.jobs.filter((j) => j.id !== id);
}

/** Run a long operation as a tracked background job. `exec` may return an AssetRecord
 *  (published automatically) and can report progress via its `update` callback.
 *  Returns a promise that settles with the job's final status — callers may await it
 *  for local UX, but nothing breaks if they've unmounted meanwhile. */
export function runJob(opts: {
  gameId: string;
  assetKey?: string | null;
  kind: JobKind;
  tag?: string;
  label: string;
  exec: (update: (msg: string) => void) => Promise<AssetRecord | null | void>;
}): Promise<{ status: "done" | "error"; error?: string; record?: AssetRecord }> {
  const job: Job = {
    id: nextId++,
    gameId: opts.gameId,
    assetKey: opts.assetKey ?? null,
    kind: opts.kind,
    tag: opts.tag ?? opts.kind,
    label: opts.label,
    status: "running",
    message: "",
    startedAt: Date.now(),
  };
  jobsState.jobs.push(job);
  const live = () => jobsState.jobs.find((j) => j.id === job.id);

  return (async () => {
    try {
      const result = await opts.exec((msg) => {
        const j = live();
        if (j) j.message = msg;
      });
      let record: AssetRecord | undefined;
      if (result && typeof result === "object" && "key" in result) {
        record = result as AssetRecord;
        pushRecord(record);
      }
      const j = live();
      if (j) {
        j.status = "done";
        j.message = "";
      }
      if (opts.kind === "anchor") jobsState.anchorRev++;
      setTimeout(() => dismissJob(job.id), 6000);
      return { status: "done" as const, record };
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      const j = live();
      if (j) {
        j.status = "error";
        j.message = message;
      }
      setTimeout(() => dismissJob(job.id), 15000);
      return { status: "error" as const, error: message };
    }
  })();
}

/** Whether a running job with this asset + tag exists (drives per-button busy states). */
export function runningTag(assetKey: string, tag: string): boolean {
  return jobsState.jobs.some(
    (j) => j.status === "running" && j.assetKey === assetKey && j.tag === tag,
  );
}
