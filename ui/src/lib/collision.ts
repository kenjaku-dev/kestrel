/* Kestrel Phase 3b — the collision dialog.
 *
 * A blocking, human-in-the-loop decision made in the middle of a running file
 * operation. The engine has stopped on ONE destination and is waiting; the
 * window behind is still live. Three things have to be true at once here:
 *
 *   1. The user is told what each choice DOES to the file that is already
 *      there — not just what the button is called. The consequence sentences
 *      are written per destination kind because they genuinely differ: in the
 *      engine, `Overwrite` on a folder MERGES (`create_dir` swallows
 *      AlreadyExists, then the children recurse — kestrel-fs/src/ops.rs,
 *      `copy_recursive`), it does not delete the folder and its contents.
 *      Saying "it will be replaced" for a folder would be a lie.
 *   2. `abort` and `overwrite` must not read alike. They are different in
 *      kind, not degree: one destroys data, one stops work. Overwrite is the
 *      only filled danger control; abort is an outlined caution control,
 *      isolated from it by a divider, and it carries its own glyph and its
 *      own sentence.
 *   3. Nothing may claim the operation is progressing. It is not: the engine
 *      thread is blocked inside the collision handshake. The job row and the
 *      dialog both say "paused".
 *
 * Structure, deliberately split so most of it is testable without a DOM (this
 * project runs `node --test` with no jsdom, and a claim about behaviour that
 * can only be checked by looking is not evidence):
 *   - `collisionCopy()`   pure. The words, the tab order, the tones.
 *   - `planCollisionKey()` pure. The keyboard contract, including the one
 *     non-obvious rule: Esc is consumed and does nothing.
 *   - `CollisionDialog`   the DOM, and only the DOM.
 *
 * Why there is no "apply to all", stated once here because it is the load-
 * bearing decision in this file: the engine keys `AnsweredCollisions` by
 * destination path precisely so an answer CAN be scoped. A blanket policy
 * would answer every remaining colliding path in the tree at once, silently —
 * `kestrel-fs/src/ops.rs` documents exactly that failure. The dialog says so
 * in as many words, so the absence reads as a promise about what happens next
 * rather than as a missing feature.
 *
 * Safari 13 / WebKitGTK floor: no `<dialog>` (15.4), no `inert` (15.5), no
 * `:has()`, no flex `gap`, no `?.`. Focus is managed by hand and the trap is
 * explicit.
 */

/** The frozen decision vocabulary. Nothing is added, nothing is reordered in
 *  a way that hides a choice. */
import type {
  CollisionDecision,
  CollisionKindDto,
  JobId,
} from "./types";
import type { CollisionState, OpJob } from "./ops";
import type { KestrelIpc } from "./ipc";

/* ------------------------------------------------------------------ */
/* Pure part 1: what the dialog says                                  */
/* ------------------------------------------------------------------ */

/** How a control reads. Three registers, deliberately distinct:
 *  - `neutral`     keeps what is there
 *  - `caution`     stops the operation, may leave a part-written file
 *  - `destructive` replaces data
 *  Colour is never the only channel: every tone also has its own glyph and
 *  its own sentence. */
export type DecisionTone = "neutral" | "caution" | "destructive";

export interface CollisionOptionCopy {
  decision: CollisionDecision;
  /** Position in the tab order, which is also the visual order. */
  tabIndex: number;
  /** Decorative; rendered `aria-hidden`. Pairs with `tone`, never replaces it. */
  glyph: string;
  label: string;
  consequence: string;
  tone: DecisionTone;
}

export interface CollisionCopy {
  /** Small kicker above the title: the op and the fact that it stopped. */
  op: string;
  title: string;
  /** "from / to" path rows. */
  srcLabel: string;
  src: string;
  dstLabel: string;
  dst: string;
  /** What the destination currently holds. */
  existingGlyph: string;
  existing: string;
  /** The full prose block, referenced by aria-describedby. */
  description: string;
  /** The honesty statement: this is paused, and nothing is moving. */
  paused: string;
  /** Why there is no "apply to all". True, and stated positively. */
  sequential: string;
  /** Keyboard model, including what Esc does. */
  hint: string;
  options: CollisionOptionCopy[];
}

