/* Kestrel Phase 3b — collision-dialog tests.
 *
 * What is under test, and why it is worth a file:
 *
 *   1. `normalizeOpEvent` on the NEW `collision` variant — including the one
 *      case that must DROP the event (a collision with no `dst`). A dialog
 *      built for a path we cannot name would offer a choice that settles
 *      nothing, leaving the engine blocked forever.
 *   2. `OpManager`'s paused state: a collision genuinely pauses, it is not
 *      `running` and not terminal, and the row's phase text says so.
 *   3. Answer routing: the right `id`, the right `dst`, the right decision —
 *      and every way an answer can be WRONG is refused, including a stale one
 *      after the op has completed.
 *   4. `collisionCopy`: that overwrite and abort do not read alike, that
 *      overwrite says something true about a FOLDER (the engine merges there;
 *      "it will be replaced" would be a lie), and that no "apply to all"
 *      affordance is offered in the copy at all.
 *   5. `planCollisionKey`: Esc consumes and does nothing. This is the single
 *      most dangerous default in the whole feature, so it is pinned.
 *
 * `ui/test/fixtures/op-events.json` is NOT edited here: it is generated from
 * the Rust side (`KESTREL_UPDATE_GOLDEN=1 cargo test ... op_event_bytes_match_
 * the_golden_fixture`) and read by BOTH halves. Hand-writing a collision case
 * into it would desynchronise the two and make the golden suite a liar. Until
 * the backend lane regenerates it, the collision payloads below are written as
 * inline JSON strings built from the FROZEN CONTRACT shape, and every test
 * that asserts on them says which they are.
 *
 * Node-only (bundled to /tmp by esbuild and run under `node --test`); nothing
 * here may migrate into `src/`.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { normalizeOpEvent, normalizePendingCollisions } from "../src/lib/ipc";
import type { KestrelIpc } from "../src/lib/ipc";
import { OpManager, type CollisionState, type OpsView } from "../src/lib/ops";
import type { CmdError, JobId, OpEventDto, OpJob, OpRequestDto } from "../src/lib/types";
import {
  DECISION_ORDER,
  DEFAULT_FOCUS_DECISION,
  collisionCopy,
  planCollisionKey,
  questionFrom,
  type CollisionQuestion,
} from "../src/lib/collision";

/* ------------------------------------------------------------------ */
/* Fake IPC that records answers                                      */
/* ------------------------------------------------------------------ */

interface Answered {
  id: JobId;
  dst: string;
  decision: string;
}

interface Fake {
  ipc: KestrelIpc;
  answers: Answered[];
  cancels: JobId[];
  /** Rows the fake backend will report from `opPendingCollisions()`. */
  pending: Array<{ id: JobId; dst: string; kind: string }>;
  /** Feed a raw JSON string through the SHIPPED normalizer, as a Channel does. */
  deliver(jobId: JobId, json: string): void;
  handler(jobId: JobId): (ev: OpEventDto) => void;
}

function fakeIpc(): Fake {
  const handlers = new Map<JobId, (ev: OpEventDto) => void>();
  const answers: Answered[] = [];
  const cancels: JobId[] = [];
  const pending: Array<{ id: JobId; dst: string; kind: string }> = [];
  let next = 501;
  const fake: Fake = {
    ipc: null as unknown as KestrelIpc,
    answers,
    cancels,
    pending,
    handler: (): never => {
      throw new Error("not installed yet");
    },
    deliver: (): void => {},
  };
  const ipc = {
    backend: "mock",
    scanStart: (): never => {
      throw new Error("unused in collision tests");
    },
    scanCancel: (): never => {
      throw new Error("unused in collision tests");
    },
    stat: (): never => {
      throw new Error("unused in collision tests");
    },
    openPath: (): never => {
      throw new Error("unused in collision tests");
    },
    watchSubscribe: (): never => {
      throw new Error("unused in collision tests");
    },
    watchUnsubscribe: (): never => {
      throw new Error("unused in collision tests");
    },
    opStart: (_req: OpRequestDto, onEvent: (ev: OpEventDto) => void): Promise<JobId> => {
      const id = next++;
      handlers.set(id, onEvent);
      return Promise.resolve(id);
    },
    opCancel: (id: JobId): Promise<void> => {
      cancels.push(id);
      return Promise.resolve();
    },
    opCollisionAnswer: (
      id: JobId,
      dst: string,
      decision: string
    ): Promise<void> => {
      answers.push({ id, dst, decision });
      return Promise.resolve();
    },
    // Drains the queue: a caller seeds `pending` per call, so each poll sees
    // exactly the rows that call is meant to observe.
    opPendingCollisions: (): Promise<
      Array<{ id: JobId; dst: string; kind: string }>
    > =>
      Promise.resolve(pending.splice(0, pending.length)),
  } as unknown as KestrelIpc;
  fake.ipc = ipc;
  fake.handler = (jobId) => {
    const h = handlers.get(jobId);
    assert.ok(h, "no live handler for job " + jobId);
    return h as (ev: OpEventDto) => void;
  };
  fake.deliver = (jobId, json) => {
    const ev = normalizeOpEvent(JSON.parse(json));
    assert.ok(ev, "payload must normalize: " + json);
    (handlers.get(jobId) as (e: OpEventDto) => void)(ev as OpEventDto);
  };
  return fake;
}

interface ViewStub extends OpsView {
  errors: CmdError[];
  settled: number;
  opened: CollisionState[];
  closed: JobId[];
}

function viewStub(): ViewStub {
  const stub: ViewStub = {
    errors: [],
    settled: 0,
    opened: [],
    closed: [],
    onJobsChanged: (): void => {},
    onOpError: (error) => stub.errors.push(error),
    onSettled: (): void => {
      stub.settled++;
    },
    onCollision: (job) => {
      if (job.collision) stub.opened.push(job.collision);
    },
    onCollisionClosed: (id) => stub.closed.push(id),
  };
  return stub;
}

