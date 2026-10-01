/* Kestrel Phase 3a — wire-seam test (frontend half).
 *
 * The Rust half lives in `src-tauri/src/dto.rs`
 * (`op_event_bytes_match_the_golden_fixture`): it asserts that the exact
 * serialized bytes are committed to `test/fixtures/op-events.json`.
 *
 * THIS file is the other half: it reads that same fixture — real serialized
 * bytes, never hand-written payloads — `JSON.parse`s each case (exactly what
 * the webview receives over the Tauri Channel) and pushes the result through
 * the SHIPPED normalizer (`normalizeOpEvent` in `src/lib/ipc.ts`, imported
 * from source, not reimplemented) and the SHIPPED job manager (`OpManager`
 * in `src/lib/ops.ts`).
 *
 * Why this exists: an earlier version of this project shipped a UI running
 * entirely on `MockIpc` that looked correct on screen while nothing tested
 * the boundary. `tsc` cannot catch a wire mismatch — TypeScript types are
 * erased at runtime, so a `doneBytes` → `done_bytes` rename in the Rust
 * serializer compiles cleanly on both sides and then silently zeroes every
 * progress bar. Every golden assertion below pins an exact value, so
 * corrupting the fixture (renaming a field, flipping a tag, mis-nesting the
 * error) turns the suite red. That is the point: a test that cannot fail is
 * not evidence.
 *
 * Node-only (runs under `node --test` via esbuild; bundled to
 * `node_modules/.cache`, never imported by `src/`), so no Safari13
 * constraints apply here — but nothing in this file may migrate into `src/`.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// The shipped code under test. No reimplementation, no parallel copy.
import { normalizeOpEvent } from "../src/lib/ipc";
import type { KestrelIpc } from "../src/lib/ipc";
import { OpManager, isAlreadyExistsKind } from "../src/lib/ops";
import type {
  CmdError,
  JobId,
  OpEventDto,
  OpJob,
  OpRequestDto,
} from "../src/lib/types";
import type { OpsView } from "../src/lib/ops";

/* ------------------------------------------------------------------ */
/* Fixture: real Rust bytes, read from disk at test time               */
/* ------------------------------------------------------------------ */

interface GoldenCase {
  name: string;
  json: string;
}

function loadGolden(): GoldenCase[] {
  const path = resolve(process.cwd(), "test/fixtures/op-events.json");
  return JSON.parse(readFileSync(path, "utf8")) as GoldenCase[];
}

const GOLDEN = loadGolden();

/** The exact bytes for one committed case, as the webview would receive them. */
function rawJson(name: string): string {
  const found = GOLDEN.find(function (c) {
    return c.name === name;
  });
  assert.ok(found, "golden fixture must contain case " + name);
  return (found as GoldenCase).json;
}

/** `JSON.parse` (what the Channel delivers) → the shipped normalizer. */
function throughNormalizer(name: string): OpEventDto {
  const parsed: unknown = JSON.parse(rawJson(name));
  const ev = normalizeOpEvent(parsed);
  assert.ok(ev, "real Rust bytes for " + name + " must normalize, got null");
  return ev as OpEventDto;
}

function narrowProgress(ev: OpEventDto): {
  id: number;
  phase: string;
  doneBytes: number;
  totalBytes: number;
  doneItems: number;
  totalItems: number;
} {
  assert.equal(ev.type, "progress");
  if (ev.type !== "progress") throw new Error("unreachable");
  return ev;
}

function narrowDone(ev: OpEventDto): { id: number; summary: string } {
  assert.equal(ev.type, "done");
  if (ev.type !== "done") throw new Error("unreachable");
  return ev;
}

function narrowError(ev: OpEventDto): { id: number; error: CmdError } {
  assert.equal(ev.type, "error");
  if (ev.type !== "error") throw new Error("unreachable");
  return ev;
}

/* ------------------------------------------------------------------ */
/* Golden seam: every committed case through the real normalizer       */
/* ------------------------------------------------------------------ */