/**
 * Decision order. Skip first and focused; the two answers to "what about this
 * file" sit together; the one that stops everything is isolated on the right.
 * The destructive control is never the default focus, which is also §4.7's
 * rule ("the `default` action is never the destructive one for Enter").
 */
export const DECISION_ORDER: CollisionDecision[] = ["skip", "overwrite", "abort"];

/** Focus lands here on open: the choice with the least consequence, and the
 *  first one in the tab order. Exported so a test can pin it. */
export const DEFAULT_FOCUS_DECISION: CollisionDecision = "skip";

export function indexOfDecision(decision: CollisionDecision): number {
  for (let i = 0; i < DECISION_ORDER.length; i++)
    if (DECISION_ORDER[i] === decision) return i;
  return 0;
}

/** Single word for what is sitting at the destination. */
export function collisionKindNoun(kind: CollisionKindDto): string {
  if (kind === "dir") return "folder";
  if (kind === "symlink") return "symlink";
  if (kind === "file") return "file";
  return "entry";
}

/** The kind glyph, matching `format.ts kindGlyph`'s single-letter language
 *  rather than inventing an icon set: a folder is D, a link is L, anything
 *  unclassifiable is ?. */
export function collisionKindGlyph(kind: CollisionKindDto): string {
  if (kind === "dir") return "D";
  if (kind === "symlink") return "L";
  if (kind === "file") return "F";
  return "?";
}

/**
 * What `skip` leaves alone. Read off the engine: `Skip` on a directory
 * returns before the children are walked, so it means the whole subtree is
 * untouched — which is a much bigger promise than skipping one file, and the
 * user is entitled to know it.
 */
function skipConsequence(kind: CollisionKindDto, op: "copy" | "move"): string {
  const verb = op === "copy" ? "copied" : "moved";
  if (kind === "dir")
    return (
      "The folder that is there is left exactly as it is, contents included. " +
      "Nothing inside it is " +
      verb +
      ", and the rest of the operation carries on."
    );
  return (
    "The destination keeps exactly what it has now. This one is not " +
    verb +
    ", and the rest of the operation carries on."
  );
}

/**
 * What `overwrite` does to what is already there.
 *
 * These four sentences are deliberately not interchangeable. In the engine:
 *  - a file is truncated and rewritten (`copy_file_contents`);
 *  - a symlink is replaced by rename, and its TARGET is never followed or
 *    touched (`copy_symlink` links the link, then renames over the name);
 *  - a folder MERGES — `create_dir` swallows `AlreadyExists` and the children
 *    are then recursed into, so files not present in the source survive and
 *    only same-named files are replaced;
 *  - anything else (socket, fifo, device) is simply overwritten.
 * "It will be replaced" would be wrong for two of those four.
 */
function overwriteConsequence(
  kind: CollisionKindDto,
  op: "copy" | "move"
): string {
  const verb = op === "copy" ? "copied" : "moved";
  // "part of this copy", not "part of this " + verb: the participle reads as
  // "not part of this copied", which is ungrammatical and, worse, unclear.
  const noun = op === "copy" ? "copy" : "move";
  if (kind === "dir")
    return (
      "Kestrel writes into the folder that is already there. Files inside " +
      "it that are not part of this " +
      noun +
      " are kept; any file with the same name is replaced."
    );
  // Same fix in the other three: the object being replaced takes a
  // participle, the operation takes a noun.
  if (kind === "symlink")
    return (
      "The link that is there is replaced by the one being " +
      verb +
      ". Whatever it pointed at is left alone."
    );
  if (kind === "other")
    return (
      "The entry that is there is replaced by the one being " +
      verb +
      ". What it was is gone."
    );
  return (
    "The file that is there now is deleted and the one being " +
    verb +
    " takes its place. What is in it now cannot be recovered."
  );
}

/** What `abort` stops, and what it leaves behind. Identical for copy and
 *  move except for the past tense; the part-written-file warning is the
 *  engine's own documented behaviour on cancel (`CopyOptions.cancel`: "leaving
 *  a partial destination behind, which the caller is expected to clean up or
 *  report"), so it is stated rather than softened. */
