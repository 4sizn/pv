import { describe, expect, test } from "bun:test";
import {
  consumeLocationInvitation,
  createInvitationLink,
  parseInvitation,
  parseOrigin,
  type RoomInvitation,
} from "../src/invitation";

const room: RoomInvitation = {
  version: 1,
  roomId: "r_fixture",
  roomToken: "test-only-room-secret/+?=&",
  signalingOrigin: "http://localhost:8787",
  iceServers: [
    { urls: ["stun:stun.example.test:3478"] },
    {
      urls: [
        "turn:relay.example.test:3478?transport=udp",
        "turns:relay.example.test:5349?transport=tcp",
      ],
      username: "3600:p_fixture",
      credential: "test-only-ephemeral-password",
    },
  ],
};

function linkFor(value: unknown): string {
  return `http://localhost:1420/#${new URLSearchParams({ invite: JSON.stringify(value) })}`;
}

/** Only location/history are needed; restore the global even when an assertion fails. */
function withLocation(href: string, assertion: (replacements: unknown[][]) => void): void {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  let location = new URL(href);
  const replacements: unknown[][] = [];
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      get location() {
        return location;
      },
      history: {
        replaceState(state: unknown, title: string, next: string) {
          replacements.push([state, title, next]);
          location = new URL(next, location);
        },
      },
    },
  });
  try {
    assertion(replacements);
  } finally {
    if (previous) Object.defineProperty(globalThis, "window", previous);
    else Reflect.deleteProperty(globalThis, "window");
  }
}

describe("signaling origin validation", () => {
  test("canonicalizes an HTTP(S) origin while preserving an explicit local port", () => {
    expect(parseOrigin(" HTTPS://SIGNAL.EXAMPLE.TEST:443/ ")).toBe("https://signal.example.test");
    expect(parseOrigin("http://localhost:8787/")).toBe("http://localhost:8787");
    expect(parseOrigin("http://[::1]:8787/")).toBe("http://[::1]:8787");
  });

  test("rejects credentials, endpoint paths, queries, fragments and non-HTTP schemes", () => {
    for (const value of [
      "",
      "localhost:8787",
      "https://user:secret@signal.example.test",
      "https://signal.example.test/rooms",
      "https://signal.example.test?token=secret",
      "https://signal.example.test#token=secret",
      "ws://localhost:8787",
      "file:///tmp/signaling",
      "javascript:alert(1)",
      "tauri://localhost",
    ]) {
      expect(() => parseOrigin(value)).toThrow();
    }
  });
});

describe("invitation links", () => {
  test("roundtrips browser and packaged-host invitations with secrets only in the fragment", () => {
    for (const href of [
      "http://localhost:1420/lab?old=value#stale",
      "tauri://localhost/index.html?old=value#stale",
      "http://tauri.localhost/index.html?old=value#stale",
    ]) {
      withLocation(href, () => {
        const link = createInvitationLink(room);
        const result = new URL(link);
        expect(result.protocol).toBe(new URL(href).protocol);
        expect(result.host).toBe(new URL(href).host);
        expect(result.pathname).toBe(new URL(href).pathname);
        expect(result.search).toBe("");
        expect(link.split("#")[0]).not.toContain(room.roomToken);
        expect(
          JSON.parse(new URLSearchParams(result.hash.slice(1)).get("invite") ?? "null"),
        ).toEqual(room);
        expect(parseInvitation(`  ${link}\n`)).toEqual(room);
      });
    }
  });

  test("requires a full link with invite in its fragment, not a token, raw JSON or query", () => {
    for (const value of [
      "",
      room.roomToken,
      JSON.stringify(room),
      `#${new URLSearchParams({ invite: JSON.stringify(room) })}`,
      `http://localhost:1420/?${new URLSearchParams({ invite: JSON.stringify(room) })}`,
      "http://localhost:1420/#unrelated=value",
      "http://localhost:1420/#invite=%7Bmalformed-json",
    ]) {
      expect(() => parseInvitation(value)).toThrow();
    }
  });

  test("enforces the maximum whole-link length before parsing", () => {
    const prefix = `${linkFor(room)}&padding=`;
    const atLimit = `${prefix}${"x".repeat(32_768 - prefix.length)}`;
    expect(atLimit.length).toBe(32_768);
    expect(parseInvitation(atLimit)).toEqual(room);
    expect(() => parseInvitation(`${atLimit}x`)).toThrow("너무 깁니다");
  });

  test("rejects malformed top-level values and missing or mistyped required fields", () => {
    const invalid: unknown[] = [null, false, 42, "room", [], {}];
    for (const patch of [
      { version: 2 },
      { version: "1" },
      { roomId: "" },
      { roomId: 123 },
      { roomToken: "" },
      { roomToken: null },
      { signalingOrigin: null },
      { signalingOrigin: "javascript:alert(1)" },
      { iceServers: null },
      { iceServers: {} },
    ]) {
      invalid.push({ ...room, ...patch });
    }
    for (const field of ["version", "roomId", "roomToken", "signalingOrigin", "iceServers"]) {
      const missing: Record<string, unknown> = { ...room };
      delete missing[field];
      invalid.push(missing);
    }
    for (const value of invalid) expect(() => parseInvitation(linkFor(value))).toThrow();
  });

  test("copies only admission/ICE fields from an untrusted payload", () => {
    const invitation = {
      ...room,
      role: "parent",
      peerId: "forged-peer",
      deviceToken: "forged-device-secret",
      iceServers: [
        { urls: "stun:stun.example.test", injected: "discard", credentialType: "unknown" },
      ],
    };
    expect(parseInvitation(linkFor(invitation))).toEqual({
      ...room,
      iceServers: [{ urls: ["stun:stun.example.test"] }],
    });
  });

  test("consumes an invitation once and clears its fragment before another read", () => {
    const link = linkFor(room).replace("1420/", "1420/lab?old=value");
    withLocation(link, (replacements) => {
      expect(consumeLocationInvitation()).toBe(link);
      expect(replacements).toEqual([[null, "", "/lab"]]);
      expect(consumeLocationInvitation()).toBe("");
      expect(replacements).toHaveLength(1);
    });
  });

  test("leaves unrelated location fragments untouched", () => {
    withLocation("http://localhost:1420/lab?view=grid#help", (replacements) => {
      expect(consumeLocationInvitation()).toBe("");
      expect(replacements).toHaveLength(0);
    });
  });
});

