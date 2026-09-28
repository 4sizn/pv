import type { Participant } from "./types";

/** Product-only policy. The media SDK never sees these roles. */
export class ParentViewPolicy {
  canControl(controller: Participant, target: Participant): boolean {
    return (
      controller.peerId !== target.peerId && controller.role === "child" && target.role === "parent"
    );
  }
}
