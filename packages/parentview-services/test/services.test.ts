import { describe, expect, test } from "bun:test";
import { Subject } from "rxjs";
import {
  DeviceService,
  NetworkService,
  type RemoteInput,
  type SessionContext,
  SupportSessionService,
} from "../src";

const context: SessionContext = {
  local: { peerId: "receiver", role: "parent" },
  participants: [
    { peerId: "receiver", role: "parent" },
    { peerId: "helper-a", role: "child" },
    { peerId: "helper-b", role: "child" },
  ],
  connection: { roomId: "room", roomToken: "invite", deviceToken: "device" },
};

function setup(join: () => Promise<void> = async () => {}) {
  const executed: RemoteInput[] = [];
  let leaves = 0;
  let releases = 0;
  let available = true;
  const service = new SupportSessionService({
    media: {
      join,
      leave: async () => {
        leaves++;
      },
    },
    input: {
      isAvailable: () => available,
      execute: async (input) => {
        executed.push(input);
      },
      releaseAll: async () => {
        releases++;
      },
    },
  });
  return {
    service,
    executed,
    setAvailable: (value: boolean) => {
      available = value;
    },
    counts: () => ({ leaves, releases }),
  };
}

const pointer: RemoteInput = { type: "pointer", x: 0.5, y: 0.5, action: "move" };

test("reentrant end notifications share one cleanup operation", async () => {
  const { service, counts } = setup();
  const ending: Promise<void>[] = [];
  service.state$.subscribe((state) => {
    if (state.connection === "ending") ending.push(service.end());
  });
  service.request(context);
  await service.approve();
  const original = service.end();
  expect(ending).toHaveLength(1);
  expect(ending[0]).toBe(original);
  await original;
  expect(counts()).toEqual({ leaves: 1, releases: 1 });
  await service.destroy();
});

test("synchronous subscriber cancellation prevents starting a media join", async () => {
  let joins = 0;
  const { service } = setup(async () => {
    joins++;
  });
  let ending = Promise.resolve();
  service.state$.subscribe((state) => {
    if (state.connection === "joining") ending = service.end();
  });
  service.request(context);
  await service.approve();
  await ending;
  expect(joins).toBe(0);
  expect(service.snapshot.connection).toBe("ended");
  await service.destroy();
});

test("failed cleanup blocks new sessions and can be retried, including destruction", async () => {
  let fail = true;
  let releases = 0;
  const service = new SupportSessionService({
    media: { join: async () => {}, leave: async () => {} },
    input: {
      isAvailable: () => true,
      execute: async () => {},
      releaseAll: async () => {
        releases++;
        if (fail) throw new Error("OS release failed");
      },
    },
  });
  service.request(context);
  await service.approve();
  await expect(service.end()).rejects.toThrow("cleanup failed");
  expect(service.snapshot.connection).toBe("ending");
  expect(() => service.request(context)).toThrow("already open");
  let completed = false;
  service.state$.subscribe({
    complete: () => {
      completed = true;
    },
  });
  await expect(service.destroy()).rejects.toThrow("cleanup failed");
  expect(completed).toBe(false);
  fail = false;
  await service.destroy();
  expect(releases).toBe(3);
  expect(completed).toBe(true);
});