describe("golden seam: real Rust bytes through normalizeOpEvent", function () {
  it("the fixture covers exactly the nine committed cases", function () {
    assert.deepEqual(
      GOLDEN.map(function (c) {
        return c.name;
      }),
      [
        "progress_copying_with_known_totals",
        "progress_measuring_with_unknown_totals",
        "progress_deleting_has_no_byte_total",
        "progress_beyond_u32_byte_counts",
        "done_names_the_file",
        "error_cancelled_for_a_user_cancel",
        "error_already_exists_for_a_collision",
        "error_not_found_with_a_path",
        "error_unknown_from_the_non_exhaustive_fallback",
      ]
    );
  });

  it("progress with known totals keeps every counter", function () {
    const ev = narrowProgress(
      throughNormalizer("progress_copying_with_known_totals")
    );
    assert.equal(ev.id, 7);
    assert.equal(ev.phase, "copying");
    assert.equal(ev.doneBytes, 123);
    assert.equal(ev.totalBytes, 456);
    assert.equal(ev.doneItems, 1);
    assert.equal(ev.totalItems, 9);
  });

  it("measuring carries totalBytes 0 (unknown total, never guessed)", function () {
    const ev = narrowProgress(
      throughNormalizer("progress_measuring_with_unknown_totals")
    );
    assert.equal(ev.phase, "measuring");
    assert.equal(ev.doneBytes, 0);
    assert.equal(ev.totalBytes, 0);
    assert.equal(ev.doneItems, 3);
    assert.equal(ev.totalItems, 0);
  });

  it("deleting counts entries and has no byte total", function () {
    const ev = narrowProgress(
      throughNormalizer("progress_deleting_has_no_byte_total")
    );
    assert.equal(ev.phase, "deleting");
    assert.equal(ev.doneBytes, 0);
    assert.equal(ev.totalBytes, 0);
    assert.equal(ev.doneItems, 42);
    assert.equal(ev.totalItems, 42);
  });

  it("byte counts past u32 survive with exact precision", function () {
    const ev = narrowProgress(
      throughNormalizer("progress_beyond_u32_byte_counts")
    );
    assert.equal(ev.id, 4294967296);
    assert.equal(ev.doneBytes, 5368709120);
    assert.equal(ev.totalBytes, 8589934592);
    assert.equal(ev.doneItems, 4294967296);
    assert.equal(ev.totalItems, 4294967296);
  });

  it("done carries the backend summary verbatim", function () {
    const ev = narrowDone(throughNormalizer("done_names_the_file"));
    assert.equal(ev.id, 7);
    assert.equal(ev.summary, "copied report.pdf");
  });

  it("a user cancel arrives nested as error.cancelled with a null path", function () {
    // Pin the nesting itself: the frozen contract nests under "error"
    // exactly like WatchEventDto. A flattened shape here is a break.
    const raw = JSON.parse(rawJson("error_cancelled_for_a_user_cancel")) as {
      error: { kind: string; path: unknown; message: string };
    };
    assert.equal(raw.error.kind, "cancelled");
    assert.equal(raw.error.path, null);
    const ev = narrowError(
      throughNormalizer("error_cancelled_for_a_user_cancel")
    );
    assert.equal(ev.id, 7);
    assert.equal(ev.error.kind, "cancelled");
    assert.equal(ev.error.path, null);
    assert.equal(ev.error.message, "operation cancelled");
  });

  it("a collision is an already_exists error with the destination path", function () {
    const ev = narrowError(
      throughNormalizer("error_already_exists_for_a_collision")
    );
    assert.equal(ev.id, 9);
    assert.equal(ev.error.kind, "already_exists");
    assert.equal(ev.error.path, "/tmp/dst.txt");
    assert.ok(
      ev.error.message.length > 0,
      "the message must never be empty"
    );
    assert.ok(
      isAlreadyExistsKind(ev.error.kind),
      "the UI collision predicate must recognise the backend kind"
    );
  });

  it("not_found keeps its path", function () {
    const ev = narrowError(
      throughNormalizer("error_not_found_with_a_path")
    );
    assert.equal(ev.id, 11);
    assert.equal(ev.error.kind, "not_found");
    assert.equal(ev.error.path, "/nope");
  });

  it("the non_exhaustive unknown fallback still parses as an op error", function () {
    const ev = narrowError(
      throughNormalizer("error_unknown_from_the_non_exhaustive_fallback")
    );
    assert.equal(ev.id, 13);
    assert.equal(ev.error.kind, "unknown");
    assert.equal(ev.error.path, null);
  });
});

