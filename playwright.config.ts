import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "@playwright/test";

// Playwright's automatic failure-page ARIA snapshot can contain invitation secrets too.
// This switch is verified against the locked Playwright runtime, independently of trace settings.
process.env.PLAYWRIGHT_NO_COPY_PROMPT = "1";

const fakeAudioPath = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "e2e/fixtures/continuous-tone.wav",
);

export default defineConfig({
  testDir: "./e2e",
  timeout: 90_000,
  workers: 1,
  fullyParallel: false,
  forbidOnly: true,
  retries: 0,
  use: {
    baseURL: "http://localhost:1420",
    viewport: { width: 1440, height: 1100 },
    permissions: ["camera", "microphone"],
    launchOptions: {
      channel: process.env.CI ? undefined : "chrome",
      args: [
        "--use-fake-device-for-media-stream",
        `--use-file-for-fake-audio-capture=${fakeAudioPath}`,
        "--use-fake-ui-for-media-stream",
        "--autoplay-policy=no-user-gesture-required",
      ],
    },
    // Raw traces include room/device credentials. Tests attach redacted RTC diagnostics instead.
    trace: "off",
    screenshot: "off",
    video: "off",
  },
  webServer: [
    {
      command: "cargo run --locked -p parentview-signaling",
      url: "http://127.0.0.1:8787/health",
      reuseExistingServer: false,
      timeout: 120_000,
    },
    {
      command: "bun run build:web && bun run --cwd apps/desktop preview --port 1420",
      url: "http://localhost:1420",
      reuseExistingServer: false,
      timeout: 60_000,
    },
  ],
});