/* Frozen-contract payload builders. NOT golden bytes — see the header note. */
function collisionJson(id: JobId, dst: string, kind: string): string {
  return JSON.stringify({ type: "collision", id, dst, kind });
}

/**
 * The REAL Rust bytes for the collision variant, transcribed from
 * `src-tauri/src/commands.rs::collision_event_bytes_are_frozen`, which pins
 * them with `assert_eq!(serde_json::to_string(&dto), want)` — the same
 * serialize-then-compare discipline as the 3a golden fixture, and the test
 * comment there says explicitly that "the frontend normaliser reads this exact
 * text".
 *
 * These are NOT added to `test/fixtures/op-events.json`, and that omission is
 * deliberate rather than an oversight. That file is written by the Rust side
 * (`KESTREL_UPDATE_GOLDEN=1 cargo test ... op_event_bytes_match_the_golden_
 * fixture`) and read by BOTH halves; editing it here would desynchronise the
 * two and turn a golden fixture into a hand-typed fiction. The bytes below are
 * held separately, with a pointer to their source, so the frontend can pin the
 * real text today and the golden file stays owned by the lane that generates
 * it. When the fixture is regenerated with these cases, the duplicate
 * constants here should be deleted in favour of reading the fixture.
 */
const RUST_COLLISION_BYTES: Array<{ name: string; json: string }> = [
  {
    name: "file",
    json: '{"type":"collision","id":7,"dst":"/home/u/dst.txt","kind":"file"}',
  },
  {
    name: "dir",
    json: '{"type":"collision","id":8,"dst":"/home/u/dst","kind":"dir"}',
  },
  {
    name: "symlink",
    json: '{"type":"collision","id":9,"dst":"/home/u/link","kind":"symlink"}',
  },
  {
    name: "other",
    json: '{"type":"collision","id":10,"dst":"/home/u/fifo","kind":"other"}',
  },
];

function progressJson(id: JobId, doneBytes: number, totalBytes: number): string {
  return JSON.stringify({
    type: "progress",
    id,
    phase: "copying",
    doneBytes,
    totalBytes,
    doneItems: 1,
    totalItems: 4,
  });
}

function doneJson(id: JobId): string {
  return JSON.stringify({ type: "done", id, summary: "copied report.pdf" });
}

function errorJson(id: JobId, kind: string, path: string | null): string {
  return JSON.stringify({
    type: "error",
    id,
    error: { kind, path, message: kind + ": " + (path || "-") },
  });
}

function jobOf(mgr: OpManager, id: JobId): OpJob {
  const j = mgr
    .liveJobs()
    .find((x) => x.id === id) as OpJob | undefined;
  assert.ok(j, "job " + id + " must exist");
  return j as OpJob;
}

/* ------------------------------------------------------------------ */
/* 1. Normalizing the new variant                                     */
/* ------------------------------------------------------------------ */

