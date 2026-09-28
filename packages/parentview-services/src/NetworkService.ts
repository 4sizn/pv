import { BehaviorSubject, type Observable, type Subscription } from "rxjs";
import type { NetworkEnvironmentPort, NetworkSnapshot } from "./types";

/** Environment availability is a hint; only the media controller can declare a peer connected. */
export class NetworkService {
  readonly state$: Observable<NetworkSnapshot>;
  #state: BehaviorSubject<NetworkSnapshot>;
  #subscription: Subscription;

  constructor(environment: NetworkEnvironmentPort) {
    this.#state = new BehaviorSubject({ ...environment.current() });
    this.state$ = this.#state.asObservable();
    this.#subscription = environment.changes$.subscribe((value) => this.#state.next({ ...value }));
  }

  destroy(): void {
    this.#subscription.unsubscribe();
    this.#state.complete();
  }
}
