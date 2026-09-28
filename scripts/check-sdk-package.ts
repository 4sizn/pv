import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

// Consume only packed SDK files: workspace aliases/source paths cannot hide broken exports.
const root = resolve(import.meta.dir, "..");
const scratch = await mkdtemp(join(tmpdir(), "pv-sdk-consumer-"));
const execute = (command: string, args: string[], cwd = scratch) =>
  execFileSync(command, args, { cwd, encoding: "utf8", timeout: 120_000 });

try {
  const metadata: { filename: string }[] = JSON.parse(
    execute(
      "npm",
      ["pack", "--json", "--ignore-scripts", "--pack-destination", scratch],
      join(root, "packages/media-sdk"),
    ),
  );
  const packed = metadata[0];
  if (!packed || !/^[a-z0-9._-]+\.tgz$/.test(packed.filename)) {
    throw new Error("npm did not produce the expected SDK tarball");
  }
  const sdkDirectory = join(scratch, "node_modules/@parentview/media-sdk");
  await mkdir(sdkDirectory, { recursive: true });
  execute("tar", [
    "-xzf",
    join(scratch, packed.filename),
    "-C",
    sdkDirectory,
    "--strip-components=1",
  ]);
  // RxJS is the only allowed consumer dependency. Reuse its locked installation offline.
  await symlink(join(root, "node_modules/rxjs"), join(scratch, "node_modules/rxjs"), "junction");
  await writeFile(join(scratch, "package.json"), JSON.stringify({ private: true, type: "module" }));
  await writeFile(
    join(scratch, "consumer.mjs"),
    `import assert from "node:assert/strict";
import { Subject } from "rxjs";
import { MediaClient } from "@parentview/media-sdk";
import { createBrowserMediaClient } from "@parentview/media-sdk/browser";
import { NativeDataClient } from "@parentview/media-sdk/native";

assert.equal(typeof globalThis.window, "undefined");
assert.equal(typeof createBrowserMediaClient, "function");
const events = new Subject();
let closes = 0;
let stops = 0;
const client = new MediaClient({
  signaling: {
    events$: events.asObservable(),
    connect: async () => ({ peerId: "local", peers: [] }),
    send() {}, ping() {}, close() { closes++; },
  },
  peers: { create() { throw new Error("No remote peer is expected"); } },
});
const states = [];
let completed = false;
client.state$.subscribe({ next: value => states.push(value), complete: () => { completed = true; } });
assert.equal(typeof client.state$.next, "undefined");
await client.join({ roomId: "room", roomToken: "test-invite", deviceToken: "test-device" });
await client.publish({ id: "camera", kind: "camera", track: {
  id: "capture", kind: "video", stop() { stops++; },
} });
await client.leave();
await client.leave();
await client.destroy();
await client.destroy();
assert.equal(stops, 1, "packed client must release an owned capture exactly once");
assert.equal(closes, 1, "idle teardown must not close signaling twice");
assert.equal(completed, true);
assert.deepEqual(states, ["idle", "joining", "joined", "leaving", "idle", "destroyed"]);

let nativeCloses = 0;
const nativeSnapshot = (revision, state, peerId = null, peers = [], readyPeers = []) =>
  ({ revision, state, peerId, peers, readyPeers });
const native = await NativeDataClient.create({ transport: {
  open: async () => nativeSnapshot(0, "idle"),
  readBatch: () => new Promise(() => {}),
  join: async () => nativeSnapshot(1, "joined", "local", ["remote"], ["remote"]),
  send: async () => ({ acceptedPeerIds: ["remote"], failures: [] }),
  leave: async () => nativeSnapshot(2, "idle"),
  destroy: async () => { nativeCloses++; return nativeSnapshot(3, "destroyed"); },
} });
const nativeStates = [];
let nativeCompleted = false;
native.state$.subscribe({ next: value => nativeStates.push(value), complete: () => { nativeCompleted = true; } });
for (const stream of [native.state$, native.peers$, native.readyPeers$, native.messages$, native.errors$]) {
  assert.equal(typeof stream.next, "undefined");
}
await native.join({ signalingUrl: "ws://localhost:8787/ws", roomId: "r", roomToken: "t", deviceToken: "d" });
assert.deepEqual(await native.send("data"), { acceptedPeerIds: ["remote"], failures: [] });
await native.leave();
await native.destroy();
await native.destroy();
assert.equal(nativeCloses, 1);
assert.equal(nativeCompleted, true);
assert.deepEqual(nativeStates, ["idle", "joined", "idle", "destroyed"]);
`,
  );
  execute("node", ["consumer.mjs"]);
  await writeFile(
    join(scratch, "consumer.mts"),
    `import { MediaClient, type MediaClientDependencies, type MediaState } from "@parentview/media-sdk";
import type { Observable } from "rxjs";
import { NativeDataClient, type NativeDataTransportPort, type NativeDataSendResult } from "@parentview/media-sdk/native";
declare const dependencies: MediaClientDependencies;
const client = new MediaClient(dependencies);
const states: Observable<MediaState> = client.state$;
void states;
const teardown: Promise<void> = client.destroy();
void teardown;
// @ts-expect-error Public streams must not allow consumers to mutate lifecycle state.
client.state$.next("joined");
// @ts-expect-error Product roles are not SDK session admission options.
void client.join({ roomId: "r", roomToken: "t", deviceToken: "d", role: "parent" });
// @ts-expect-error This consumer intentionally has no browser ambient types.
const browserOnly: RTCPeerConnection = {};
void browserOnly;
declare const transport: NativeDataTransportPort;
const native = await NativeDataClient.create({ transport });
const ready: Observable<readonly string[]> = native.readyPeers$;
void ready;
const receipt: NativeDataSendResult = await native.send("application data");
void receipt;
// @ts-expect-error Native snapshots remain readonly Observable projections.
native.state$.next("joined");
// @ts-expect-error The native data entry does not claim an unimplemented capture API.
native.publish({});
// @ts-expect-error Product roles are not native session admission options.
void native.join({ signalingUrl: "ws://localhost/ws", roomId: "r", roomToken: "t", deviceToken: "d", role: "parent" });
await native.destroy();
`,
  );
  await writeFile(
    join(scratch, "tsconfig.json"),
    JSON.stringify({
      compilerOptions: {
        target: "ES2022",
        module: "NodeNext",
        moduleResolution: "NodeNext",
        lib: ["ES2022"],
        types: [],
        strict: true,
        noEmit: true,
        skipLibCheck: true,
      },
      files: ["consumer.mts"],
    }),
  );
  execute("node", [join(root, "node_modules/typescript/bin/tsc"), "-p", "tsconfig.json"]);
  console.log(
    "SDK package: isolated ESM, DOM-free consumer types, readonly streams and teardown passed.",
  );
} finally {
  await rm(scratch, { recursive: true, force: true });
}