describe("normalizeOpEvent: the collision variant", function () {
  it("keeps id, dst and kind exactly as sent", function () {
    const ev = normalizeOpEvent(
      JSON.parse(collisionJson(7, "/home/a/inbox/Report.pdf", "file"))
    );
    assert.ok(ev);
    assert.equal(ev.type, "collision");
    if (ev.type !== "collision") throw new Error("unreachable");
    assert.equal(ev.id, 7);
    assert.equal(ev.dst, "/home/a/inbox/Report.pdf");
    assert.equal(ev.kind, "file");
  });

  it("accepts every frozen kind and degrades unknown ones to other", function () {
    const kinds = ["file", "dir", "symlink", "other"];
    for (const k of kinds) {
      const ev = normalizeOpEvent(
        JSON.parse(collisionJson(1, "/x", k))
      ) as { kind: string };
      assert.equal(ev.kind, k);
    }
    // A kind the contract grows later must not drop the event: it only picks
    // the wording, never whether the answer can be routed.
    const future = normalizeOpEvent(
      JSON.parse(collisionJson(1, "/x", "socket"))
    ) as { kind: string; dst: string };
    assert.equal(future.kind, "other");
    assert.equal(future.dst, "/x");
  });

  it("canonicalises a capitalised or underscored kind", function () {
    const dir = normalizeOpEvent(
      JSON.parse(collisionJson(1, "/x", "Dir"))
    ) as { kind: string };
    assert.equal(dir.kind, "dir");
    const directory = normalizeOpEvent(
      JSON.parse(collisionJson(1, "/x", "directory"))
    ) as { kind: string };
    assert.equal(directory.kind, "dir");
  });

  it("matches the type tag case-insensitively", function () {
    const ev = normalizeOpEvent(
      JSON.parse('{"type":"Collision","id":3,"dst":"/x","kind":"file"}')
    );
    assert.ok(ev);
    assert.equal(ev.type, "collision");
  });

  it("DROPS a collision with no usable dst rather than showing a dead dialog", function () {
    // The load-bearing negative. `op_collision_answer` is keyed by (id, dst);
    // answering an event whose dst we cannot name settles nothing, the engine
    // stays blocked, and the user is staring at a choice that does nothing.
    for (const raw of [
      '{"type":"collision","id":7,"kind":"file"}',
      '{"type":"collision","id":7,"dst":"","kind":"file"}',
      '{"type":"collision","id":7,"dst":null,"kind":"file"}',
      '{"type":"collision","id":7,"dst":42,"kind":"file"}',
    ]) {
      assert.equal(
        normalizeOpEvent(JSON.parse(raw)),
        null,
        "must drop: " + raw
      );
    }
  });

  it("keeps an id of 0 rather than treating it as absent", function () {
    // `numOf` turns a missing id into 0, so 0 is ambiguous by construction.
    // It must still produce an event with the id it was given, not null.
    const ev = normalizeOpEvent(
      JSON.parse(collisionJson(0, "/x", "file"))
    ) as { id: number };
    assert.equal(ev.id, 0);
  });

  it("a missing id does not drop the event; it normalises to 0", function () {
    // Matches how progress/done/error already behave. The manager drops the
    // event later by id lookup, which is the layer that owns that decision.
    const ev = normalizeOpEvent(
      JSON.parse('{"type":"collision","dst":"/x","kind":"file"}')
    ) as { id: number };
    assert.equal(ev.id, 0);
  });

  it("normalises the REAL Rust bytes for every kind, byte for byte", function () {
    // These four strings are `serde_json::to_string(&OpEventDto::Collision{
    // ..})` as pinned in the Rust suite. If the Rust serializer ever renames
    // `dst` or capitalises a kind, that Rust test fails AND this one does —
    // which is the point of holding the real text on this side.
    const expected: Array<{ name: string; id: number; dst: string; kind: string }> = [
      { name: "file", id: 7, dst: "/home/u/dst.txt", kind: "file" },
      { name: "dir", id: 8, dst: "/home/u/dst", kind: "dir" },
      { name: "symlink", id: 9, dst: "/home/u/link", kind: "symlink" },
      { name: "other", id: 10, dst: "/home/u/fifo", kind: "other" },
    ];
    assert.equal(RUST_COLLISION_BYTES.length, expected.length);
    for (let i = 0; i < expected.length; i++) {
      const want = expected[i];
      const raw = RUST_COLLISION_BYTES[i];
      const ev = normalizeOpEvent(JSON.parse(raw.json));
      assert.ok(ev, want.name + " bytes must normalize");
      assert.equal(ev.type, "collision", want.name);
      if (ev.type !== "collision") throw new Error("unreachable");
      assert.equal(ev.id, want.id, want.name + " id");
      assert.equal(ev.dst, want.dst, want.name + " dst");
      assert.equal(ev.kind, want.kind, want.name + " kind");
    }
  });

  it("a collision kind rename on the Rust side would break the normaliser, loudly", function () {
    // The counterpart to the bytes above: if `kind` stopped being sent, the
    // normaliser must not silently keep working, because "other" picks a
    // different sentence than "dir" (merge vs replace). A missing kind is a
    // content change, so it is asserted rather than tolerated.
    const noKind = normalizeOpEvent(
      JSON.parse('{"type":"collision","id":7,"dst":"/home/u/dst"}')
    ) as { kind: string; dst: string };
    assert.equal(noKind.kind, "other");
    assert.equal(noKind.dst, "/home/u/dst");
  });

  it("does not swallow the 3a error path or the other variants", function () {
    const err = normalizeOpEvent(
      JSON.parse(
        '{"type":"error","id":9,"error":{"kind":"already_exists","path":"/d","message":"x"}}'
      )
    );
    assert.ok(err && err.type === "error");
    const prog = normalizeOpEvent(JSON.parse(progressJson(2, 1, 2)));
    assert.ok(prog && prog.type === "progress");
    const done = normalizeOpEvent(JSON.parse(doneJson(2)));
    assert.ok(done && done.type === "done");
  });
});

/* ------------------------------------------------------------------ */
/* 2. A collision genuinely pauses the op                             */
/* ------------------------------------------------------------------ */

describe("ops.ts: a collision pauses the op", function () {
  it("moves the job to paused and records the destination", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a/Report.pdf", dst: "/b/Report.pdf" });
    fx.deliver(id, progressJson(id, 100, 1000));
    assert.equal(jobOf(mgr, id).status, "running");

    fx.deliver(id, collisionJson(id, "/b/Report.pdf", "file"));
    const job = jobOf(mgr, id);
    assert.equal(job.status, "paused");
    assert.equal(job.collision && job.collision.dst, "/b/Report.pdf");
    assert.equal(job.collisionCount, 1);
    assert.equal(view.opened.length, 1);
    // Still active: paused is unfinished, and the dialog is what settles it.
    assert.equal(mgr.activeCount(), 1);
  });

  it("does not advance the byte counters while paused", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, progressJson(id, 100, 1000));
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    // The engine is blocked, so the number it reported before the pause is the
    // number that stands. Claiming more progress here would be a lie.
    assert.equal(jobOf(mgr, id).doneBytes, 100);
    assert.equal(jobOf(mgr, id).unknownTotal, false);
  });

  it("progress events for a paused job do not resume it", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    fx.deliver(id, progressJson(id, 500, 1000));
    const job = jobOf(mgr, id);
    assert.equal(job.status, "paused");
    // The counters still update — the event is not dropped — but the job does
    // not go back to running while a decision is owed.
    assert.equal(job.collision && job.collision.dst, "/b/f");
  });

  it("a second collision parks again: collisions are sequential, never batched", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/dir", dst: "/b/dir" });
    fx.deliver(id, collisionJson(id, "/b/dir/one.txt", "file"));
    const first = jobOf(mgr, id).collision as CollisionState;
    fx.deliver(id, progressJson(id, 10, 100));
    fx.deliver(id, collisionJson(id, "/b/dir/two.txt", "file"));
    const second = jobOf(mgr, id).collision as CollisionState;
    assert.equal(second.dst, "/b/dir/two.txt");
    // A NEW decision, not the old one: distinct seq and distinct dst, so an
    // answer aimed at the first can never settle the second.
    assert.notEqual(second.seq, first.seq);
    assert.equal(jobOf(mgr, id).collisionCount, 2);
    assert.equal(jobOf(mgr, id).status, "paused");
  });

  it("two collisions on different ops are tracked independently", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const a = await mgr.start({ op: "copy", src: "/a/x", dst: "/b/x" });
    const b = await mgr.start({ op: "move", src: "/c/y", dst: "/d/y" });
    fx.deliver(a, collisionJson(a, "/b/x", "file"));
    fx.deliver(b, collisionJson(b, "/d/y", "dir"));
    assert.equal(jobOf(mgr, a).collision && jobOf(mgr, a).collision!.dst, "/b/x");
    assert.equal(jobOf(mgr, b).collision && jobOf(mgr, b).collision!.dst, "/d/y");
    assert.equal(mgr.activeCount(), 2);
    // Answering one leaves the other owed.
    const ca = mgr.collisionFor(a) as CollisionState;
    assert.equal(mgr.answer(ca, "skip"), true);
    assert.equal(mgr.collisionFor(a), null);
    assert.ok(mgr.collisionFor(b));
  });
});

