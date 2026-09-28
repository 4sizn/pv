import { BehaviorSubject, type Observable, Subject, type Subscription } from "rxjs";
import type { Device, DeviceCatalogPort } from "./types";

/** Owns device snapshots and its subscription, not capture tracks or pairing relationships. */
export class DeviceService {
  readonly devices$: Observable<readonly Device[]>;
  readonly errors$: Observable<Error>;
  #devices = new BehaviorSubject<readonly Device[]>([]);
  #errors = new Subject<Error>();
  #catalog: DeviceCatalogPort;
  #subscription: Subscription;
  #generation = 0;
  #destroyed = false;

  constructor(catalog: DeviceCatalogPort) {
    this.#catalog = catalog;
    this.devices$ = this.#devices.asObservable();
    this.errors$ = this.#errors.asObservable();
    this.#subscription = catalog.changes$.subscribe(() => {
      void this.refresh().catch((error: unknown) => {
        if (!this.#destroyed)
          this.#errors.next(error instanceof Error ? error : new Error(String(error)));
      });
    });
  }

  async refresh(): Promise<void> {
    if (this.#destroyed) throw new Error("DeviceService is destroyed");
    const generation = ++this.#generation;
    const devices = await this.#catalog.enumerate();
    if (this.#destroyed || generation !== this.#generation) return;
    this.#devices.next(devices.map((device) => ({ ...device })));
  }

  destroy(): void {
    if (this.#destroyed) return;
    this.#destroyed = true;
    this.#generation++;
    this.#subscription.unsubscribe();
    this.#devices.complete();
    this.#errors.complete();
  }
}