function abortConsequence(op: "copy" | "move"): string {
  const done = op === "copy" ? "copied" : "moved";
  const verb = op === "copy" ? "copy" : "move";
  return (
    "The whole " +
    verb +
    " stops now, not just this one. Anything already " +
    done +
    " stays where it is, and a file part-written at the destination may be " +
    "left behind — check it before you rely on it."
  );
}

/** Build every word of the dialog. Pure: same question in, same dialog out. */
export function collisionCopy(q: CollisionQuestion): CollisionCopy {
  const noun = collisionKindNoun(q.kind);
  const verb = q.op === "copy" ? "Copy" : "Move";
  const base = q.dst.split("/").pop() || q.dst;
  const existing =
    "There is already " +
    (q.kind === "other" ? "an entry" : "a " + noun) +
    " at that name.";

  return {
    op: verb + " · paused",
    title: base + " already exists",
    srcLabel: "from",
    src: q.src,
    dstLabel: "to",
    dst: q.dst,
    existingGlyph: collisionKindGlyph(q.kind),
    existing: existing,
    description:
      verb +
      " stopped here because the destination is taken. The file being " +
      (q.op === "copy" ? "copied" : "moved") +
      " has not replaced anything yet, and nothing will until you choose.",
    paused:
      "The operation is paused while this window is open. Nothing is being " +
      "written to the destination, and it picks up again as soon as you " +
      "choose.",
    sequential:
      "Your choice applies to this one destination only. If another name " +
      "collides later, Kestrel will ask again — there is no setting that " +
      "answers them all at once.",
    hint:
      "Tab moves between the choices and Enter picks the focused one. " +
      "Esc does nothing here, on purpose: it would silently stop the whole " +
      "operation.",
    options: [
      {
        decision: "skip",
        tabIndex: 0,
        glyph: "→",
        label: q.op === "copy" ? "Skip this file" : "Skip this file",
        consequence: skipConsequence(q.kind, q.op),
        tone: "neutral",
      },
      {
        decision: "overwrite",
        tabIndex: 1,
        glyph: "↔",
        label: "Overwrite",
        consequence: overwriteConsequence(q.kind, q.op),
        tone: "destructive",
      },
      {
        decision: "abort",
        tabIndex: 2,
        glyph: "■",
        label: q.op === "copy" ? "Stop the copy" : "Stop the move",
        consequence: abortConsequence(q.op),
        tone: "caution",
      },
    ],
  };
}

/** Everything the copy needs. Flat, so it can be built from an OpJob in one
 *  line and compared in a test without constructing a manager. */
export interface CollisionQuestion {
  id: JobId;
  src: string;
  dst: string;
  kind: CollisionKindDto;
  op: "copy" | "move";
}

export function questionFrom(job: OpJob, coll: CollisionState): CollisionQuestion {
  const req = job.req;
  // Trash and delete never reach a destination, so this is unreachable in
  // practice; fall back to the src rather than inventing a path.
  const src = "src" in req ? req.src : "";
  const dst = "dst" in req && typeof req.dst === "string" ? req.dst : coll.dst;
  return {
    id: job.id,
    src,
    dst,
    kind: coll.kind,
    op: req.op === "move" ? "move" : "copy",
  };
}

/* ------------------------------------------------------------------ */
/* Pure part 2: the keyboard contract                                 */
/* ------------------------------------------------------------------ */

export interface KeyPlan {
  /** Stop the key here (preventDefault) and do nothing else with it. */
  consume: boolean;
  /** Where focus should sit afterwards. Unchanged means "leave it alone". */
  focusIndex: number;
  /** True when the key should activate the focused control. Never set for a
   *  key the dialog consumes, because `<button>` handles Enter/Space itself
   *  and double-firing would send two answers for one press. */
  activate: boolean;
}