/* ------------------------------------------------------------------ */
/* 3. Answer routing                                                 */
/* ------------------------------------------------------------------ */

describe("ops.ts: answer routing", function () {
  it("routes the exact id, dst and decision to op_collision_answer", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a/R.pdf", dst: "/b/R.pdf" });
    fx.deliver(id, collisionJson(id, "/b/R.pdf", "file"));
    const coll = mgr.collisionFor(id) as CollisionState;

    assert.equal(mgr.answer(coll, "skip"), true);
    assert.deepEqual(fx.answers, [
      { id, dst: "/b/R.pdf", decision: "skip" },
    ]);
    // Answering a collision is NOT a cancel: nothing else went out.
    assert.deepEqual(fx.cancels, []);
    assert.equal(view.errors.length, 0);
    assert.equal(mgr.collisionFor(id), null);
    assert.equal(view.closed[0], id);
  });

  it("returns the job to running after skip or overwrite", async function () {
    for (const decision of ["skip", "overwrite"] as const) {
      const fx = fakeIpc();
      const mgr = new OpManager(fx.ipc, viewStub());
      const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
      fx.deliver(id, collisionJson(id, "/b/f", "file"));
      const coll = mgr.collisionFor(id) as CollisionState;
      mgr.answer(coll, decision);
      assert.equal(jobOf(mgr, id).status, "running", decision);
      assert.equal(jobOf(mgr, id).collision, null, decision);
    }
  });

  it("abort goes out as a collision answer and marks the job stopping", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    const coll = mgr.collisionFor(id) as CollisionState;

    assert.equal(mgr.answer(coll, "abort"), true);
    // Abort is answered through the collision channel, NOT op_cancel: the
    // engine is blocked inside the handshake and only the answer releases it.
    assert.deepEqual(fx.answers, [{ id, dst: "/b/f", decision: "abort" }]);
    assert.deepEqual(fx.cancels, []);
    assert.equal(jobOf(mgr, id).status, "cancelling");
    assert.equal(jobOf(mgr, id).collision, null);
  });

  it("the cancelled terminal error after an abort settles as cancelled, not failed", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    mgr.answer(mgr.collisionFor(id) as CollisionState, "abort");
    fx.deliver(id, errorJson(id, "cancelled", "/b/f"));
    const job = jobOf(mgr, id);
    assert.equal(job.status, "cancelled");
    assert.equal(job.settleSource, "backend");
    // Not a failure: nothing goes to the shared error sink.
    assert.equal(view.errors.length, 0);
  });

  it("REFUSES an answer naming the wrong dst", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    const coll = mgr.collisionFor(id) as CollisionState;
    const wrong: CollisionState = { ...coll, dst: "/b/OTHER" };
    assert.equal(mgr.answer(wrong, "overwrite"), false);
    assert.equal(fx.answers.length, 0);
    // The real collision is still owed, still paused.
    assert.ok(mgr.collisionFor(id));
    assert.equal(jobOf(mgr, id).status, "paused");
  });

  it("REFUSES a stale answer aimed at a previous collision of the same op", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/d", dst: "/b/d" });
    fx.deliver(id, collisionJson(id, "/b/d/one", "file"));
    const first = mgr.collisionFor(id) as CollisionState;
    fx.deliver(id, collisionJson(id, "/b/d/two", "file"));
    // The dialog for "one" answers after "two" has arrived. Same job id, so
    // only the seq distinguishes them — this is the case that would otherwise
    // overwrite the wrong file.
    assert.equal(mgr.answer(first, "overwrite"), false);
    assert.equal(fx.answers.length, 0);
    assert.equal((mgr.collisionFor(id) as CollisionState).dst, "/b/d/two");
  });

  it("REFUSES a second answer for a collision already answered", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    const coll = mgr.collisionFor(id) as CollisionState;
    assert.equal(mgr.answer(coll, "overwrite"), true);
    assert.equal(mgr.answer(coll, "overwrite"), false);
    assert.equal(fx.answers.length, 1);
  });

  it("IGNORES a stale answer after the op completed", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    const coll = mgr.collisionFor(id) as CollisionState;
    mgr.answer(coll, "skip");
    fx.deliver(id, doneJson(id));
    assert.equal(jobOf(mgr, id).status, "done");

    // The op is finished. An answer that arrives now must go nowhere: there is
    // no engine waiting on it, and sending one would be a decision with no
    // referent.
    assert.equal(mgr.answer(coll, "overwrite"), false);
    assert.equal(fx.answers.length, 1);
    assert.equal(fx.answers[0].decision, "skip");
  });

  it("IGNORES a stale answer after completion even when a collision is still recorded", async function () {
    // The guard that actually stops this is `isTerminal`, and proving it needs
    // a case where the OTHER guards cannot help. `clearCollision` runs on the
    // terminal event, so `job.collision` is normally null by the time a stale
    // answer arrives and the owed-collision guard would catch it. This case
    // puts the collision BACK after completion — the shape a lost terminal
    // event, a re-sync row, or any future code path that leaves stale state
    // would produce — and asserts the job's terminal status alone is enough to
    // refuse. Without the `isTerminal` check this answers a finished op.
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    const coll = mgr.collisionFor(id) as CollisionState;
    mgr.answer(coll, "skip");
    fx.deliver(id, doneJson(id));

    // Re-seed the stale state the guard has to survive.
    jobOf(mgr, id).collision = coll;
    assert.equal(jobOf(mgr, id).status, "done");

    assert.equal(mgr.answer(coll, "overwrite"), false);
    assert.equal(fx.answers.length, 1);
    assert.equal(fx.answers[0].decision, "skip");
  });

