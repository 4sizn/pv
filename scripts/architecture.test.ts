import { expect, test } from "bun:test";
import { inspectArchitecture } from "./architecture";

test("ESM .js specifiers resolve to TypeScript before enforcing boundaries", () => {
  const failures = inspectArchitecture([
    {
      path: "packages/media-sdk/src/core/types.ts",
      text: 'import { Policy } from "../../../parentview-services/src/Policy.js";',
    },
    { path: "packages/parentview-services/src/Policy.ts", text: "export class Policy {}" },
  ]);
  expect(failures.some((failure) => failure.rule === "sdk-isolation")).toBe(true);
});

test("SDK rejects product roles even without a product import", () => {
  const failures = inspectArchitecture([
    {
      path: "packages/media-sdk/src/core/types.ts",
      text: 'export type Role = "parent" | "child";',
    },
  ]);
  expect(failures.filter((failure) => failure.rule === "role-neutral-sdk")).toHaveLength(2);
});

test("core cannot hide a browser adapter through a re-export or dynamic import", () => {
  const failures = inspectArchitecture([
    {
      path: "packages/media-sdk/src/core/controller.ts",
      text: 'export * from "../browser"; const adapter = import("../browser");',
    },
    { path: "packages/media-sdk/src/browser/index.ts", text: "export const adapter = true;" },
  ]);
  expect(failures.filter((failure) => failure.rule === "core-dependency")).toHaveLength(2);
});

test("SDK cannot import product services using a relative path", () => {
  const failures = inspectArchitecture([
    {
      path: "packages/media-sdk/src/core/types.ts",
      text: 'import { Policy } from "../../../parentview-services/src/Policy";',
    },
    { path: "packages/parentview-services/src/Policy.ts", text: "export class Policy {}" },
  ]);
  expect(failures.some((failure) => failure.rule === "sdk-isolation")).toBe(true);
});

test("product roles are allowed in service modules but direct platform calls are not", () => {
  const valid = inspectArchitecture([
    {
      path: "packages/parentview-services/src/Policy.ts",
      text: 'export type Role = "parent" | "child";',
    },
  ]);
  expect(valid).toEqual([]);
  const invalid = inspectArchitecture([
    {
      path: "packages/parentview-services/src/Policy.ts",
      text: "navigator.mediaDevices.getUserMedia({video:true});",
    },
  ]);
  expect(invalid[0].rule).toBe("platform-free-core");
});

test("circular module dependencies are rejected", () => {
  const failures = inspectArchitecture([
    { path: "packages/media-sdk/src/core/a.ts", text: 'import "./b";' },
    { path: "packages/media-sdk/src/core/b.ts", text: 'import "./a";' },
  ]);
  expect(failures.some((failure) => failure.rule === "acyclic-dependencies")).toBe(true);
});

test("core and services reject external platform dependencies in static and runtime imports", () => {
  const failures = inspectArchitecture([
    {
      path: "packages/media-sdk/src/core/io.ts",
      text: 'import { readFile } from "node:fs"; const net = import("node:net"); const fs = require("fs");',
    },
    {
      path: "packages/parentview-services/src/io.ts",
      text: 'import { readFile } from "node:fs"; const net = import("node:net"); const fs = require("fs");',
    },
  ]);
  expect(failures.filter((failure) => failure.rule === "core-dependency")).toHaveLength(3);
  expect(failures.filter((failure) => failure.rule === "service-dependency")).toHaveLength(3);
});

test("import-type expressions cannot introduce concrete adapters or external platform types", () => {
  const failures = inspectArchitecture([
    {
      path: "packages/media-sdk/src/core/types.ts",
      text: 'type Adapter = import("../browser/peer.js").BrowserPeerFactory; type Network = import("node:net").Socket;',
    },
    {
      path: "packages/parentview-services/src/types.ts",
      text: 'type Adapter = import("@parentview/media-sdk/browser").BrowserMediaClientOptions; type Network = import("node:net").Socket;',
    },
    { path: "packages/media-sdk/src/browser/peer.ts", text: "export class BrowserPeerFactory {}" },
    {
      path: "packages/media-sdk/src/browser/index.ts",
      text: "export type BrowserMediaClientOptions = {};",
    },
  ]);
  expect(failures.filter((failure) => failure.rule === "core-dependency")).toHaveLength(2);
  expect(failures.filter((failure) => failure.rule === "service-dependency")).toHaveLength(2);
});