describe("untrusted ICE configuration", () => {
  test("accepts host-only rooms and normalizes scalar STUN URLs", () => {
    expect(parseInvitation(linkFor({ ...room, iceServers: [] })).iceServers).toEqual([]);
    expect(
      parseInvitation(linkFor({ ...room, iceServers: [{ urls: "stuns:stun.example.test:5349" }] }))
        .iceServers,
    ).toEqual([{ urls: ["stuns:stun.example.test:5349"] }]);
  });

  test("rejects malformed servers, non-ICE URLs and mistyped credential values", () => {
    for (const server of [
      null,
      "stun:stun.example.test",
      {},
      { urls: [] },
      { urls: 123 },
      { urls: ["stun:stun.example.test", 123] },
      { urls: ["https://relay.example.test"] },
      { urls: ["javascript:alert(1)"] },
      { urls: ["stun:stun.example.test"], username: 123 },
      { urls: ["stun:stun.example.test"], credential: { accessToken: "untrusted" } },
    ]) {
      expect(() => parseInvitation(linkFor({ ...room, iceServers: [server] }))).toThrow();
    }
  });

  test("rejects empty hosts and malformed ICE authority, port and transport forms", () => {
    for (const urls of [
      "stun:",
      "stun: ",
      "turn:",
      "turns:",
      "stun://stun.example.test",
      "stun:stun.example.test/path",
      "stun:stun.example.test?transport=tcp",
      "turn:relay.example.test:0",
      "turn:relay.example.test:65536",
      "turn:relay.example.test?transport=invalid",
    ]) {
      expect(() =>
        parseInvitation(
          linkFor({ ...room, iceServers: [{ urls, username: "fixture", credential: "fixture" }] }),
        ),
      ).toThrow();
    }
  });

  test("requires TURN username and credential together", () => {
    for (const server of [
      { urls: "turn:relay.example.test" },
      { urls: "turn:relay.example.test", username: "fixture" },
      { urls: "turns:relay.example.test", credential: "fixture" },
      { urls: "turn:relay.example.test", username: "", credential: "fixture" },
      { urls: "turn:relay.example.test", username: "fixture", credential: "" },
      { urls: ["stun:stun.example.test", "turn:relay.example.test"] },
    ]) {
      expect(() => parseInvitation(linkFor({ ...room, iceServers: [server] }))).toThrow();
    }
  });

  test("preserves supported secure, IPv6, uppercase scheme and TURN transport forms", () => {
    for (const url of [
      "stun:stun.example.test",
      "stuns:stun.example.test:5349",
      "TURN:relay.example.test:3478",
      "turn:[::1]:3478?transport=tcp",
      "turn:relay.example.test?transport=udp",
      "turns:relay.example.test?transport=tcp",
    ]) {
      const server = { urls: [url], username: "fixture", credential: "fixture" };
      expect(parseInvitation(linkFor({ ...room, iceServers: [server] })).iceServers).toEqual([
        server,
      ]);
    }
  });
});