it("IGNORES an answer after the op FAILED", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    const coll = mgr.collisionFor(id) as CollisionState;
    mgr.answer(coll, "skip");
    fx.deliver(id, errorJson(id, "not_found", "/nope"));
    assert.equal(jobOf(mgr, id).status, "failed");
    assert.equal(mgr.answer(coll, "overwrite"), false);
    assert.equal(fx.answers.length, 1);
  });

  it("IGNORES a collision event for a job that no longer exists", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, doneJson(id));
    mgr.dismiss(id);
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    assert.equal(mgr.collisionFor(id), null);
    assert.equal(view.opened.length, 0);
  });

  it("does not open a dialog for a collision on a cancelled op", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    mgr.cancel(id);
    // A collision that races the cancel. The op is on its way out; there is
    // nothing left to decide and no dialog should appear.
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    assert.equal(jobOf(mgr, id).status, "cancelling");
    assert.equal(jobOf(mgr, id).collision, null);
    assert.equal(view.opened.length, 0);
  });

  it("clearing a collision by cancelling voids the dialog", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    const coll = mgr.collisionFor(id) as CollisionState;
    // The job row's Cancel is reachable while paused; it must void the dialog.
    mgr.cancel(id);
    assert.equal(view.closed[0], id);
    assert.equal(mgr.collisionFor(id), null);
    assert.equal(mgr.answer(coll, "skip"), false);
    assert.equal(fx.answers.length, 0);
  });

  it("a paused job cannot be dismissed out from under the dialog", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    mgr.dismiss(id);
    assert.equal(mgr.liveJobs().length, 1);
    assert.ok(mgr.collisionFor(id));
    // …and it can be dismissed once the op has actually finished.
    mgr.answer(mgr.collisionFor(id) as CollisionState, "skip");
    fx.deliver(id, doneJson(id));
    mgr.dismiss(id);
    assert.equal(mgr.liveJobs().length, 0);
  });

  it("surfaces an answer failure instead of looking answered", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const failing: KestrelIpc = Object.assign(Object.create(Object.getPrototypeOf(fx.ipc) as object), fx.ipc, {
      opCollisionAnswer: (): Promise<void> =>
        Promise.reject(new Error("op_collision_answer is not registered")),
    }) as unknown as KestrelIpc;
    const mgr = new OpManager(failing, view);
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.handler(id)(normalizeOpEvent(JSON.parse(collisionJson(id, "/b/f", "file"))) as OpEventDto);
    mgr.answer(mgr.collisionFor(id) as CollisionState, "overwrite");
    await new Promise((r) => setTimeout(r, 0));
    // The dialog already closed (the decision was made), but the failure is
    // reported rather than swallowed: the engine is still blocked and the user
    // must know.
    assert.equal(view.errors.length, 1);
    assert.equal(view.errors[0].path, "/b/f");
    assert.match(view.errors[0].message, /still waiting/);
  });
});

/* ------------------------------------------------------------------ */
/* 3b. Re-sync after a mount that never saw the event                 */
/* ------------------------------------------------------------------ */

describe("normalizePendingCollisions", function () {
  it("keeps id, dst and kind for each row", function () {
    const rows = normalizePendingCollisions([
      { id: 7, dst: "/home/u/dst.txt", kind: "file" },
      { id: 8, dst: "/home/u/dst", kind: "dir" },
    ]);
    assert.deepEqual(rows, [
      { id: 7, dst: "/home/u/dst.txt", kind: "file" },
      { id: 8, dst: "/home/u/dst", kind: "dir" },
    ]);
  });

  it("drops a row with no usable dst rather than showing an unanswerable one", function () {
    // Same rule as the event normaliser, same reason: `answer` is keyed by
    // (id, dst), so a row we cannot name is a question with no answer key.
    const rows = normalizePendingCollisions([
      { id: 7, kind: "file" },
      { id: 7, dst: "", kind: "file" },
      { id: 7, dst: null, kind: "file" },
      { id: 8, dst: "/ok", kind: "dir" },
      null,
      "nonsense",
    ]);
    assert.deepEqual(rows, [{ id: 8, dst: "/ok", kind: "dir" }]);
  });

  it("canonicalises kinds and degrades unknown ones to other", function () {
    const rows = normalizePendingCollisions([
      { id: 1, dst: "/a", kind: "Dir" },
      { id: 2, dst: "/b", kind: "SYMLINK" },
      { id: 3, dst: "/c", kind: "fifo" },
      { id: 4, dst: "/d" },
    ]);
    assert.deepEqual(
      rows.map((r) => r.kind),
      ["dir", "symlink", "other", "other"]
    );
  });

  it("a non-array is empty, not a throw", function () {
    for (const bad of [null, undefined, 42, "x", {}]) {
      assert.deepEqual(normalizePendingCollisions(bad), []);
    }
  });
});

