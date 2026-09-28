import { inspectArchitecture, readSources } from "./architecture";

const sources = readSources(process.cwd());
const violations = inspectArchitecture(sources);
for (const violation of violations) {
  console.error(`${violation.path}: [${violation.rule}] ${violation.detail}`);
}
if (violations.length) process.exitCode = 1;
else
  console.log(
    `Architecture: ${sources.length} source files passed role, dependency and cycle checks.`,
  );