describe("product consent and control stay outside media", () => {
  test("approval, screen consent and control grant are three separate gates", async () => {
    const { service, executed } = setup();
    service.request(context);
    expect(() => service.grantControl("helper-a")).toThrow("not active");
    await service.approve();
    expect(() => service.grantControl("helper-a")).toThrow("Screen consent");
    service.confirmScreenConsent();
    await expect(service.receiveInput("helper-a", pointer)).rejects.toThrow(
      "No current control grant",
    );
    service.grantControl("helper-a");
    await service.receiveInput("helper-a", pointer);
    expect(executed).toHaveLength(1);
    expect(() => service.grantControl("helper-b")).toThrow("Revoke");
    await service.revokeScreenConsent();
    await expect(service.receiveInput("helper-a", pointer)).rejects.toThrow();
    expect(executed).toHaveLength(1);
    await service.destroy();
  });

  test("unpaired senders, self-control and invalid coordinates cannot reach native input", async () => {
    const { service, executed } = setup();
    service.request(context);
    await service.approve();
    service.confirmScreenConsent();
    expect(() => service.grantControl("stranger")).toThrow();
    expect(() => service.grantControl("receiver")).toThrow();
    service.grantControl("helper-a");
    await expect(service.receiveInput("helper-b", pointer)).rejects.toThrow();
    await expect(service.receiveInput("helper-a", { ...pointer, x: Number.NaN })).rejects.toThrow(
      "coordinates",
    );
    await expect(
      service.receiveInput("helper-a", { type: "key", action: "down", key: "" }),
    ).rejects.toThrow("Invalid key");
    expect(executed).toHaveLength(0);
    await service.destroy();
  });

  test("queued input from a revoked grant cannot be replayed under a new grant", async () => {
    const { service, executed } = setup();
    service.request(context);
    await service.approve();
    service.confirmScreenConsent();
    service.grantControl("helper-a");
    const pending = service.receiveInput("helper-a", pointer);
    const released = service.revokeControl();
    service.grantControl("helper-a");
    await expect(pending).rejects.toThrow("No current control grant");
    await released;
    expect(executed).toHaveLength(0);
    await service.destroy();
  });

  test("OS permission loss immediately blocks an existing grant", async () => {
    const { service, setAvailable, executed } = setup();
    service.request(context);
    await service.approve();
    service.confirmScreenConsent();
    service.grantControl("helper-a");
    setAvailable(false);
    await expect(service.receiveInput("helper-a", pointer)).rejects.toThrow("permission");
    expect(executed).toHaveLength(0);
    await service.destroy();
  });

  test("late join completion does not resurrect an ended session", async () => {
    let resolveJoin: () => void = () => {};
    const joined = new Promise<void>((resolve) => {
      resolveJoin = resolve;
    });
    const { service, counts } = setup(() => joined);
    service.request(context);
    const approving = service.approve();
    await service.end();
    resolveJoin();
    await approving;
    expect(service.snapshot.connection).toBe("ended");
    expect(counts().leaves).toBe(1);
    let completed = 0;
    service.state$.subscribe({
      complete: () => {
        completed++;
      },
    });
    await Promise.all([service.destroy(), service.destroy()]);
    expect(completed).toBe(1);
    expect(() => service.request(context)).toThrow("destroyed");
  });

  test("caller mutation cannot alter the role used by an active session", async () => {
    const participants = context.participants.map((p) => ({ ...p }));
    const { service } = setup();
    service.request({ ...context, participants });
    participants[1].role = "parent";
    await service.approve();
    service.confirmScreenConsent();
    expect(() => service.grantControl("helper-a")).not.toThrow();
    await service.destroy();
  });
});

test("device refresh ignores stale results and destroys its environment subscription", async () => {
  const changes$ = new Subject<void>();
  const resolvers: ((value: { id: string; kind: "camera"; label: string }[]) => void)[] = [];
  const service = new DeviceService({
    changes$,
    enumerate: () =>
      new Promise((resolve) => {
        resolvers.push(resolve);
      }),
  });
  const snapshots: string[][] = [];
  service.devices$.subscribe((devices) => snapshots.push(devices.map((d) => d.id)));
  const old = service.refresh();
  const recent = service.refresh();
  resolvers[1]([{ id: "new", kind: "camera", label: "New" }]);
  await recent;
  resolvers[0]([{ id: "old", kind: "camera", label: "Old" }]);
  await old;
  expect(snapshots).toEqual([[], ["new"]]);
  service.destroy();
  changes$.next();
  expect(resolvers).toHaveLength(2);
  await expect(service.refresh()).rejects.toThrow("destroyed");
});

test("network hints never execute connection work and unsubscribe on destruction", () => {
  const changes$ = new Subject<{ availability: "online" | "offline" }>();
  const service = new NetworkService({ current: () => ({ availability: "unknown" }), changes$ });
  const values: string[] = [];
  service.state$.subscribe((state) => values.push(state.availability));
  changes$.next({ availability: "online" });
  service.destroy();
  changes$.next({ availability: "offline" });
  expect(values).toEqual(["unknown", "online"]);
});