describe("ops.ts: adopting a collision discovered by asking", function () {
  it("pauses the job and opens the dialog for an adopted row", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, progressJson(id, 40, 400));
    // The event never arrived — this window mounted while the op was paused.
    assert.equal(jobOf(mgr, id).status, "running");
    assert.equal(mgr.collisionFor(id), null);

    const ok = mgr.adoptCollision(jobOf(mgr, id), {
      id,
      dst: "/b/f",
      kind: "dir",
      seq: 0,
    });
    assert.equal(ok, true);
    assert.equal(jobOf(mgr, id).status, "paused");
    assert.equal(view.opened.length, 1);
    // And the counters did not move: an adopted collision is still a pause.
    assert.equal(jobOf(mgr, id).doneBytes, 40);
  });

  it("an adopted collision answers through the same guards", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    mgr.adoptCollision(jobOf(mgr, id), { id, dst: "/b/f", kind: "file", seq: 0 });
    const coll = mgr.collisionFor(id) as CollisionState;
    assert.equal(mgr.answer({ ...coll, dst: "/b/WRONG" }, "overwrite"), false);
    assert.equal(mgr.answer(coll, "overwrite"), true);
    assert.deepEqual(fx.answers, [{ id, dst: "/b/f", decision: "overwrite" }]);
  });

  it("a streamed seq can never match an adopted collision", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/d", dst: "/b/d" });
    // Adopted (seq 0), then the real event arrives for the SAME path (seq 1).
    // The guard refuses to stack them, so the adopted answer object is stale.
    mgr.adoptCollision(jobOf(mgr, id), { id, dst: "/b/d/x", kind: "file", seq: 0 });
    const adopted = mgr.collisionFor(id) as CollisionState;
    fx.deliver(id, collisionJson(id, "/b/d/x", "file"));
    const streamed = mgr.collisionFor(id) as CollisionState;
    assert.notEqual(adopted.seq, streamed.seq);
    // Answering with the adopted one is refused even though dst matches —
    // which is the case a dst-only guard would wave through.
    assert.equal(mgr.answer(adopted, "overwrite"), false);
    assert.equal(fx.answers.length, 0);
    assert.equal(mgr.answer(streamed, "overwrite"), true);
  });

  it("refuses to adopt onto a job that is not live", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, doneJson(id));
    const done = jobOf(mgr, id);
    assert.equal(mgr.adoptCollision(done, { id, dst: "/b/f", kind: "file", seq: 0 }), false);
    assert.equal(mgr.collisionFor(id), null);
    assert.equal(jobOf(mgr, id).status, "done");
  });

  it("refuses to stack a second collision onto one already owed", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });
    fx.deliver(id, collisionJson(id, "/b/f", "file"));
    assert.equal(
      mgr.adoptCollision(jobOf(mgr, id), { id, dst: "/b/other", kind: "file", seq: 0 }),
      false
    );
    assert.equal((mgr.collisionFor(id) as CollisionState).dst, "/b/f");
  });

it("a re-sync while the same collision is still owed opens nothing new", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a/f", dst: "/b/f" });

    // First poll: the backend is blocked and nothing was ever streamed.
    fx.pending.push({ id, dst: "/b/f", kind: "file" });
    const first = await fx.ipc.opPendingCollisions();
    assert.equal(first.length, 1);
    assert.equal(mgr.adoptCollision(jobOf(mgr, id), { ...first[0], seq: 0 }), true);
    assert.equal(view.opened.length, 1);

    // Second poll, same dst. The op is genuinely still blocked, so the backend
    // correctly reports it again — and the UI must NOT stack a second copy of
    // the same question on top of the first. This is the same guard
    // `adoptCollision` applies when a streamed event follows an adopted row.
    fx.pending.push({ id, dst: "/b/f", kind: "file" });
    const second = await fx.ipc.opPendingCollisions();
    const known = mgr.collisionFor(id) as CollisionState;
    // This is precisely the condition `main.ts syncCollisions` checks.
    if (!known || known.dst !== second[0].dst)
      mgr.adoptCollision(jobOf(mgr, id), { ...second[0], seq: 0 });
    assert.equal(view.opened.length, 1);
    assert.equal((mgr.collisionFor(id) as CollisionState).dst, "/b/f");
  });

  it("a re-sync reporting a DIFFERENT dst opens a new question", async function () {
    const fx = fakeIpc();
    const view = viewStub();
    const mgr = new OpManager(fx.ipc, view);
    const id = await mgr.start({ op: "copy", src: "/a/d", dst: "/b/d" });
    fx.pending.push({ id, dst: "/b/d/one", kind: "file" });
    const first = await fx.ipc.opPendingCollisions();
    mgr.adoptCollision(jobOf(mgr, id), { ...first[0], seq: 0 });
    assert.equal(view.opened.length, 1);

    // The first is answered and the op has moved on to a second collision.
    mgr.answer(mgr.collisionFor(id) as CollisionState, "skip");
    fx.pending.push({ id, dst: "/b/d/two", kind: "dir" });
    const second = await fx.ipc.opPendingCollisions();
    assert.notEqual(second[0].dst, first[0].dst);
    assert.equal(mgr.adoptCollision(jobOf(mgr, id), { ...second[0], seq: 0 }), true);
    assert.equal(view.opened.length, 2);
    assert.equal((mgr.collisionFor(id) as CollisionState).dst, "/b/d/two");
  });
});

/* ------------------------------------------------------------------ */
/* 4. The copy: consequences, and abort vs overwrite                  */
/* ------------------------------------------------------------------ */