/**
 * The whole keyboard model, in one pure function, so it can be tested.
 *
 * Tab / Shift+Tab cycle the three options and WRAP — a trap. Without the
 * wrap, Shift+Tab off the first option would drop focus onto the file list
 * behind the dialog, which is the one thing a blocking dialog must not allow.
 *
 * Esc is consumed and changes nothing. This is the deliberate non-obvious
 * rule of the whole phase: Esc has become "cancel" in every other dialog a
 * user has ever met, so here it would stop a running operation and leave a
 * part-written file behind. Spending a reflex keystroke on that is not a
 * decision. The dialog says so on screen instead of leaving Esc as a mystery.
 */
export function planCollisionKey(
  key: string,
  shift: boolean,
  focused: number,
  count: number
): KeyPlan {
  const here = focused < 0 ? 0 : focused;
  if (key === "Tab") {
    if (count <= 1) return { consume: true, focusIndex: here, activate: false };
    const step = shift ? -1 : 1;
    // Wrap: a modal dialog must not be escapable with the keyboard alone.
    let next = here + step;
    if (next < 0) next = count - 1;
    if (next > count - 1) next = 0;
    return { consume: true, focusIndex: next, activate: false };
  }
  if (key === "Escape" || key === "Esc")
    return { consume: true, focusIndex: here, activate: false };
  if (key === "Enter" || key === " ")
    // Not consumed: the native <button> turns these into a click. Reporting
    // `activate` lets the caller assert the mapping without doubling it.
    return { consume: false, focusIndex: here, activate: true };
  return { consume: false, focusIndex: here, activate: false };
}

/* ------------------------------------------------------------------ */
/* The DOM                                                             */
/* ------------------------------------------------------------------ */

interface QueueEntry {
  coll: CollisionState;
  question: CollisionQuestion;
  copy: CollisionCopy;
}

function noop(): void {
  /* best-effort promise sink */
}

function text(tag: string, cls: string, value: string): HTMLElement {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  n.textContent = value;
  return n;
}

/**
 * One modal dialog over the whole window, plus a queue.
 *
 * Several ops can be in flight and each can be blocked, so two collisions
 * can be owed at once. A modal can only show one, and dropping the other
 * would leave an engine blocked forever with nobody to answer it. So the
 * dialog queues: one question at a time, which is also the honest picture of
 * a sequential decision process.
 */
export class CollisionDialog {
  private layer: HTMLDivElement;
  private scrim: HTMLDivElement;
  private card: HTMLDivElement;
  private buttons: HTMLButtonElement[] = [];
  private queue: QueueEntry[] = [];
  private open: QueueEntry | null = null;
  private focusIndex = 0;
  /** Where focus came from, so it can go back. */
  private restoreTo: HTMLElement | null = null;

