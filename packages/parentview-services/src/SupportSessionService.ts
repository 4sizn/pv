import { BehaviorSubject, type Observable, Subject } from "rxjs";
import { ParentViewPolicy } from "./ParentViewPolicy";
import type {
  MediaSessionPort,
  RemoteInput,
  RemoteInputPort,
  SessionContext,
  SupportSnapshot,
} from "./types";

const INITIAL: SupportSnapshot = Object.freeze({
  approval: "idle",
  connection: "idle",
  screenConsent: "not-granted",
  controllerPeerId: null,
});

/**
 * Owns one product session's consent and input grant, never media negotiation.
 * Dependencies are narrow ports. Injected media client lifetime belongs to its composition root.
 * end() revokes authority before async cleanup; destroy() also completes owned streams.
 */
export class SupportSessionService {
  readonly state$: Observable<SupportSnapshot>;
  readonly errors$: Observable<Error>;
  #state = new BehaviorSubject<SupportSnapshot>(INITIAL);
  #errors = new Subject<Error>();
  #media: MediaSessionPort;
  #input: RemoteInputPort;
  #policy: ParentViewPolicy;
  #context: SessionContext | null = null;
  #generation = 0;
  #inputGeneration = 0;
  #destroyed = false;
  #cleanup: Promise<void> | null = null;
  #destruction: Promise<void> | null = null;
  #inputQueue: Promise<void> = Promise.resolve();

  constructor(options: {
    media: MediaSessionPort;
    input: RemoteInputPort;
    policy?: ParentViewPolicy;
  }) {
    this.#media = options.media;
    this.#input = options.input;
    this.#policy = options.policy ?? new ParentViewPolicy();
    this.state$ = this.#state.asObservable();
    this.errors$ = this.#errors.asObservable();
  }

  get snapshot(): SupportSnapshot {
    return this.#state.value;
  }

  request(context: SessionContext): void {
    this.#assertAlive();
    if (this.#context || this.#cleanup) throw new Error("A support session is already open");
    const ids = context.participants.map((participant) => participant.peerId);
    if (ids.length < 2 || ids.length > 4 || new Set(ids).size !== ids.length) {
      throw new Error("A session needs two to four distinct authenticated participants");
    }
    if (
      !context.participants.some(
        (p) => p.peerId === context.local.peerId && p.role === context.local.role,
      )
    ) {
      throw new Error("Local identity must match authenticated membership");
    }
    // Snapshot caller-owned data so later mutation cannot change an existing authorization grant.
    this.#context = {
      local: { ...context.local },
      participants: context.participants.map((p) => ({ ...p })),
      connection: { ...context.connection },
    };
    this.#generation++;
    this.#state.next(Object.freeze({ ...INITIAL, approval: "pending" }));
  }