/* ------------------------------------------------------------------ */
/* The mismatches that would actually bite                            */
/* ------------------------------------------------------------------ */

describe("wire mismatches that would bite", function () {
  it("field casing is camelCase: snake_case bytes zero the counters", function () {
    // If the Rust serializer ever renamed doneBytes → done_bytes, tsc would
    // stay green on both sides and every bar would read 0. The golden pins
    // camelCase; this pins what a rename would cost.
    const ev = normalizeOpEvent(
      JSON.parse(
        '{"type":"progress","id":7,"phase":"copying",' +
          '"done_bytes":123,"total_bytes":456,' +
          '"done_items":1,"total_items":9}'
      )
    );
    assert.ok(ev, "shape still parses (numbers default, nothing throws)");
    const p = narrowProgress(ev as OpEventDto);
    assert.equal(p.doneBytes, 0);
    assert.equal(p.totalBytes, 0);
    // …while the real bytes keep their values (the golden test above).
    assert.equal(
      narrowProgress(throughNormalizer("progress_copying_with_known_totals"))
        .doneBytes,
      123
    );
  });

  it("the error must nest under error: a string error drops the event", function () {
    const bad = normalizeOpEvent(
      JSON.parse('{"type":"error","id":7,"error":"oops"}')
    );
    assert.equal(bad, null);
    const missing = normalizeOpEvent(JSON.parse('{"type":"error","id":7}'));
    assert.equal(missing, null);
  });

  it("unknown and missing type tags drop the event", function () {
    assert.equal(
      normalizeOpEvent(
        JSON.parse(
          '{"type":"collision","id":7,"phase":"copying",' +
            '"doneBytes":1,"totalBytes":2,"doneItems":1,"totalItems":2}'
        )
      ),
      null
    );
    assert.equal(
      normalizeOpEvent(JSON.parse('{"id":7,"phase":"copying"}')),
      null
    );
    assert.equal(normalizeOpEvent("BatchEnd"), null);
    assert.equal(normalizeOpEvent(null), null);
  });

  it("phase values: every frozen phase parses, unknown falls back to copying", function () {
    function phaseOf(rawPhase: string): string {
      const ev = normalizeOpEvent(
        JSON.parse(
          '{"type":"progress","id":7,"phase":' +
            JSON.stringify(rawPhase) +
            ',"doneBytes":0,"totalBytes":0,"doneItems":0,"totalItems":0}'
        )
      );
      assert.ok(ev);
      return narrowProgress(ev as OpEventDto).phase;
    }
    assert.equal(phaseOf("measuring"), "measuring");
    assert.equal(phaseOf("copying"), "copying");
    assert.equal(phaseOf("deleting"), "deleting");
    // A future backend phase degrades to copying rather than dropping the row.
    assert.equal(phaseOf("scanning"), "copying");
  });

  it("totalBytes 0 during measuring means indeterminate in the job", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a", dst: "/b" });
    fx.deliver(id, rawJson("progress_measuring_with_unknown_totals"), id);
    const job = mgr.liveJobs()[0];
    assert.equal(job.unknownTotal, true);
    fx.deliver(id, rawJson("progress_copying_with_known_totals"), id);
    assert.equal(mgr.liveJobs()[0].unknownTotal, false);
  });
});

/* ------------------------------------------------------------------ */
/* Frontend op logic (ops.ts), driven by real bytes where possible     */
/* ------------------------------------------------------------------ */

