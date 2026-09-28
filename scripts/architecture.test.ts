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