  constructor(
    host: HTMLElement,
    private app: HTMLElement,
    private mgr: {
      collisionFor(id: JobId): CollisionState | null;
      answer(coll: CollisionState, decision: CollisionDecision): boolean;
      liveJobs(): OpJob[];
    },
    private ipc: KestrelIpc,
    private returnFocus: () => HTMLElement | null
  ) {
    this.layer = document.createElement("div");
    this.layer.className = "k-collision-layer";
    this.layer.setAttribute("hidden", "");
    // Not decorative and not interactive, so it is not in the a11y tree as a
    // landmark. It is named so a screen-reader user landing in the middle of
    // the window can find their way back out to the app once it closes.
    this.layer.setAttribute("role", "presentation");
    this.layer.setAttribute("data-backend", ipc.backend);

    this.scrim = document.createElement("div");
    this.scrim.className = "k-collision-scrim";
    // The scrim swallows the click rather than dismissing. Light-dismiss
    // would be "close without answering", which is the abort-by-accident
    // this phase exists to avoid; an unanswered collision leaves the op
    // blocked, so the dialog must stay until one of the three is chosen.
    this.scrim.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
    });
    this.layer.appendChild(this.scrim);

    this.card = document.createElement("div");
    this.card.className = "k-collision";
    this.card.id = "collision-dialog";
    this.card.setAttribute("role", "dialog");
    this.card.setAttribute("aria-modal", "true");
    this.card.setAttribute("aria-labelledby", "collision-title");
    // The description target holds prose only — no interactive elements
    // inside it, so a screen reader reads the situation, not a menu.
    this.card.setAttribute("aria-describedby", "collision-desc");
    this.card.setAttribute("tabindex", "-1");
    this.card.addEventListener("keydown", (e) => this.onKey(e));
    this.layer.appendChild(this.card);

    host.appendChild(this.layer);
  }

  /** True while a question is on screen. Used by main.ts to keep the file
   *  list's op shortcuts from firing behind the dialog. */
  get isOpen(): boolean {
    return this.open !== null;
  }

  /** A collision arrived: remember it and show it if nothing is up. */
  enqueue(job: OpJob, coll: CollisionState): void {
    const question = questionFrom(job, coll);
    this.queue.push({
      coll,
      question,
      copy: collisionCopy(question),
    });
    if (!this.open) this.showNext();
  }

  /** This job no longer owes an answer. Drop it wherever it is in the queue. */
  release(id: JobId): void {
    const kept: QueueEntry[] = [];
    for (let i = 0; i < this.queue.length; i++)
      if (this.queue[i].coll.id !== id) kept.push(this.queue[i]);
    this.queue = kept;
    if (this.open && this.open.coll.id === id) {
      this.hide();
      this.showNext();
    }
  }

  /** Show the first queued question that is still genuinely owed. Entries can
   *  be voided without a release (an answer routed elsewhere, a job settled),
   *  so each candidate is re-checked against the manager rather than trusted. */
  private showNext(): void {
    while (this.queue.length > 0) {
      const entry = this.queue[0];
      const cur = this.mgr.collisionFor(entry.coll.id);
      if (cur && cur.seq === entry.coll.seq && cur.dst === entry.coll.dst) {
        this.paint(entry);
        return;
      }
      this.queue.shift();
    }
  }

  private paint(entry: QueueEntry): void {
    this.open = entry;
    const c = entry.copy;
    const card = this.card;
    while (card.firstChild) card.removeChild(card.firstChild);
    this.buttons = [];

    card.appendChild(text("p", "k-collision-op", c.op));

    // `<h1>`, not `<h2>`: the app frame has no headings of its own, so an `h2`
    // here would be a skipped level with nothing above it. While the dialog is
    // open it is a top-level context in its own right, and its title is the
    // first heading a screen reader reaches. One `h1`, inside a dialog that
    // replaces the app in the a11y tree — not a second one alongside the app.
    const title = document.createElement("h1");
    title.className = "k-collision-title";
    title.id = "collision-title";
    title.textContent = c.title;
    card.appendChild(title);

    // The two paths. `code` because they are paths, and mono because a path
    // read in a proportional face is easy to misread.
    const paths = document.createElement("div");
    paths.className = "k-collision-paths";
    paths.appendChild(text("span", "k-collision-path-label", c.srcLabel));
    const src = text("code", "k-collision-path", c.src);
    paths.appendChild(src);
    paths.appendChild(text("span", "k-collision-path-label", c.dstLabel));
    const dst = text("code", "k-collision-path k-collision-path-dst", c.dst);
    dst.title = c.dst;
    paths.appendChild(dst);
    card.appendChild(paths);

    const existing = document.createElement("p");
    existing.className = "k-collision-existing";
    const g = text("span", "k-collision-glyph", c.existingGlyph);
    g.setAttribute("aria-hidden", "true"); // decorative: the sentence says it
    existing.appendChild(g);
    existing.appendChild(text("span", "", c.existing));
    card.appendChild(existing);

    // One prose target for aria-describedby: situation, then the honesty
    // statement about being paused. `status` because the op really did change
    // state and a screen reader user is owed that unprompted.
    const desc = document.createElement("p");
    desc.className = "k-collision-desc";
    desc.id = "collision-desc";
    desc.setAttribute("role", "status");
    desc.textContent = c.description + " " + c.paused;
    card.appendChild(desc);

    const list = document.createElement("ul");
    list.className = "k-collision-options";
    for (let i = 0; i < c.options.length; i++) {
      const opt = c.options[i];
      const li = document.createElement("li");
      li.className = "k-collision-option-slot is-tone-" + opt.tone;
      if (opt.decision === "abort") li.className += " is-separate";
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "k-collision-option";
      btn.setAttribute("data-decision", opt.decision);
      const glyph = text("span", "k-collision-option-glyph", opt.glyph);
      glyph.setAttribute("aria-hidden", "true");
      const body = document.createElement("span");
      body.className = "k-collision-option-body";
      body.appendChild(text("span", "k-collision-option-label", opt.label));
      body.appendChild(
        text("span", "k-collision-option-detail", opt.consequence)
      );
      btn.appendChild(glyph);
      btn.appendChild(body);
      const self = this;
      btn.addEventListener("click", () => self.choose(opt.decision));
      li.appendChild(btn);
      list.appendChild(li);
      this.buttons.push(btn);
    }
    card.appendChild(list);

    const seq = document.createElement("p");
    seq.className = "k-collision-seq";
    seq.textContent = c.sequential;
    card.appendChild(seq);

    card.appendChild(text("p", "k-collision-hint", c.hint));

    // Focus first, THEN hide the app from assistive tech. Doing it the other
    // way round would put `aria-hidden` on the subtree that currently owns
    // focus, which is an ARIA violation and makes the dialog unreachable.
    this.layer.removeAttribute("hidden");
    this.restoreTo = this.returnFocus();
    this.focusIndex = indexOfDecision(DEFAULT_FOCUS_DECISION);
    this.focusOption();
    this.app.setAttribute("aria-hidden", "true");
  }

  private hide(): void {
    this.open = null;
    this.buttons = [];
    this.layer.setAttribute("hidden", "");
    this.app.removeAttribute("aria-hidden");
    const back = this.restoreTo;
    this.restoreTo = null;
    if (back && back.focus) back.focus();
  }

  private focusOption(): void {
    const b = this.buttons[this.focusIndex];
    if (b && b.focus) b.focus();
  }

  private onKey(e: KeyboardEvent): void {
    const plan = planCollisionKey(
      e.key,
      e.shiftKey,
      this.focusIndex,
      this.buttons.length
    );
    if (plan.consume) {
      e.preventDefault();
      // Enter/Space are never consumed, so the native button's click still
      // fires. Anything else that lands here does nothing — that is Esc.
      if (plan.focusIndex !== this.focusIndex) {
        this.focusIndex = plan.focusIndex;
        this.focusOption();
      }
    }
  }

  /**
   * Answer the question on screen.
   *
   * The dialog closes on SUCCESS only. `answer` refuses a stale, doubled or
   * mistargeted answer, and if it refuses, this dialog is still the one the
   * user is looking at — closing it would leave a blocked operation with no
   * way to answer it. The refusal path therefore leaves the dialog up and
   * re-reads what is actually owed.
   */
  private choose(decision: CollisionDecision): void {
    const entry = this.open;
    if (!entry) return;
    if (this.mgr.answer(entry.coll, decision)) {
      this.release(entry.coll.id);
      return;
    }
    // Refused. Re-sync with the manager and, if it now owes nothing, close.
    const cur = this.mgr.collisionFor(entry.coll.id);
    if (!cur || cur.seq !== entry.coll.seq || cur.dst !== entry.coll.dst) {
      this.release(entry.coll.id);
    }
  }

  /** Every answer this dialog has sent. Dev-only introspection, mirroring
   *  the mock's own log so a run in the real binary can be checked. */
  static answersOf(ipc: KestrelIpc): Array<{
    id: JobId;
    dst: string;
    decision: CollisionDecision;
  }> {
    const m = ipc as unknown as Record<string, unknown>;
    const fn = m["collisionAnswers"];
    if (typeof fn !== "function") return [];
    try {
      return (fn as () => Array<{ id: JobId; dst: string; decision: CollisionDecision }>).call(
        ipc
      );
    } catch {
      return [];
    }
  }

  /** Close without answering (window teardown). Not a user action: an open
   *  dialog is never dismissed without a decision by design. */
  dispose(): void {
    if (this.open) this.hide();
    if (this.layer.parentNode) this.layer.parentNode.removeChild(this.layer);
  }
}

export { noop as collisionNoop };