type OpHandler = (ev: OpEventDto) => void;

interface Fake {
  ipc: KestrelIpc;
  cancels: JobId[];
  /** Feed a raw JSON string through JSON.parse + the real normalizer. */
  deliver(jobId: JobId, json: string, eventId: JobId): void;
  handler(jobId: JobId): OpHandler;
}

function fakeIpc(): Fake {
  const handlers = new Map<JobId, OpHandler>();
  let next = 101;
  const cancels: JobId[] = [];
  const ipc = {
    backend: "mock",
    scanStart: function (): Promise<JobId> {
      throw new Error("unused in op tests");
    },
    scanCancel: function (): Promise<void> {
      throw new Error("unused in op tests");
    },
    stat: function (): Promise<never> {
      throw new Error("unused in op tests");
    },
    openPath: function (): Promise<never> {
      throw new Error("unused in op tests");
    },
    watchSubscribe: function (): Promise<JobId> {
      throw new Error("unused in op tests");
    },
    watchUnsubscribe: function (): Promise<void> {
      throw new Error("unused in op tests");
    },
    opStart: function (_req: OpRequestDto, onEvent: OpHandler): Promise<JobId> {
      const id = next++;
      handlers.set(id, onEvent);
      return Promise.resolve(id);
    },
    opCancel: function (id: JobId): Promise<void> {
      cancels.push(id);
      return Promise.resolve();
    },
  } as unknown as KestrelIpc;
  function handler(jobId: JobId): OpHandler {
    const h = handlers.get(jobId);
    assert.ok(h, "no live handler for job " + jobId);
    return h as OpHandler;
  }
  return {
    ipc,
    cancels,
    handler,
    deliver: function (jobId: JobId, json: string, eventId: JobId): void {
      // Rewrite the golden id onto this job: the bytes keep their real
      // shape, only the routing id changes.
      const raw = JSON.parse(json) as Record<string, unknown>;
      raw["id"] = eventId;
      const ev = normalizeOpEvent(raw);
      assert.ok(ev, "fixture bytes must still normalize after id rewrite");
      handler(jobId)(ev as OpEventDto);
    },
  };
}

interface ViewStub extends OpsView {
  errors: CmdError[];
  settled: number;
  lastJobs: OpJob[];
}

function viewStub(): ViewStub {
  const stub: ViewStub = {
    errors: [],
    settled: 0,
    lastJobs: [],
    onJobsChanged: function (jobs: OpJob[]): void {
      stub.lastJobs = jobs;
    },
    onOpError: function (error: CmdError): void {
      stub.errors.push(error);
    },
    onSettled: function (): void {
      stub.settled++;
    },
  };
  return stub;
}

function progressJson(
  id: JobId,
  phase: string,
  doneBytes: number,
  totalBytes: number
): string {
  return JSON.stringify({
    type: "progress",
    id,
    phase,
    doneBytes,
    totalBytes,
    doneItems: 0,
    totalItems: 0,
  });
}

describe("ops.ts: progress monotonicity as consumed", function () {
  it("monotonic progress counts events with no backwards jumps", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a", dst: "/b" });
    fx.deliver(id, progressJson(id, "copying", 100, 1000), id);
    fx.deliver(id, progressJson(id, "copying", 200, 1000), id);
    fx.deliver(id, progressJson(id, "copying", 300, 1000), id);
    const job = mgr.liveJobs()[0];
    assert.equal(job.progressEvents, 3);
    assert.equal(job.backwardsJumps, 0);
    assert.equal(job.doneBytes, 300);
  });

  it("a same-phase drop counts a backwards jump", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a", dst: "/b" });
    fx.deliver(id, progressJson(id, "copying", 400, 1000), id);
    fx.deliver(id, progressJson(id, "copying", 150, 1000), id);
    const job = mgr.liveJobs()[0];
    assert.equal(job.progressEvents, 2);
    assert.equal(job.backwardsJumps, 1);
  });

  it("a phase change resets the baseline instead of counting a jump", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a", dst: "/b" });
    fx.deliver(id, rawJson("progress_measuring_with_unknown_totals"), id);
    // measuring hands a known total to copying, which restarts from 0.
    fx.deliver(id, progressJson(id, "copying", 0, 1000), id);
    const job = mgr.liveJobs()[0];
    assert.equal(job.progressEvents, 2);
    assert.equal(job.backwardsJumps, 0);
  });
});

