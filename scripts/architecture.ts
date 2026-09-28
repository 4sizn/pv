import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, normalize, relative } from "node:path";
import ts from "typescript";

export type Source = { path: string; text: string };
export type Violation = { path: string; rule: string; detail: string };
const forbiddenRuntime = new Set([
  "window",
  "document",
  "navigator",
  "RTCPeerConnection",
  "MediaStream",
  "MediaStreamTrack",
  "WebSocket",
]);

export function inspectArchitecture(sources: readonly Source[]): Violation[] {
  const violations: Violation[] = [];
  const paths = new Set(sources.map((source) => source.path));
  const graph = new Map<string, string[]>();
  const aliases: Record<string, string> = {
    "@parentview/media-sdk": "packages/media-sdk/src/index.ts",
    "@parentview/media-sdk/browser": "packages/media-sdk/src/browser/index.ts",
    "@parentview/media-sdk/native": "packages/media-sdk/src/native/index.ts",
    "@parentview/services": "packages/parentview-services/src/index.ts",
  };
  for (const source of sources) {
    const sdk = source.path.startsWith("packages/media-sdk/src/");
    const core = source.path.startsWith("packages/media-sdk/src/core/");
    const native = source.path.startsWith("packages/media-sdk/src/native/");
    const portableRoot = native ? "packages/media-sdk/src/native/" : "packages/media-sdk/src/core/";
    const services = source.path.startsWith("packages/parentview-services/src/");
    const dependencies: string[] = [];
    graph.set(source.path, dependencies);
    const report = (rule: string, detail: string) =>
      violations.push({ path: source.path, rule, detail });
    const file = ts.createSourceFile(source.path, source.text, ts.ScriptTarget.Latest, true);
    const inspectImport = (specifier: string) => {
      const base = specifier.startsWith(".")
        ? normalize(join(dirname(source.path), specifier))
        : aliases[specifier];
      const resolved = base
        ? [base, base.replace(/\.[cm]?js$/, ".ts"), `${base}.ts`, `${base}/index.ts`].find((path) =>
            paths.has(path),
          )
        : undefined;
      if (resolved) dependencies.push(resolved);
      const external = !specifier.startsWith(".");
      const rxjs = specifier === "rxjs" || specifier.startsWith("rxjs/");
      if (
        sdk &&
        (specifier.includes("@parentview/services") ||
          resolved?.startsWith("packages/parentview-services/"))
      ) {
        report("sdk-isolation", `SDK imports product services: ${specifier}`);
      }
      if (sdk && (specifier.startsWith("@tauri-apps/") || resolved?.startsWith("apps/"))) {
        report("sdk-isolation", `SDK imports a host application: ${specifier}`);
      }
      if (
        (core || native) &&
        (specifier.startsWith("@tauri-apps/") ||
          specifier.includes("/browser") ||
          (external && !rxjs && !resolved?.startsWith(portableRoot)) ||
          (resolved && !resolved.startsWith(portableRoot)))
      ) {
        report(
          "core-dependency",
          `Portable dependency is outside its local ports/RxJS: ${specifier}`,
        );
      }
      if (
        services &&
        (specifier.includes("/browser") ||
          specifier.startsWith("@tauri-apps/") ||
          (resolved?.startsWith("packages/media-sdk/src/") &&
            resolved !== aliases["@parentview/media-sdk"] &&
            resolved !== aliases["@parentview/media-sdk/native"]) ||
          (external &&
            !rxjs &&
            specifier !== "@parentview/media-sdk" &&
            specifier !== "@parentview/media-sdk/native" &&
            !resolved?.startsWith("packages/parentview-services/src/")) ||
          resolved?.startsWith("apps/"))
      ) {
        report(
          "service-dependency",
          `Service dependency is outside local ports/RxJS/public SDK: ${specifier}`,
        );
      }
    };
    const visit = (node: ts.Node): void => {
      if (
        (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) &&
        node.moduleSpecifier &&
        ts.isStringLiteral(node.moduleSpecifier)
      ) {
        inspectImport(node.moduleSpecifier.text);
      }
      if (
        ts.isImportTypeNode(node) &&
        ts.isLiteralTypeNode(node.argument) &&
        ts.isStringLiteralLike(node.argument.literal)
      ) {
        inspectImport(node.argument.literal.text);
      }
      if (
        ts.isCallExpression(node) &&
        (node.expression.kind === ts.SyntaxKind.ImportKeyword ||
          (ts.isIdentifier(node.expression) && node.expression.text === "require"))
      ) {
        const argument = node.arguments[0];
        if (argument && ts.isStringLiteral(argument)) inspectImport(argument.text);
        else if (core || native || services)
          report("dynamic-dependency", "Nonliteral imports can bypass layer checks");
      }
      if (sdk && ts.isStringLiteralLike(node) && ["parent", "child"].includes(node.text)) {
        report("role-neutral-sdk", `Product role literal: ${node.text}`);
      }
      if (sdk && ts.isIdentifier(node) && /^(ParentView|Parent|Child)([A-Z_]|$)/.test(node.text)) {
        report("role-neutral-sdk", `Product role type/member: ${node.text}`);
      }
      if (
        (core || native || services) &&
        ts.isIdentifier(node) &&
        forbiddenRuntime.has(node.text)
      ) {
        report("platform-free-core", `Concrete platform API: ${node.text}`);
      }
      ts.forEachChild(node, visit);
    };
    visit(file);
  }
  const done = new Set<string>();
  const active = new Set<string>();
  const visit = (path: string, chain: string[]): void => {
    if (active.has(path)) {
      violations.push({ path, rule: "acyclic-dependencies", detail: [...chain, path].join(" → ") });
      return;
    }
    if (done.has(path)) return;
    active.add(path);
    for (const dependency of graph.get(path) ?? []) visit(dependency, [...chain, path]);
    active.delete(path);
    done.add(path);
  };
  for (const path of graph.keys()) visit(path, []);
  return violations;
}

export function readSources(root: string): Source[] {
  const sources: Source[] = [];
  function walk(directory: string): void {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (["node_modules", "dist", "target", ".git", "gen"].includes(entry.name)) continue;
      const path = join(directory, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.name.endsWith(".ts") && !entry.name.endsWith(".test.ts")) {
        sources.push({
          path: relative(root, path).replaceAll("\\", "/"),
          text: readFileSync(path, "utf8"),
        });
      }
    }
  }
  for (const folder of ["packages", "apps"]) walk(join(root, folder));
  return sources;
}
