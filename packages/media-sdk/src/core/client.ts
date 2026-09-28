import { MediaController } from "./controller.js";
import type { MediaClientDependencies } from "./ports.js";
import type { JoinOptions, Publication } from "./types.js";

/** Public facade only. The controller is the single lifecycle/state authority. */
export class MediaClient {
  private readonly controller: MediaController;
  readonly state$;
  readonly peers$;
  readonly tracks$;
  readonly errors$;
  readonly messages$;

  constructor(dependencies: MediaClientDependencies) {
    this.controller = new MediaController(dependencies);
    this.state$ = this.controller.state$;
    this.peers$ = this.controller.peers$;
    this.tracks$ = this.controller.tracks$;
    this.errors$ = this.controller.errors$;
    this.messages$ = this.controller.messages$;
  }

  join(options: JoinOptions): Promise<void> {
    return this.controller.join(options);
  }
  publish(publication: Publication): Promise<void> {
    return this.controller.publish(publication);
  }
  unpublish(id: string): Promise<void> {
    return this.controller.unpublish(id);
  }
  send(data: string): void {
    this.controller.send(data);
  }
  leave(): Promise<void> {
    return this.controller.leave();
  }
  destroy(): Promise<void> {
    return this.controller.destroy();
  }
}