describe("ops.ts: cancel-report routing", function () {
  it("a terminal cancelled error after cancel stays cancelled, never failed", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a", dst: "/b" });
    mgr.cancel(id);
    assert.deepEqual(fx.cancels, [id]);
    fx.deliver(id, rawJson("error_cancelled_for_a_user_cancel"), id);
    const job = mgr.liveJobs()[0];
    assert.equal(job.status, "cancelled");
    assert.equal(job.settleSource, "backend");
    assert.equal(view.errors.length, 0);
    assert.equal(view.settled, 1);
  });

  it("a late done after cancel never revives the job", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a", dst: "/b" });
    mgr.cancel(id);
    fx.deliver(id, rawJson("done_names_the_file"), id);
    assert.equal(mgr.liveJobs()[0].status, "cancelling");
  });

  it("a cancelled-kind error with no local cancel is still a failure", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a", dst: "/b" });
    fx.deliver(id, rawJson("error_cancelled_for_a_user_cancel"), id);
    assert.equal(mgr.liveJobs()[0].status, "failed");
    assert.equal(view.errors.length, 1);
  });

  it("already_exists fails the job through the shared error sink, no dialog", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a", dst: "/b" });
    fx.deliver(id, rawJson("error_already_exists_for_a_collision"), id);
    const job = mgr.liveJobs()[0];
    assert.equal(job.status, "failed");
    assert.equal(job.error && job.error.kind, "already_exists");
    assert.equal(view.errors.length, 1);
    assert.equal(view.errors[0].path, "/tmp/dst.txt");
    assert.equal(view.settled, 1);
  });
});

describe("ops.ts: job-manager concurrency", function () {
  it("two in-flight ops track independently by backend id", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const a = await mgr.start({ op: "copy", src: "/a", dst: "/b" });
    const b = await mgr.start({ op: "trash", src: "/c" });
    assert.equal(mgr.activeCount(), 2);
    fx.deliver(a, progressJson(a, "copying", 100, 1000), a);
    fx.deliver(b, progressJson(b, "deleting", 5, 0), b);
    fx.deliver(a, progressJson(a, "copying", 200, 1000), a);
    const jobs = mgr.liveJobs();
    const ja = jobs.find(function (j) {
      return j.id === a;
    }) as OpJob;
    const jb = jobs.find(function (j) {
      return j.id === b;
    }) as OpJob;
    assert.equal(ja.doneBytes, 200);
    assert.equal(jb.doneItems, 0);
    assert.equal(jb.phase, "deleting");
    assert.equal(jb.unknownTotal, true);
    // Settling one leaves the other running.
    fx.deliver(a, rawJson("done_names_the_file"), a);
    assert.equal(
      (mgr.liveJobs().find(function (j) {
        return j.id === a;
      }) as OpJob).status,
      "done"
    );
    assert.equal(
      (mgr.liveJobs().find(function (j) {
        return j.id === b;
      }) as OpJob).status,
      "running"
    );
    assert.equal(mgr.activeCount(), 1);
  });

  it("events for unknown ids are ignored and terminal rows dismiss", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "delete", src: "/x", recursive: true });
    const before = mgr.liveJobs().length;
    fx.handler(id)({ type: "done", id: 999999, summary: "ghost" });
    assert.equal(mgr.liveJobs().length, before);
    fx.deliver(id, rawJson("done_names_the_file"), id);
    // A running/done guard: dismiss drops only the terminal row.
    mgr.dismiss(id);
    assert.equal(mgr.liveJobs().length, 0);
  });
});