  async approve(): Promise<void> {
    this.#assertAlive();
    const context = this.#context;
    if (!context || this.snapshot.approval !== "pending") throw new Error("No pending request");
    const generation = this.#generation;
    this.#update({ approval: "approved", connection: "joining" });
    // Observable subscribers may synchronously end/destroy during the state notification.
    if (generation !== this.#generation || this.#destroyed) return;
    try {
      await this.#media.join(context.connection);
      if (generation !== this.#generation || this.#destroyed) return;
      this.#update({ connection: "active" });
    } catch (cause) {
      if (generation !== this.#generation || this.#destroyed) return;
      const error = cause instanceof Error ? cause : new Error(String(cause));
      this.#errors.next(error);
      await this.end();
      throw error;
    }
  }

  async reject(): Promise<void> {
    this.#assertAlive();
    if (this.snapshot.approval !== "pending") throw new Error("No pending request");
    this.#update({ approval: "rejected" });
    await this.end();
  }

  /** Called only after local user consent AND a successful OS capture permission result. */
  confirmScreenConsent(): void {
    this.#assertActive();
    this.#update({ screenConsent: "granted" });
  }

  async revokeScreenConsent(): Promise<void> {
    this.#assertAlive();
    this.#inputGeneration++;
    this.#update({ screenConsent: "revoked", controllerPeerId: null });
    await this.#releaseInput();
  }

  grantControl(peerId: string): void {
    this.#assertActive();
    const context = this.#context;
    const controller = context?.participants.find((participant) => participant.peerId === peerId);
    if (!context || !controller || !this.#policy.canControl(controller, context.local)) {
      throw new Error("This participant cannot control the local device");
    }
    if (this.snapshot.screenConsent !== "granted")
      throw new Error("Screen consent is required first");
    if (!this.#input.isAvailable()) throw new Error("Native remote input is unavailable");
    if (this.snapshot.controllerPeerId && this.snapshot.controllerPeerId !== peerId) {
      throw new Error("Revoke the existing control grant before granting another");
    }
    this.#inputGeneration++;
    this.#update({ controllerPeerId: peerId });
  }

  async revokeControl(): Promise<void> {
    this.#assertAlive();
    this.#inputGeneration++;
    this.#update({ controllerPeerId: null });
    await this.#releaseInput();
  }

  /** peerId must come from the authenticated media transport, never from a payload field. */
  receiveInput(peerId: string, input: RemoteInput): Promise<void> {
    const generation = this.#generation;
    const inputGeneration = this.#inputGeneration;
    const request = { ...input };
    const operation = this.#inputQueue.then(async () => {
      this.#assertActive();
      if (
        generation !== this.#generation ||
        inputGeneration !== this.#inputGeneration ||
        this.snapshot.controllerPeerId !== peerId
      ) {
        throw new Error("No current control grant for this sender");
      }
      if (this.snapshot.screenConsent !== "granted" || !this.#input.isAvailable()) {
        throw new Error("Capture consent or native permission is no longer valid");
      }
      if (
        !request ||
        !["pointer", "key"].includes(request.type) ||
        !["down", "up", ...(request.type === "pointer" ? ["move"] : [])].includes(request.action)
      ) {
        throw new Error("Invalid input command");
      }
      if (
        request.type === "pointer" &&
        (!Number.isFinite(request.x) ||
          !Number.isFinite(request.y) ||
          request.x < 0 ||
          request.x > 1 ||
          request.y < 0 ||
          request.y > 1)
      ) {
        throw new Error("Pointer coordinates must be normalized to [0, 1]");
      }
      if (
        request.type === "key" &&
        (typeof request.key !== "string" || request.key.length === 0 || request.key.length > 64)
      ) {
        throw new Error("Invalid key");
      }
      await this.#input.execute(request);
    });
    this.#inputQueue = operation.catch(() => {});
    return operation;
  }

  end(): Promise<void> {
    if (this.#cleanup) return this.#cleanup;
    if (!this.#context) return Promise.resolve();
    this.#generation++;
    this.#inputGeneration++;
    // Assign cleanup before emitting to make reentrant end() return the same operation.
    this.#cleanup = Promise.resolve()
      .then(async () => {
        const results = await Promise.allSettled([this.#releaseInput(), this.#media.leave()]);
        const failures = results.filter((r): r is PromiseRejectedResult => r.status === "rejected");
        for (const failure of failures) this.#errors.next(new Error(String(failure.reason)));
        if (failures.length)
          throw new AggregateError(
            failures.map((r) => r.reason),
            "Session cleanup failed",
          );
        this.#context = null;
        this.#update({ connection: "ended" });
      })
      .finally(() => {
        this.#cleanup = null;
      });
    this.#update({ connection: "ending", screenConsent: "revoked", controllerPeerId: null });
    return this.#cleanup;
  }

  destroy(): Promise<void> {
    if (this.#destruction) return this.#destruction;
    this.#destroyed = true;
    this.#destruction = Promise.resolve()
      .then(() => this.end())
      .then(() => {
        this.#state.complete();
        this.#errors.complete();
      })
      .catch((error: unknown) => {
        // Still terminal to commands, but resource release may be retried after an OS failure.
        this.#destruction = null;
        throw error;
      });
    return this.#destruction;
  }

  #releaseInput(): Promise<void> {
    const operation = this.#inputQueue.then(() => this.#input.releaseAll());
    this.#inputQueue = operation.catch(() => {});
    return operation;
  }

  #update(patch: Partial<SupportSnapshot>): void {
    this.#state.next(Object.freeze({ ...this.snapshot, ...patch }));
  }

  #assertAlive(): void {
    if (this.#destroyed) throw new Error("SupportSessionService is destroyed");
  }

  #assertActive(): void {
    this.#assertAlive();
    if (!this.#context || this.snapshot.connection !== "active")
      throw new Error("Session is not active");
  }
}