describe("collisionCopy: what each choice says", function () {
  function q(kind: string, op: "copy" | "move" = "copy"): CollisionQuestion {
    return {
      id: 7,
      src: "/home/achraf/notes/Report.pdf",
      dst: "/home/achraf/inbox/Report.pdf",
      kind: kind as CollisionQuestion["kind"],
      op,
    };
  }

  it("shows both paths and names the op and the paused state", function () {
    const c = collisionCopy(q("file"));
    assert.equal(c.src, "/home/achraf/notes/Report.pdf");
    assert.equal(c.dst, "/home/achraf/inbox/Report.pdf");
    assert.match(c.op, /^Copy/);
    assert.match(c.op, /paused/);
    assert.match(c.title, /Report\.pdf/);
    // The honesty statement the brief asks for: it is paused, and nothing is
    // moving while the window is open.
    assert.match(c.paused, /paused/);
    assert.match(c.paused, /Nothing is being written/);
  });

  it("says a move is a move, not a copy", function () {
    const c = collisionCopy(q("file", "move"));
    assert.match(c.op, /^Move/);
    assert.match(c.description, /Move/);
    const abort = c.options.find((o) => o.decision === "abort");
    assert.match((abort as { consequence: string }).consequence, /whole move stops/);
  });

  it("offers exactly three choices, in a stable order", function () {
    const c = collisionCopy(q("file"));
    assert.deepEqual(
      c.options.map((o) => o.decision),
      ["skip", "overwrite", "abort"]
    );
    assert.deepEqual(DECISION_ORDER, ["skip", "overwrite", "abort"]);
    assert.deepEqual(
      c.options.map((o) => o.tabIndex),
      [0, 1, 2]
    );
  });

  it("puts focus on skip, never on the destructive choice", function () {
    assert.equal(DEFAULT_FOCUS_DECISION, "skip");
    const c = collisionCopy(q("file"));
    const focused = c.options.find((o) => o.decision === DEFAULT_FOCUS_DECISION);
    assert.ok(focused);
    assert.notEqual((focused as { tone: string }).tone, "destructive");
  });

  it("abort and overwrite do not read alike: different tone, glyph, and words", function () {
    const c = collisionCopy(q("file"));
    const ow = c.options.find((o) => o.decision === "overwrite") as { tone: string; glyph: string; consequence: string };
    const ab = c.options.find((o) => o.decision === "abort") as { tone: string; glyph: string; consequence: string };
    // Not just a different colour: three separate channels, because colour is
    // never the only one (and WCAG non-text contrast aside, a red-on-red pair
    // would be indistinguishable to a red-green colourblind reader).
    assert.notEqual(ow.tone, ab.tone);
    assert.equal(ow.tone, "destructive");
    assert.equal(ab.tone, "caution");
    assert.notEqual(ow.glyph, ab.glyph);
    assert.notEqual(ow.consequence, ab.consequence);
    // Overwrite speaks about data; abort speaks about the operation stopping.
    assert.match(ow.consequence, /cannot be recovered/);
    assert.match(ab.consequence, /stops now/);
    assert.doesNotMatch(ab.consequence, /cannot be recovered/);
  });

  it("skip is described as leaving the destination exactly as it is", function () {
    const c = collisionCopy(q("file"));
    const sk = c.options.find((o) => o.decision === "skip") as { consequence: string; tone: string };
    assert.match(sk.consequence, /keeps exactly what it has now/);
    assert.equal(sk.tone, "neutral");
  });

  it("abort warns about a part-written file rather than implying it is clean", function () {
    const c = collisionCopy(q("file"));
    const ab = c.options.find((o) => o.decision === "abort") as { consequence: string };
    assert.match(ab.consequence, /part-written/);
    assert.match(ab.consequence, /check it/);
  });

  it("overwrite on a FOLDER says it merges, because the engine merges there", function () {
    // The one that is easiest to get wrong: in `copy_recursive`, create_dir
    // swallows AlreadyExists and the children are then recursed into, so a
    // folder is NOT deleted and replaced. Saying "replaced" would be a lie.
    const c = collisionCopy(q("dir"));
    const ow = c.options.find((o) => o.decision === "overwrite") as { consequence: string };
    assert.match(ow.consequence, /writes into the folder that is already there/);
    assert.match(ow.consequence, /not part of this copy are kept/);
    assert.doesNotMatch(ow.consequence, /deleted/);
  });

  it("skip on a FOLDER says the whole subtree is left alone", function () {
    // `Skip` on a directory returns before its children are walked, so the
    // promise is much larger than skipping one file — and must say so.
    const c = collisionCopy(q("dir"));
    const sk = c.options.find((o) => o.decision === "skip") as { consequence: string };
    assert.match(sk.consequence, /contents included/);
    assert.match(sk.consequence, /Nothing inside it/);
  });

  it("overwrite on a SYMLINK says the link is replaced and the target is not touched", function () {
    const c = collisionCopy(q("symlink"));
    const ow = c.options.find((o) => o.decision === "overwrite") as { consequence: string };
    assert.match(ow.consequence, /link that is there is replaced/);
    assert.match(ow.consequence, /left alone/);
  });

  it("names what is already at the destination, by kind", function () {
    assert.match(collisionCopy(q("file")).existing, /There is already a file/);
    assert.match(collisionCopy(q("dir")).existing, /There is already a folder/);
    assert.match(collisionCopy(q("symlink")).existing, /There is already a symlink/);
    assert.match(collisionCopy(q("other")).existing, /There is already an entry/);
  });

  it("offers no apply-to-all, and says why in as many words", function () {
    const c = collisionCopy(q("file"));
    // The engine keys answers by destination path; a blanket policy would
    // answer every remaining path at once. So there must be no such control…
    const labels = c.options.map((o) => o.label.toLowerCase()).join(" ");
    assert.doesNotMatch(labels, /all|every|remaining|batch/);
    // …and the absence is stated positively, so it reads as a promise.
    assert.match(c.sequential, /this one destination only/);
    assert.match(c.sequential, /ask again/);
    assert.match(c.sequential, /no setting that answers them all/);
  });

  it("every choice carries a consequence sentence, not a bare verb", function () {
    for (const kind of ["file", "dir", "symlink", "other"]) {
      for (const o of collisionCopy(q(kind)).options) {
        assert.ok(
          o.consequence.length > 40,
          kind + "/" + o.decision + " consequence is too thin: " + o.consequence
        );
        assert.ok(o.label.length > 0);
        assert.ok(o.glyph.length > 0);
      }
    }
  });

  it("the three consequences are all different from one another", function () {
    for (const kind of ["file", "dir", "symlink", "other"]) {
      const texts = collisionCopy(q(kind)).options.map((o) => o.consequence);
      assert.equal(new Set(texts).size, 3, kind + ": two choices read identically");
    }
  });
});

