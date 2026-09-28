import { expect, test } from "bun:test";
import type { NativeMediaSnapshot } from "@parentview/media-sdk/native";
import { type NativeInvoke, TauriNativeMediaTransport } from "../src/native-transport";

const snapshot: NativeMediaSnapshot = {
  revision: 0,
  state: "idle",
  peerId: null,
  peers: [],
  readyPeers: [],
  localSources: [],
  remoteSources: [],
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test("transport binds all native commands to its own host handle and destroys once", async () => {
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const invoke: NativeInvoke = async <T>(command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "native_data_document") return "document-1" as T;
    if (command === "native_data_open") return { clientId: "owned", snapshot } as T;
    if (command === "native_data_read") return { snapshot, events: [] } as T;
    if (command === "native_data_send") return { acceptedPeerIds: [], failures: [] } as T;
    return snapshot as T;
  };
  const transport = new TauriNativeMediaTransport({ invoke });
  await expect(transport.send("before open")).rejects.toThrow("not open");
  expect(calls).toHaveLength(0);
  await Promise.all([transport.open(), transport.open()]);
  await transport.join({
    signalingUrl: "ws://localhost/ws",
    roomId: "test-room",
    roomToken: "synthetic-room-token",
    deviceToken: "synthetic-device-token",
  });
  await transport.readBatch();
  await transport.send("text");
  await transport.leave();
  await Promise.all([transport.destroy(), transport.destroy()]);
  expect(calls.filter(({ command }) => command === "native_data_open")).toHaveLength(1);
  expect(calls[0].command).toBe("native_data_document");
  expect(calls[1]).toEqual({ command: "native_data_open", args: { documentId: "document-1" } });
  expect(calls.filter(({ command }) => command === "native_data_destroy")).toHaveLength(1);
  expect(calls.slice(2).every(({ args }) => args?.clientId === "owned")).toBe(true);
  await expect(transport.readBatch()).rejects.toThrow("not open");
  await expect(transport.open()).rejects.toThrow("closed");
});

test("transport destruction closes a handle that arrives after opening was cancelled", async () => {
  const opening = deferred<{ clientId: string; snapshot: NativeMediaSnapshot }>();
  const destroyed: unknown[] = [];
  let openStarted = false;
  const transport = new TauriNativeMediaTransport({
    invoke: async <T>(command: string, args?: Record<string, unknown>) => {
      if (command === "native_data_document") return "document-1" as T;
      if (command === "native_data_open") {
        openStarted = true;
        return (await opening.promise) as T;
      }
      destroyed.push(args?.clientId);
      return { ...snapshot, state: "destroyed", revision: 1 } as T;
    },
  });
  const pending = transport.open().catch((error: unknown) => error);
  for (let index = 0; index < 10 && !openStarted; index++) await Promise.resolve();
  expect(openStarted).toBe(true);
  const closing = transport.destroy();
  opening.resolve({ clientId: "late", snapshot });
  expect(await pending).toMatchObject({ message: "Native transport closed while opening" });
  expect((await closing).state).toBe("destroyed");
  expect(destroyed).toEqual(["late"]);
});

test("disposing while the document lease is pending never allocates a native client", async () => {
  const lease = deferred<string>();
  const calls: string[] = [];
  const transport = new TauriNativeMediaTransport({
    invoke: async <T>(command: string) => {
      calls.push(command);
      if (command !== "native_data_document") throw new Error("No client should be allocated");
      return (await lease.promise) as T;
    },
  });
  const opening = transport.open().catch((error: unknown) => error);
  const closing = transport.destroy().catch((error: unknown) => error);
  lease.resolve("document-1");
  expect(String(await opening)).toContain("closed while opening");
  expect(String(await closing)).toContain("no open client");
  expect(calls).toEqual(["native_data_document"]);
});

test("a queued open preserves its old document lease and propagates navigation rejection", async () => {
  const gate = deferred<void>();
  let document = "document-1";
  let requested: unknown;
  const transport = new TauriNativeMediaTransport({
    invoke: async <T>(command: string, args?: Record<string, unknown>) => {
      if (command === "native_data_document") return document as T;
      if (command === "native_data_open") {
        requested = args?.documentId;
        await gate.promise;
        if (requested !== document) throw new Error("stale_document");
        return { clientId: "unexpected", snapshot } as T;
      }
      throw new Error("No old-document native handle should exist");
    },
  });
  const opening = transport.open().catch((error: unknown) => error);
  for (let index = 0; index < 10 && requested === undefined; index++) await Promise.resolve();
  expect(requested).toBe("document-1");
  document = "document-2";
  gate.resolve();
  expect(String(await opening)).toContain("stale_document");
  await expect(transport.destroy()).rejects.toThrow("no open client");
});

test("an invalid document lease is rejected before native allocation", async () => {
  const calls: string[] = [];
  const transport = new TauriNativeMediaTransport({
    invoke: async <T>(command: string) => {
      calls.push(command);
      return {} as T;
    },
  });
  await expect(transport.open()).rejects.toThrow("invalid document lease");
  expect(calls).toEqual(["native_data_document"]);
});