test("services cannot reach private SDK modules through relative imports", () => {
  const failures = inspectArchitecture([
    {
      path: "packages/parentview-services/src/session.ts",
      text: `
        import { MediaController } from "../../media-sdk/src/core/controller.js";
        export { MediaController } from "../../media-sdk/src/core/controller.js";
        const controller = import("../../media-sdk/src/core/controller.js");
        type Controller = import("../../media-sdk/src/core/controller.js").MediaController;
      `,
    },
    {
      path: "packages/media-sdk/src/core/controller.ts",
      text: "export class MediaController {}",
    },
  ]);
  expect(failures.filter((failure) => failure.rule === "service-dependency")).toHaveLength(4);
});

test("SDK role literals are rejected when written with backticks", () => {
  const failures = inspectArchitecture([
    {
      path: "packages/media-sdk/src/core/types.ts",
      text: "export const roles = [`parent`, `child`]; export const source = `camera`;",
    },
  ]);
  expect(failures.filter((failure) => failure.rule === "role-neutral-sdk")).toHaveLength(2);
});

test("import-type references participate in dependency cycle detection", () => {
  const failures = inspectArchitecture([
    { path: "packages/media-sdk/src/core/a.ts", text: 'export type A = import("./b.js").B;' },
    { path: "packages/media-sdk/src/core/b.ts", text: 'export type B = import("./a.js").A;' },
  ]);
  expect(failures.some((failure) => failure.rule === "acyclic-dependencies")).toBe(true);
});

test("layer rules preserve RxJS, local ports, public SDK types and concrete adapter imports", () => {
  expect(
    inspectArchitecture([
      {
        path: "packages/media-sdk/src/core/controller.ts",
        text: 'import { Observable } from "rxjs"; import { map } from "rxjs/operators"; type Port = import("./ports.js").Port;',
      },
      { path: "packages/media-sdk/src/core/ports.ts", text: "export type Port = {};" },
      {
        path: "packages/media-sdk/src/index.ts",
        text: 'export type { Port } from "./core/ports.js";',
      },
      {
        path: "packages/parentview-services/src/session.ts",
        text: 'import { Subject } from "rxjs"; import type { Port } from "@parentview/media-sdk"; type PublicPort = import("@parentview/media-sdk").Port; export type { Port } from "../../media-sdk/src/index.js"; export type { Local } from "./ports.js";',
      },
      { path: "packages/parentview-services/src/ports.ts", text: "export type Local = {};" },
      {
        path: "packages/media-sdk/src/browser/peer.ts",
        text: 'import { connect } from "node:net"; import { Observable } from "rxjs"; const connection = new RTCPeerConnection();',
      },
    ]),
  ).toEqual([]);
});

test("native facade rejects platforms and a second TypeScript session controller", () => {
  const failures = inspectArchitecture([
    {
      path: "packages/media-sdk/src/native/client.ts",
      text: 'import "@tauri-apps/api/core"; import "node:net"; import "../browser/index.js"; import "../core/controller.js"; navigator.onLine; import(variable); export type Role = "parent";',
    },
    { path: "packages/media-sdk/src/browser/index.ts", text: "export {};" },
    { path: "packages/media-sdk/src/core/controller.ts", text: "export {};" },
  ]);
  expect(failures.filter((failure) => failure.rule === "core-dependency")).toHaveLength(4);
  expect(failures.some((failure) => failure.rule === "platform-free-core")).toBe(true);
  expect(failures.some((failure) => failure.rule === "dynamic-dependency")).toBe(true);
  expect(failures.some((failure) => failure.rule === "role-neutral-sdk")).toBe(true);
});

test("services may use the public native SDK but cannot reach its private implementation", () => {
  const sources = [
    { path: "packages/media-sdk/src/native/index.ts", text: 'export * from "./types.js";' },
    { path: "packages/media-sdk/src/native/types.ts", text: "export type Port = {};" },
    {
      path: "packages/parentview-services/src/session.ts",
      text: 'import type { Port } from "@parentview/media-sdk/native";',
    },
  ];
  expect(inspectArchitecture(sources)).toEqual([]);
  sources[2].text = 'import type { Port } from "../../media-sdk/src/native/types.js";';
  expect(
    inspectArchitecture(sources).some((failure) => failure.rule === "service-dependency"),
  ).toBe(true);
});