/* ------------------------------------------------------------------ */
/* 5. The keyboard contract                                           */
/* ------------------------------------------------------------------ */

describe("planCollisionKey: the keyboard model", function () {
  const N = DECISION_ORDER.length;

  it("Esc is consumed and changes nothing", function () {
    // The dangerous default of the whole feature: Esc means "cancel"
    // everywhere else, and spending it here would stop a running operation
    // and leave a part-written file behind.
    for (const focused of [0, 1, 2]) {
      const p = planCollisionKey("Escape", false, focused, N);
      assert.equal(p.consume, true);
      assert.equal(p.focusIndex, focused, "Esc must not move focus");
      assert.equal(p.activate, false, "Esc must not answer anything");
    }
    // The legacy spelling, for renderers that report it.
    assert.equal(planCollisionKey("Esc", false, 0, N).consume, true);
  });

  it("Tab cycles the three choices and wraps at both ends", function () {
    assert.equal(planCollisionKey("Tab", false, 0, N).focusIndex, 1);
    assert.equal(planCollisionKey("Tab", false, 1, N).focusIndex, 2);
    // Wrapping forward off the last: a modal must not be escapable by keyboard.
    assert.equal(planCollisionKey("Tab", false, 2, N).focusIndex, 0);
    assert.equal(planCollisionKey("Tab", true, 0, N).focusIndex, 2);
    assert.equal(planCollisionKey("Tab", true, 2, N).focusIndex, 1);
    assert.equal(planCollisionKey("Tab", true, 1, N).focusIndex, 0);
  });

  it("Tab always consumes, so focus cannot escape to the list behind", function () {
    for (let i = 0; i < N; i++) {
      assert.equal(planCollisionKey("Tab", false, i, N).consume, true);
      assert.equal(planCollisionKey("Tab", true, i, N).consume, true);
    }
  });

  it("Enter and Space activate but are not consumed, so the button fires once", function () {
    for (const key of ["Enter", " "]) {
      const p = planCollisionKey(key, false, 1, N);
      assert.equal(p.activate, true);
      assert.equal(p.consume, false);
      assert.equal(p.focusIndex, 1);
    }
  });

  it("arrow keys are left alone: Tab is the model, arrows do nothing special", function () {
    for (const key of ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"]) {
      const p = planCollisionKey(key, false, 1, N);
      assert.equal(p.consume, false);
      assert.equal(p.activate, false);
      assert.equal(p.focusIndex, 1);
    }
  });

  it("survives a degenerate option count without throwing or looping", function () {
    // The wrap maths has a modulo in it; a count of 0 or 1 must not divide by
    // zero or index outside the array.
    assert.equal(planCollisionKey("Tab", false, 0, 0).focusIndex, 0);
    assert.equal(planCollisionKey("Tab", false, 0, 1).focusIndex, 0);
    assert.equal(planCollisionKey("Tab", true, 0, 1).focusIndex, 0);
    assert.equal(planCollisionKey("Tab", false, -1, N).focusIndex, 1);
  });

  it("every focus index stays inside the option list", function () {
    for (let i = 0; i < N; i++) {
      for (const shift of [false, true]) {
        for (const key of ["Tab", "Enter", " ", "Escape", "a"]) {
          const p = planCollisionKey(key, shift, i, N);
          assert.ok(p.focusIndex >= 0 && p.focusIndex < N, key + " escaped");
        }
      }
    }
  });
});

/* ------------------------------------------------------------------ */
/* 6. Building the question from a job                                */
/* ------------------------------------------------------------------ */

describe("questionFrom: op job → dialog question", function () {
  it("takes src and dst from the request and kind from the collision", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({
      op: "copy",
      src: "/home/a/notes/x.pdf",
      dst: "/home/a/inbox/x.pdf",
    });
    fx.deliver(id, collisionJson(id, "/home/a/inbox/x.pdf", "symlink"));
    const job = jobOf(mgr, id);
    const coll = job.collision as CollisionState;
    const question = questionFrom(job, coll);
    assert.equal(question.src, "/home/a/notes/x.pdf");
    assert.equal(question.dst, "/home/a/inbox/x.pdf");
    assert.equal(question.kind, "symlink");
    assert.equal(question.op, "copy");
    assert.equal(question.id, id);
  });

  it("a move question reads as a move", async function () {
    const fx = fakeIpc();
    const mgr = new OpManager(fx.ipc, viewStub());
    const id = await mgr.start({ op: "move", src: "/a/x", dst: "/b/x" });
    fx.deliver(id, collisionJson(id, "/b/x", "file"));
    const job = jobOf(mgr, id);
    const question = questionFrom(job, job.collision as CollisionState);
    assert.equal(question.op, "move");
    assert.match(collisionCopy(question).op, /^Move/);
  });
});